use std::env;
use std::process::ExitCode;

use az::cli;
use az::llm::ollama::OllamaClient;
use az::segmenter::segment_session;
use az::session::SessionMode;

const USAGE: &str = "\
usage:
  cargo run --bin segment -- --session <id> [--model <name>] [--mode private|connected]
  cargo run --bin segment -- --session <id> --list
  cargo run --bin segment -- --show <segmentation_id>

env:
  AZ_L0_PATH       (défaut ./data/l0.sqlite)
  AZ_OLLAMA_URL    (défaut http://localhost:11434)
  AZ_LLM_MODEL     (défaut gemma4:e2b)
  AZ_SESSION_MODE  (défaut private — override par --mode)
";

enum Mode {
    Run,
    List,
    Show(String),
}

fn main() -> anyhow::Result<ExitCode> {
    let args: Vec<String> = env::args().skip(1).collect();
    if args.is_empty() {
        eprint!("{USAGE}");
        return Ok(ExitCode::from(2));
    }

    let mut session: Option<String> = None;
    let mut model: Option<String> = None;
    let mut session_mode_arg: Option<String> = None;
    let mut mode = Mode::Run;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--session" => {
                i += 1;
                session = Some(
                    args.get(i)
                        .ok_or_else(|| anyhow::anyhow!("--session attend un ID"))?
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
            "--mode" => {
                i += 1;
                session_mode_arg = Some(
                    args.get(i)
                        .ok_or_else(|| anyhow::anyhow!("--mode attend private|connected"))?
                        .clone(),
                );
            }
            "--list" => mode = Mode::List,
            "--show" => {
                i += 1;
                let id = args
                    .get(i)
                    .ok_or_else(|| anyhow::anyhow!("--show attend un segmentation_id"))?
                    .clone();
                mode = Mode::Show(id);
            }
            "-h" | "--help" => {
                print!("{USAGE}");
                return Ok(ExitCode::SUCCESS);
            }
            other => anyhow::bail!("argument inconnu: {other}"),
        }
        i += 1;
    }

    let (l0, mut l1, _path) = cli::open_l0_l1()?;

    match mode {
        Mode::List => {
            let sid = session.ok_or_else(|| anyhow::anyhow!("--list requiert --session <id>"))?;
            let segs = l1.list_segmentations(&sid)?;
            if segs.is_empty() {
                println!("(aucune segmentation pour cette session)");
            } else {
                for s in &segs {
                    println!(
                        "{}  {}  model={}  prompt={}  notes={}",
                        s.created_at,
                        s.id,
                        s.model,
                        s.prompt_version,
                        s.notes.as_deref().unwrap_or("-")
                    );
                }
            }
        }
        Mode::Show(seg_id) => {
            let blocks = l1.blocks(&seg_id, az::session::ReadFilter::All)?;
            if blocks.is_empty() {
                println!("(aucun bloc — segmentation_id inconnu ?)");
            }
            for b in &blocks {
                println!(
                    "\n[bloc {} seq={}] topic = {}",
                    b.id,
                    b.seq,
                    b.topic.as_deref().unwrap_or("(none)")
                );
                println!("  content : {}", b.content);
                let srcs = l1.block_sources(&b.id)?;
                println!("  sources L0 : {}", srcs.join(", "));
            }
        }
        Mode::Run => {
            let sid = session.ok_or_else(|| anyhow::anyhow!("--session <id> requis"))?;
            let default_model =
                env::var("AZ_LLM_MODEL").unwrap_or_else(|_| "gemma4:e2b".to_string());
            let model_name = model.unwrap_or(default_model);
            let session_mode = SessionMode::resolve(session_mode_arg.as_deref())?;
            let llm = OllamaClient::from_env();
            eprintln!(
                "[az/segment] session={sid} mode={} model={model_name}{}",
                session_mode.as_str(),
                if matches!(session_mode, SessionMode::Connected) {
                    " — filtre sensitivity ACTIF"
                } else {
                    ""
                }
            );
            let seg = segment_session(&l0, &mut l1, &llm, &model_name, &sid, session_mode)?;
            let n = l1.blocks(&seg.id, az::session::ReadFilter::All)?.len();
            println!("segmentation_id = {}", seg.id);
            println!("{n} bloc(s) écrits");
        }
    }
    Ok(ExitCode::SUCCESS)
}
