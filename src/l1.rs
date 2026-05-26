use std::path::{Path, PathBuf};

use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};

use crate::db;
pub use crate::db::DbError as L1Error;
pub use crate::db::Result;
use crate::session::ReadFilter;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Segmentation {
    pub id: String,
    pub created_at: String,
    pub session_id: String,
    pub model: String,
    pub prompt_version: String,
    pub notes: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Block {
    pub id: String,
    pub segmentation_id: String,
    pub seq: i64,
    pub topic: Option<String>,
    pub content: String,
    pub sensitivity: bool,
}

pub struct L1Store {
    conn: Connection,
    path: PathBuf,
}

impl L1Store {
    pub fn open<P: AsRef<Path>>(path: P, key: &[u8; 32]) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let conn = db::open(&path, key)?;
        Ok(Self { conn, path })
    }

    pub fn record(
        &mut self,
        seg: &Segmentation,
        blocks: &[Block],
        block_sources: &[(String, String)],
    ) -> Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute(
            "INSERT INTO l1_segmentations (id, created_at, session_id, model, prompt_version, notes) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                seg.id,
                seg.created_at,
                seg.session_id,
                seg.model,
                seg.prompt_version,
                seg.notes,
            ],
        )?;
        for b in blocks {
            tx.execute(
                "INSERT INTO l1_blocks (id, segmentation_id, seq, topic, content, sensitivity) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    b.id,
                    b.segmentation_id,
                    b.seq,
                    b.topic,
                    b.content,
                    b.sensitivity as i64,
                ],
            )?;
        }
        for (block_id, transcript_id) in block_sources {
            tx.execute(
                "INSERT INTO l1_block_sources (block_id, transcript_id) VALUES (?1, ?2)",
                params![block_id, transcript_id],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn list_segmentations(&self, session_id: &str) -> Result<Vec<Segmentation>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, created_at, session_id, model, prompt_version, notes \
             FROM l1_segmentations \
             WHERE session_id = ?1 \
             ORDER BY created_at DESC",
        )?;
        let rows = stmt.query_map(params![session_id], row_to_segmentation)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    pub fn blocks(&self, segmentation_id: &str, filter: ReadFilter) -> Result<Vec<Block>> {
        let sql = match filter {
            ReadFilter::All => {
                "SELECT id, segmentation_id, seq, topic, content, sensitivity \
                 FROM l1_blocks WHERE segmentation_id = ?1 ORDER BY seq ASC"
            }
            ReadFilter::ExcludeSensitive => {
                "SELECT id, segmentation_id, seq, topic, content, sensitivity \
                 FROM l1_blocks WHERE segmentation_id = ?1 AND sensitivity = 0 ORDER BY seq ASC"
            }
        };
        let mut stmt = self.conn.prepare(sql)?;
        let rows = stmt.query_map(params![segmentation_id], row_to_block)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    /// Récupère TOUTES les segmentations (toutes sessions). Pour exports.
    pub fn all_segmentations(&self) -> Result<Vec<Segmentation>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, created_at, session_id, model, prompt_version, notes \
             FROM l1_segmentations ORDER BY created_at ASC",
        )?;
        let rows = stmt.query_map([], row_to_segmentation)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    /// Liste (id, content) pour tous les blocs L1. Pas de filtre.
    pub fn all_blocks_with_content(&self) -> Result<Vec<(String, String)>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, content FROM l1_blocks ORDER BY segmentation_id, seq")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    pub fn block_sources(&self, block_id: &str) -> Result<Vec<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT transcript_id FROM l1_block_sources WHERE block_id = ?1")?;
        let rows = stmt.query_map(params![block_id], |r| r.get(0))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

fn row_to_segmentation(row: &rusqlite::Row) -> rusqlite::Result<Segmentation> {
    Ok(Segmentation {
        id: row.get(0)?,
        created_at: row.get(1)?,
        session_id: row.get(2)?,
        model: row.get(3)?,
        prompt_version: row.get(4)?,
        notes: row.get(5)?,
    })
}

fn row_to_block(row: &rusqlite::Row) -> rusqlite::Result<Block> {
    Ok(Block {
        id: row.get(0)?,
        segmentation_id: row.get(1)?,
        seq: row.get(2)?,
        topic: row.get(3)?,
        content: row.get(4)?,
        sensitivity: row.get::<_, i64>(5)? != 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::l0::{L0Entry, L0Store};

    fn tmp(name: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("az-l1-test-{}-{}.sqlite", std::process::id(), name));
        let _ = std::fs::remove_file(&p);
        let _ = std::fs::remove_file(p.with_extension("sqlite-wal"));
        let _ = std::fs::remove_file(p.with_extension("sqlite-shm"));
        p
    }

    fn t(id: &str, session: &str, content: &str) -> L0Entry {
        L0Entry {
            id: id.into(),
            timestamp: "2026-01-01T00:00:00Z".into(),
            content: content.into(),
            source: "chat".into(),
            session_id: session.into(),
            sensitivity: true,
        }
    }

    #[test]
    fn record_roundtrip() {
        let path = tmp("roundtrip");
        let l0 = L0Store::open(&path, &db::test_key()).unwrap();
        l0.append(&t("tx1", "S", "a")).unwrap();
        l0.append(&t("tx2", "S", "b")).unwrap();
        let mut l1 = L1Store::open(&path, &db::test_key()).unwrap();
        let seg = Segmentation {
            id: "seg1".into(),
            created_at: "2026-05-26T10:00:00Z".into(),
            session_id: "S".into(),
            model: "m".into(),
            prompt_version: "v1".into(),
            notes: None,
        };
        let blocks = vec![
            Block {
                id: "b1".into(),
                segmentation_id: "seg1".into(),
                seq: 0,
                topic: Some("Sujet A".into()),
                content: "contenu A".into(),
                sensitivity: true,
            },
            Block {
                id: "b2".into(),
                segmentation_id: "seg1".into(),
                seq: 1,
                topic: None,
                content: "contenu B".into(),
                sensitivity: false,
            },
        ];
        let sources = vec![
            ("b1".to_string(), "tx1".to_string()),
            ("b1".to_string(), "tx2".to_string()),
            ("b2".to_string(), "tx2".to_string()),
        ];
        l1.record(&seg, &blocks, &sources).unwrap();

        let segs = l1.list_segmentations("S").unwrap();
        assert_eq!(segs.len(), 1);
        assert_eq!(segs[0].id, "seg1");

        let bs = l1.blocks("seg1", crate::session::ReadFilter::All).unwrap();
        assert_eq!(bs.len(), 2);
        assert_eq!(bs[0].seq, 0);
        assert_eq!(bs[0].topic.as_deref(), Some("Sujet A"));
        assert!(!bs[1].sensitivity);

        let s1 = l1.block_sources("b1").unwrap();
        assert_eq!(s1.len(), 2);
        let s2 = l1.block_sources("b2").unwrap();
        assert_eq!(s2, vec!["tx2".to_string()]);
    }

    #[test]
    fn multiple_segmentations_per_session_coexist() {
        let path = tmp("versions");
        let l0 = L0Store::open(&path, &db::test_key()).unwrap();
        l0.append(&t("tx1", "S", "a")).unwrap();
        let mut l1 = L1Store::open(&path, &db::test_key()).unwrap();
        for (i, model) in ["m-v1", "m-v2"].iter().enumerate() {
            let seg = Segmentation {
                id: format!("seg-{i}"),
                created_at: format!("2026-05-26T10:0{i}:00Z"),
                session_id: "S".into(),
                model: (*model).into(),
                prompt_version: "v1".into(),
                notes: None,
            };
            let block = Block {
                id: format!("b-{i}"),
                segmentation_id: seg.id.clone(),
                seq: 0,
                topic: None,
                content: "x".into(),
                sensitivity: true,
            };
            l1.record(&seg, &[block], &[(format!("b-{i}"), "tx1".to_string())])
                .unwrap();
        }
        let segs = l1.list_segmentations("S").unwrap();
        assert_eq!(segs.len(), 2);
    }
}
