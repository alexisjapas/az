use std::env;
use std::process::ExitCode;

use az::cli;
use az::derivation::{DerivationRule, RecipeToShopping, all_rules};
use az::l2::L2Store;
use az::l3::{L3Store, Link, Page};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use uuid::Uuid;

const USAGE: &str = "\
usage:
  links list --src <kind:id>                        # arêtes sortantes
  links list --dst <kind:id>                        # arêtes entrantes
  links add  --src <kind:id> --dst <kind:id> --rel <type>
  links remove <link_id>

  links pages [--archived]                          # par défaut : non archivées
  links create-page <title> [--description \"...\"]
  links activate <page_id>
  links archive  <page_id>

  links derive [--rule recipe-to-shopping|all]      # applique les règles ; idempotent

kind ∈ { fact, block, transcript, page, shopping_item }
env: AZ_L0_PATH (défaut ./data/l0.sqlite)
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
        "list" => cmd_list(&args[1..]),
        "add" => cmd_add(&args[1..]),
        "remove" => cmd_remove(&args[1..]),
        "pages" => cmd_pages(&args[1..]),
        "create-page" => cmd_create_page(&args[1..]),
        "activate" => cmd_activate(&args[1..]),
        "archive" => cmd_archive(&args[1..]),
        "derive" => cmd_derive(&args[1..]),
        other => {
            eprintln!("sous-commande inconnue: {other}");
            eprint!("{USAGE}");
            Ok(ExitCode::from(2))
        }
    }
}

fn parse_kind_id(s: &str) -> anyhow::Result<(String, String)> {
    let (k, i) = s
        .split_once(':')
        .ok_or_else(|| anyhow::anyhow!("attendu kind:id (ex: fact:abc123), reçu '{s}'"))?;
    Ok((k.to_string(), i.to_string()))
}

fn open_l3() -> anyhow::Result<L3Store> {
    let path = cli::resolve_l0_path();
    let auth = cli::authenticate(&path)?;
    Ok(L3Store::open(&path, auth.key())?)
}

fn cmd_list(args: &[String]) -> anyhow::Result<ExitCode> {
    let mut src: Option<String> = None;
    let mut dst: Option<String> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--src" => {
                i += 1;
                src = Some(
                    args.get(i)
                        .ok_or_else(|| anyhow::anyhow!("--src attend kind:id"))?
                        .clone(),
                );
            }
            "--dst" => {
                i += 1;
                dst = Some(
                    args.get(i)
                        .ok_or_else(|| anyhow::anyhow!("--dst attend kind:id"))?
                        .clone(),
                );
            }
            other => anyhow::bail!("argument inconnu: {other}"),
        }
        i += 1;
    }
    let l3 = open_l3()?;
    let links = match (src, dst) {
        (Some(s), None) => {
            let (k, id) = parse_kind_id(&s)?;
            l3.list_outgoing(&k, &id)?
        }
        (None, Some(d)) => {
            let (k, id) = parse_kind_id(&d)?;
            l3.list_incoming(&k, &id)?
        }
        _ => anyhow::bail!("list attend --src OU --dst (un seul)"),
    };
    if links.is_empty() {
        println!("(aucun lien)");
        return Ok(ExitCode::SUCCESS);
    }
    for l in &links {
        let meta = l.metadata.as_deref().unwrap_or("");
        println!(
            "{}  {}:{} --[{}]--> {}:{}  ({})  {}",
            &l.id[..8.min(l.id.len())],
            l.src_kind,
            short_id(&l.src_id),
            l.rel_type,
            l.dst_kind,
            short_id(&l.dst_id),
            l.derived_by,
            meta
        );
    }
    println!("\n{} lien(s)", links.len());
    Ok(ExitCode::SUCCESS)
}

fn cmd_add(args: &[String]) -> anyhow::Result<ExitCode> {
    let mut src: Option<String> = None;
    let mut dst: Option<String> = None;
    let mut rel: Option<String> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--src" => {
                i += 1;
                src = Some(
                    args.get(i)
                        .ok_or_else(|| anyhow::anyhow!("--src attend kind:id"))?
                        .clone(),
                );
            }
            "--dst" => {
                i += 1;
                dst = Some(
                    args.get(i)
                        .ok_or_else(|| anyhow::anyhow!("--dst attend kind:id"))?
                        .clone(),
                );
            }
            "--rel" => {
                i += 1;
                rel = Some(
                    args.get(i)
                        .ok_or_else(|| anyhow::anyhow!("--rel attend un type"))?
                        .clone(),
                );
            }
            other => anyhow::bail!("argument inconnu: {other}"),
        }
        i += 1;
    }
    let src = src.ok_or_else(|| anyhow::anyhow!("--src requis"))?;
    let dst = dst.ok_or_else(|| anyhow::anyhow!("--dst requis"))?;
    let rel = rel.ok_or_else(|| anyhow::anyhow!("--rel requis"))?;
    let (src_kind, src_id) = parse_kind_id(&src)?;
    let (dst_kind, dst_id) = parse_kind_id(&dst)?;

    let l3 = open_l3()?;
    let now = OffsetDateTime::now_utc().format(&Rfc3339)?;
    let id = Uuid::new_v4().to_string();
    l3.add_link(&Link {
        id: id.clone(),
        src_kind,
        src_id,
        dst_kind,
        dst_id,
        rel_type: rel,
        derived_by: "manual".into(),
        metadata: None,
        created_at: now,
    })?;
    println!("lien créé : {id}");
    Ok(ExitCode::SUCCESS)
}

fn cmd_remove(args: &[String]) -> anyhow::Result<ExitCode> {
    let id = args
        .first()
        .ok_or_else(|| anyhow::anyhow!("remove attend un link_id"))?;
    let l3 = open_l3()?;
    l3.remove_link(id)?;
    println!("supprimé");
    Ok(ExitCode::SUCCESS)
}

fn cmd_pages(args: &[String]) -> anyhow::Result<ExitCode> {
    let show_archived = args.iter().any(|a| a == "--archived");
    let l3 = open_l3()?;
    let all = l3.list_pages()?;
    let pages: Vec<_> = all
        .into_iter()
        .filter(|p| {
            if show_archived {
                p.archived_at.is_some()
            } else {
                p.archived_at.is_none()
            }
        })
        .collect();
    if pages.is_empty() {
        println!("(aucune page)");
        return Ok(ExitCode::SUCCESS);
    }
    for p in &pages {
        let active = if p.is_active { "*" } else { " " };
        println!(
            "{active} {}  {}  {}",
            &p.id[..8.min(p.id.len())],
            p.title,
            p.description.as_deref().unwrap_or("")
        );
    }
    println!("\n{} page(s)  (* = active)", pages.len());
    Ok(ExitCode::SUCCESS)
}

fn cmd_create_page(args: &[String]) -> anyhow::Result<ExitCode> {
    let mut title: Option<String> = None;
    let mut description: Option<String> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--description" => {
                i += 1;
                description = Some(
                    args.get(i)
                        .ok_or_else(|| anyhow::anyhow!("--description attend une string"))?
                        .clone(),
                );
            }
            other if title.is_none() => title = Some(other.to_string()),
            other => anyhow::bail!("argument inattendu: {other}"),
        }
        i += 1;
    }
    let title = title.ok_or_else(|| anyhow::anyhow!("title positionnel requis"))?;
    let l3 = open_l3()?;
    let id = Uuid::new_v4().to_string();
    let now = OffsetDateTime::now_utc().format(&Rfc3339)?;
    l3.add_page(&Page {
        id: id.clone(),
        title,
        description,
        is_active: false,
        created_at: now,
        archived_at: None,
    })?;
    println!("page créée : {id}");
    println!("active-la avec : cargo run --bin links -- activate {id}");
    Ok(ExitCode::SUCCESS)
}

fn cmd_activate(args: &[String]) -> anyhow::Result<ExitCode> {
    let id = args
        .first()
        .ok_or_else(|| anyhow::anyhow!("activate attend un page_id"))?;
    let mut l3 = open_l3()?;
    l3.activate_page(id)?;
    println!("page active : {id}");
    Ok(ExitCode::SUCCESS)
}

fn cmd_archive(args: &[String]) -> anyhow::Result<ExitCode> {
    let id = args
        .first()
        .ok_or_else(|| anyhow::anyhow!("archive attend un page_id"))?;
    let l3 = open_l3()?;
    let now = OffsetDateTime::now_utc().format(&Rfc3339)?;
    l3.archive_page(id, &now)?;
    println!("archivée");
    Ok(ExitCode::SUCCESS)
}

fn cmd_derive(args: &[String]) -> anyhow::Result<ExitCode> {
    let mut rule: Option<String> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--rule" => {
                i += 1;
                rule = Some(
                    args.get(i)
                        .ok_or_else(|| anyhow::anyhow!("--rule attend un nom"))?
                        .clone(),
                );
            }
            other => anyhow::bail!("argument inconnu: {other}"),
        }
        i += 1;
    }

    let path = cli::resolve_l0_path();
    let auth = cli::authenticate(&path)?;
    let l2 = L2Store::open(&path, auth.key())?;
    let mut l3 = L3Store::open(&path, auth.key())?;

    let rules: Vec<Box<dyn DerivationRule>> = match rule.as_deref() {
        Some("all") | None => all_rules(),
        Some("recipe-to-shopping") => vec![Box::new(RecipeToShopping)],
        Some(other) => anyhow::bail!("règle inconnue: {other}"),
    };

    let mut total = 0usize;
    for r in rules {
        let n = r.apply(&l2, &mut l3)?;
        println!("[{}] {n} lien(s) créés", r.name());
        total += n;
    }
    println!("Total : {total} lien(s)");
    Ok(ExitCode::SUCCESS)
}

fn short_id(s: &str) -> &str {
    &s[..8.min(s.len())]
}
