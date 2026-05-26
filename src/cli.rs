use std::env;
use std::path::PathBuf;

use crate::auth::Authenticator;
use crate::l0::L0Store;
use crate::l1::L1Store;
use crate::l2::L2Store;

pub const ENV_L0_PATH: &str = "AZ_L0_PATH";
pub const DEFAULT_L0_PATH: &str = "./data/l0.sqlite";

#[derive(Debug, thiserror::Error)]
pub enum CliError {
    #[error("auth: {0}")]
    Auth(#[from] crate::auth::AuthError),
    #[error("db: {0}")]
    Db(#[from] crate::db::DbError),
}

pub type Result<T> = std::result::Result<T, CliError>;

pub fn resolve_l0_path() -> PathBuf {
    env::var(ENV_L0_PATH)
        .unwrap_or_else(|_| DEFAULT_L0_PATH.to_string())
        .into()
}

pub fn authenticate(path: &std::path::Path) -> Result<Authenticator> {
    Ok(Authenticator::from_env_or_prompt(path)?)
}

/// Variante pratique : résout le chemin, authentifie, ouvre L0.
pub fn open_l0() -> Result<(L0Store, PathBuf)> {
    let path = resolve_l0_path();
    let auth = authenticate(&path)?;
    let l0 = L0Store::open(&path, auth.key())?;
    Ok((l0, path))
}

/// Pour les binaires qui touchent L0 et L1 : une seule auth, deux stores.
pub fn open_l0_l1() -> Result<(L0Store, L1Store, PathBuf)> {
    let path = resolve_l0_path();
    let auth = authenticate(&path)?;
    let l0 = L0Store::open(&path, auth.key())?;
    let l1 = L1Store::open(&path, auth.key())?;
    Ok((l0, l1, path))
}

/// Variante pour le binaire `facts` (L1 + L2).
pub fn open_l1_l2() -> Result<(L1Store, L2Store, PathBuf)> {
    let path = resolve_l0_path();
    let auth = authenticate(&path)?;
    let l1 = L1Store::open(&path, auth.key())?;
    let l2 = L2Store::open(&path, auth.key())?;
    Ok((l1, l2, path))
}

/// Ouvre uniquement L2.
pub fn open_l2() -> Result<(L2Store, PathBuf)> {
    let path = resolve_l0_path();
    let auth = authenticate(&path)?;
    let l2 = L2Store::open(&path, auth.key())?;
    Ok((l2, path))
}
