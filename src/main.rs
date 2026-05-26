use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::{Context, Result};
use az::audio::AudioCapture;
use az::cli;
use az::l0::L0Entry;
use az::stt::{SpeechToText, whisper::WhisperStt};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use uuid::Uuid;

fn main() -> Result<()> {
    let model_path = std::env::var("AZ_WHISPER_MODEL")
        .context("AZ_WHISPER_MODEL doit pointer vers un modèle ggml-*.bin")?;
    let lang = std::env::var("AZ_WHISPER_LANG").unwrap_or_else(|_| "auto".to_string());

    eprintln!("[az] chargement du modèle whisper: {model_path}");
    let mut stt = WhisperStt::load(&model_path, &lang)?;

    let (store, _path) = cli::open_l0()?;
    let session_id = Uuid::new_v4().to_string();
    eprintln!("[az] session {session_id}");
    eprintln!("[az] L0: {}", store.path().display());

    let capture = AudioCapture::start()?;
    eprintln!(
        "[az] entrée audio: {} Hz, {} canal(aux) → ré-échantillonné en 16 kHz mono",
        capture.input_sample_rate(),
        capture.input_channels()
    );
    let utterances = capture.utterances();

    let running = Arc::new(AtomicBool::new(true));
    {
        let r = running.clone();
        ctrlc::set_handler(move || {
            eprintln!("\n[az] arrêt demandé…");
            r.store(false, Ordering::SeqCst);
        })?;
    }

    let debug = std::env::var("AZ_DEBUG")
        .ok()
        .is_some_and(|v| v != "0" && !v.is_empty());
    eprintln!(
        "[az] capture démarrée — parlez (Ctrl-C pour arrêter){}",
        if debug { " [debug=on]" } else { "" }
    );

    while running.load(Ordering::SeqCst) {
        match utterances.recv_timeout(Duration::from_millis(200)) {
            Ok(samples) => {
                let dur_s = samples.len() as f32 / 16_000.0;
                if debug {
                    eprintln!(
                        "[az] énoncé détecté: {} échantillons (~{:.2}s) → whisper",
                        samples.len(),
                        dur_s
                    );
                }
                let t0 = std::time::Instant::now();
                match stt.transcribe(&samples) {
                    Ok(t) => {
                        if debug {
                            eprintln!(
                                "[az] whisper OK en {:.2}s, texte=\"{}\"",
                                t0.elapsed().as_secs_f32(),
                                t.text
                            );
                        }
                        let text = t.text.trim();
                        if text.is_empty() {
                            if debug {
                                eprintln!(
                                    "[az] (transcription vide — souvent bruit / phrase trop courte)"
                                );
                            }
                            continue;
                        }
                        println!("> {text}");
                        let timestamp = OffsetDateTime::now_utc()
                            .format(&Rfc3339)
                            .unwrap_or_else(|_| String::from("?"));
                        let entry = L0Entry {
                            id: Uuid::new_v4().to_string(),
                            timestamp,
                            content: text.to_string(),
                            source: "voice".to_string(),
                            session_id: session_id.clone(),
                            sensitivity: true,
                        };
                        if let Err(e) = store.append(&entry) {
                            eprintln!("[az] échec écriture L0: {e}");
                        }
                    }
                    Err(e) => eprintln!("[az] erreur transcription: {e}"),
                }
            }
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => continue,
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
        }
    }

    eprintln!("[az] session terminée");
    Ok(())
}
