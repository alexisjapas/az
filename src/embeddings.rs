use std::path::{Path, PathBuf};

use rusqlite::{Connection, params};

use crate::db;
pub use crate::db::DbError as EmbeddingsError;
pub use crate::db::Result;
use crate::session::ReadFilter;

pub const TARGET_TRANSCRIPT: &str = "transcript";
pub const TARGET_BLOCK: &str = "block";

/// Sérialise un vecteur f32 en little-endian.
pub fn pack_f32(v: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(v.len() * 4);
    for x in v {
        out.extend_from_slice(&x.to_le_bytes());
    }
    out
}

/// Désérialise un blob little-endian f32.
pub fn unpack_f32(blob: &[u8]) -> Vec<f32> {
    let mut out = Vec::with_capacity(blob.len() / 4);
    for chunk in blob.chunks_exact(4) {
        let arr: [u8; 4] = chunk.try_into().expect("chunk de 4 octets");
        out.push(f32::from_le_bytes(arr));
    }
    out
}

/// Similarité cosinus entre deux vecteurs. Renvoie 0.0 si l'un est nul ou
/// de tailles différentes (défensif, pas de panic).
pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let mut dot = 0.0f32;
    let mut na = 0.0f32;
    let mut nb = 0.0f32;
    for (&x, &y) in a.iter().zip(b.iter()) {
        dot += x * y;
        na += x * x;
        nb += y * y;
    }
    let denom = na.sqrt() * nb.sqrt();
    if denom == 0.0 { 0.0 } else { dot / denom }
}

#[derive(Debug, Clone)]
pub struct EmbeddedTarget {
    pub target_type: String,
    pub target_id: String,
    pub model: String,
    pub dim: usize,
}

#[derive(Debug, Clone)]
pub struct SearchHit {
    pub target_type: String,
    pub target_id: String,
    pub score: f32,
    pub content: String,
    pub sensitivity: bool,
}

/// (target_id, content, sensitivity, vector) — résultat interne d'un fetch.
type FetchedTarget = (String, String, bool, Vec<f32>);

pub struct EmbeddingsStore {
    conn: Connection,
    path: PathBuf,
}

impl EmbeddingsStore {
    pub fn open<P: AsRef<Path>>(path: P, key: &[u8; 32]) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let conn = db::open(&path, key)?;
        Ok(Self { conn, path })
    }

    pub fn upsert(
        &self,
        target_type: &str,
        target_id: &str,
        model: &str,
        vector: &[f32],
        now: &str,
    ) -> Result<()> {
        let blob = pack_f32(vector);
        self.conn.execute(
            "INSERT INTO embeddings (target_type, target_id, model, dim, vector, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6) \
             ON CONFLICT(target_type, target_id, model) DO UPDATE SET \
               dim = excluded.dim, vector = excluded.vector, created_at = excluded.created_at",
            params![
                target_type,
                target_id,
                model,
                vector.len() as i64,
                blob,
                now
            ],
        )?;
        Ok(())
    }

    /// IDs déjà embeddés pour un (target_type, model) donné — utilisé pour itérer
    /// idempotemment.
    pub fn existing_ids(&self, target_type: &str, model: &str) -> Result<Vec<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT target_id FROM embeddings WHERE target_type = ?1 AND model = ?2")?;
        let rows = stmt.query_map(params![target_type, model], |r| r.get(0))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    /// Récupère tous les vecteurs d'un (target_type, model) avec leur contenu
    /// joint depuis la table source (transcripts ou l1_blocks). Filtre
    /// `sensitivity` appliqué côté SQL pour qu'un transcript filtré ne soit
    /// même pas chargé en mémoire.
    fn fetch_targets(
        &self,
        target_type: &str,
        model: &str,
        filter: ReadFilter,
    ) -> Result<Vec<FetchedTarget>> {
        let (join_table, content_col, sens_col, where_sens) = match target_type {
            TARGET_TRANSCRIPT => (
                "transcripts",
                "transcripts.content",
                "transcripts.sensitivity",
                match filter {
                    ReadFilter::All => "",
                    ReadFilter::ExcludeSensitive => " AND transcripts.sensitivity = 0",
                },
            ),
            TARGET_BLOCK => (
                "l1_blocks",
                "l1_blocks.content",
                "l1_blocks.sensitivity",
                match filter {
                    ReadFilter::All => "",
                    ReadFilter::ExcludeSensitive => " AND l1_blocks.sensitivity = 0",
                },
            ),
            other => {
                return Err(EmbeddingsError::Sqlite(
                    rusqlite::Error::InvalidParameterName(format!("target_type inconnu: {other}")),
                ));
            }
        };
        let sql = format!(
            "SELECT embeddings.target_id, {content_col}, {sens_col}, embeddings.vector \
             FROM embeddings JOIN {join_table} ON {join_table}.id = embeddings.target_id \
             WHERE embeddings.target_type = ?1 AND embeddings.model = ?2{where_sens}"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(params![target_type, model], |row| {
            let id: String = row.get(0)?;
            let content: String = row.get(1)?;
            let sens: i64 = row.get(2)?;
            let blob: Vec<u8> = row.get(3)?;
            Ok((id, content, sens != 0, unpack_f32(&blob)))
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    /// Recherche top-k par similarité cosinus, en linéaire sur la donnée filtrée.
    ///
    /// Si `target_types` est vide, parcourt à la fois transcripts et blocks.
    pub fn search(
        &self,
        target_types: &[&str],
        model: &str,
        query_vec: &[f32],
        k: usize,
        filter: ReadFilter,
    ) -> Result<Vec<SearchHit>> {
        let types: Vec<&str> = if target_types.is_empty() {
            vec![TARGET_TRANSCRIPT, TARGET_BLOCK]
        } else {
            target_types.to_vec()
        };
        let mut all: Vec<SearchHit> = Vec::new();
        for tt in types {
            let rows = self.fetch_targets(tt, model, filter)?;
            for (id, content, sens, vec) in rows {
                let score = cosine(query_vec, &vec);
                all.push(SearchHit {
                    target_type: tt.to_string(),
                    target_id: id,
                    score,
                    content,
                    sensitivity: sens,
                });
            }
        }
        all.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        all.truncate(k);
        Ok(all)
    }

    pub fn count(&self) -> Result<u64> {
        let n: i64 = self
            .conn
            .query_row("SELECT count(*) FROM embeddings", [], |r| r.get(0))?;
        Ok(n as u64)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!(
            "az-emb-test-{}-{}.sqlite",
            std::process::id(),
            name
        ));
        let _ = std::fs::remove_file(&p);
        let _ = std::fs::remove_file(p.with_extension("sqlite-wal"));
        let _ = std::fs::remove_file(p.with_extension("sqlite-shm"));
        p
    }

    #[test]
    fn cosine_is_one_for_identical() {
        let a = vec![1.0, 2.0, 3.0];
        let s = cosine(&a, &a);
        assert!((s - 1.0).abs() < 1e-6);
    }

    #[test]
    fn cosine_is_zero_for_orthogonal() {
        let a = vec![1.0, 0.0, 0.0];
        let b = vec![0.0, 1.0, 0.0];
        let s = cosine(&a, &b);
        assert!(s.abs() < 1e-6);
    }

    #[test]
    fn cosine_handles_zero_vector() {
        let a = vec![0.0; 3];
        let b = vec![1.0, 2.0, 3.0];
        assert_eq!(cosine(&a, &b), 0.0);
    }

    #[test]
    fn pack_unpack_roundtrip() {
        let v = vec![1.5f32, -2.25, 0.0, std::f32::consts::PI];
        let blob = pack_f32(&v);
        let back = unpack_f32(&blob);
        assert_eq!(v, back);
    }

    fn seed_transcript(path: &PathBuf, id: &str, content: &str, sensitive: bool) {
        let l0 = crate::l0::L0Store::open(path, &db::test_key()).unwrap();
        l0.append(&crate::l0::L0Entry {
            id: id.into(),
            timestamp: "2026-01-01T00:00:00Z".into(),
            content: content.into(),
            source: "chat".into(),
            session_id: "S".into(),
            sensitivity: sensitive,
        })
        .unwrap();
    }

    #[test]
    fn search_orders_by_similarity() {
        let path = tmp("order");
        seed_transcript(&path, "t1", "alpha", false);
        seed_transcript(&path, "t2", "beta", false);
        seed_transcript(&path, "t3", "gamma", false);
        let store = EmbeddingsStore::open(&path, &db::test_key()).unwrap();
        store
            .upsert(TARGET_TRANSCRIPT, "t1", "m", &[1.0, 0.0, 0.0], "n")
            .unwrap();
        store
            .upsert(TARGET_TRANSCRIPT, "t2", "m", &[0.9, 0.1, 0.0], "n")
            .unwrap();
        store
            .upsert(TARGET_TRANSCRIPT, "t3", "m", &[0.0, 0.0, 1.0], "n")
            .unwrap();
        let hits = store
            .search(
                &[TARGET_TRANSCRIPT],
                "m",
                &[1.0, 0.0, 0.0],
                3,
                ReadFilter::All,
            )
            .unwrap();
        assert_eq!(hits.len(), 3);
        assert_eq!(hits[0].target_id, "t1");
        assert_eq!(hits[1].target_id, "t2");
        assert_eq!(hits[2].target_id, "t3");
    }

    #[test]
    fn search_respects_filter() {
        let path = tmp("filter");
        seed_transcript(&path, "secret", "info sensible", true);
        seed_transcript(&path, "ouvert", "info publique", false);
        let store = EmbeddingsStore::open(&path, &db::test_key()).unwrap();
        store
            .upsert(TARGET_TRANSCRIPT, "secret", "m", &[1.0, 0.0], "n")
            .unwrap();
        store
            .upsert(TARGET_TRANSCRIPT, "ouvert", "m", &[0.99, 0.01], "n")
            .unwrap();

        let all = store
            .search(&[TARGET_TRANSCRIPT], "m", &[1.0, 0.0], 5, ReadFilter::All)
            .unwrap();
        assert_eq!(all.len(), 2);
        let safe = store
            .search(
                &[TARGET_TRANSCRIPT],
                "m",
                &[1.0, 0.0],
                5,
                ReadFilter::ExcludeSensitive,
            )
            .unwrap();
        assert_eq!(safe.len(), 1);
        assert_eq!(safe[0].target_id, "ouvert");
    }

    #[test]
    fn upsert_is_idempotent() {
        let path = tmp("upsert");
        seed_transcript(&path, "t1", "x", false);
        let store = EmbeddingsStore::open(&path, &db::test_key()).unwrap();
        store
            .upsert(TARGET_TRANSCRIPT, "t1", "m", &[1.0, 2.0], "n1")
            .unwrap();
        store
            .upsert(TARGET_TRANSCRIPT, "t1", "m", &[3.0, 4.0], "n2")
            .unwrap();
        assert_eq!(store.count().unwrap(), 1);
        let ids = store.existing_ids(TARGET_TRANSCRIPT, "m").unwrap();
        assert_eq!(ids, vec!["t1".to_string()]);
    }
}
