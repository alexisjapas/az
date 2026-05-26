use std::env;
use std::process::ExitCode;

use az::auth::{self, Authenticator, ENV_PASSWORD, SALT_SIZE};
use az::backup::{self, salt_path_for};
use az::cli;
use az::db;
use rand::RngCore;

const USAGE: &str = "\
usage:
  cargo run --bin rekey

Change le mot de passe maître de la DB chiffrée.
Prompts : ancien mot de passe, puis nouveau (× 2 confirmation).
L'opération est interactive ; AZ_PASSWORD est ignorée par sécurité.
";

fn main() -> anyhow::Result<ExitCode> {
    let args: Vec<String> = env::args().skip(1).collect();
    if args.iter().any(|a| matches!(a.as_str(), "-h" | "--help")) {
        print!("{USAGE}");
        return Ok(ExitCode::SUCCESS);
    }

    // Étape 1 : ouvrir avec l'ancien mot de passe via Authenticator standard.
    // On force l'utilisation du prompt (pas l'env var) pour cette opération
    // sensible — on désactive AZ_PASSWORD temporairement.
    let path = cli::resolve_l0_path();
    let restore_env = env::var(ENV_PASSWORD).ok();
    unsafe { env::remove_var(ENV_PASSWORD) };

    eprintln!("[az/rekey] DB : {}", path.display());
    let old_auth = Authenticator::from_env_or_prompt(&path)?;
    let conn = db::open(&path, old_auth.key())?;
    eprintln!("[az/rekey] ancien mot de passe OK.");

    // Étape 2 : prompt nouveau mot de passe + confirmation.
    let new_password = rpassword::prompt_password("Nouveau mot de passe: ")?;
    if new_password.is_empty() {
        eprintln!("mot de passe vide refusé");
        return Ok(ExitCode::from(2));
    }
    let confirm = rpassword::prompt_password("Confirmer le mot de passe: ")?;
    if new_password != confirm {
        eprintln!("les deux saisies diffèrent — abandon, DB inchangée");
        return Ok(ExitCode::from(2));
    }

    // Étape 3 : générer nouveau salt + dériver nouvelle clé.
    let mut new_salt = [0u8; SALT_SIZE];
    rand::thread_rng().fill_bytes(&mut new_salt);
    let new_key = auth::derive_key(&new_password, &new_salt)?;

    // Étape 4 : pré-écrire le salt dans un fichier compagnon (pas encore
    // installé). Si rekey échoue, on supprime et la DB reste sur l'ancien
    // salt + ancienne clé → l'utilisateur peut continuer à ouvrir.
    let salt_path = salt_path_for(&path);
    let mut staged = salt_path.clone();
    let mut name = staged
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("salt path sans nom"))?
        .to_os_string();
    name.push(".new");
    staged.set_file_name(name);
    std::fs::write(&staged, new_salt)?;

    // Étape 5 : PRAGMA rekey.
    let rekey_result = backup::rekey_db(&conn, &new_key);
    if let Err(e) = rekey_result {
        let _ = std::fs::remove_file(&staged);
        // Restaure AZ_PASSWORD si on l'avait retiré.
        if let Some(v) = restore_env {
            unsafe { env::set_var(ENV_PASSWORD, v) };
        }
        anyhow::bail!("échec PRAGMA rekey: {e}");
    }
    drop(conn);

    // Étape 6 : installer le nouveau salt atomiquement.
    backup::write_atomic(&salt_path, &new_salt)?;
    let _ = std::fs::remove_file(&staged);

    // Étape 7 : vérification finale en ré-ouvrant.
    let _ = db::open(&path, &new_key)?;
    eprintln!("[az/rekey] OK : ancien mot de passe révoqué, nouveau actif.");
    eprintln!("[az/rekey] pensez à mettre à jour AZ_PASSWORD si vous l'utilisez.");

    if let Some(v) = restore_env {
        unsafe { env::set_var(ENV_PASSWORD, v) };
    }
    Ok(ExitCode::SUCCESS)
}
