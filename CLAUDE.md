# AZ — Guide de reprise

Assistant personnel local-first en Rust. Capture vocale + textuelle → SQLite chiffré (SQLCipher) → segmentation LLM (Ollama) → faits typés validés par l'utilisateur → graphe de dérivations → recherche sémantique.

Le manifeste vision est dans `README.md`. Ce fichier est ton aide-mémoire pour reprendre vite.

## État au dernier checkpoint

| Phase | Statut | Détail |
|---|---|---|
| 1 — Fondations | ✅ | L0 SQLite chiffré SQLCipher, auth Argon2id + mot de passe, captures voix (whisper.cpp via STT) + texte (REPL `chat`), `vacuum`, `backup`/`restore`, `rekey` |
| 2 — Conversation | ✅ | LLM Ollama, L0→L1 segmentation, mode session privé/connecté, filtre `sensitivity` côté SQL pour ce qui alimente le LLM |
| 3 — Faits typés | 🟡 | L2 (faits versionnés, validation REPL `y/n/e/s/q` avec `$EDITOR`), L3 (liens + pages + règle `recipe-to-shopping`), recherche sémantique (embeddings Ollama + cosine linéaire). **Manque** : L3 richer (plus de règles, navigation), index vectoriel quand >10K vecteurs |
| 4 — Multi-appareil & extensibilité | 🟢 | **UI + pipeline LLM + voix** (CH10–CH21) : Tauri+Solid, 30 commandes Tauri, 6 vues. **CH15** : boutons UI pour segmenter (L0→L1), extraire faits (L1→L2 drafts), recalculer embeddings. **CH18** : capture vocale streaming VAD dans Capture (whisper.cpp + cpal), events Tauri `voice/transcript` `voice/level` `voice/error`, jauge RMS live. **CH19** : bouton "Traiter cette session" enchaîne segment + extract + nav auto vers Valider. **CH20** : badge "N drafts" dans la sidebar (store global `ui/src/store.ts`). **CH21** : embeddings_run en background après segment/extract. **Manque** : TTS, sync, plugins, L3 enrichi, maintenance UI (backup/rekey/vacuum), export, validation L2 en lot |

73 tests unitaires, clippy `--all-targets -D warnings` clean. Schéma DB version 5.

## Architecture (dans `src/`)

```
audio.rs / stt/{mod,whisper}.rs     # capture micro + VAD + whisper.cpp
auth.rs                             # Argon2id + rpassword + salt file
backup.rs                           # VACUUM INTO, restore, rekey_db, vacuum_in_place
cli.rs                              # open_l0 / open_l0_l1 / open_l1_l2 / open_l2 helpers
db.rs                               # PRAGMA key + migrations idempotentes v1→v5
derivation.rs                       # trait DerivationRule + RecipeToShopping
embeddings.rs                       # pack/unpack f32 LE, cosine, EmbeddingsStore
extractor.rs                        # L1 blocks → LLM (Ollama JSON mode) → L2 drafts
l0.rs / l1.rs / l2.rs / l3.rs       # stores (thin wrappers sur Connection chiffrée)
llm/{mod,ollama.rs}                 # trait Llm + EmbeddingsLlm + impl HTTP Ollama
segmenter.rs                        # L0 → LLM → L1 blocks (sensitivity héritée des sources)
session.rs                          # SessionMode {Private,Connected} + ReadFilter
bin/{az,chat,query,segment,         # 12 binaires
     facts,embed,export,links,
     backup,rekey,vacuum,mic-check}
```

## UI Tauri (workspace Cargo, dans `ui/`)

```
ui/
├── package.json / vite.config.ts / tsconfig.json / index.html
├── src/                             # frontend SolidJS + TS
│   ├── index.tsx                    # render <App />
│   ├── App.tsx                      # gate auth + shell sidebar + routing local
│   ├── Login.tsx                    # création OU déverrouillage selon salt
│   ├── auth.ts                      # wrappers invoke pour auth
│   ├── api.ts                       # wrappers invoke typés (toutes commandes)
│   ├── views/{Dashboard,Capture,Captures,Facts,Review,Search}.tsx
│   └── styles.css
└── src-tauri/                       # backend Tauri 2 (crate az-ui)
    ├── Cargo.toml                   # dépend de `az` en path = "../.."
    ├── build.rs                     # tauri_build::build()
    ├── tauri.conf.json              # identifier dev.az.app, fenêtre "main", devUrl :1420
    ├── capabilities/default.json    # core:default sur la fenêtre "main"
    ├── icons/icon.png               # 512x512 RGBA placeholder
    └── src/{main.rs,lib.rs}         # AppState { db_path, key, mode } + 24 commandes
```

`AppState` (dans `lib.rs`) garde la clé `Mutex<Option<[u8;32]>>` + le `SessionMode` global. Chaque commande qui touche la DB ouvre sa propre connexion via les stores. La conversion des noms d'arguments Tauri est camelCase côté JS → snake_case côté Rust (ex: `sessionId` → `session_id`).

Le `Cargo.toml` racine est un workspace (`members = ["ui/src-tauri"]`) **et** un package `az` simultanément. Tauri 2.11 / wry 0.55 / Solid 1.9 / Vite 5.

## Conventions techniques

- **Variables, strings utilisateur, messages d'erreur, commentaires : en français.** Code Rust (types, fonctions) en anglais.
- **Pas d'emoji** dans les outputs CLI ou les messages utilisateur sauf demande explicite. Marqueurs ASCII (`[s]`, `V/D`, `*`).
- **Erreurs** : `thiserror` dans les modules (`DbError`, `LlmError`, `AuthError`, etc.), `anyhow` dans les binaires.
- **Tests** : utilisent `db::test_key()` (clé constante `[0xAB; 32]`) au lieu de l'auth interactive. Fichiers temporaires via `std::env::temp_dir()` + `std::process::id()` + nom de test.
- **Stores** : chacun ouvre sa propre connexion SQLCipher via `db::open(path, key)`. WAL mode, foreign_keys ON, cipher_compatibility=4.
- **Mode session** : flag `--mode private|connected` sur les binaires LLM-consumers ; override env `AZ_SESSION_MODE`. Le filtre `ReadFilter::ExcludeSensitive` s'applique en SQL (`WHERE sensitivity = 0`), pas en filtrage applicatif.
- **Linker C++** : flake.nix met `LD_LIBRARY_PATH` pour libstdc++ + alsa. **Toujours lancer dans `nix develop`** sinon l'audio (cpal/alsa) et whisper.cpp ne linkent pas.
- **Modèles Ollama** par défaut : `gemma4:e2b` (LLM) et `nomic-embed-text` (embeddings). Override via `AZ_LLM_MODEL` / `AZ_EMBED_MODEL` ou `--model`.

## Variables d'environnement

| Var | Défaut | Rôle |
|---|---|---|
| `AZ_L0_PATH` | `./data/l0.sqlite` | Fichier DB |
| `AZ_PASSWORD` | (prompt) | Bypass prompt mot de passe (scripts/tests) |
| `AZ_SESSION_MODE` | `private` | Override mode session |
| `AZ_WHISPER_MODEL` | (requis pour voice) | Chemin ggml-*.bin |
| `AZ_WHISPER_LANG` | `auto` | Langue STT |
| `AZ_OLLAMA_URL` | `http://localhost:11434` | Endpoint Ollama |
| `AZ_LLM_MODEL` | `gemma4:e2b` | Modèle de génération |
| `AZ_EMBED_MODEL` | `nomic-embed-text` | Modèle d'embeddings |

## Build / test / commandes utiles

```bash
# Toujours dans nix develop (sinon link errors C++/alsa + WebKit)
nix develop

# Backend Rust
cargo build --bins
cargo test --lib                           # 73 passed actuellement
cargo clippy --all-targets -- -D warnings  # doit être clean

# UI Tauri (depuis ui/)
cd ui
pnpm install                               # une fois
pnpm tauri dev                             # lance Vite :1420 + fenêtre Tauri
pnpm build                                 # frontend uniquement -> dist/
pnpm tauri build                           # bundle release

# Plan de test fonctionnel complet :
# voir ~/.claude/plans/impl-mente-une-solution-de-calm-dolphin.md
# (section "Plan de test")
```

## Pièges déjà rencontrés

- **`PRAGMA key` doit être le premier statement** sur la connexion SQLCipher, avant tout autre PRAGMA ou requête. Voir `db::open`.
- `full_n_segments()` de **whisper-rs 0.13** retourne `Result<i32>`, pas `i32` (différent des anciens docs).
- **`RUST_BACKTRACE=1`** est set par flake.nix → les `Error: ...` propres deviennent bruyants. Préfixer `RUST_BACKTRACE=0` dans les tests utilisateur pour des messages lisibles.
- **`rekey` ignore volontairement `AZ_PASSWORD`** (sécurité). Ne peut pas être automatisé.
- **Migration plain → chiffré** : non gérée. Si une DB plain pré-existe, la supprimer (`rm data/l0.sqlite*`) avant la première run chiffrée.
- **Backup `VACUUM INTO`** sur SQLCipher préserve le chiffrement (même clé). Le fichier `.salt` doit être copié manuellement à côté (fait automatiquement par `backup create`).
- **L1 sensitivity** : aujourd'hui héritée par OR des sources. Si tout est halluciné par le LLM, défaut conservateur = `true`.
- **Tauri 2 + `frontendDist`** : la macro `generate_context!` valide à la compilation que `ui/dist/` existe et que `icons/icon.png` est en RGBA. Faire un `pnpm build` une fois après clone (ou utiliser directement `pnpm tauri dev` qui s'en charge).
- **Webkit2gtk dans nix** : le flake exporte `LD_LIBRARY_PATH` + `XDG_DATA_DIRS` ; sortir du `nix develop` casse aussi la fenêtre Tauri en plus de l'audio.
- **UI default DB path** : la résolution par défaut côté UI est `$HOME/.local/share/az/l0.sqlite` (XDG), **pas** `data/l0.sqlite` relatif au CWD. Sinon le binaire Tauri, lancé avec CWD = `ui/`, créerait la DB dans `ui/data/` et le watcher Vite déclencherait un full-reload à chaque écriture WAL. **Pour partager la base avec le CLI**, exporter `AZ_L0_PATH=$HOME/Dev/az/data/l0.sqlite` avant `pnpm tauri dev`.
- **Vite ignored** : `vite.config.ts` exclut `*.sqlite`, `*.sqlite-*`, `*.salt`, `data/`, `backups/`, `exports/` pour la même raison.

## Workflow de collaboration

L'utilisateur préfère :
1. Travail découpé en **chantiers** (CH1, CH2, …) avec checkpoint `cargo test + clippy` entre chaque.
2. **Auto mode** quand il dit "enchaîne 2-3 chantiers" — pas de question intermédiaire.
3. Plan rédigé dans `~/.claude/plans/impl-mente-une-solution-de-calm-dolphin.md` avant d'attaquer du code non trivial.
4. Quand un point est ambigu : 2-4 options claires via `AskUserQuestion`, pas du texte libre.
5. Décisions de design exposées en table : "j'ai choisi X parce que Y".

Chantiers livrés à date : CH1 (chat REPL), CH2 (Ollama + L1), CH3 (chiffrement), CH4 (sessions+filtre), CH5 (L2), CH6 (embeddings), CH7 (fix L1 sensitivity + exports), CH8 (L3), CH9 (backup + rekey), CH10 (UI Tauri squelette : workspace + SolidJS + commande `app_info`), CH11 (auth login/create/lock UI : clé chiffrée en mémoire dans `AppState`), CH12 (lecture L0/L1/L2/L3 + FTS + sémantique : mode session toggle, Dashboard/Captures/Faits/Recherche), CH13 (capture texte UI : `session_new` + `transcript_append`, Entrée = envoyer / Maj+Entrée = saut de ligne, toggle sensible), CH14 (validation L2 graphique : vue `Review` itère les drafts, Valider/Éditer/Rejeter/Passer, sources L0 affichées, JSON pretty edit, valide la syntaxe avant écriture), CH15 (pipeline LLM dans l'UI : commandes `segment_run`/`extract_facts`/`embeddings_run`, boutons "Segmenter session" + table des segmentations avec "Extraire faits" dans Captures, carte "Recalculer embeddings" sur Dashboard, idempotent), CH18 (voix dans l'UI : `audio_check_config`/`audio_start_recording`/`audio_stop_recording` + events `voice/transcript`/`voice/level`/`voice/error`, jauge RMS, `AudioCapture::levels()` ajouté à `src/audio.rs`, worker thread car `cpal::Stream` n'est pas Send), CH19 (continuité Capture→Valider : bouton "Traiter cette session" enchaîne segment + extract + nav auto via prop `onGoToReview`), CH20 (badge sidebar : store global `ui/src/store.ts` avec `draftsCount`/`refreshDraftsCount`/`resetDraftsCount`, pastille à côté de "Valider drafts"), CH21 (embeddings auto en background après segment/extract via `api.embeddingsRun().catch(() => {})`), + fixes (L2 review preserve sources, `vacuum` bin, restart bug fix CWD-relative DB).

## Pour reprendre

1. `cd ~/Dev/az && nix develop`
2. Lire la dernière conversation OU le plan dans `~/.claude/plans/impl-mente-une-solution-de-calm-dolphin.md`.
3. `cargo test --lib` pour confirmer le state.
4. Demander à l'utilisateur ce qu'il veut attaquer ensuite. Si pas d'idée, suggérer dans cet ordre :
   - **CH16 — Maintenance UI** : backup/restore/rekey/vacuum exposés dans l'UI Tauri (petit)
   - **CH17 — Export UI** : JSON/Markdown/JSONL depuis l'UI (petit)
   - **TTS** (boucle le pipeline vocal, symétrise STT — moyen)
   - **Validation L2 en lot** : bouton "Tout valider" pour les drafts triviaux
   - **L3 plus riche** : autres règles de dérivation, navigation par page, FTS sur les liens
   - **Index vectoriel** (sqlite-vec) quand >10K embeddings
   - **Sync multi-machine** (CRDT ou log d'opérations — gros)
   - **Plugins/imports** (Obsidian, Apple Notes → L0)
