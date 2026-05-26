use std::collections::HashMap;
use std::env;
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;

use az::cli;
use az::l0::L0Entry;
use az::l1::{Block, Segmentation};
use az::l2::Fact;
use serde::Serialize;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

const USAGE: &str = "\
usage:
  cargo run --bin export -- --format json     [--output <path>] [--out-stdout]
  cargo run --bin export -- --format markdown [--output <path>] [--out-stdout]
  cargo run --bin export -- --format jsonl    --target transcripts|blocks|facts [--output <path>] [--out-stdout]

Sortie par défaut : ./exports/az-<timestamp>.{json,md,jsonl}

ATTENTION : l'export N'EST PAS chiffré. Si vous l'envoyez ailleurs, re-chiffrez-le vous-même (gpg, age, etc).
";

#[derive(Debug, Serialize)]
struct ExportRoot {
    exported_at: String,
    schema_version: u32,
    transcripts: Vec<L0Entry>,
    segmentations: Vec<SegmentationExport>,
    facts: Vec<FactExport>,
}

#[derive(Debug, Serialize)]
struct SegmentationExport {
    #[serde(flatten)]
    segmentation: Segmentation,
    blocks: Vec<BlockExport>,
}

#[derive(Debug, Serialize)]
struct BlockExport {
    #[serde(flatten)]
    block: Block,
    sources: Vec<String>,
}

#[derive(Debug, Serialize)]
struct FactExport {
    #[serde(flatten)]
    fact: Fact,
    sources: Vec<String>,
}

enum Format {
    Json,
    Markdown,
    Jsonl,
}

enum JsonlTarget {
    Transcripts,
    Blocks,
    Facts,
}

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

    let mut format: Option<Format> = None;
    let mut output: Option<PathBuf> = None;
    let mut out_stdout = false;
    let mut jsonl_target: Option<JsonlTarget> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--format" => {
                i += 1;
                format = Some(match args.get(i).map(|s| s.as_str()) {
                    Some("json") => Format::Json,
                    Some("markdown") | Some("md") => Format::Markdown,
                    Some("jsonl") => Format::Jsonl,
                    Some(other) => anyhow::bail!("format inconnu: {other}"),
                    None => anyhow::bail!("--format attend json|markdown|jsonl"),
                });
            }
            "--output" => {
                i += 1;
                output = Some(
                    args.get(i)
                        .ok_or_else(|| anyhow::anyhow!("--output attend un chemin"))?
                        .into(),
                );
            }
            "--out-stdout" => out_stdout = true,
            "--target" => {
                i += 1;
                jsonl_target = Some(match args.get(i).map(|s| s.as_str()) {
                    Some("transcripts") => JsonlTarget::Transcripts,
                    Some("blocks") => JsonlTarget::Blocks,
                    Some("facts") => JsonlTarget::Facts,
                    Some(other) => anyhow::bail!("target inconnue: {other}"),
                    None => anyhow::bail!("--target attend transcripts|blocks|facts"),
                });
            }
            other => anyhow::bail!("argument inconnu: {other}"),
        }
        i += 1;
    }
    let format = format.ok_or_else(|| anyhow::anyhow!("--format requis"))?;

    let path = cli::resolve_l0_path();
    let auth = cli::authenticate(&path)?;
    let l0 = az::l0::L0Store::open(&path, auth.key())?;
    let l1 = az::l1::L1Store::open(&path, auth.key())?;
    let l2 = az::l2::L2Store::open(&path, auth.key())?;

    let now = OffsetDateTime::now_utc().format(&Rfc3339)?;

    let payload: Vec<u8> = match (format, jsonl_target) {
        (Format::Jsonl, Some(target)) => render_jsonl(&l0, &l1, &l2, target)?,
        (Format::Jsonl, None) => anyhow::bail!("--target requis avec --format jsonl"),
        (Format::Json, _) => {
            let root = build_root(&l0, &l1, &l2, &now)?;
            serde_json::to_vec_pretty(&root)?
        }
        (Format::Markdown, _) => render_markdown(&l0, &l1, &l2, &now)?,
    };

    if out_stdout {
        std::io::stdout().write_all(&payload)?;
        return Ok(ExitCode::SUCCESS);
    }

    let out_path = output.unwrap_or_else(|| default_path(&now, &format_from_payload(&args)));
    if let Some(parent) = out_path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent)?;
    }
    fs::write(&out_path, &payload)?;
    eprintln!(
        "[az/export] écrit {} octets dans {}",
        payload.len(),
        out_path.display()
    );
    eprintln!("[az/export] rappel : le fichier n'est PAS chiffré.");
    Ok(ExitCode::SUCCESS)
}

fn format_from_payload(args: &[String]) -> String {
    let i = args.iter().position(|a| a == "--format");
    if let Some(i) = i
        && let Some(f) = args.get(i + 1)
    {
        return match f.as_str() {
            "json" => "json".into(),
            "markdown" | "md" => "md".into(),
            "jsonl" => "jsonl".into(),
            _ => "bin".into(),
        };
    }
    "bin".into()
}

fn default_path(timestamp: &str, ext: &str) -> PathBuf {
    let safe_ts = timestamp.replace(':', "-");
    PathBuf::from(format!("./exports/az-{safe_ts}.{ext}"))
}

fn build_root(
    l0: &az::l0::L0Store,
    l1: &az::l1::L1Store,
    l2: &az::l2::L2Store,
    now: &str,
) -> anyhow::Result<ExportRoot> {
    let transcripts = l0.all_entries()?;
    let segs = l1.all_segmentations()?;
    let mut segmentations = Vec::with_capacity(segs.len());
    for s in segs {
        let blocks = l1.blocks(&s.id, az::session::ReadFilter::All)?;
        let mut blocks_export = Vec::with_capacity(blocks.len());
        for b in blocks {
            let sources = l1.block_sources(&b.id)?;
            blocks_export.push(BlockExport { block: b, sources });
        }
        segmentations.push(SegmentationExport {
            segmentation: s,
            blocks: blocks_export,
        });
    }
    let all_facts = l2.all_facts()?;
    let mut facts = Vec::with_capacity(all_facts.len());
    for f in all_facts {
        let sources = l2.fact_sources(&f.id, f.version)?;
        facts.push(FactExport { fact: f, sources });
    }
    Ok(ExportRoot {
        exported_at: now.to_string(),
        schema_version: 4,
        transcripts,
        segmentations,
        facts,
    })
}

fn render_jsonl(
    l0: &az::l0::L0Store,
    l1: &az::l1::L1Store,
    l2: &az::l2::L2Store,
    target: JsonlTarget,
) -> anyhow::Result<Vec<u8>> {
    let mut out = Vec::new();
    match target {
        JsonlTarget::Transcripts => {
            for e in l0.all_entries()? {
                let line = serde_json::to_string(&e)?;
                out.extend_from_slice(line.as_bytes());
                out.push(b'\n');
            }
        }
        JsonlTarget::Blocks => {
            for s in l1.all_segmentations()? {
                for b in l1.blocks(&s.id, az::session::ReadFilter::All)? {
                    let sources = l1.block_sources(&b.id)?;
                    let line = serde_json::to_string(&BlockExport { block: b, sources })?;
                    out.extend_from_slice(line.as_bytes());
                    out.push(b'\n');
                }
            }
        }
        JsonlTarget::Facts => {
            for f in l2.all_facts()? {
                let sources = l2.fact_sources(&f.id, f.version)?;
                let line = serde_json::to_string(&FactExport { fact: f, sources })?;
                out.extend_from_slice(line.as_bytes());
                out.push(b'\n');
            }
        }
    }
    Ok(out)
}

fn render_markdown(
    l0: &az::l0::L0Store,
    l1: &az::l1::L1Store,
    l2: &az::l2::L2Store,
    now: &str,
) -> anyhow::Result<Vec<u8>> {
    let mut out = String::new();
    out.push_str(&format!("# AZ — export {now}\n\n"));

    // L0 groupé par session
    let transcripts = l0.all_entries()?;
    let mut by_session: HashMap<String, Vec<&L0Entry>> = HashMap::new();
    for t in &transcripts {
        by_session.entry(t.session_id.clone()).or_default().push(t);
    }
    let mut sessions: Vec<_> = by_session.keys().cloned().collect();
    sessions.sort();

    out.push_str(&format!("## L0 — Transcripts ({} entrées)\n\n", transcripts.len()));
    for sid in &sessions {
        let entries = &by_session[sid];
        out.push_str(&format!(
            "### Session `{sid}` ({} entrées)\n\n",
            entries.len()
        ));
        for e in entries {
            let sens_marker = if e.sensitivity { "[s] " } else { "    " };
            out.push_str(&format!(
                "- `{}` [{}] {sens_marker}{}\n",
                e.timestamp, e.source, e.content
            ));
        }
        out.push('\n');
    }

    // L1
    let segs = l1.all_segmentations()?;
    out.push_str(&format!("## L1 — Segmentations ({})\n\n", segs.len()));
    for s in &segs {
        out.push_str(&format!(
            "### `{}` — session `{}` (model={}, prompt={})\n\n",
            s.id, s.session_id, s.model, s.prompt_version
        ));
        let blocks = l1.blocks(&s.id, az::session::ReadFilter::All)?;
        for b in &blocks {
            let sens = if b.sensitivity { "[s] " } else { "    " };
            out.push_str(&format!(
                "#### Bloc {} — {}{}\n\n> {}\n\n",
                b.seq,
                sens,
                b.topic.as_deref().unwrap_or("(sans topic)"),
                b.content.replace('\n', " ")
            ));
            let sources = l1.block_sources(&b.id)?;
            if !sources.is_empty() {
                out.push_str(&format!("sources L0 : `{}`\n\n", sources.join("`, `")));
            }
        }
    }

    // L2
    let facts = l2.all_facts()?;
    out.push_str(&format!("## L2 — Faits ({})\n\n", facts.len()));
    for f in &facts {
        let status = if f.validated_at.is_some() { "V" } else { "D" };
        let sens = if f.sensitivity { "[s]" } else { "   " };
        out.push_str(&format!(
            "### `{}` v{} — {} type=`{}` {sens} {}\n\n",
            &f.id[..8.min(f.id.len())],
            f.version,
            status,
            f.fact_type,
            f.validated_at.as_deref().unwrap_or("(draft)")
        ));
        let pretty = match serde_json::from_str::<serde_json::Value>(&f.payload) {
            Ok(v) => serde_json::to_string_pretty(&v).unwrap_or_else(|_| f.payload.clone()),
            Err(_) => f.payload.clone(),
        };
        out.push_str("```json\n");
        out.push_str(&pretty);
        out.push_str("\n```\n\n");
        let sources = l2.fact_sources(&f.id, f.version)?;
        if !sources.is_empty() {
            out.push_str(&format!("sources L0 : `{}`\n\n", sources.join("`, `")));
        }
    }

    Ok(out.into_bytes())
}
