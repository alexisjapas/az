use std::fs;
use std::process::ExitCode;

use az::backup;
use az::cli;
use az::db;

const USAGE: &str = "\
usage:
  cargo run --bin vacuum

Compacte la DB en place (checkpoint WAL + VACUUM). Lit / réécrit le
fichier mais n'altère pas le contenu logique ni la clé de chiffrement.
À lancer après de grosses suppressions (re-segmentations, purges).

env: AZ_L0_PATH (défaut ./data/l0.sqlite)
";

fn main() -> anyhow::Result<ExitCode> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| matches!(a.as_str(), "-h" | "--help")) {
        print!("{USAGE}");
        return Ok(ExitCode::SUCCESS);
    }

    let path = cli::resolve_l0_path();
    let size_before = fs::metadata(&path).map(|m| m.len()).unwrap_or(0);

    let auth = cli::authenticate(&path)?;
    let conn = db::open(&path, auth.key())?;

    eprintln!("[az/vacuum] taille avant : {size_before} octets");
    backup::vacuum_in_place(&conn)?;
    drop(conn);

    let size_after = fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    let diff = size_before as i64 - size_after as i64;
    eprintln!("[az/vacuum] taille après : {size_after} octets ({diff:+} octets)");
    Ok(ExitCode::SUCCESS)
}
