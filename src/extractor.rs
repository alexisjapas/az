use serde::{Deserialize, Serialize};
use thiserror::Error;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use uuid::Uuid;

use crate::l1::{Block, L1Store};
use crate::l2::{Fact, L2Store};
use crate::llm::{GenerationRequest, Llm, LlmError};
use crate::session::SessionMode;

pub const PROMPT_VERSION: &str = "v1";

pub const PROMPT_V1_SYSTEM: &str = "\
Tu es un assistant qui extrait des faits typés à partir de blocs thématiques (L1).

Pour chaque fait identifie :
- `type` : un type court en string (ex: \"note\", \"event\", \"measurement\", \"transaction\", \"recipe\"). \
La liste est ouverte — propose des types pertinents au contenu.
- `payload` : un objet JSON décrivant le fait (champs libres, adaptés au type).
- `transcript_ids` : liste des `id` de transcripts L0 qui ont nourri le fait.
- `block_id` : id du bloc L1 d'origine (parmi les blocs fournis).
- `sensitivity` : booléen, défaut `true` (conservateur).

N'invente pas de faits qui ne sont pas dans les blocs. Pas de spéculation, pas de remplissage.
Si un bloc ne contient pas de fait actionnable, n'émets rien pour ce bloc.

Réponds UNIQUEMENT en JSON conforme :
{\"facts\":[{\"type\":string,\"payload\":object,\"transcript_ids\":[string,...],\"block_id\":string,\"sensitivity\":bool},...]}";

#[derive(Debug, Error)]
pub enum ExtractError {
    #[error("aucun bloc pour la segmentation {0}")]
    EmptySegmentation(String),
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
struct BlockForPrompt<'a> {
    block_id: &'a str,
    topic: Option<&'a str>,
    content: &'a str,
    transcript_ids: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct LlmFact {
    #[serde(rename = "type")]
    fact_type: String,
    payload: serde_json::Value,
    #[serde(default)]
    transcript_ids: Vec<String>,
    #[serde(default)]
    block_id: Option<String>,
    #[serde(default = "default_true")]
    sensitivity: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Deserialize)]
struct LlmResponse {
    facts: Vec<LlmFact>,
}

/// Extrait des faits L2 depuis une segmentation L1.
///
/// - `mode = Connected` ⇒ les blocs sensibles sont exclus du prompt envoyé au LLM.
/// - Tous les faits extraits sont écrits comme **drafts** (`validated_at = NULL`).
/// - Retourne la liste des drafts insérés (ordre du LLM).
pub fn extract_from_segmentation(
    l1: &L1Store,
    l2: &mut L2Store,
    llm: &dyn Llm,
    model: &str,
    segmentation_id: &str,
    mode: SessionMode,
) -> Result<Vec<Fact>, ExtractError> {
    let blocks: Vec<Block> = l1.blocks(segmentation_id, mode.read_filter())?;
    if blocks.is_empty() {
        return Err(ExtractError::EmptySegmentation(segmentation_id.to_string()));
    }

    // Construit le prompt user : pour chaque bloc, on attache ses transcript IDs.
    let mut prompt_items: Vec<BlockForPrompt> = Vec::with_capacity(blocks.len());
    for b in &blocks {
        let transcript_ids = l1.block_sources(&b.id)?;
        prompt_items.push(BlockForPrompt {
            block_id: &b.id,
            topic: b.topic.as_deref(),
            content: &b.content,
            transcript_ids,
        });
    }
    let user_prompt = serde_json::to_string(&prompt_items)
        .map_err(|e| ExtractError::Parse(format!("sérialisation prompt: {e}")))?;

    let resp = llm.generate(GenerationRequest {
        system: PROMPT_V1_SYSTEM.to_string(),
        user: user_prompt,
        model: model.to_string(),
        temperature: 0.2,
        json_mode: true,
    })?;

    let parsed: LlmResponse = serde_json::from_str(&resp.text).map_err(|e| {
        let snippet = &resp.text[..resp.text.len().min(500)];
        ExtractError::Parse(format!("{e} :: réponse brute (tronquée): {snippet}"))
    })?;

    // Construit une map block_id → bool présent dans la segmentation filtrée.
    let known_blocks: std::collections::HashSet<&str> =
        blocks.iter().map(|b| b.id.as_str()).collect();

    let now = OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .map_err(|e| ExtractError::Time(e.to_string()))?;

    let mut inserted: Vec<Fact> = Vec::new();
    for f in parsed.facts {
        // Défensif : on n'écrit que des faits dont le block_id est connu (sinon ignore).
        // Si block_id absent, on accepte tout de même (peut être null).
        if let Some(bid) = &f.block_id
            && !known_blocks.contains(bid.as_str())
        {
            continue;
        }
        let payload_str = serde_json::to_string(&f.payload)
            .map_err(|e| ExtractError::Parse(format!("payload JSON: {e}")))?;
        let fact = Fact {
            id: Uuid::new_v4().to_string(),
            version: 1,
            fact_type: f.fact_type,
            payload: payload_str,
            block_id: f.block_id,
            sensitivity: f.sensitivity,
            created_at: now.clone(),
            validated_at: None,
        };
        l2.insert(&fact, &f.transcript_ids)?;
        inserted.push(fact);
    }

    Ok(inserted)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use crate::l0::{L0Entry, L0Store};
    use crate::l1::{Block as L1Block, L1Store, Segmentation};
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
        p.push(format!("az-extract-{}-{}.sqlite", std::process::id(), name));
        let _ = std::fs::remove_file(&p);
        let _ = std::fs::remove_file(p.with_extension("sqlite-wal"));
        let _ = std::fs::remove_file(p.with_extension("sqlite-shm"));
        p
    }

    fn seed_pipeline(path: &PathBuf) -> (String, String, String, String) {
        // Crée un transcript sensible et un non sensible, une segmentation avec 2 blocs.
        let l0 = L0Store::open(path, &db::test_key()).unwrap();
        l0.append(&L0Entry {
            id: "t_sens".into(),
            timestamp: "2026-01-01T00:00:00Z".into(),
            content: "j'ai depense 50 euros".into(),
            source: "chat".into(),
            session_id: "S".into(),
            sensitivity: true,
        })
        .unwrap();
        l0.append(&L0Entry {
            id: "t_safe".into(),
            timestamp: "2026-01-01T00:00:01Z".into(),
            content: "il pleuvra demain".into(),
            source: "chat".into(),
            session_id: "S".into(),
            sensitivity: false,
        })
        .unwrap();

        let mut l1 = L1Store::open(path, &db::test_key()).unwrap();
        let seg = Segmentation {
            id: "seg1".into(),
            created_at: "2026-05-26T10:00:00Z".into(),
            session_id: "S".into(),
            model: "m".into(),
            prompt_version: "v1".into(),
            notes: None,
        };
        let blocks = vec![
            L1Block {
                id: "b_sens".into(),
                segmentation_id: "seg1".into(),
                seq: 0,
                topic: Some("Finance".into()),
                content: "Dépense de 50 euros".into(),
                sensitivity: true,
            },
            L1Block {
                id: "b_safe".into(),
                segmentation_id: "seg1".into(),
                seq: 1,
                topic: Some("Météo".into()),
                content: "Pluie prévue".into(),
                sensitivity: false,
            },
        ];
        let sources = vec![
            ("b_sens".to_string(), "t_sens".to_string()),
            ("b_safe".to_string(), "t_safe".to_string()),
        ];
        l1.record(&seg, &blocks, &sources).unwrap();
        (
            "seg1".to_string(),
            "b_sens".to_string(),
            "b_safe".to_string(),
            "S".to_string(),
        )
    }

    #[test]
    fn extracts_from_blocks_with_mock_llm() {
        let path = tmp("extract");
        let (seg_id, _bs, _bk, _s) = seed_pipeline(&path);
        let l1 = L1Store::open(&path, &db::test_key()).unwrap();
        let mut l2 = L2Store::open(&path, &db::test_key()).unwrap();
        let mock = MockLlm {
            last: Mutex::new(None),
            response: r#"{"facts":[
                {"type":"transaction","payload":{"amount":50,"currency":"EUR"},"transcript_ids":["t_sens"],"block_id":"b_sens","sensitivity":true},
                {"type":"event","payload":{"event":"rain","when":"tomorrow"},"transcript_ids":["t_safe"],"block_id":"b_safe","sensitivity":false}
            ]}"#.into(),
        };
        let drafts =
            extract_from_segmentation(&l1, &mut l2, &mock, "m", &seg_id, SessionMode::Private)
                .unwrap();
        assert_eq!(drafts.len(), 2);
        assert!(drafts.iter().any(|f| f.fact_type == "transaction"));
        assert!(drafts.iter().any(|f| f.fact_type == "event"));

        // Vérifie qu'on est bien en draft state.
        let listed = l2.list_drafts().unwrap();
        assert_eq!(listed.len(), 2);
    }

    #[test]
    fn connected_mode_drops_sensitive_blocks_from_prompt() {
        let path = tmp("connected");
        let (seg_id, _, _, _) = seed_pipeline(&path);
        let l1 = L1Store::open(&path, &db::test_key()).unwrap();
        let mut l2 = L2Store::open(&path, &db::test_key()).unwrap();
        let mock = MockLlm {
            last: Mutex::new(None),
            response: r#"{"facts":[]}"#.into(),
        };
        extract_from_segmentation(&l1, &mut l2, &mock, "m", &seg_id, SessionMode::Connected)
            .unwrap();
        let req = mock.last.lock().unwrap().clone().unwrap();
        assert!(!req.user.contains("b_sens"));
        assert!(!req.user.contains("Finance"));
        assert!(req.user.contains("b_safe"));
        assert!(req.user.contains("Météo"));
    }

    #[test]
    fn hallucinated_block_id_is_dropped() {
        let path = tmp("hallu");
        let (seg_id, _, _, _) = seed_pipeline(&path);
        let l1 = L1Store::open(&path, &db::test_key()).unwrap();
        let mut l2 = L2Store::open(&path, &db::test_key()).unwrap();
        let mock = MockLlm {
            last: Mutex::new(None),
            response: r#"{"facts":[
                {"type":"x","payload":{},"transcript_ids":[],"block_id":"ghost-block","sensitivity":true},
                {"type":"y","payload":{},"transcript_ids":[],"block_id":"b_safe","sensitivity":false}
            ]}"#.into(),
        };
        let drafts =
            extract_from_segmentation(&l1, &mut l2, &mock, "m", &seg_id, SessionMode::Private)
                .unwrap();
        assert_eq!(drafts.len(), 1);
        assert_eq!(drafts[0].fact_type, "y");
    }

    #[test]
    fn empty_segmentation_errors() {
        let path = tmp("empty");
        let _ = L0Store::open(&path, &db::test_key()).unwrap();
        let l1 = L1Store::open(&path, &db::test_key()).unwrap();
        let mut l2 = L2Store::open(&path, &db::test_key()).unwrap();
        let mock = MockLlm {
            last: Mutex::new(None),
            response: r#"{"facts":[]}"#.into(),
        };
        let err = extract_from_segmentation(
            &l1,
            &mut l2,
            &mock,
            "m",
            "missing-seg",
            SessionMode::Private,
        )
        .unwrap_err();
        assert!(matches!(err, ExtractError::EmptySegmentation(_)));
    }
}
