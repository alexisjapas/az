//! Backend Tauri pour AZ. Expose des commandes typées vers le frontend SolidJS.
//!
//! État partagé : la clé chiffrée dérivée du mot de passe est gardée en mémoire
//! sous `Mutex<Option<[u8; 32]>>`. Le mode session (private/connected) est lui
//! aussi global. Chaque commande qui touche la DB ouvre sa propre connexion via
//! les stores `L0Store`, `L1Store`, etc. (cohérent avec le pattern CLI).

use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use az::auth::{self, KEY_SIZE, SALT_SIZE};
use az::audio::AudioCapture;
use az::db;
use az::embeddings::{EmbeddingsStore, SearchHit, TARGET_BLOCK, TARGET_TRANSCRIPT};
use az::extractor::extract_from_segmentation;
use az::l0::{L0Entry, L0Store};
use az::l1::{Block, L1Store, Segmentation};
use az::l2::{Fact, L2Store};
use az::l3::{L3Store, Link, Page};
use az::llm::EmbeddingsLlm;
use az::llm::ollama::OllamaClient;
use az::segmenter::segment_session;
use az::session::SessionMode;
use az::stt::SpeechToText;
use az::stt::whisper::WhisperStt;
use crossbeam_channel::RecvTimeoutError;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

const DEFAULT_EMBED_MODEL_ENV: &str = "AZ_EMBED_MODEL";
const DEFAULT_EMBED_MODEL: &str = "nomic-embed-text";
const DEFAULT_LLM_MODEL_ENV: &str = "AZ_LLM_MODEL";
const DEFAULT_LLM_MODEL: &str = "gemma4:e2b";

/// Chemin par défaut de la base L0.
///
/// Ordre de résolution :
/// 1. `AZ_L0_PATH` (env, identique aux binaires CLI — utile pour partager la
///    même base entre UI et CLI : `export AZ_L0_PATH=$HOME/Dev/az/data/l0.sqlite`).
/// 2. `$HOME/.local/share/az/l0.sqlite` — emplacement standard XDG, **absolu**
///    pour ne pas dépendre du CWD du binaire Tauri (qui change selon le mode
///    de lancement et ferait apparaître la DB dans `ui/data/`, où le watcher
///    Vite la verrait muter et reloaderait l'app).
/// 3. Fallback relatif `data/l0.sqlite` si aucun `HOME` (rare).
fn default_db_path() -> PathBuf {
    if let Ok(p) = std::env::var("AZ_L0_PATH") {
        return PathBuf::from(p);
    }
    if let Ok(home) = std::env::var("HOME") {
        return PathBuf::from(home)
            .join(".local")
            .join("share")
            .join("az")
            .join("l0.sqlite");
    }
    PathBuf::from("data/l0.sqlite")
}

fn embed_model() -> String {
    std::env::var(DEFAULT_EMBED_MODEL_ENV).unwrap_or_else(|_| DEFAULT_EMBED_MODEL.to_string())
}

fn llm_model() -> String {
    std::env::var(DEFAULT_LLM_MODEL_ENV).unwrap_or_else(|_| DEFAULT_LLM_MODEL.to_string())
}

/// Handle vers une session de capture vocale en cours. Le thread worker possède
/// l'`AudioCapture` (cpal::Stream n'est pas Send sur Linux/ALSA), donc on ne
/// stocke ici que le drapeau d'arrêt et le JoinHandle.
struct RecordingHandle {
    stop_flag: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

/// État applicatif partagé entre toutes les commandes Tauri.
struct AppState {
    db_path: PathBuf,
    key: Mutex<Option<[u8; KEY_SIZE]>>,
    mode: Mutex<SessionMode>,
    recording: Mutex<Option<RecordingHandle>>,
}

impl AppState {
    fn new() -> Self {
        Self {
            db_path: default_db_path(),
            key: Mutex::new(None),
            mode: Mutex::new(SessionMode::DEFAULT),
            recording: Mutex::new(None),
        }
    }

    fn is_unlocked(&self) -> bool {
        self.key.lock().expect("mutex empoisonné").is_some()
    }

    fn current_mode(&self) -> SessionMode {
        *self.mode.lock().expect("mutex empoisonné")
    }

    /// Récupère la clé courante. Renvoie une erreur si verrouillé.
    fn require_key(&self) -> Result<[u8; KEY_SIZE], String> {
        self.key
            .lock()
            .expect("mutex empoisonné")
            .ok_or_else(|| "base verrouillée".to_string())
    }
}

fn map_db_err(e: db::DbError) -> String {
    match e {
        db::DbError::WrongKey => "mot de passe invalide".into(),
        other => other.to_string(),
    }
}

#[derive(Serialize)]
struct AppInfo {
    name: &'static str,
    version: &'static str,
    db_path: String,
    db_exists: bool,
    salt_exists: bool,
    unlocked: bool,
    mode: &'static str,
}

#[derive(Serialize)]
struct AuthStatus {
    db_path: String,
    db_exists: bool,
    salt_exists: bool,
    unlocked: bool,
}

#[derive(Serialize)]
struct Summary {
    transcripts: u64,
    segmentations: u64,
    facts_total: u64,
    facts_drafts: u64,
    embeddings: u64,
    pages: u64,
    links: u64,
}

#[derive(Serialize)]
struct SessionInfo {
    id: String,
    transcripts: u64,
    first_at: String,
    last_at: String,
}

#[derive(Serialize)]
struct SearchHitDto {
    target_type: String,
    target_id: String,
    score: f32,
    content: String,
    sensitivity: bool,
}

impl From<SearchHit> for SearchHitDto {
    fn from(h: SearchHit) -> Self {
        Self {
            target_type: h.target_type,
            target_id: h.target_id,
            score: h.score,
            content: h.content,
            sensitivity: h.sensitivity,
        }
    }
}

// ------------------- Commandes auth + info -------------------

#[tauri::command]
fn app_info(state: State<'_, AppState>) -> AppInfo {
    let path = &state.db_path;
    AppInfo {
        name: env!("CARGO_PKG_NAME"),
        version: env!("CARGO_PKG_VERSION"),
        db_path: path.to_string_lossy().to_string(),
        db_exists: path.exists(),
        salt_exists: auth::salt_path(path).exists(),
        unlocked: state.is_unlocked(),
        mode: state.current_mode().as_str(),
    }
}

#[tauri::command]
fn auth_status(state: State<'_, AppState>) -> AuthStatus {
    let path = &state.db_path;
    AuthStatus {
        db_path: path.to_string_lossy().to_string(),
        db_exists: path.exists(),
        salt_exists: auth::salt_path(path).exists(),
        unlocked: state.is_unlocked(),
    }
}

#[tauri::command]
fn auth_unlock(password: String, state: State<'_, AppState>) -> Result<(), String> {
    if password.is_empty() {
        return Err("mot de passe vide".into());
    }
    let path = &state.db_path;
    let salt_path = auth::salt_path(path);
    if !salt_path.exists() {
        return Err(format!(
            "aucune base existante à {} — utiliser auth_create pour en créer une",
            path.display()
        ));
    }
    let salt_bytes = fs::read(&salt_path).map_err(|e| format!("lecture salt: {e}"))?;
    if salt_bytes.len() != SALT_SIZE {
        return Err(format!(
            "fichier salt corrompu (attendu {SALT_SIZE} octets, lu {})",
            salt_bytes.len()
        ));
    }
    let key = auth::derive_key(&password, &salt_bytes).map_err(|e| e.to_string())?;
    let _conn = db::open(path, &key).map_err(map_db_err)?;
    *state.key.lock().expect("mutex empoisonné") = Some(key);
    Ok(())
}

#[tauri::command]
fn auth_create(
    password: String,
    confirm: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    if password.is_empty() {
        return Err("mot de passe vide".into());
    }
    if password != confirm {
        return Err("les deux mots de passe ne correspondent pas".into());
    }
    let path = &state.db_path;
    let salt_path = auth::salt_path(path);
    if salt_path.exists() {
        return Err(format!(
            "une base existe déjà à {} — utiliser auth_unlock pour la déverrouiller",
            path.display()
        ));
    }
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent).map_err(|e| format!("création du dossier: {e}"))?;
    }
    use rand::RngCore;
    let mut salt = [0u8; SALT_SIZE];
    rand::thread_rng().fill_bytes(&mut salt);
    fs::write(&salt_path, salt).map_err(|e| format!("écriture salt: {e}"))?;
    let key = auth::derive_key(&password, &salt).map_err(|e| e.to_string())?;
    let _conn = db::open(path, &key).map_err(|e| e.to_string())?;
    *state.key.lock().expect("mutex empoisonné") = Some(key);
    Ok(())
}

#[tauri::command]
fn auth_lock(state: State<'_, AppState>) {
    *state.key.lock().expect("mutex empoisonné") = None;
}

// ------------------- Commandes mode session -------------------

#[tauri::command]
fn session_get_mode(state: State<'_, AppState>) -> &'static str {
    state.current_mode().as_str()
}

#[tauri::command]
fn session_set_mode(mode: String, state: State<'_, AppState>) -> Result<(), String> {
    let m = SessionMode::parse(&mode).map_err(|e| e.to_string())?;
    *state.mode.lock().expect("mutex empoisonné") = m;
    Ok(())
}

// ------------------- Commandes lecture -------------------

#[tauri::command]
fn summary(state: State<'_, AppState>) -> Result<Summary, String> {
    let key = state.require_key()?;
    let path = &state.db_path;
    let conn = db::open(path, &key).map_err(map_db_err)?;
    let q = |sql: &str| -> Result<u64, String> {
        conn.query_row(sql, [], |r| r.get::<_, i64>(0))
            .map(|n| n as u64)
            .map_err(|e| e.to_string())
    };
    Ok(Summary {
        transcripts: q("SELECT count(*) FROM transcripts")?,
        segmentations: q("SELECT count(*) FROM l1_segmentations")?,
        facts_total: q("SELECT count(*) FROM l2_facts_current")?,
        facts_drafts: q("SELECT count(*) FROM l2_facts WHERE validated_at IS NULL")?,
        embeddings: q("SELECT count(*) FROM embeddings")?,
        pages: q("SELECT count(*) FROM l3_pages")?,
        links: q("SELECT count(*) FROM l3_links")?,
    })
}

#[tauri::command]
fn list_sessions(state: State<'_, AppState>) -> Result<Vec<SessionInfo>, String> {
    let key = state.require_key()?;
    let path = &state.db_path;
    let conn = db::open(path, &key).map_err(map_db_err)?;
    let mut stmt = conn
        .prepare(
            "SELECT session_id, count(*), min(timestamp), max(timestamp) \
             FROM transcripts GROUP BY session_id ORDER BY max(timestamp) DESC",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| {
            Ok(SessionInfo {
                id: r.get::<_, String>(0)?,
                transcripts: r.get::<_, i64>(1)? as u64,
                first_at: r.get::<_, String>(2)?,
                last_at: r.get::<_, String>(3)?,
            })
        })
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(|e| e.to_string())?);
    }
    Ok(out)
}

#[tauri::command]
fn list_transcripts(
    session_id: Option<String>,
    limit: Option<usize>,
    state: State<'_, AppState>,
) -> Result<Vec<L0Entry>, String> {
    let key = state.require_key()?;
    let store = L0Store::open(&state.db_path, &key).map_err(map_db_err)?;
    let filter = state.current_mode().read_filter();
    let entries = match session_id {
        Some(sid) => store
            .list_session(&sid, filter)
            .map_err(|e| e.to_string())?,
        None => store.all_entries().map_err(|e| e.to_string())?,
    };
    let entries = match filter {
        az::session::ReadFilter::All => entries,
        az::session::ReadFilter::ExcludeSensitive => {
            entries.into_iter().filter(|e| !e.sensitivity).collect()
        }
    };
    let entries = match limit {
        Some(n) => entries.into_iter().take(n).collect(),
        None => entries,
    };
    Ok(entries)
}

#[tauri::command]
fn list_segmentations(
    session_id: Option<String>,
    state: State<'_, AppState>,
) -> Result<Vec<Segmentation>, String> {
    let key = state.require_key()?;
    let store = L1Store::open(&state.db_path, &key).map_err(map_db_err)?;
    match session_id {
        Some(sid) => store.list_segmentations(&sid).map_err(|e| e.to_string()),
        None => store.all_segmentations().map_err(|e| e.to_string()),
    }
}

#[tauri::command]
fn list_blocks(segmentation_id: String, state: State<'_, AppState>) -> Result<Vec<Block>, String> {
    let key = state.require_key()?;
    let store = L1Store::open(&state.db_path, &key).map_err(map_db_err)?;
    let filter = state.current_mode().read_filter();
    store
        .blocks(&segmentation_id, filter)
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn list_facts(
    fact_type: Option<String>,
    validated_only: Option<bool>,
    state: State<'_, AppState>,
) -> Result<Vec<Fact>, String> {
    let key = state.require_key()?;
    let store = L2Store::open(&state.db_path, &key).map_err(map_db_err)?;
    let filter = state.current_mode().read_filter();
    let facts = match (fact_type, validated_only.unwrap_or(false)) {
        (Some(t), true) => store
            .list_by_type(&t, filter)
            .map_err(|e| e.to_string())?
            .into_iter()
            .filter(|f| f.validated_at.is_some())
            .collect(),
        (Some(t), false) => store.list_by_type(&t, filter).map_err(|e| e.to_string())?,
        (None, true) => store
            .list_validated_current(filter)
            .map_err(|e| e.to_string())?,
        (None, false) => store.list_current(filter).map_err(|e| e.to_string())?,
    };
    Ok(facts)
}

#[tauri::command]
fn list_drafts(state: State<'_, AppState>) -> Result<Vec<Fact>, String> {
    let key = state.require_key()?;
    let store = L2Store::open(&state.db_path, &key).map_err(map_db_err)?;
    store.list_drafts().map_err(|e| e.to_string())
}

#[tauri::command]
fn list_pages(state: State<'_, AppState>) -> Result<Vec<Page>, String> {
    let key = state.require_key()?;
    let store = L3Store::open(&state.db_path, &key).map_err(map_db_err)?;
    store.list_pages().map_err(|e| e.to_string())
}

#[tauri::command]
fn list_links(
    src_kind: Option<String>,
    src_id: Option<String>,
    dst_kind: Option<String>,
    dst_id: Option<String>,
    state: State<'_, AppState>,
) -> Result<Vec<Link>, String> {
    let key = state.require_key()?;
    let store = L3Store::open(&state.db_path, &key).map_err(map_db_err)?;
    match (src_kind, src_id, dst_kind, dst_id) {
        (Some(k), Some(i), _, _) => store.list_outgoing(&k, &i).map_err(|e| e.to_string()),
        (_, _, Some(k), Some(i)) => store.list_incoming(&k, &i).map_err(|e| e.to_string()),
        _ => Err("préciser au moins src_kind+src_id ou dst_kind+dst_id".into()),
    }
}

// ------------------- Validation L2 -------------------

fn now_rfc3339() -> String {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_else(|_| "?".into())
}

/// Retourne les transcripts L0 qui ont alimenté ce fait (jointure
/// `l2_fact_sources` × `transcripts`). Utilisé par la vue de validation pour
/// donner le contexte avant d'approuver.
#[tauri::command]
fn fact_sources(
    id: String,
    version: i64,
    state: State<'_, AppState>,
) -> Result<Vec<L0Entry>, String> {
    let key = state.require_key()?;
    let conn = db::open(&state.db_path, &key).map_err(map_db_err)?;
    let mut stmt = conn
        .prepare(
            "SELECT t.id, t.timestamp, t.content, t.source, t.session_id, t.sensitivity \
             FROM l2_fact_sources s \
             JOIN transcripts t ON t.id = s.transcript_id \
             WHERE s.fact_id = ?1 AND s.version = ?2 \
             ORDER BY t.timestamp ASC",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(rusqlite::params![id, version], |r| {
            Ok(L0Entry {
                id: r.get(0)?,
                timestamp: r.get(1)?,
                content: r.get(2)?,
                source: r.get(3)?,
                session_id: r.get(4)?,
                sensitivity: r.get::<_, i64>(5)? != 0,
            })
        })
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(|e| e.to_string())?);
    }
    Ok(out)
}

#[tauri::command]
fn fact_validate(id: String, version: i64, state: State<'_, AppState>) -> Result<(), String> {
    let key = state.require_key()?;
    let store = L2Store::open(&state.db_path, &key).map_err(map_db_err)?;
    store
        .validate(&id, version, &now_rfc3339())
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn fact_update_and_validate(
    id: String,
    version: i64,
    payload: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    // Vérifie que le payload est du JSON valide avant d'écrire.
    serde_json::from_str::<serde_json::Value>(&payload)
        .map_err(|e| format!("payload JSON invalide: {e}"))?;
    let key = state.require_key()?;
    let store = L2Store::open(&state.db_path, &key).map_err(map_db_err)?;
    store
        .update_payload_and_validate(&id, version, &payload, &now_rfc3339())
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn fact_reject(id: String, version: i64, state: State<'_, AppState>) -> Result<(), String> {
    let key = state.require_key()?;
    let store = L2Store::open(&state.db_path, &key).map_err(map_db_err)?;
    store.delete(&id, version).map_err(|e| e.to_string())
}

// ------------------- Capture -------------------

/// Génère un identifiant de session frais. Pas d'écriture DB ici — la session
/// matérialise seulement quand un premier transcript y est rattaché.
#[tauri::command]
fn session_new() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// Ajoute un transcript texte (source `chat`) à la session indiquée. Renvoie
/// l'entrée écrite (avec id et timestamp générés côté backend).
#[tauri::command]
fn transcript_append(
    session_id: String,
    content: String,
    sensitive: bool,
    state: State<'_, AppState>,
) -> Result<L0Entry, String> {
    let content = content.trim().to_string();
    if content.is_empty() {
        return Err("contenu vide".into());
    }
    if session_id.trim().is_empty() {
        return Err("session_id manquant".into());
    }
    let key = state.require_key()?;
    let store = L0Store::open(&state.db_path, &key).map_err(map_db_err)?;
    let timestamp = time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_else(|_| "?".into());
    let entry = L0Entry {
        id: uuid::Uuid::new_v4().to_string(),
        timestamp,
        content,
        source: "chat".into(),
        session_id,
        sensitivity: sensitive,
    };
    store.append(&entry).map_err(|e| e.to_string())?;
    Ok(entry)
}

// ------------------- Recherche -------------------

#[tauri::command]
fn search_plain(
    query: String,
    limit: Option<usize>,
    state: State<'_, AppState>,
) -> Result<Vec<L0Entry>, String> {
    let key = state.require_key()?;
    let store = L0Store::open(&state.db_path, &key).map_err(map_db_err)?;
    let limit = limit.unwrap_or(20);
    let filter = state.current_mode().read_filter();
    let entries = store.search(&query, limit).map_err(|e| e.to_string())?;
    let entries = match filter {
        az::session::ReadFilter::All => entries,
        az::session::ReadFilter::ExcludeSensitive => {
            entries.into_iter().filter(|e| !e.sensitivity).collect()
        }
    };
    Ok(entries)
}

#[tauri::command]
fn search_semantic(
    query: String,
    limit: Option<usize>,
    state: State<'_, AppState>,
) -> Result<Vec<SearchHitDto>, String> {
    let key = state.require_key()?;
    let model = embed_model();
    let llm = OllamaClient::from_env();
    let vec = llm
        .embed(&model, &query)
        .map_err(|e| format!("embedding Ollama ({model}): {e}"))?;
    let store = EmbeddingsStore::open(&state.db_path, &key).map_err(map_db_err)?;
    let filter = state.current_mode().read_filter();
    let hits = store
        .search(&[], &model, &vec, limit.unwrap_or(10), filter)
        .map_err(|e| e.to_string())?;
    Ok(hits.into_iter().map(SearchHitDto::from).collect())
}

// ------------------- Pipeline LLM -------------------

#[derive(Serialize)]
struct SegmentRunResult {
    segmentation_id: String,
    blocks_count: u64,
    elapsed_ms: u128,
    model: String,
    mode: &'static str,
}

#[derive(Serialize)]
struct ExtractFactsResult {
    drafts_count: u64,
    elapsed_ms: u128,
    model: String,
    mode: &'static str,
}

#[derive(Serialize)]
struct EmbedTargetReport {
    target: String,
    candidates: u64,
    added: u64,
    skipped: u64,
}

#[derive(Serialize)]
struct EmbeddingsRunResult {
    model: String,
    added: u64,
    skipped: u64,
    elapsed_ms: u128,
    per_target: Vec<EmbedTargetReport>,
}

/// Lance la segmentation L0 -> L1 pour une session. Synchrone : la commande
/// rend la main une fois le LLM Ollama terminé. Tauri sérialise les commandes
/// async par défaut côté JS, donc l'UI peut désactiver le bouton pendant
/// l'attente sans logique d'events.
#[tauri::command]
fn segment_run(
    session_id: String,
    model: Option<String>,
    state: State<'_, AppState>,
) -> Result<SegmentRunResult, String> {
    let key = state.require_key()?;
    let mode = state.current_mode();
    let model = model
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(llm_model);
    let l0 = L0Store::open(&state.db_path, &key).map_err(map_db_err)?;
    let mut l1 = L1Store::open(&state.db_path, &key).map_err(map_db_err)?;
    let llm = OllamaClient::from_env();
    let start = std::time::Instant::now();
    let seg = segment_session(&l0, &mut l1, &llm, &model, &session_id, mode)
        .map_err(|e| e.to_string())?;
    let blocks_count = l1
        .blocks(&seg.id, az::session::ReadFilter::All)
        .map_err(|e| e.to_string())?
        .len() as u64;
    Ok(SegmentRunResult {
        segmentation_id: seg.id,
        blocks_count,
        elapsed_ms: start.elapsed().as_millis(),
        model,
        mode: mode.as_str(),
    })
}

/// Lance l'extraction L1 -> L2 (drafts) pour une segmentation.
#[tauri::command]
fn extract_facts(
    segmentation_id: String,
    model: Option<String>,
    state: State<'_, AppState>,
) -> Result<ExtractFactsResult, String> {
    let key = state.require_key()?;
    let mode = state.current_mode();
    let model = model
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(llm_model);
    let l1 = L1Store::open(&state.db_path, &key).map_err(map_db_err)?;
    let mut l2 = L2Store::open(&state.db_path, &key).map_err(map_db_err)?;
    let llm = OllamaClient::from_env();
    let start = std::time::Instant::now();
    let drafts = extract_from_segmentation(&l1, &mut l2, &llm, &model, &segmentation_id, mode)
        .map_err(|e| e.to_string())?;
    Ok(ExtractFactsResult {
        drafts_count: drafts.len() as u64,
        elapsed_ms: start.elapsed().as_millis(),
        model,
        mode: mode.as_str(),
    })
}

/// Recalcule les embeddings manquants pour les cibles demandées (par défaut
/// `transcripts` et `blocks`). Idempotent : ignore les paires (target, model)
/// déjà présentes.
#[tauri::command]
fn embeddings_run(
    targets: Option<Vec<String>>,
    model: Option<String>,
    state: State<'_, AppState>,
) -> Result<EmbeddingsRunResult, String> {
    let key = state.require_key()?;
    let model = model
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(embed_model);
    let targets = targets.unwrap_or_else(|| vec!["transcripts".to_string(), "blocks".to_string()]);
    let store = EmbeddingsStore::open(&state.db_path, &key).map_err(map_db_err)?;
    let l0 = L0Store::open(&state.db_path, &key).map_err(map_db_err)?;
    let l1 = L1Store::open(&state.db_path, &key).map_err(map_db_err)?;
    let llm = OllamaClient::from_env();

    let start = std::time::Instant::now();
    let mut per_target = Vec::with_capacity(targets.len());
    let mut total_added = 0u64;
    let mut total_skipped = 0u64;

    for t in &targets {
        let (target_type, pairs) = match t.as_str() {
            "transcripts" => (
                TARGET_TRANSCRIPT,
                l0.all_with_content().map_err(|e| e.to_string())?,
            ),
            "blocks" => (
                TARGET_BLOCK,
                l1.all_blocks_with_content().map_err(|e| e.to_string())?,
            ),
            other => return Err(format!("cible inconnue: {other} (transcripts|blocks)")),
        };
        let existing: std::collections::HashSet<String> = store
            .existing_ids(target_type, &model)
            .map_err(|e| e.to_string())?
            .into_iter()
            .collect();
        let mut added = 0u64;
        let mut skipped = 0u64;
        for (id, text) in &pairs {
            if existing.contains(id) {
                skipped += 1;
                continue;
            }
            let v = llm
                .embed(&model, text)
                .map_err(|e| format!("embed {target_type}:{id}: {e}"))?;
            store
                .upsert(target_type, id, &model, &v, &now_rfc3339())
                .map_err(|e| e.to_string())?;
            added += 1;
        }
        total_added += added;
        total_skipped += skipped;
        per_target.push(EmbedTargetReport {
            target: t.clone(),
            candidates: pairs.len() as u64,
            added,
            skipped,
        });
    }

    Ok(EmbeddingsRunResult {
        model,
        added: total_added,
        skipped: total_skipped,
        elapsed_ms: start.elapsed().as_millis(),
        per_target,
    })
}

// ------------------- Capture vocale (STT) -------------------

const ENV_WHISPER_MODEL: &str = "AZ_WHISPER_MODEL";
const ENV_WHISPER_LANG: &str = "AZ_WHISPER_LANG";

#[derive(Serialize)]
struct AudioConfig {
    model_set: bool,
    model_path: Option<String>,
    language: String,
}

#[derive(Clone, Serialize)]
struct VoiceLevelEvent {
    rms: f32,
}

#[derive(Clone, Serialize)]
struct VoiceErrorEvent {
    message: String,
}

/// Renseigne l'UI sur la disponibilité du modèle whisper. Ne charge pas le
/// modèle (chargement coûteux, fait à `audio_start_recording`).
#[tauri::command]
fn audio_check_config() -> AudioConfig {
    let model_path = std::env::var(ENV_WHISPER_MODEL).ok();
    let language = std::env::var(ENV_WHISPER_LANG).unwrap_or_else(|_| "auto".to_string());
    AudioConfig {
        model_set: model_path.as_ref().is_some_and(|p| !p.trim().is_empty()),
        model_path,
        language,
    }
}

/// Démarre une capture vocale streaming pour la session donnée. Chaque utterance
/// détectée par le VAD est transcrite et appended au L0 (source `voice`).
/// Émet sur le canal d'événements Tauri :
/// - `voice/transcript` : `L0Entry` à chaque utterance transcrite (texte non vide)
/// - `voice/level` : niveau RMS f32 par frame de 30 ms (jauge UI)
/// - `voice/error` : erreur non fatale (échec de transcription d'une utterance)
#[tauri::command]
fn audio_start_recording(
    app: AppHandle,
    session_id: String,
    sensitive: bool,
    state: State<'_, AppState>,
) -> Result<(), String> {
    if session_id.trim().is_empty() {
        return Err("session_id manquant".into());
    }
    let key = state.require_key()?;
    let model_path = std::env::var(ENV_WHISPER_MODEL)
        .map_err(|_| format!("variable {ENV_WHISPER_MODEL} non définie"))?;
    if model_path.trim().is_empty() {
        return Err(format!("variable {ENV_WHISPER_MODEL} vide"));
    }
    let language = std::env::var(ENV_WHISPER_LANG).unwrap_or_else(|_| "auto".to_string());

    {
        let guard = state.recording.lock().expect("mutex empoisonné");
        if guard.is_some() {
            return Err("une capture est déjà en cours".into());
        }
    }

    let db_path = state.db_path.clone();
    let stop_flag = Arc::new(AtomicBool::new(false));
    let stop_clone = stop_flag.clone();

    let worker = thread::spawn(move || {
        // Tout ce qui suit est `Send`-libre car exécuté sur ce thread dédié.
        // En cas d'échec, on émet `voice/error` puis on sort proprement.
        let mut stt = match WhisperStt::load(&model_path, language.as_str()) {
            Ok(s) => s,
            Err(e) => {
                let _ = app.emit(
                    "voice/error",
                    VoiceErrorEvent {
                        message: format!("chargement whisper: {e}"),
                    },
                );
                return;
            }
        };
        let capture = match AudioCapture::start() {
            Ok(c) => c,
            Err(e) => {
                let _ = app.emit(
                    "voice/error",
                    VoiceErrorEvent {
                        message: format!("capture audio: {e}"),
                    },
                );
                return;
            }
        };
        let store = match L0Store::open(&db_path, &key) {
            Ok(s) => s,
            Err(e) => {
                let _ = app.emit(
                    "voice/error",
                    VoiceErrorEvent {
                        message: format!("ouverture L0: {e}"),
                    },
                );
                return;
            }
        };

        let utterances = capture.utterances();
        let levels = capture.levels();

        while !stop_clone.load(Ordering::Relaxed) {
            // Drain les niveaux disponibles (jauge UI). Limité par tour pour
            // garantir qu'on revient écouter les utterances rapidement.
            for _ in 0..16 {
                match levels.try_recv() {
                    Ok(rms) => {
                        let _ = app.emit("voice/level", VoiceLevelEvent { rms });
                    }
                    Err(_) => break,
                }
            }
            // Attente courte d'une utterance pour laisser respirer le CPU.
            match utterances.recv_timeout(Duration::from_millis(50)) {
                Ok(samples) => match stt.transcribe(&samples) {
                    Ok(t) => {
                        let text = t.text.trim().to_string();
                        if text.is_empty() {
                            continue;
                        }
                        let timestamp = now_rfc3339();
                        let entry = L0Entry {
                            id: uuid::Uuid::new_v4().to_string(),
                            timestamp,
                            content: text,
                            source: "voice".into(),
                            session_id: session_id.clone(),
                            sensitivity: sensitive,
                        };
                        if let Err(e) = store.append(&entry) {
                            let _ = app.emit(
                                "voice/error",
                                VoiceErrorEvent {
                                    message: format!("append L0: {e}"),
                                },
                            );
                            continue;
                        }
                        let _ = app.emit("voice/transcript", &entry);
                    }
                    Err(e) => {
                        let _ = app.emit(
                            "voice/error",
                            VoiceErrorEvent {
                                message: format!("transcription: {e}"),
                            },
                        );
                    }
                },
                Err(RecvTimeoutError::Timeout) => continue,
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }
        // capture drop ici -> arrêt du cpal::Stream + fin du thread VAD interne.
        drop(capture);
        // Signale au front que la dernière jauge doit retomber à 0.
        let _ = app.emit("voice/level", VoiceLevelEvent { rms: 0.0 });
    });

    *state.recording.lock().expect("mutex empoisonné") = Some(RecordingHandle {
        stop_flag,
        worker: Some(worker),
    });
    Ok(())
}

/// Arrête la capture vocale en cours. Idempotent : ne renvoie pas d'erreur si
/// aucune capture n'est active.
#[tauri::command]
fn audio_stop_recording(state: State<'_, AppState>) -> Result<(), String> {
    let mut handle = match state.recording.lock().expect("mutex empoisonné").take() {
        Some(h) => h,
        None => return Ok(()),
    };
    handle.stop_flag.store(true, Ordering::Relaxed);
    if let Some(worker) = handle.worker.take() {
        // Le worker sort de sa boucle en au plus ~50ms (recv_timeout). On lui
        // laisse une marge généreuse pour drop cpal proprement.
        let _ = worker.join();
    }
    Ok(())
}

// ------------------- Boot -------------------

pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            app.manage(AppState::new());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            app_info,
            auth_status,
            auth_unlock,
            auth_create,
            auth_lock,
            session_get_mode,
            session_set_mode,
            summary,
            list_sessions,
            list_transcripts,
            list_segmentations,
            list_blocks,
            list_facts,
            list_drafts,
            list_pages,
            list_links,
            session_new,
            transcript_append,
            search_plain,
            search_semantic,
            fact_sources,
            fact_validate,
            fact_update_and_validate,
            fact_reject,
            segment_run,
            extract_facts,
            embeddings_run,
            audio_check_config,
            audio_start_recording,
            audio_stop_recording,
        ])
        .run(tauri::generate_context!())
        .expect("erreur lors du lancement de l'application Tauri");
}
