use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use uuid::Uuid;

use crate::l0::L0Store;
use crate::l1::{Block, L1Store, Segmentation};
use crate::llm::{GenerationRequest, Llm, LlmError};
use crate::session::SessionMode;

pub const PROMPT_VERSION: &str = "v1";

pub const PROMPT_V1_SYSTEM: &str = "\
Tu es un assistant qui segmente des transcripts en blocs thématiques cohérents.

Pour chaque bloc :
- identifie un sujet court (topic, ≤ 60 caractères) ;
- liste les `id` des transcripts qui le composent dans l'ordre chronologique ;
- écris un `content` paraphrasé propre, en français, fidèle aux sources.

Réponds UNIQUEMENT avec un JSON conforme au schéma :
{\"blocks\":[{\"topic\":string,\"transcript_ids\":[string,...],\"content\":string},...]}

Sources fournies dans le prochain message sous forme JSON : [{\"id\":string,\"timestamp\":string,\"content\":string},...].";

#[derive(Debug, Error)]
pub enum SegmentError {
    #[error("aucun transcript pour la session {0}")]
    EmptySession(String),
    #[error("llm: {0}")]
    Llm(#[from] LlmError),
    #[error("parsing réponse LLM: {0}")]
    Parse(String),
    #[error("db: {0}")]
    Db(#[from] crate::db::DbError),
    #[error("temps: {0}")]
    Time(String),
}

#[derive(Debug, Serialize)]
struct TranscriptForPrompt<'a> {
    id: &'a str,
    timestamp: &'a str,
    content: &'a str,
}

#[derive(Debug, Deserialize)]
struct LlmBlock {
    topic: String,
    transcript_ids: Vec<String>,
    content: String,
}

#[derive(Debug, Deserialize)]
struct LlmResponse {
    blocks: Vec<LlmBlock>,
}

pub fn segment_session(
    l0: &L0Store,
    l1: &mut L1Store,
    llm: &dyn Llm,
    model: &str,
    session_id: &str,
    mode: SessionMode,
) -> Result<Segmentation, SegmentError> {
    let transcripts = l0.list_session(session_id, mode.read_filter())?;
    if transcripts.is_empty() {
        return Err(SegmentError::EmptySession(session_id.to_string()));
    }

    let prompt_items: Vec<_> = transcripts
        .iter()
        .map(|t| TranscriptForPrompt {
            id: &t.id,
            timestamp: &t.timestamp,
            content: &t.content,
        })
        .collect();
    let user_prompt = serde_json::to_string(&prompt_items)
        .map_err(|e| SegmentError::Parse(format!("sérialisation prompt: {e}")))?;

    let resp = llm.generate(GenerationRequest {
        system: PROMPT_V1_SYSTEM.to_string(),
        user: user_prompt,
        model: model.to_string(),
        temperature: 0.2,
        json_mode: true,
    })?;

    let parsed: LlmResponse = serde_json::from_str(&resp.text).map_err(|e| {
        let snippet = &resp.text[..resp.text.len().min(500)];
        SegmentError::Parse(format!("{e} :: réponse brute (tronquée): {snippet}"))
    })?;

    // Map id → transcript pour : (1) check anti-hallucination, (2) hériter de sensitivity.
    let tx_by_id: HashMap<&str, &crate::l0::L0Entry> =
        transcripts.iter().map(|t| (t.id.as_str(), t)).collect();

    let segmentation_id = Uuid::new_v4().to_string();
    let created_at = OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .map_err(|e| SegmentError::Time(e.to_string()))?;

    let seg = Segmentation {
        id: segmentation_id.clone(),
        created_at,
        session_id: session_id.to_string(),
        model: model.to_string(),
        prompt_version: PROMPT_VERSION.to_string(),
        notes: None,
    };

    let mut blocks: Vec<Block> = Vec::with_capacity(parsed.blocks.len());
    let mut sources: Vec<(String, String)> = Vec::new();

    for (seq, b) in parsed.blocks.into_iter().enumerate() {
        let block_id = Uuid::new_v4().to_string();

        // Hérite sensitivity = OR(sources.sensitivity).
        // Si AUCUN ID source n'est connu (LLM a tout halluciné), défaut conservateur = true.
        let known_sources: Vec<&crate::l0::L0Entry> = b
            .transcript_ids
            .iter()
            .filter_map(|tid| tx_by_id.get(tid.as_str()).copied())
            .collect();
        let sensitivity = if known_sources.is_empty() {
            true
        } else {
            known_sources.iter().any(|t| t.sensitivity)
        };

        blocks.push(Block {
            id: block_id.clone(),
            segmentation_id: segmentation_id.clone(),
            seq: seq as i64,
            topic: Some(b.topic),
            content: b.content,
            sensitivity,
        });
        for tid in b.transcript_ids {
            // Défensif : si le LLM hallucine un ID inconnu, on l'écarte plutôt
            // que de faire échouer la FK contrainte côté DB.
            if tx_by_id.contains_key(tid.as_str()) {
                sources.push((block_id.clone(), tid));
            }
        }
    }

    l1.record(&seg, &blocks, &sources)?;
    Ok(seg)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use crate::l0::{L0Entry, L0Store};
    use crate::l1::L1Store;
    use crate::llm::GenerationResponse;
    use std::path::PathBuf;
    use std::sync::Mutex;

    struct MockLlm {
        last: Mutex<Option<GenerationRequest>>,
        response: String,
    }

    impl Llm for MockLlm {
        fn generate(&self, req: GenerationRequest) -> Result<GenerationResponse, LlmError> {
            *self.last.lock().unwrap() = Some(req);
            Ok(GenerationResponse {
                text: self.response.clone(),
            })
        }
    }

    fn tmp(name: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("az-seg-{}-{}.sqlite", std::process::id(), name));
        let _ = std::fs::remove_file(&p);
        let _ = std::fs::remove_file(p.with_extension("sqlite-wal"));
        let _ = std::fs::remove_file(p.with_extension("sqlite-shm"));
        p
    }

    fn seed(l0: &L0Store, session: &str, items: &[(&str, &str)]) {
        for (id, content) in items {
            l0.append(&L0Entry {
                id: (*id).into(),
                timestamp: "2026-01-01T00:00:00Z".into(),
                content: (*content).into(),
                source: "chat".into(),
                session_id: session.into(),
                sensitivity: true,
            })
            .unwrap();
        }
    }

    fn seed_mixed(l0: &L0Store, session: &str) {
        l0.append(&L0Entry {
            id: "sens".into(),
            timestamp: "2026-01-01T00:00:00Z".into(),
            content: "j'ai depense 50 euros".into(),
            source: "chat".into(),
            session_id: session.into(),
            sensitivity: true,
        })
        .unwrap();
        l0.append(&L0Entry {
            id: "safe".into(),
            timestamp: "2026-01-01T00:00:01Z".into(),
            content: "il fera beau demain".into(),
            source: "chat".into(),
            session_id: session.into(),
            sensitivity: false,
        })
        .unwrap();
    }

    #[test]
    fn segments_and_writes_l1() {
        let path = tmp("ok");
        let l0 = L0Store::open(&path, &db::test_key()).unwrap();
        seed(
            &l0,
            "S",
            &[
                ("t1", "j'ai mange une pomme"),
                ("t2", "elle etait tres bonne"),
                ("t3", "demain il fera beau"),
            ],
        );

        let mut l1 = L1Store::open(&path, &db::test_key()).unwrap();
        let mock = MockLlm {
            last: Mutex::new(None),
            response: r#"{"blocks":[
                {"topic":"Pomme","transcript_ids":["t1","t2"],"content":"L'utilisateur décrit une pomme savoureuse."},
                {"topic":"Météo","transcript_ids":["t3"],"content":"Prévision pour demain."}
            ]}"#
            .into(),
        };
        let seg =
            segment_session(&l0, &mut l1, &mock, "test-model", "S", SessionMode::Private).unwrap();

        // Le request envoyé au LLM doit avoir json_mode=true et temperature basse.
        let req = mock.last.lock().unwrap().clone().unwrap();
        assert!(req.json_mode);
        assert!(req.temperature < 0.5);
        assert_eq!(req.model, "test-model");

        let blocks = l1.blocks(&seg.id, crate::session::ReadFilter::All).unwrap();
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].seq, 0);
        assert_eq!(blocks[0].topic.as_deref(), Some("Pomme"));
        assert_eq!(blocks[1].topic.as_deref(), Some("Météo"));

        let s1 = l1.block_sources(&blocks[0].id).unwrap();
        assert!(s1.contains(&"t1".to_string()) && s1.contains(&"t2".to_string()));
        let s2 = l1.block_sources(&blocks[1].id).unwrap();
        assert_eq!(s2, vec!["t3".to_string()]);
    }

    #[test]
    fn hallucinated_ids_are_dropped() {
        let path = tmp("hallu");
        let l0 = L0Store::open(&path, &db::test_key()).unwrap();
        seed(&l0, "S", &[("real", "vrai contenu")]);
        let mut l1 = L1Store::open(&path, &db::test_key()).unwrap();
        let mock = MockLlm {
            last: Mutex::new(None),
            response:
                r#"{"blocks":[{"topic":"X","transcript_ids":["real","ghost"],"content":"x"}]}"#
                    .into(),
        };
        let seg = segment_session(&l0, &mut l1, &mock, "m", "S", SessionMode::Private).unwrap();
        let blocks = l1.blocks(&seg.id, crate::session::ReadFilter::All).unwrap();
        let srcs = l1.block_sources(&blocks[0].id).unwrap();
        assert_eq!(srcs, vec!["real".to_string()]);
    }

    #[test]
    fn empty_session_returns_error() {
        let path = tmp("empty");
        let l0 = L0Store::open(&path, &db::test_key()).unwrap();
        let mut l1 = L1Store::open(&path, &db::test_key()).unwrap();
        let mock = MockLlm {
            last: Mutex::new(None),
            response: "{}".into(),
        };
        let err =
            segment_session(&l0, &mut l1, &mock, "m", "missing", SessionMode::Private).unwrap_err();
        assert!(matches!(err, SegmentError::EmptySession(_)));
    }

    #[test]
    fn connected_mode_drops_sensitive_transcripts() {
        let path = tmp("connected");
        let l0 = L0Store::open(&path, &db::test_key()).unwrap();
        seed_mixed(&l0, "S");
        let mut l1 = L1Store::open(&path, &db::test_key()).unwrap();
        let mock = MockLlm {
            last: Mutex::new(None),
            response: r#"{"blocks":[{"topic":"Météo","transcript_ids":["safe"],"content":"x"}]}"#
                .into(),
        };
        segment_session(&l0, &mut l1, &mock, "m", "S", SessionMode::Connected).unwrap();
        let req = mock.last.lock().unwrap().clone().unwrap();
        // Le prompt user ne doit PAS contenir l'ID sensible ni son contenu.
        assert!(
            !req.user.contains("\"sens\""),
            "ID sensible présent dans le prompt: {}",
            req.user
        );
        assert!(
            !req.user.contains("50 euros"),
            "contenu sensible présent: {}",
            req.user
        );
        // Le transcript non sensible doit y être.
        assert!(req.user.contains("\"safe\""));
    }

    #[test]
    fn private_mode_keeps_sensitive_transcripts() {
        let path = tmp("private");
        let l0 = L0Store::open(&path, &db::test_key()).unwrap();
        seed_mixed(&l0, "S");
        let mut l1 = L1Store::open(&path, &db::test_key()).unwrap();
        let mock = MockLlm {
            last: Mutex::new(None),
            response:
                r#"{"blocks":[{"topic":"X","transcript_ids":["safe","sens"],"content":"x"}]}"#
                    .into(),
        };
        segment_session(&l0, &mut l1, &mock, "m", "S", SessionMode::Private).unwrap();
        let req = mock.last.lock().unwrap().clone().unwrap();
        assert!(req.user.contains("\"sens\""));
        assert!(req.user.contains("50 euros"));
    }

    #[test]
    fn block_inherits_sensitivity_from_sources() {
        let path = tmp("inherit");
        let l0 = L0Store::open(&path, &db::test_key()).unwrap();
        seed_mixed(&l0, "S");
        let mut l1 = L1Store::open(&path, &db::test_key()).unwrap();
        let mock = MockLlm {
            last: Mutex::new(None),
            response: r#"{"blocks":[
                {"topic":"A","transcript_ids":["safe"],"content":"x"},
                {"topic":"B","transcript_ids":["sens"],"content":"y"},
                {"topic":"C","transcript_ids":["safe","sens"],"content":"z"}
            ]}"#
            .into(),
        };
        let seg = segment_session(&l0, &mut l1, &mock, "m", "S", SessionMode::Private).unwrap();
        let blocks = l1.blocks(&seg.id, crate::session::ReadFilter::All).unwrap();
        assert_eq!(blocks.len(), 3);
        assert!(
            !blocks[0].sensitivity,
            "bloc issu uniquement de 'safe' devrait être non sensible"
        );
        assert!(
            blocks[1].sensitivity,
            "bloc issu de 'sens' doit rester sensible"
        );
        assert!(
            blocks[2].sensitivity,
            "bloc mixte (safe + sens) doit être sensible (OR conservateur)"
        );
    }

    #[test]
    fn block_with_only_unknown_sources_defaults_sensitive() {
        let path = tmp("unknown");
        let l0 = L0Store::open(&path, &db::test_key()).unwrap();
        // Seed UN transcript safe mais le LLM ne référence que des IDs inconnus.
        l0.append(&L0Entry {
            id: "real".into(),
            timestamp: "2026-01-01T00:00:00Z".into(),
            content: "x".into(),
            source: "chat".into(),
            session_id: "S".into(),
            sensitivity: false,
        })
        .unwrap();
        let mut l1 = L1Store::open(&path, &db::test_key()).unwrap();
        let mock = MockLlm {
            last: Mutex::new(None),
            response:
                r#"{"blocks":[{"topic":"X","transcript_ids":["ghost1","ghost2"],"content":"z"}]}"#
                    .into(),
        };
        let seg = segment_session(&l0, &mut l1, &mock, "m", "S", SessionMode::Private).unwrap();
        let blocks = l1.blocks(&seg.id, crate::session::ReadFilter::All).unwrap();
        assert_eq!(blocks.len(), 1);
        assert!(
            blocks[0].sensitivity,
            "défaut conservateur quand toutes les sources sont hallucinées"
        );
    }

    #[test]
    fn invalid_json_returns_parse_error() {
        let path = tmp("parse");
        let l0 = L0Store::open(&path, &db::test_key()).unwrap();
        seed(&l0, "S", &[("t1", "x")]);
        let mut l1 = L1Store::open(&path, &db::test_key()).unwrap();
        let mock = MockLlm {
            last: Mutex::new(None),
            response: "pas du JSON".into(),
        };
        let err = segment_session(&l0, &mut l1, &mock, "m", "S", SessionMode::Private).unwrap_err();
        assert!(matches!(err, SegmentError::Parse(_)));
    }
}
