use std::env;
use std::process::ExitCode;

use az::cli;
use az::embeddings::{EmbeddingsStore, TARGET_BLOCK, TARGET_TRANSCRIPT};
use az::llm::EmbeddingsLlm;
use az::llm::ollama::OllamaClient;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

const USAGE: &str = "\
usage:
  cargo run --bin embed -- [--targets transcripts,blocks] [--model nomic-embed-text]

env:
  AZ_L0_PATH      (défaut ./data/l0.sqlite)
  AZ_OLLAMA_URL   (défaut http://localhost:11434)
  AZ_EMBED_MODEL  (défaut nomic-embed-text)

Idempotent : ne ré-embedde pas une cible déjà couverte avec le même modèle.
";

const DEFAULT_MODEL: &str = "nomic-embed-text";

fn main() -> anyhow::Result<ExitCode> {
    let args: Vec<String> = env::args().skip(1).collect();
    let mut targets_arg = "transcripts,blocks".to_string();
    let mut model: Option<String> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--targets" => {
                i += 1;
                targets_arg = args
                    .get(i)
                    .ok_or_else(|| anyhow::anyhow!("--targets attend une liste"))?
                    .clone();
            }
            "--model" => {
                i += 1;
                model = Some(
                    args.get(i)
                        .ok_or_else(|| anyhow::anyhow!("--model attend un nom"))?
                        .clone(),
                );
            }
            "-h" | "--help" => {
                print!("{USAGE}");
                return Ok(ExitCode::SUCCESS);
            }
            other => anyhow::bail!("argument inconnu: {other}"),
        }
        i += 1;
    }

    let model_name = model
        .or_else(|| env::var("AZ_EMBED_MODEL").ok())
        .unwrap_or_else(|| DEFAULT_MODEL.to_string());

    let path = cli::resolve_l0_path();
    let auth = cli::authenticate(&path)?;
    let store = EmbeddingsStore::open(&path, auth.key())?;
    let l0 = az::l0::L0Store::open(&path, auth.key())?;
    let l1 = az::l1::L1Store::open(&path, auth.key())?;
    let llm = OllamaClient::from_env();

    let targets: Vec<&str> = targets_arg.split(',').map(|s| s.trim()).collect();
    eprintln!(
        "[az/embed] modèle={model_name} cibles={} via Ollama",
        targets.join(",")
    );

    let mut total_new = 0usize;
    let mut total_skip = 0usize;

    for target in targets {
        let (target_type, pairs) = match target {
            "transcripts" => (TARGET_TRANSCRIPT, l0.all_with_content()?),
            "blocks" => (TARGET_BLOCK, l1.all_blocks_with_content()?),
            other => anyhow::bail!("cible inconnue: {other} (transcripts|blocks)"),
        };
        let existing: std::collections::HashSet<String> = store
            .existing_ids(target_type, &model_name)?
            .into_iter()
            .collect();
        eprintln!(
            "[az/embed] {target_type}: {} candidats, {} déjà embeddés",
            pairs.len(),
            existing.len()
        );
        for (id, text) in &pairs {
            if existing.contains(id) {
                total_skip += 1;
                continue;
            }
            let v = llm
                .embed(&model_name, text)
                .map_err(|e| anyhow::anyhow!("échec embed pour {target_type}:{id}: {e}"))?;
            let now = OffsetDateTime::now_utc().format(&Rfc3339)?;
            store.upsert(target_type, id, &model_name, &v, &now)?;
            total_new += 1;
            if total_new.is_multiple_of(10) {
                eprintln!("[az/embed] {total_new} embeddings écrits…");
            }
        }
    }

    println!("OK: {total_new} embedding(s) ajouté(s), {total_skip} sauté(s) (déjà présents).");
    Ok(ExitCode::SUCCESS)
}
