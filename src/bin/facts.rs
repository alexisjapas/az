use std::env;
use std::io::{self, BufRead, Write};
use std::process::{Command, ExitCode};

use az::cli;
use az::extractor::extract_from_segmentation;
use az::l2::Fact;
use az::llm::ollama::OllamaClient;
use az::session::{ReadFilter, SessionMode};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

const USAGE: &str = "\
usage:
  cargo run --bin facts -- extract --segmentation <id> [--model <name>] [--mode private|connected]
  cargo run --bin facts -- review
  cargo run --bin facts -- list [--type <t>] [--drafts | --validated]
  cargo run --bin facts -- show <fact_id>

env:
  AZ_L0_PATH       (défaut ./data/l0.sqlite)
  AZ_OLLAMA_URL    (défaut http://localhost:11434)
  AZ_LLM_MODEL     (défaut gemma4:e2b)
  AZ_SESSION_MODE  (défaut private — override par --mode)
  EDITOR           éditeur utilisé pour le mode 'e' du review (défaut vi)
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
        "extract" => cmd_extract(&args[1..]),
        "review" => cmd_review(),
        "list" => cmd_list(&args[1..]),
        "show" => cmd_show(&args[1..]),
        other => {
            eprintln!("sous-commande inconnue: {other}");
            eprint!("{USAGE}");
            Ok(ExitCode::from(2))
        }
    }
}

fn cmd_extract(args: &[String]) -> anyhow::Result<ExitCode> {
    let mut segmentation: Option<String> = None;
    let mut model: Option<String> = None;
    let mut mode_arg: Option<String> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--segmentation" => {
                i += 1;
                segmentation = Some(
                    args.get(i)
                        .ok_or_else(|| anyhow::anyhow!("--segmentation attend un id"))?
                        .clone(),
                );
            }
            "--model" => {
                i += 1;
                model = Some(args.get(i).ok_or_else(|| anyhow::anyhow!("--model attend un nom"))?.clone());
            }
            "--mode" => {
                i += 1;
                mode_arg = Some(args.get(i).ok_or_else(|| anyhow::anyhow!("--mode attend private|connected"))?.clone());
            }
            other => anyhow::bail!("argument inconnu: {other}"),
        }
        i += 1;
    }
    let seg_id = segmentation.ok_or_else(|| anyhow::anyhow!("--segmentation <id> requis"))?;
    let session_mode = SessionMode::resolve(mode_arg.as_deref())?;
    let default_model = env::var("AZ_LLM_MODEL").unwrap_or_else(|_| "gemma4:e2b".to_string());
    let model_name = model.unwrap_or(default_model);

    let (l1, mut l2, _path) = cli::open_l1_l2()?;
    let llm = OllamaClient::from_env();
    eprintln!(
        "[az/facts] extraction segmentation={seg_id} mode={} model={model_name}",
        session_mode.as_str()
    );
    let drafts = extract_from_segmentation(&l1, &mut l2, &llm, &model_name, &seg_id, session_mode)?;
    println!("{} fait(s) en draft", drafts.len());
    for d in &drafts {
        println!("  [{}] type={} sensitive={}", &d.id[..8], d.fact_type, d.sensitivity);
    }
    println!("\nLance `cargo run --bin facts -- review` pour valider chaque fait.");
    Ok(ExitCode::SUCCESS)
}

enum ReviewAction {
    Validate,
    Delete,
    Skip,
    Edit,
    Quit,
}

fn cmd_review() -> anyhow::Result<ExitCode> {
    let (mut l2, _path) = cli::open_l2()?;
    let drafts = l2.list_drafts()?;
    if drafts.is_empty() {
        println!("(aucun draft à valider)");
        return Ok(ExitCode::SUCCESS);
    }
    let total = drafts.len();
    println!("{total} draft(s) à valider — touches : y=valider, n=supprimer, e=éditer, s=passer, q=quitter\n");

    let mut validated = 0;
    let mut deleted = 0;
    let mut edited = 0;

    for (idx, fact) in drafts.into_iter().enumerate() {
        println!("--- fait {}/{total} ---", idx + 1);
        print_fact(&fact)?;

        match prompt_action()? {
            ReviewAction::Validate => {
                let now = OffsetDateTime::now_utc().format(&Rfc3339)?;
                l2.validate(&fact.id, fact.version, &now)?;
                validated += 1;
                println!("→ validé\n");
            }
            ReviewAction::Delete => {
                l2.delete(&fact.id, fact.version)?;
                deleted += 1;
                println!("→ supprimé\n");
            }
            ReviewAction::Edit => {
                let new_payload = edit_with_editor(&fact.payload)?;
                l2.delete(&fact.id, fact.version)?;
                let now = OffsetDateTime::now_utc().format(&Rfc3339)?;
                let updated = Fact {
                    payload: new_payload,
                    validated_at: Some(now),
                    ..fact
                };
                // Pas de sources rebrancher dans cette V1 — le user ne les édite pas.
                l2.insert(&updated, &[])?;
                edited += 1;
                println!("→ édité et validé\n");
            }
            ReviewAction::Skip => {
                println!("→ passé\n");
            }
            ReviewAction::Quit => {
                println!("→ arrêt anticipé");
                break;
            }
        }
    }
    println!("\nRésumé : {validated} validé(s), {edited} édité(s), {deleted} supprimé(s)");
    Ok(ExitCode::SUCCESS)
}

fn prompt_action() -> anyhow::Result<ReviewAction> {
    let stdin = io::stdin();
    loop {
        print!("[y/n/e/s/q] > ");
        io::stdout().flush().ok();
        let mut line = String::new();
        if stdin.lock().read_line(&mut line)? == 0 {
            return Ok(ReviewAction::Quit);
        }
        match line.trim() {
            "y" | "Y" => return Ok(ReviewAction::Validate),
            "n" | "N" => return Ok(ReviewAction::Delete),
            "e" | "E" => return Ok(ReviewAction::Edit),
            "s" | "S" => return Ok(ReviewAction::Skip),
            "q" | "Q" => return Ok(ReviewAction::Quit),
            _ => eprintln!("réponse invalide : tapez y / n / e / s / q"),
        }
    }
}

fn edit_with_editor(initial: &str) -> anyhow::Result<String> {
    let editor = env::var("EDITOR").unwrap_or_else(|_| "vi".to_string());
    let mut tmp = env::temp_dir();
    tmp.push(format!(
        "az-fact-edit-{}-{}.json",
        std::process::id(),
        uuid::Uuid::new_v4()
    ));
    // Pretty-print pour aider l'utilisateur.
    let pretty = match serde_json::from_str::<serde_json::Value>(initial) {
        Ok(v) => serde_json::to_string_pretty(&v).unwrap_or_else(|_| initial.to_string()),
        Err(_) => initial.to_string(),
    };
    std::fs::write(&tmp, pretty)?;
    let status = Command::new(&editor).arg(&tmp).status()?;
    if !status.success() {
        anyhow::bail!("éditeur a quitté avec {status}");
    }
    let edited = std::fs::read_to_string(&tmp)?;
    let _ = std::fs::remove_file(&tmp);
    // Valide que c'est du JSON et re-sérialise compact.
    let v: serde_json::Value = serde_json::from_str(&edited)
        .map_err(|e| anyhow::anyhow!("JSON invalide après édition: {e}"))?;
    Ok(serde_json::to_string(&v)?)
}

fn cmd_list(args: &[String]) -> anyhow::Result<ExitCode> {
    let mut fact_type: Option<String> = None;
    let mut filter: Option<&str> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--type" => {
                i += 1;
                fact_type = Some(args.get(i).ok_or_else(|| anyhow::anyhow!("--type attend un nom"))?.clone());
            }
            "--drafts" => filter = Some("drafts"),
            "--validated" => filter = Some("validated"),
            other => anyhow::bail!("argument inconnu: {other}"),
        }
        i += 1;
    }

    let (l2, _path) = cli::open_l2()?;
    let facts = match (fact_type.as_deref(), filter) {
        (Some(t), Some("drafts")) => {
            l2.list_drafts()?.into_iter().filter(|f| f.fact_type == t).collect()
        }
        (Some(t), _) => l2.list_by_type(t, ReadFilter::All)?,
        (None, Some("drafts")) => l2.list_drafts()?,
        (None, Some("validated")) => l2.list_validated_current(ReadFilter::All)?,
        (None, None) => l2.list_current(ReadFilter::All)?,
        _ => Vec::new(),
    };
    if facts.is_empty() {
        println!("(aucun résultat)");
        return Ok(ExitCode::SUCCESS);
    }
    for f in &facts {
        let status = if f.validated_at.is_some() { "V" } else { "D" };
        let sens = if f.sensitivity { "[s]" } else { "   " };
        println!(
            "{} {} {} type={} id={} payload={}",
            status,
            sens,
            f.created_at,
            f.fact_type,
            &f.id[..8.min(f.id.len())],
            truncate(&f.payload, 80)
        );
    }
    println!("\n{} fait(s)", facts.len());
    Ok(ExitCode::SUCCESS)
}

fn cmd_show(args: &[String]) -> anyhow::Result<ExitCode> {
    let id = args.first().ok_or_else(|| anyhow::anyhow!("show <fact_id> requis"))?;
    let (l2, _path) = cli::open_l2()?;
    let versions = l2.get_versions(id)?;
    if versions.is_empty() {
        println!("(aucun fait avec id={id})");
        return Ok(ExitCode::SUCCESS);
    }
    for f in &versions {
        println!("\n=== version {} ===", f.version);
        print_fact(f)?;
        let sources = l2.fact_sources(&f.id, f.version)?;
        if !sources.is_empty() {
            println!("  sources L0 : {}", sources.join(", "));
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn print_fact(f: &Fact) -> anyhow::Result<()> {
    println!("  id        : {}", f.id);
    println!("  type      : {}", f.fact_type);
    println!("  sensitive : {}", f.sensitivity);
    println!("  created   : {}", f.created_at);
    println!(
        "  validated : {}",
        f.validated_at.as_deref().unwrap_or("(non validé)")
    );
    if let Some(b) = &f.block_id {
        println!("  block_id  : {b}");
    }
    let pretty = match serde_json::from_str::<serde_json::Value>(&f.payload) {
        Ok(v) => serde_json::to_string_pretty(&v).unwrap_or_else(|_| f.payload.clone()),
        Err(_) => f.payload.clone(),
    };
    println!("  payload   :");
    for line in pretty.lines() {
        println!("    {line}");
    }
    Ok(())
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(n).collect();
        out.push('…');
        out
    }
}
