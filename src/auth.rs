use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use argon2::{Algorithm, Argon2, Params, Version};
use rand::RngCore;
use thiserror::Error;

pub const KEY_SIZE: usize = 32;
pub const SALT_SIZE: usize = 16;
pub const ENV_PASSWORD: &str = "AZ_PASSWORD";

#[derive(Debug, Error)]
pub enum AuthError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("prompt mot de passe: {0}")]
    Prompt(String),
    #[error("les deux mots de passe ne correspondent pas")]
    PasswordMismatch,
    #[error("mot de passe vide")]
    EmptyPassword,
    #[error("dérivation Argon2: {0}")]
    Argon2(String),
    #[error("fichier salt corrompu (taille attendue {expected}, lue {got})")]
    SaltCorrupt { expected: usize, got: usize },
}

pub type Result<T> = std::result::Result<T, AuthError>;

pub struct Authenticator {
    key: [u8; KEY_SIZE],
}

impl Authenticator {
    pub fn key(&self) -> &[u8; KEY_SIZE] {
        &self.key
    }

    /// Résout la clé à partir d'un mot de passe (env ou prompt interactif)
    /// et du salt stocké à côté de la base.
    pub fn from_env_or_prompt(db_path: &Path) -> Result<Self> {
        let salt_path = salt_path(db_path);
        let salt_exists = salt_path.exists();
        let env_password = std::env::var(ENV_PASSWORD).ok();

        let (password, salt) = match (env_password, salt_exists) {
            (Some(p), true) => (p, read_salt(&salt_path)?),
            (Some(p), false) => (p, generate_and_write_salt(&salt_path)?),
            (None, true) => {
                let p = prompt("Mot de passe: ")?;
                if p.is_empty() {
                    return Err(AuthError::EmptyPassword);
                }
                (p, read_salt(&salt_path)?)
            }
            (None, false) => {
                eprintln!(
                    "[az/auth] première ouverture — création d'une base chiffrée à {}",
                    db_path.display()
                );
                let p1 = prompt("Nouveau mot de passe: ")?;
                if p1.is_empty() {
                    return Err(AuthError::EmptyPassword);
                }
                let p2 = prompt("Confirmer le mot de passe: ")?;
                if p1 != p2 {
                    return Err(AuthError::PasswordMismatch);
                }
                (p1, generate_and_write_salt(&salt_path)?)
            }
        };
        let key = derive_key(&password, &salt)?;
        Ok(Self { key })
    }
}

pub fn derive_key(password: &str, salt: &[u8]) -> Result<[u8; KEY_SIZE]> {
    let argon = Argon2::new(Algorithm::Argon2id, Version::V0x13, Params::default());
    let mut out = [0u8; KEY_SIZE];
    argon
        .hash_password_into(password.as_bytes(), salt, &mut out)
        .map_err(|e| AuthError::Argon2(e.to_string()))?;
    Ok(out)
}

pub fn salt_path(db_path: &Path) -> PathBuf {
    let mut s = db_path.as_os_str().to_owned();
    s.push(".salt");
    PathBuf::from(s)
}

fn prompt(msg: &str) -> Result<String> {
    rpassword::prompt_password(msg).map_err(|e| AuthError::Prompt(e.to_string()))
}

fn read_salt(path: &Path) -> Result<[u8; SALT_SIZE]> {
    let mut f = fs::File::open(path)?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf)?;
    if buf.len() != SALT_SIZE {
        return Err(AuthError::SaltCorrupt {
            expected: SALT_SIZE,
            got: buf.len(),
        });
    }
    let mut salt = [0u8; SALT_SIZE];
    salt.copy_from_slice(&buf);
    Ok(salt)
}

fn generate_and_write_salt(path: &Path) -> Result<[u8; SALT_SIZE]> {
    let mut salt = [0u8; SALT_SIZE];
    rand::thread_rng().fill_bytes(&mut salt);
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent)?;
    }
    let mut f = fs::File::create(path)?;
    f.write_all(&salt)?;
    Ok(salt)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derive_is_deterministic() {
        let salt = [7u8; SALT_SIZE];
        let k1 = derive_key("hello", &salt).unwrap();
        let k2 = derive_key("hello", &salt).unwrap();
        assert_eq!(k1, k2);
    }

    #[test]
    fn derive_differs_with_salt() {
        let k1 = derive_key("hello", &[1u8; SALT_SIZE]).unwrap();
        let k2 = derive_key("hello", &[2u8; SALT_SIZE]).unwrap();
        assert_ne!(k1, k2);
    }

    #[test]
    fn derive_differs_with_password() {
        let salt = [3u8; SALT_SIZE];
        let k1 = derive_key("alpha", &salt).unwrap();
        let k2 = derive_key("beta", &salt).unwrap();
        assert_ne!(k1, k2);
    }

    #[test]
    fn salt_path_appends_extension() {
        let p = salt_path(Path::new("/tmp/foo.sqlite"));
        assert_eq!(p, PathBuf::from("/tmp/foo.sqlite.salt"));
    }
}
