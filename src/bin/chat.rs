use std::io::{self, BufRead, Write};

use az::cli;
use az::l0::L0Entry;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use uuid::Uuid;

const HELP: &str = "\
commandes:
  /exit, /quit, Ctrl-D    quitter
  /session                réimprimer l'ID de session courant
  /safe <texte>           écrire l'énoncé avec sensitivity=false (visible en mode connecté)
  /help                   afficher cette aide
";

fn main() -> anyhow::Result<()> {
    let (store, _path) = cli::open_l0()?;
    let session_id = Uuid::new_v4().to_string();

    eprintln!("[az/chat] session {session_id}");
    eprintln!("[az/chat] L0: {}", store.path().display());
    eprintln!("[az/chat] tapez du texte (une ligne = un énoncé), /help pour les commandes");

    let stdin = io::stdin();
    let mut stdout = io::stdout();
    let mut line = String::new();

    loop {
        print!("> ");
        stdout.flush().ok();
        line.clear();
        let n = stdin.lock().read_line(&mut line)?;
        if n == 0 {
            println!();
            break;
        }
        let text = line.trim_end_matches(['\n', '\r']).trim();

        if text.is_empty() {
            continue;
        }
        match text {
            "/exit" | "/quit" => break,
            "/help" => {
                eprint!("{HELP}");
                continue;
            }
            "/session" => {
                eprintln!("session_id = {session_id}");
                continue;
            }
            _ => {}
        }

        // /safe <texte> → écrit avec sensitivity=false
        let (content, sensitivity) = if let Some(rest) = text.strip_prefix("/safe ") {
            let r = rest.trim();
            if r.is_empty() {
                eprintln!("[az/chat] /safe attend un texte après l'espace");
                continue;
            }
            (r.to_string(), false)
        } else if text == "/safe" {
            eprintln!("[az/chat] /safe attend un texte (ex: /safe demain il pleut)");
            continue;
        } else {
            (text.to_string(), true)
        };

        let timestamp = OffsetDateTime::now_utc()
            .format(&Rfc3339)
            .unwrap_or_else(|_| String::from("?"));
        let entry = L0Entry {
            id: Uuid::new_v4().to_string(),
            timestamp,
            content,
            source: "chat".to_string(),
            session_id: session_id.clone(),
            sensitivity,
        };
        if let Err(e) = store.append(&entry) {
            eprintln!("[az/chat] échec écriture L0: {e}");
        }
    }

    eprintln!("[az/chat] session terminée (session_id = {session_id})");
    Ok(())
}
