use std::path::{Path, PathBuf};

use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};

use crate::db;
pub use crate::db::DbError as L0Error;
pub use crate::db::Result;
use crate::session::ReadFilter;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct L0Entry {
    pub id: String,
    pub timestamp: String,
    pub content: String,
    pub source: String,
    pub session_id: String,
    pub sensitivity: bool,
}

pub struct L0Store {
    conn: Connection,
    path: PathBuf,
}

impl L0Store {
    pub fn open<P: AsRef<Path>>(path: P, key: &[u8; 32]) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let conn = db::open(&path, key)?;
        Ok(Self { conn, path })
    }

    pub fn append(&self, entry: &L0Entry) -> Result<()> {
        self.conn.execute(
            "INSERT INTO transcripts (id, timestamp, content, source, session_id, sensitivity) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                entry.id,
                entry.timestamp,
                entry.content,
                entry.source,
                entry.session_id,
                entry.sensitivity as i64,
            ],
        )?;
        Ok(())
    }

    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<L0Entry>> {
        let mut stmt = self.conn.prepare(
            "SELECT t.id, t.timestamp, t.content, t.source, t.session_id, t.sensitivity \
             FROM transcripts t \
             JOIN transcripts_fts f ON f.rowid = t.rowid \
             WHERE transcripts_fts MATCH ?1 \
             ORDER BY rank \
             LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![query, limit as i64], row_to_entry)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    pub fn list_session(&self, session_id: &str, filter: ReadFilter) -> Result<Vec<L0Entry>> {
        let sql = match filter {
            ReadFilter::All => {
                "SELECT id, timestamp, content, source, session_id, sensitivity \
                 FROM transcripts WHERE session_id = ?1 ORDER BY timestamp ASC"
            }
            ReadFilter::ExcludeSensitive => {
                "SELECT id, timestamp, content, source, session_id, sensitivity \
                 FROM transcripts WHERE session_id = ?1 AND sensitivity = 0 ORDER BY timestamp ASC"
            }
        };
        let mut stmt = self.conn.prepare(sql)?;
        let rows = stmt.query_map(params![session_id], row_to_entry)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    /// Récupère TOUS les transcripts (toutes sessions, sans filtre). Utilisé
    /// par les exports — l'appelant décide quoi faire de la sensitivity.
    pub fn all_entries(&self) -> Result<Vec<L0Entry>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, timestamp, content, source, session_id, sensitivity \
             FROM transcripts ORDER BY timestamp ASC",
        )?;
        let rows = stmt.query_map([], row_to_entry)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    /// Liste (id, content) pour tous les transcripts. Utilisé pour les passes
    /// d'embedding sur l'ensemble du corpus. Pas de filtre — c'est à
    /// l'appelant de décider quoi exclure.
    pub fn all_with_content(&self) -> Result<Vec<(String, String)>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, content FROM transcripts ORDER BY timestamp ASC")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    pub fn count(&self) -> Result<u64> {
        let n: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM transcripts", [], |r| r.get(0))?;
        Ok(n as u64)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

fn row_to_entry(row: &rusqlite::Row) -> rusqlite::Result<L0Entry> {
    Ok(L0Entry {
        id: row.get(0)?,
        timestamp: row.get(1)?,
        content: row.get(2)?,
        source: row.get(3)?,
        session_id: row.get(4)?,
        sensitivity: row.get::<_, i64>(5)? != 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_path(name: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("az-l0-test-{}-{}.sqlite", std::process::id(), name));
        let _ = std::fs::remove_file(&p);
        let _ = std::fs::remove_file(p.with_extension("sqlite-wal"));
        let _ = std::fs::remove_file(p.with_extension("sqlite-shm"));
        p
    }

    fn entry(id: &str, session: &str, content: &str) -> L0Entry {
        L0Entry {
            id: id.into(),
            timestamp: "2026-01-01T00:00:00Z".into(),
            content: content.into(),
            source: "voice".into(),
            session_id: session.into(),
            sensitivity: true,
        }
    }

    #[test]
    fn append_and_count() {
        let path = tmp_path("count");
        let store = L0Store::open(&path, &db::test_key()).unwrap();
        store.append(&entry("1", "s1", "bonjour")).unwrap();
        store.append(&entry("2", "s1", "deuxième")).unwrap();
        assert_eq!(store.count().unwrap(), 2);
    }

    #[test]
    fn fts_finds_content() {
        let path = tmp_path("fts");
        let store = L0Store::open(&path, &db::test_key()).unwrap();
        store
            .append(&entry("1", "s1", "j'ai mangé une pomme"))
            .unwrap();
        store
            .append(&entry("2", "s1", "demain il fera beau"))
            .unwrap();
        let r = store.search("pomme", 10).unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].id, "1");
    }

    #[test]
    fn fts_ignores_diacritics() {
        let path = tmp_path("diacritics");
        let store = L0Store::open(&path, &db::test_key()).unwrap();
        store
            .append(&entry("1", "s1", "il fait été chaud"))
            .unwrap();
        let r = store.search("ete", 10).unwrap();
        assert_eq!(r.len(), 1, "le tokenizer doit ignorer les accents");
    }

    #[test]
    fn list_session_filters() {
        let path = tmp_path("session");
        let store = L0Store::open(&path, &db::test_key()).unwrap();
        store.append(&entry("1", "sA", "a1")).unwrap();
        store.append(&entry("2", "sA", "a2")).unwrap();
        store.append(&entry("3", "sB", "b1")).unwrap();
        let a = store.list_session("sA", ReadFilter::All).unwrap();
        let b = store.list_session("sB", ReadFilter::All).unwrap();
        assert_eq!(a.len(), 2);
        assert_eq!(b.len(), 1);
        assert!(a.iter().all(|e| e.session_id == "sA"));
    }

    #[test]
    fn list_session_excludes_sensitive_when_filtered() {
        let path = tmp_path("filter");
        let store = L0Store::open(&path, &db::test_key()).unwrap();
        store
            .append(&L0Entry {
                id: "1".into(),
                timestamp: "2026-01-01T00:00:00Z".into(),
                content: "secret".into(),
                source: "chat".into(),
                session_id: "S".into(),
                sensitivity: true,
            })
            .unwrap();
        store
            .append(&L0Entry {
                id: "2".into(),
                timestamp: "2026-01-01T00:00:01Z".into(),
                content: "ouvert".into(),
                source: "chat".into(),
                session_id: "S".into(),
                sensitivity: false,
            })
            .unwrap();
        store
            .append(&L0Entry {
                id: "3".into(),
                timestamp: "2026-01-01T00:00:02Z".into(),
                content: "secret2".into(),
                source: "chat".into(),
                session_id: "S".into(),
                sensitivity: true,
            })
            .unwrap();

        let all = store.list_session("S", ReadFilter::All).unwrap();
        assert_eq!(all.len(), 3);
        let safe = store
            .list_session("S", ReadFilter::ExcludeSensitive)
            .unwrap();
        assert_eq!(safe.len(), 1);
        assert_eq!(safe[0].id, "2");
        assert!(!safe[0].sensitivity);
    }

    #[test]
    fn open_creates_parent_dirs() {
        let mut dir = std::env::temp_dir();
        dir.push(format!("az-l0-parent-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("nested").join("l0.sqlite");
        let store = L0Store::open(&path, &db::test_key()).unwrap();
        store.append(&entry("x", "s", "c")).unwrap();
        assert!(path.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reopen_preserves_data() {
        let path = tmp_path("reopen");
        {
            let s = L0Store::open(&path, &db::test_key()).unwrap();
            s.append(&entry("1", "s1", "persistant")).unwrap();
        }
        let s = L0Store::open(&path, &db::test_key()).unwrap();
        assert_eq!(s.count().unwrap(), 1);
        let r = s.search("persistant", 10).unwrap();
        assert_eq!(r.len(), 1);
    }
}
