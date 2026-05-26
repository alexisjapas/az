//! Helpers pour la sauvegarde externe chiffrée et le changement de clé.
//!
//! Backup : `VACUUM INTO` produit un fichier SQLCipher complet et propre,
//! chiffré avec la même clé que la source. On copie le salt à côté pour que la
//! restauration puisse re-dériver la clé depuis un mot de passe.
//!
//! Rekey : `PRAGMA rekey` ré-encrypte la DB en place avec une nouvelle clé.
//! On génère un nouveau salt, on écrit dans un fichier `.salt.new`, on
//! exécute PRAGMA rekey, et seulement si ça réussit on atomise le renommage.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use rusqlite::Connection;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum BackupError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("db: {0}")]
    Db(#[from] crate::db::DbError),
    #[error("la destination {0} existe déjà (utilisez --force pour écraser)")]
    DestinationExists(PathBuf),
    #[error("source manquante: {0}")]
    SourceMissing(PathBuf),
    #[error("salt source manquant: {0}")]
    SaltMissing(PathBuf),
}

pub type Result<T> = std::result::Result<T, BackupError>;

/// Copie SQLCipher → SQLCipher via `VACUUM INTO`. La destination est
/// chiffrée avec la même clé que la source ouverte sur `conn`.
pub fn vacuum_into(conn: &Connection, dest: &Path) -> Result<()> {
    if let Some(parent) = dest.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent)?;
    }
    let escaped = dest.to_string_lossy().replace('\'', "''");
    let sql = format!("VACUUM INTO '{escaped}'");
    conn.execute_batch(&sql)?;
    Ok(())
}

/// Copie le fichier `.salt` associé à `src_db` vers `dest_db.salt`.
pub fn copy_salt(src_db: &Path, dest_db: &Path) -> Result<()> {
    let src_salt = salt_path_for(src_db);
    let dest_salt = salt_path_for(dest_db);
    if !src_salt.exists() {
        return Err(BackupError::SaltMissing(src_salt));
    }
    fs::copy(&src_salt, &dest_salt)?;
    Ok(())
}

/// Calcule le chemin du salt côte à côte avec la DB.
pub fn salt_path_for(db_path: &Path) -> PathBuf {
    let mut s = db_path.as_os_str().to_owned();
    s.push(".salt");
    PathBuf::from(s)
}

/// Compacte la DB en place : checkpoint du WAL puis `VACUUM`. Récupère
/// l'espace des pages libérées (deletes, segmentations re-segmentées, etc).
pub fn vacuum_in_place(conn: &Connection) -> Result<()> {
    // Truncate du WAL avant VACUUM pour éviter de garder l'historique inutile.
    conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE); VACUUM;")?;
    Ok(())
}

/// Lance `PRAGMA rekey = "x'<hex>'"` sur la connexion ouverte. Doit être
/// appelé avec la connexion déjà déverrouillée par l'ancienne clé.
pub fn rekey_db(conn: &Connection, new_key: &[u8; 32]) -> Result<()> {
    conn.pragma_update(None, "rekey", format!("x'{}'", hex::encode(new_key)))?;
    Ok(())
}

/// Écrit un buffer atomiquement : tempfile → fsync → rename.
pub fn write_atomic(path: &Path, content: &[u8]) -> Result<()> {
    let mut tmp = path.to_path_buf();
    let mut name = tmp
        .file_name()
        .ok_or_else(|| std::io::Error::other("chemin sans nom de fichier"))?
        .to_os_string();
    name.push(".tmp");
    tmp.set_file_name(name);
    if let Some(parent) = tmp.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent)?;
    }
    {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(content)?;
        f.sync_data()?;
    }
    fs::rename(&tmp, path)?;
    Ok(())
}

/// Restaure une DB depuis un chemin source.
pub fn restore(src: &Path, dest: &Path, force: bool) -> Result<()> {
    if !src.exists() {
        return Err(BackupError::SourceMissing(src.to_path_buf()));
    }
    let src_salt = salt_path_for(src);
    if !src_salt.exists() {
        return Err(BackupError::SaltMissing(src_salt));
    }
    if dest.exists() && !force {
        return Err(BackupError::DestinationExists(dest.to_path_buf()));
    }
    if let Some(parent) = dest.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent)?;
    }
    // Nettoie WAL/SHM résiduels côté dest pour éviter incohérences.
    let _ = fs::remove_file(dest.with_extension("sqlite-wal"));
    let _ = fs::remove_file(dest.with_extension("sqlite-shm"));
    fs::copy(src, dest)?;
    fs::copy(src_salt, salt_path_for(dest))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    fn tmp(name: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!(
            "az-backup-test-{}-{}.sqlite",
            std::process::id(),
            name
        ));
        let _ = fs::remove_file(&p);
        let _ = fs::remove_file(p.with_extension("sqlite-wal"));
        let _ = fs::remove_file(p.with_extension("sqlite-shm"));
        let _ = fs::remove_file(salt_path_for(&p));
        p
    }

    #[test]
    fn vacuum_into_produces_readable_db() {
        let src = tmp("vacuum_src");
        let dest = tmp("vacuum_dst");
        {
            let conn = db::open(&src, &db::test_key()).unwrap();
            conn.execute(
                "INSERT INTO transcripts VALUES ('x','2026','c','chat','s',1)",
                [],
            )
            .unwrap();
            vacuum_into(&conn, &dest).unwrap();
        }
        // Ré-ouvre la destination avec la même clé : doit voir la ligne.
        let conn = db::open(&dest, &db::test_key()).unwrap();
        let n: i64 = conn
            .query_row("SELECT count(*) FROM transcripts", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 1);
    }

    #[test]
    fn vacuum_into_preserves_encryption() {
        let src = tmp("vacuum_enc_src");
        let dest = tmp("vacuum_enc_dst");
        {
            let conn = db::open(&src, &db::test_key()).unwrap();
            vacuum_into(&conn, &dest).unwrap();
        }
        // Le header ne doit pas être en clair.
        let bytes = fs::read(&dest).unwrap();
        assert_ne!(&bytes[..bytes.len().min(16)], b"SQLite format 3\0");
        // Et une mauvaise clé doit échouer.
        let err = db::open(&dest, &[0u8; 32]).unwrap_err();
        assert!(matches!(err, db::DbError::WrongKey));
    }

    #[test]
    fn write_atomic_replaces_existing() {
        let mut p = std::env::temp_dir();
        p.push(format!("az-atomic-{}-test.bin", std::process::id()));
        let _ = fs::remove_file(&p);
        write_atomic(&p, b"alpha").unwrap();
        assert_eq!(fs::read(&p).unwrap(), b"alpha");
        write_atomic(&p, b"beta_longer").unwrap();
        assert_eq!(fs::read(&p).unwrap(), b"beta_longer");
        let _ = fs::remove_file(&p);
    }

    #[test]
    fn restore_refuses_existing_without_force() {
        let src = tmp("restore_src");
        let dest = tmp("restore_dst");
        // Crée src + salt
        {
            let _ = db::open(&src, &db::test_key()).unwrap();
            fs::write(salt_path_for(&src), [7u8; 16]).unwrap();
            fs::write(&dest, b"existing").unwrap();
        }
        let err = restore(&src, &dest, false).unwrap_err();
        assert!(matches!(err, BackupError::DestinationExists(_)));
        // force = true doit marcher
        restore(&src, &dest, true).unwrap();
        assert!(salt_path_for(&dest).exists());
    }

    #[test]
    fn restore_refuses_missing_source() {
        let mut src = std::env::temp_dir();
        src.push("az-restore-missing-xyz.sqlite");
        let dest = tmp("restore_missing_dst");
        let _ = fs::remove_file(&src);
        let err = restore(&src, &dest, false).unwrap_err();
        assert!(matches!(err, BackupError::SourceMissing(_)));
    }

    #[test]
    fn vacuum_in_place_leaves_db_usable() {
        let path = tmp("vacuum_inplace");
        let conn = db::open(&path, &db::test_key()).unwrap();
        // Insère puis supprime pour créer de l'espace libérable.
        conn.execute_batch(
            "INSERT INTO transcripts VALUES ('a','2026','x','chat','s',1);\
             INSERT INTO transcripts VALUES ('b','2026','y','chat','s',0);\
             DELETE FROM transcripts WHERE id = 'a';",
        )
        .unwrap();
        vacuum_in_place(&conn).unwrap();
        let n: i64 = conn
            .query_row("SELECT count(*) FROM transcripts", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 1);
        // La table doit encore être lisible et le user_version conservé.
        let v: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert!(v >= 5);
    }

    #[test]
    fn rekey_changes_key() {
        let path = tmp("rekey");
        let key_a = [1u8; 32];
        let key_b = [2u8; 32];
        {
            let conn = db::open(&path, &key_a).unwrap();
            conn.execute(
                "INSERT INTO transcripts VALUES ('x','2026','c','chat','s',1)",
                [],
            )
            .unwrap();
            rekey_db(&conn, &key_b).unwrap();
        }
        // L'ancienne clé doit échouer.
        let err = db::open(&path, &key_a).unwrap_err();
        assert!(matches!(err, db::DbError::WrongKey));
        // La nouvelle clé doit marcher.
        let conn = db::open(&path, &key_b).unwrap();
        let n: i64 = conn
            .query_row("SELECT count(*) FROM transcripts", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 1);
    }
}
