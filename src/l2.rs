use std::path::{Path, PathBuf};

use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};

use crate::db;
pub use crate::db::DbError as L2Error;
pub use crate::db::Result;
use crate::session::ReadFilter;

/// Un fait L2 — draft (validated_at == None) ou validé.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Fact {
    pub id: String,
    pub version: i64,
    pub fact_type: String,
    pub payload: String, // JSON sérialisé
    pub block_id: Option<String>,
    pub sensitivity: bool,
    pub created_at: String,
    pub validated_at: Option<String>,
}

pub struct L2Store {
    conn: Connection,
    path: PathBuf,
}

impl L2Store {
    pub fn open<P: AsRef<Path>>(path: P, key: &[u8; 32]) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let conn = db::open(&path, key)?;
        Ok(Self { conn, path })
    }

    /// Insère un fait + ses sources L0 en une transaction.
    /// Le fait peut être un draft (validated_at = None) ou déjà validé.
    pub fn insert(&mut self, fact: &Fact, sources: &[String]) -> Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute(
            "INSERT INTO l2_facts (id, version, fact_type, payload, block_id, sensitivity, created_at, validated_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                fact.id,
                fact.version,
                fact.fact_type,
                fact.payload,
                fact.block_id,
                fact.sensitivity as i64,
                fact.created_at,
                fact.validated_at,
            ],
        )?;
        for transcript_id in sources {
            tx.execute(
                "INSERT INTO l2_fact_sources (fact_id, version, transcript_id) VALUES (?1, ?2, ?3)",
                params![fact.id, fact.version, transcript_id],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Marque un draft comme validé.
    pub fn validate(&self, id: &str, version: i64, now: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE l2_facts SET validated_at = ?3 WHERE id = ?1 AND version = ?2",
            params![id, version, now],
        )?;
        Ok(())
    }

    /// Supprime un draft (avec ses sources via FK CASCADE).
    pub fn delete(&self, id: &str, version: i64) -> Result<()> {
        self.conn.execute(
            "DELETE FROM l2_facts WHERE id = ?1 AND version = ?2",
            params![id, version],
        )?;
        Ok(())
    }

    pub fn list_drafts(&self) -> Result<Vec<Fact>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, version, fact_type, payload, block_id, sensitivity, created_at, validated_at \
             FROM l2_facts WHERE validated_at IS NULL ORDER BY created_at ASC",
        )?;
        let rows = stmt.query_map([], row_to_fact)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    /// Toutes les versions actuelles (MAX version par id), validées ou non,
    /// avec filtre `sensitivity`.
    pub fn list_current(&self, filter: ReadFilter) -> Result<Vec<Fact>> {
        let sql = match filter {
            ReadFilter::All => {
                "SELECT id, version, fact_type, payload, block_id, sensitivity, created_at, validated_at \
                 FROM l2_facts_current ORDER BY fact_type, created_at"
            }
            ReadFilter::ExcludeSensitive => {
                "SELECT id, version, fact_type, payload, block_id, sensitivity, created_at, validated_at \
                 FROM l2_facts_current WHERE sensitivity = 0 ORDER BY fact_type, created_at"
            }
        };
        let mut stmt = self.conn.prepare(sql)?;
        let rows = stmt.query_map([], row_to_fact)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    pub fn list_validated_current(&self, filter: ReadFilter) -> Result<Vec<Fact>> {
        let v = self.list_current(filter)?;
        Ok(v.into_iter().filter(|f| f.validated_at.is_some()).collect())
    }

    pub fn list_by_type(&self, fact_type: &str, filter: ReadFilter) -> Result<Vec<Fact>> {
        let sql = match filter {
            ReadFilter::All => {
                "SELECT id, version, fact_type, payload, block_id, sensitivity, created_at, validated_at \
                 FROM l2_facts_current WHERE fact_type = ?1 ORDER BY created_at"
            }
            ReadFilter::ExcludeSensitive => {
                "SELECT id, version, fact_type, payload, block_id, sensitivity, created_at, validated_at \
                 FROM l2_facts_current WHERE fact_type = ?1 AND sensitivity = 0 ORDER BY created_at"
            }
        };
        let mut stmt = self.conn.prepare(sql)?;
        let rows = stmt.query_map(params![fact_type], row_to_fact)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    pub fn get_versions(&self, id: &str) -> Result<Vec<Fact>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, version, fact_type, payload, block_id, sensitivity, created_at, validated_at \
             FROM l2_facts WHERE id = ?1 ORDER BY version ASC",
        )?;
        let rows = stmt.query_map(params![id], row_to_fact)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    pub fn fact_sources(&self, id: &str, version: i64) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT transcript_id FROM l2_fact_sources WHERE fact_id = ?1 AND version = ?2",
        )?;
        let rows = stmt.query_map(params![id, version], |r| r.get(0))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    /// Renvoie MAX(version) + 1 pour un id donné, ou 1 si inconnu.
    pub fn next_version(&self, id: &str) -> Result<i64> {
        let v: Option<i64> = self
            .conn
            .query_row(
                "SELECT MAX(version) FROM l2_facts WHERE id = ?1",
                params![id],
                |r| r.get(0),
            )
            .ok()
            .flatten();
        Ok(v.map(|n| n + 1).unwrap_or(1))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

fn row_to_fact(row: &rusqlite::Row) -> rusqlite::Result<Fact> {
    Ok(Fact {
        id: row.get(0)?,
        version: row.get(1)?,
        fact_type: row.get(2)?,
        payload: row.get(3)?,
        block_id: row.get(4)?,
        sensitivity: row.get::<_, i64>(5)? != 0,
        created_at: row.get(6)?,
        validated_at: row.get(7)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("az-l2-test-{}-{}.sqlite", std::process::id(), name));
        let _ = std::fs::remove_file(&p);
        let _ = std::fs::remove_file(p.with_extension("sqlite-wal"));
        let _ = std::fs::remove_file(p.with_extension("sqlite-shm"));
        p
    }

    fn fact(id: &str, version: i64, fact_type: &str, validated: bool) -> Fact {
        Fact {
            id: id.into(),
            version,
            fact_type: fact_type.into(),
            payload: r#"{"x":1}"#.into(),
            block_id: None,
            sensitivity: true,
            created_at: "2026-05-26T10:00:00Z".into(),
            validated_at: if validated {
                Some("2026-05-26T10:01:00Z".into())
            } else {
                None
            },
        }
    }

    #[test]
    fn insert_and_get_versions() {
        let path = tmp("versions");
        let mut store = L2Store::open(&path, &db::test_key()).unwrap();
        store.insert(&fact("f1", 1, "note", false), &[]).unwrap();
        store.insert(&fact("f1", 2, "note", false), &[]).unwrap();
        let v = store.get_versions("f1").unwrap();
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].version, 1);
        assert_eq!(v[1].version, 2);
    }

    #[test]
    fn next_version_starts_at_1() {
        let path = tmp("next1");
        let store = L2Store::open(&path, &db::test_key()).unwrap();
        assert_eq!(store.next_version("absent").unwrap(), 1);
    }

    #[test]
    fn next_version_increments() {
        let path = tmp("nextn");
        let mut store = L2Store::open(&path, &db::test_key()).unwrap();
        store.insert(&fact("f", 1, "note", true), &[]).unwrap();
        store.insert(&fact("f", 2, "note", true), &[]).unwrap();
        assert_eq!(store.next_version("f").unwrap(), 3);
    }

    #[test]
    fn validate_marks_validated_at() {
        let path = tmp("validate");
        let mut store = L2Store::open(&path, &db::test_key()).unwrap();
        store.insert(&fact("f", 1, "note", false), &[]).unwrap();
        let drafts = store.list_drafts().unwrap();
        assert_eq!(drafts.len(), 1);
        store.validate("f", 1, "2026-05-26T11:00:00Z").unwrap();
        let drafts = store.list_drafts().unwrap();
        assert!(drafts.is_empty());
        let v = store.list_current(ReadFilter::All).unwrap();
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].validated_at.as_deref(), Some("2026-05-26T11:00:00Z"));
    }

    #[test]
    fn list_current_returns_max_version() {
        let path = tmp("current");
        let mut store = L2Store::open(&path, &db::test_key()).unwrap();
        store.insert(&fact("f", 1, "note", true), &[]).unwrap();
        store.insert(&fact("f", 2, "note", true), &[]).unwrap();
        let cur = store.list_current(ReadFilter::All).unwrap();
        assert_eq!(cur.len(), 1);
        assert_eq!(cur[0].version, 2);
    }

    #[test]
    fn list_current_respects_filter() {
        let path = tmp("filter");
        let mut store = L2Store::open(&path, &db::test_key()).unwrap();
        let mut f1 = fact("a", 1, "note", true);
        f1.sensitivity = true;
        let mut f2 = fact("b", 1, "note", true);
        f2.sensitivity = false;
        store.insert(&f1, &[]).unwrap();
        store.insert(&f2, &[]).unwrap();
        let all = store.list_current(ReadFilter::All).unwrap();
        assert_eq!(all.len(), 2);
        let safe = store.list_current(ReadFilter::ExcludeSensitive).unwrap();
        assert_eq!(safe.len(), 1);
        assert_eq!(safe[0].id, "b");
    }

    #[test]
    fn delete_draft_removes_sources_via_cascade() {
        // Seed un transcript pour pouvoir poser une source FK.
        let path = tmp("delete");
        let l0 = crate::l0::L0Store::open(&path, &db::test_key()).unwrap();
        l0.append(&crate::l0::L0Entry {
            id: "t1".into(),
            timestamp: "2026-01-01T00:00:00Z".into(),
            content: "x".into(),
            source: "chat".into(),
            session_id: "S".into(),
            sensitivity: true,
        })
        .unwrap();
        let mut store = L2Store::open(&path, &db::test_key()).unwrap();
        store
            .insert(&fact("f", 1, "note", false), &["t1".to_string()])
            .unwrap();
        let s = store.fact_sources("f", 1).unwrap();
        assert_eq!(s, vec!["t1".to_string()]);
        store.delete("f", 1).unwrap();
        let s = store.fact_sources("f", 1).unwrap();
        assert!(s.is_empty(), "les sources doivent disparaître avec le fait");
    }

    #[test]
    fn list_by_type_filters() {
        let path = tmp("by_type");
        let mut store = L2Store::open(&path, &db::test_key()).unwrap();
        store.insert(&fact("a", 1, "note", true), &[]).unwrap();
        store.insert(&fact("b", 1, "event", true), &[]).unwrap();
        store.insert(&fact("c", 1, "note", true), &[]).unwrap();
        let notes = store.list_by_type("note", ReadFilter::All).unwrap();
        assert_eq!(notes.len(), 2);
        let events = store.list_by_type("event", ReadFilter::All).unwrap();
        assert_eq!(events.len(), 1);
    }
}
