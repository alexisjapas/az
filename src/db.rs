use std::path::Path;

use rusqlite::Connection;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum DbError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("mot de passe invalide ou base corrompue")]
    WrongKey,
}

pub type Result<T> = std::result::Result<T, DbError>;

const SCHEMA_V1: &str = "
CREATE TABLE IF NOT EXISTS transcripts (
    id          TEXT PRIMARY KEY,
    timestamp   TEXT NOT NULL,
    content     TEXT NOT NULL,
    source      TEXT NOT NULL,
    session_id  TEXT NOT NULL,
    sensitivity INTEGER NOT NULL CHECK (sensitivity IN (0, 1))
);

CREATE INDEX IF NOT EXISTS idx_transcripts_session   ON transcripts(session_id);
CREATE INDEX IF NOT EXISTS idx_transcripts_timestamp ON transcripts(timestamp);

CREATE VIRTUAL TABLE IF NOT EXISTS transcripts_fts USING fts5(
    content,
    content='transcripts',
    content_rowid='rowid',
    tokenize='unicode61 remove_diacritics 2'
);

CREATE TRIGGER IF NOT EXISTS transcripts_ai
AFTER INSERT ON transcripts BEGIN
    INSERT INTO transcripts_fts(rowid, content) VALUES (new.rowid, new.content);
END;
";

const SCHEMA_V2: &str = "
CREATE TABLE IF NOT EXISTS l1_segmentations (
    id              TEXT PRIMARY KEY,
    created_at      TEXT NOT NULL,
    session_id      TEXT NOT NULL,
    model           TEXT NOT NULL,
    prompt_version  TEXT NOT NULL,
    notes           TEXT
);
CREATE INDEX IF NOT EXISTS idx_l1_segmentations_session ON l1_segmentations(session_id);

CREATE TABLE IF NOT EXISTS l1_blocks (
    id              TEXT PRIMARY KEY,
    segmentation_id TEXT NOT NULL REFERENCES l1_segmentations(id) ON DELETE CASCADE,
    seq             INTEGER NOT NULL,
    topic           TEXT,
    content         TEXT NOT NULL,
    sensitivity     INTEGER NOT NULL CHECK (sensitivity IN (0,1))
);
CREATE INDEX IF NOT EXISTS idx_l1_blocks_segmentation ON l1_blocks(segmentation_id);

CREATE TABLE IF NOT EXISTS l1_block_sources (
    block_id        TEXT NOT NULL REFERENCES l1_blocks(id) ON DELETE CASCADE,
    transcript_id   TEXT NOT NULL REFERENCES transcripts(id),
    PRIMARY KEY (block_id, transcript_id)
);
";

const SCHEMA_V3: &str = "
CREATE TABLE IF NOT EXISTS l2_facts (
    id            TEXT NOT NULL,
    version       INTEGER NOT NULL,
    fact_type     TEXT NOT NULL,
    payload       TEXT NOT NULL,
    block_id      TEXT REFERENCES l1_blocks(id),
    sensitivity   INTEGER NOT NULL CHECK (sensitivity IN (0,1)),
    created_at    TEXT NOT NULL,
    validated_at  TEXT,
    PRIMARY KEY (id, version)
);
CREATE INDEX IF NOT EXISTS idx_l2_facts_type      ON l2_facts(fact_type);
CREATE INDEX IF NOT EXISTS idx_l2_facts_block     ON l2_facts(block_id);
CREATE INDEX IF NOT EXISTS idx_l2_facts_validated ON l2_facts(validated_at);

CREATE TABLE IF NOT EXISTS l2_fact_sources (
    fact_id        TEXT NOT NULL,
    version        INTEGER NOT NULL,
    transcript_id  TEXT NOT NULL REFERENCES transcripts(id),
    PRIMARY KEY (fact_id, version, transcript_id),
    FOREIGN KEY (fact_id, version) REFERENCES l2_facts(id, version) ON DELETE CASCADE
);

CREATE VIEW IF NOT EXISTS l2_facts_current AS
SELECT * FROM l2_facts f
WHERE version = (SELECT MAX(version) FROM l2_facts WHERE id = f.id);
";

const SCHEMA_V4: &str = "
CREATE TABLE IF NOT EXISTS embeddings (
    target_type  TEXT NOT NULL,
    target_id    TEXT NOT NULL,
    model        TEXT NOT NULL,
    dim          INTEGER NOT NULL,
    vector       BLOB NOT NULL,
    created_at   TEXT NOT NULL,
    PRIMARY KEY (target_type, target_id, model)
);
CREATE INDEX IF NOT EXISTS idx_embeddings_type  ON embeddings(target_type);
CREATE INDEX IF NOT EXISTS idx_embeddings_model ON embeddings(model);
";

const TARGET_VERSION: i64 = 4;

pub fn open(path: &Path, key: &[u8; 32]) -> Result<Connection> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)?;
    }
    let conn = Connection::open(path)?;

    // PRAGMA key DOIT être le premier statement sur la connexion sqlcipher,
    // avant tout autre PRAGMA ou requête.
    conn.pragma_update(None, "key", format!("x'{}'", hex::encode(key)))?;
    conn.pragma_update(None, "cipher_compatibility", 4)?;

    // Vérification effective de la clé : si elle est mauvaise sqlcipher
    // n'échoue pas sur `PRAGMA key` mais sur la première lecture.
    conn.query_row("SELECT count(*) FROM sqlite_master", [], |_| Ok(()))
        .map_err(|_| DbError::WrongKey)?;

    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    migrate(&conn)?;
    Ok(conn)
}

pub fn migrate(conn: &Connection) -> Result<()> {
    let current: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if current < 1 {
        conn.execute_batch(SCHEMA_V1)?;
    }
    if current < 2 {
        conn.execute_batch(SCHEMA_V2)?;
    }
    if current < 3 {
        conn.execute_batch(SCHEMA_V3)?;
    }
    if current < 4 {
        conn.execute_batch(SCHEMA_V4)?;
    }
    conn.pragma_update(None, "user_version", TARGET_VERSION)?;
    Ok(())
}

/// Clé déterministe pour les tests internes au crate. Pas un secret.
#[cfg(test)]
pub fn test_key() -> [u8; 32] {
    [0xABu8; 32]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn tmp(name: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("az-db-test-{}-{}.sqlite", std::process::id(), name));
        let _ = std::fs::remove_file(&p);
        let _ = std::fs::remove_file(p.with_extension("sqlite-wal"));
        let _ = std::fs::remove_file(p.with_extension("sqlite-shm"));
        p
    }

    #[test]
    fn fresh_open_runs_full_migration() {
        let path = tmp("fresh");
        let conn = open(&path, &test_key()).unwrap();
        let v: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, TARGET_VERSION);
        let cnt: i64 = conn.query_row(
            "SELECT count(*) FROM sqlite_master WHERE type='table' AND name IN ('transcripts','l1_segmentations','l1_blocks','l1_block_sources','l2_facts','l2_fact_sources','embeddings')",
            [], |r| r.get(0)).unwrap();
        assert_eq!(cnt, 7);
    }

    #[test]
    fn reopen_with_same_key_is_idempotent() {
        let path = tmp("reopen");
        let key = test_key();
        let _ = open(&path, &key).unwrap();
        let _ = open(&path, &key).unwrap();
        let conn = open(&path, &key).unwrap();
        let v: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, TARGET_VERSION);
    }

    #[test]
    fn open_with_wrong_key_fails() {
        let path = tmp("wrong");
        let key_a = [1u8; 32];
        let key_b = [2u8; 32];

        // crée la DB avec key_a
        {
            let conn = open(&path, &key_a).unwrap();
            conn.execute(
                "INSERT INTO transcripts VALUES ('x','2026','c','s','sess',1)",
                [],
            )
            .unwrap();
        }
        // mauvaise clé → WrongKey
        let err = open(&path, &key_b).unwrap_err();
        assert!(matches!(err, DbError::WrongKey), "attendu WrongKey, got {err:?}");
    }

    #[test]
    fn encrypted_file_is_not_plain_sqlite() {
        let path = tmp("encrypted");
        let _ = open(&path, &test_key()).unwrap();
        // Le header d'un fichier SQLite plain commence par "SQLite format 3\0".
        // SQLCipher chiffre TOUT le fichier y compris ce header.
        let bytes = std::fs::read(&path).unwrap();
        let header = &bytes[..bytes.len().min(16)];
        assert_ne!(header, b"SQLite format 3\0");
    }
}
