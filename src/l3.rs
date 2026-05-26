use std::path::{Path, PathBuf};

use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};

use crate::db;
pub use crate::db::DbError as L3Error;
pub use crate::db::Result;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Link {
    pub id: String,
    pub src_kind: String,
    pub src_id: String,
    pub dst_kind: String,
    pub dst_id: String,
    pub rel_type: String,
    pub derived_by: String,
    pub metadata: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Page {
    pub id: String,
    pub title: String,
    pub description: Option<String>,
    pub is_active: bool,
    pub created_at: String,
    pub archived_at: Option<String>,
}

pub struct L3Store {
    conn: Connection,
    path: PathBuf,
}

impl L3Store {
    pub fn open<P: AsRef<Path>>(path: P, key: &[u8; 32]) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let conn = db::open(&path, key)?;
        Ok(Self { conn, path })
    }

    pub fn add_link(&self, link: &Link) -> Result<()> {
        self.conn.execute(
            "INSERT INTO l3_links (id, src_kind, src_id, dst_kind, dst_id, rel_type, derived_by, metadata, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                link.id,
                link.src_kind,
                link.src_id,
                link.dst_kind,
                link.dst_id,
                link.rel_type,
                link.derived_by,
                link.metadata,
                link.created_at,
            ],
        )?;
        Ok(())
    }

    pub fn remove_link(&self, id: &str) -> Result<()> {
        self.conn
            .execute("DELETE FROM l3_links WHERE id = ?1", params![id])?;
        Ok(())
    }

    pub fn list_outgoing(&self, src_kind: &str, src_id: &str) -> Result<Vec<Link>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, src_kind, src_id, dst_kind, dst_id, rel_type, derived_by, metadata, created_at \
             FROM l3_links WHERE src_kind = ?1 AND src_id = ?2 ORDER BY created_at",
        )?;
        let rows = stmt.query_map(params![src_kind, src_id], row_to_link)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    pub fn list_incoming(&self, dst_kind: &str, dst_id: &str) -> Result<Vec<Link>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, src_kind, src_id, dst_kind, dst_id, rel_type, derived_by, metadata, created_at \
             FROM l3_links WHERE dst_kind = ?1 AND dst_id = ?2 ORDER BY created_at",
        )?;
        let rows = stmt.query_map(params![dst_kind, dst_id], row_to_link)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    /// Vérifie si un lien existe déjà avec un src+rel_type+derived_by donné — utile
    /// pour rendre les règles de dérivation idempotentes.
    pub fn exists_derived(
        &self,
        src_kind: &str,
        src_id: &str,
        rel_type: &str,
        derived_by: &str,
    ) -> Result<bool> {
        let n: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM l3_links \
             WHERE src_kind = ?1 AND src_id = ?2 AND rel_type = ?3 AND derived_by = ?4",
            params![src_kind, src_id, rel_type, derived_by],
            |r| r.get(0),
        )?;
        Ok(n > 0)
    }

    pub fn add_page(&self, page: &Page) -> Result<()> {
        self.conn.execute(
            "INSERT INTO l3_pages (id, title, description, is_active, created_at, archived_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                page.id,
                page.title,
                page.description,
                page.is_active as i64,
                page.created_at,
                page.archived_at,
            ],
        )?;
        Ok(())
    }

    /// Active la page donnée et désactive toutes les autres, en une transaction.
    pub fn activate_page(&mut self, id: &str) -> Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute("UPDATE l3_pages SET is_active = 0", [])?;
        let n = tx.execute(
            "UPDATE l3_pages SET is_active = 1, archived_at = NULL WHERE id = ?1",
            params![id],
        )?;
        if n == 0 {
            tx.rollback()?;
            return Err(crate::db::DbError::Sqlite(
                rusqlite::Error::QueryReturnedNoRows,
            ));
        }
        tx.commit()?;
        Ok(())
    }

    pub fn archive_page(&self, id: &str, now: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE l3_pages SET is_active = 0, archived_at = ?2 WHERE id = ?1",
            params![id, now],
        )?;
        Ok(())
    }

    pub fn list_pages(&self) -> Result<Vec<Page>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, title, description, is_active, created_at, archived_at \
             FROM l3_pages ORDER BY created_at ASC",
        )?;
        let rows = stmt.query_map([], row_to_page)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    pub fn active_page(&self) -> Result<Option<Page>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, title, description, is_active, created_at, archived_at \
             FROM l3_pages WHERE is_active = 1 LIMIT 1",
        )?;
        let row = stmt.query_row([], row_to_page).ok();
        Ok(row)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

fn row_to_link(row: &rusqlite::Row) -> rusqlite::Result<Link> {
    Ok(Link {
        id: row.get(0)?,
        src_kind: row.get(1)?,
        src_id: row.get(2)?,
        dst_kind: row.get(3)?,
        dst_id: row.get(4)?,
        rel_type: row.get(5)?,
        derived_by: row.get(6)?,
        metadata: row.get(7)?,
        created_at: row.get(8)?,
    })
}

fn row_to_page(row: &rusqlite::Row) -> rusqlite::Result<Page> {
    Ok(Page {
        id: row.get(0)?,
        title: row.get(1)?,
        description: row.get(2)?,
        is_active: row.get::<_, i64>(3)? != 0,
        created_at: row.get(4)?,
        archived_at: row.get(5)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("az-l3-test-{}-{}.sqlite", std::process::id(), name));
        let _ = std::fs::remove_file(&p);
        let _ = std::fs::remove_file(p.with_extension("sqlite-wal"));
        let _ = std::fs::remove_file(p.with_extension("sqlite-shm"));
        p
    }

    fn link(id: &str, src: (&str, &str), dst: (&str, &str), rel: &str) -> Link {
        Link {
            id: id.into(),
            src_kind: src.0.into(),
            src_id: src.1.into(),
            dst_kind: dst.0.into(),
            dst_id: dst.1.into(),
            rel_type: rel.into(),
            derived_by: "manual".into(),
            metadata: None,
            created_at: "2026-05-26T10:00:00Z".into(),
        }
    }

    fn page(id: &str, title: &str, active: bool) -> Page {
        Page {
            id: id.into(),
            title: title.into(),
            description: None,
            is_active: active,
            created_at: "2026-05-26T10:00:00Z".into(),
            archived_at: None,
        }
    }

    #[test]
    fn add_and_list_outgoing() {
        let path = tmp("outgoing");
        let store = L3Store::open(&path, &db::test_key()).unwrap();
        store
            .add_link(&link("l1", ("fact", "f1"), ("page", "p1"), "belongs_to"))
            .unwrap();
        store
            .add_link(&link("l2", ("fact", "f1"), ("block", "b1"), "derives_from"))
            .unwrap();
        let out = store.list_outgoing("fact", "f1").unwrap();
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn list_incoming() {
        let path = tmp("incoming");
        let store = L3Store::open(&path, &db::test_key()).unwrap();
        store
            .add_link(&link("l1", ("fact", "f1"), ("page", "p1"), "belongs_to"))
            .unwrap();
        store
            .add_link(&link("l2", ("fact", "f2"), ("page", "p1"), "belongs_to"))
            .unwrap();
        let inc = store.list_incoming("page", "p1").unwrap();
        assert_eq!(inc.len(), 2);
    }

    #[test]
    fn only_one_active_page() {
        let path = tmp("active");
        let mut store = L3Store::open(&path, &db::test_key()).unwrap();
        store.add_page(&page("a", "A", true)).unwrap();
        store.add_page(&page("b", "B", false)).unwrap();
        store.activate_page("b").unwrap();
        let pages = store.list_pages().unwrap();
        let active: Vec<_> = pages.iter().filter(|p| p.is_active).collect();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].id, "b");
    }

    #[test]
    fn activate_unknown_page_errors() {
        let path = tmp("unknown");
        let mut store = L3Store::open(&path, &db::test_key()).unwrap();
        let err = store.activate_page("ghost").unwrap_err();
        assert!(matches!(err, crate::db::DbError::Sqlite(_)));
    }

    #[test]
    fn exists_derived_returns_true_after_insert() {
        let path = tmp("derived");
        let store = L3Store::open(&path, &db::test_key()).unwrap();
        let mut l = link("l1", ("fact", "f1"), ("shopping_item", "x"), "derives_to");
        l.derived_by = "rule:recipe-to-shopping".into();
        store.add_link(&l).unwrap();
        assert!(
            store
                .exists_derived("fact", "f1", "derives_to", "rule:recipe-to-shopping")
                .unwrap()
        );
        assert!(
            !store
                .exists_derived("fact", "f1", "derives_to", "manual")
                .unwrap()
        );
    }

    #[test]
    fn archive_page_clears_active() {
        let path = tmp("archive");
        let store = L3Store::open(&path, &db::test_key()).unwrap();
        store.add_page(&page("a", "A", true)).unwrap();
        store.archive_page("a", "2026-05-26T11:00:00Z").unwrap();
        let pages = store.list_pages().unwrap();
        assert!(!pages[0].is_active);
        assert_eq!(
            pages[0].archived_at.as_deref(),
            Some("2026-05-26T11:00:00Z")
        );
    }
}
