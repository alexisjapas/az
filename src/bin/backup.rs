use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::ExitCode;

use az::backup::{self, salt_path_for};
use az::cli;
use az::db;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

const USAGE: &str = "\
usage:
  cargo run --bin backup -- create [--output <path>]
  cargo run --bin backup -- restore --from <path> [--force]
  cargo run --bin backup -- info <path>

Défaut --output : ./backups/az-<timestamp>.sqlite

Le backup est chiffré (même clé que la source). Le salt `.salt` est copié
à côté du fichier — il faut conserver les DEUX pour pouvoir restaurer.
";

fn main() -> anyhow::Result<ExitCode> {
    let args: Vec<String> = env::args().skip(1).collect();
    if args.is_empty() {
        eprint!("{USAGE}");
        return Ok(ExitCode::from(2));
    }
    if matches!(args[0].as_str(), "-h" | "--help") {
        print!("{USAGE}");
        return Ok(ExitCode::SUCCESS);
    }
    match args[0].as_str() {
        "create" => cmd_create(&args[1..]),
        "restore" => cmd_restore(&args[1..]),
        "info" => cmd_info(&args[1..]),
        other => {
            eprintln!("sous-commande inconnue: {other}");
            eprint!("{USAGE}");
            Ok(ExitCode::from(2))
        }
    }
}

fn cmd_create(args: &[String]) -> anyhow::Result<ExitCode> {
    let mut output: Option<PathBuf> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--output" => {
                i += 1;
                output = Some(
                    args.get(i)
                        .ok_or_else(|| anyhow::anyhow!("--output attend un chemin"))?
                        .into(),
                );
            }
            other => anyhow::bail!("argument inconnu: {other}"),
        }
        i += 1;
    }

    let src_path = cli::resolve_l0_path();
    let auth = cli::authenticate(&src_path)?;
    let conn = db::open(&src_path, auth.key())?;

    let now = OffsetDateTime::now_utc().format(&Rfc3339)?;
    let dest = output.unwrap_or_else(|| {
        let safe_ts = now.replace(':', "-");
        PathBuf::from(format!("./backups/az-{safe_ts}.sqlite"))
    });

    if dest.exists() {
        anyhow::bail!(
            "la destination {} existe déjà — supprimez-la ou choisissez un autre chemin",
            dest.display()
        );
    }

    backup::vacuum_into(&conn, &dest)?;
    backup::copy_salt(&src_path, &dest)?;

    let size = fs::metadata(&dest)?.len();
    eprintln!("[az/backup] OK : {} ({} octets)", dest.display(), size);
    eprintln!(
        "[az/backup] salt copié vers {}",
        salt_path_for(&dest).display()
    );
    eprintln!("[az/backup] conservez les DEUX fichiers ensemble.");
    Ok(ExitCode::SUCCESS)
}

fn cmd_restore(args: &[String]) -> anyhow::Result<ExitCode> {
    let mut from: Option<PathBuf> = None;
    let mut force = false;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--from" => {
                i += 1;
                from = Some(
                    args.get(i)
                        .ok_or_else(|| anyhow::anyhow!("--from attend un chemin"))?
                        .into(),
                );
            }
            "--force" => force = true,
            other => anyhow::bail!("argument inconnu: {other}"),
        }
        i += 1;
    }
    let src = from.ok_or_else(|| anyhow::anyhow!("--from <path> requis"))?;
    let dest = cli::resolve_l0_path();
    backup::restore(&src, &dest, force)?;
    eprintln!(
        "[az/backup] restauré {} → {} (et salt)",
        src.display(),
        dest.display()
    );
    eprintln!(
        "[az/backup] le mot de passe de la DB est celui de la sauvegarde, pas votre mot de passe actuel."
    );
    Ok(ExitCode::SUCCESS)
}

fn cmd_info(args: &[String]) -> anyhow::Result<ExitCode> {
    let p = args
        .first()
        .ok_or_else(|| anyhow::anyhow!("info attend un chemin"))?;
    let path = PathBuf::from(p);
    if !path.exists() {
        anyhow::bail!("introuvable : {}", path.display());
    }
    let meta = fs::metadata(&path)?;
    let salt = salt_path_for(&path);
    println!("fichier      : {}", path.display());
    println!("taille       : {} octets", meta.len());
    if let Ok(modified) = meta.modified()
        && let Ok(dur) = modified.duration_since(std::time::UNIX_EPOCH)
    {
        println!("modifié      : {}s depuis epoch", dur.as_secs());
    }
    let bytes = fs::read(&path)?;
    let header = &bytes[..bytes.len().min(16)];
    let plain = header == b"SQLite format 3\0";
    println!(
        "chiffré      : {}",
        if plain { "NON (en clair)" } else { "oui" }
    );
    println!(
        "salt présent : {}",
        if salt.exists() {
            format!("oui ({})", salt.display())
        } else {
            "NON (restauration impossible sans un salt cohérent)".to_string()
        }
    );
    Ok(ExitCode::SUCCESS)
}
