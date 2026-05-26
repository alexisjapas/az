use std::env;
use std::process::ExitCode;

use az::cli;
use az::embeddings::EmbeddingsStore;
use az::l0::L0Entry;
use az::llm::EmbeddingsLlm;
use az::llm::ollama::OllamaClient;
use az::session::{ReadFilter, SessionMode};

const USAGE: &str = "\
usage:
  cargo run --bin query -- <termes...>                    # recherche FTS L0
  cargo run --bin query -- --session <id>                 # tous les énoncés d'une session
  cargo run --bin query -- --count                        # nombre total d'énoncés
  cargo run --bin query -- --semantic <texte> [--limit N] [--mode private|connected]
                                                          # recherche sémantique top-k

env:
  AZ_L0_PATH       (défaut ./data/l0.sqlite)
  AZ_OLLAMA_URL    (défaut http://localhost:11434)
  AZ_EMBED_MODEL   (défaut nomic-embed-text)
  AZ_SESSION_MODE  (défaut private — override par --mode)
";

const DEFAULT_EMBED_MODEL: &str = "nomic-embed-text";

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
        "--semantic" => cmd_semantic(&args[1..]),
        "--count" => cmd_count(),
        "--session" => cmd_session(&args[1..]),
        _ => cmd_fts(&args),
    }
}

fn cmd_count() -> anyhow::Result<ExitCode> {
    let (store, _path) = cli::open_l0()?;
    println!("{}", store.count()?);
    Ok(ExitCode::SUCCESS)
}

fn cmd_session(args: &[String]) -> anyhow::Result<ExitCode> {
    let id = args
        .first()
        .ok_or_else(|| anyhow::anyhow!("--session attend un session_id"))?;
    let (store, _path) = cli::open_l0()?;
    let entries = store.list_session(id, ReadFilter::All)?;
    print_entries(&entries);
    Ok(ExitCode::SUCCESS)
}

fn cmd_fts(args: &[String]) -> anyhow::Result<ExitCode> {
    let (store, _path) = cli::open_l0()?;
    let query = args.join(" ");
    let entries = store.search(&query, 50)?;
    print_entries(&entries);
    Ok(ExitCode::SUCCESS)
}

fn cmd_semantic(args: &[String]) -> anyhow::Result<ExitCode> {
    let mut text_parts: Vec<String> = Vec::new();
    let mut limit: usize = 5;
    let mut mode_arg: Option<String> = None;
    let mut model: Option<String> = None;
    let mut targets: Option<String> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--limit" => {
                i += 1;
                limit = args
                    .get(i)
                    .ok_or_else(|| anyhow::anyhow!("--limit attend un entier"))?
                    .parse()
                    .map_err(|e| anyhow::anyhow!("--limit invalide: {e}"))?;
            }
            "--mode" => {
                i += 1;
                mode_arg = Some(
                    args.get(i)
                        .ok_or_else(|| anyhow::anyhow!("--mode attend private|connected"))?
                        .clone(),
                );
            }
            "--model" => {
                i += 1;
                model = Some(
                    args.get(i)
                        .ok_or_else(|| anyhow::anyhow!("--model attend un nom"))?
                        .clone(),
                );
            }
            "--targets" => {
                i += 1;
                targets = Some(
                    args.get(i)
                        .ok_or_else(|| {
                            anyhow::anyhow!(
                                "--targets attend transcripts|blocks|transcripts,blocks"
                            )
                        })?
                        .clone(),
                );
            }
            other => text_parts.push(other.to_string()),
        }
        i += 1;
    }
    if text_parts.is_empty() {
        anyhow::bail!("--semantic attend du texte à rechercher");
    }
    let text = text_parts.join(" ");
    let session_mode = SessionMode::resolve(mode_arg.as_deref())?;
    let model_name = model
        .or_else(|| env::var("AZ_EMBED_MODEL").ok())
        .unwrap_or_else(|| DEFAULT_EMBED_MODEL.to_string());

    let path = cli::resolve_l0_path();
    let auth = cli::authenticate(&path)?;
    let emb = EmbeddingsStore::open(&path, auth.key())?;

    let llm = OllamaClient::from_env();
    let q_vec = llm.embed(&model_name, &text)?;

    let target_types: Vec<&str> = if let Some(t) = &targets {
        t.split(',').map(|s| s.trim()).collect()
    } else {
        Vec::new()
    };
    let hits = emb.search(
        &target_types,
        &model_name,
        &q_vec,
        limit,
        session_mode.read_filter(),
    )?;
    if hits.is_empty() {
        println!("(aucun résultat sémantique)");
        return Ok(ExitCode::SUCCESS);
    }
    for h in &hits {
        let sens = if h.sensitivity { "[s]" } else { "   " };
        let short_id = &h.target_id[..8.min(h.target_id.len())];
        println!(
            "{sens} {:>6.3} {} {} :: {}",
            h.score, h.target_type, short_id, h.content
        );
    }
    println!(
        "\n{} résultat(s) (modèle={model_name}, mode={})",
        hits.len(),
        session_mode.as_str()
    );
    Ok(ExitCode::SUCCESS)
}

fn print_entries(entries: &[L0Entry]) {
    if entries.is_empty() {
        println!("(aucun résultat)");
        return;
    }
    for e in entries {
        let short_session = &e.session_id[..8.min(e.session_id.len())];
        let sens = if e.sensitivity { "[s]" } else { "   " };
        println!(
            "{} {} [{}] {} :: {}",
            sens, e.timestamp, e.source, short_session, e.content
        );
    }
    println!("\n{} résultat(s)", entries.len());
}
