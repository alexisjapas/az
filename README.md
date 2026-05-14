# AZ

> Assistant personnel local-first, sécurisé et modulaire pour capturer, structurer et interroger ses données personnelles via un LLM local.

## Vision

Construire un "second cerveau" augmenté par l'IA, fonctionnant entièrement en local, dans lequel la saisie passe par la conversation naturelle et l'utilisateur garde un contrôle déterministe sur l'exposition de ses données sensibles.

## Principes directeurs

- **Local-first** : aucune donnée ne quitte la machine sans consentement explicite.
- **Confidentialité déterministe** : le filtrage des données sensibles repose sur des règles de base de données, pas sur l'interprétation du LLM.
- **Capture par conversation** : la saisie se fait au fil d'une discussion avec l'assistant ; l'utilisateur valide les faits extraits avant persistance.
- **Auditabilité** : toute transformation et toute modification sont versionnées.
- **Modulaire** : LLM, TTS, STT, stockage sont interchangeables.

## Architecture

### Stack technique

| Couche | Technologie | Justification |
|--------|------------|---------------|
| Cœur applicatif | Rust | Sécurité mémoire, performance, concurrence |
| Interface desktop | Tauri | Webview légère, intégration native |
| Stockage | Base de données embarquée (SQLite ou équivalent) | Performance des requêtes, indexation, FTS |
| Chiffrement | `ring` ou `libsodium` (AES-256-GCM) | Standards éprouvés |
| Dérivation de clé | Argon2id | Standard moderne de KDF |
| LLM | Runtime local (`llama.cpp`, `ort`) | Inférence CPU/GPU, modèles GGUF |
| STT | `whisper.cpp` | Reconnaissance vocale locale |
| TTS | `piper-tts` ou équivalent | Synthèse vocale locale |

### Structure modulaire

Monorepo, séparation stricte entre :

- **Moteur de données** : stockage chiffré, indexation, requêtes, versioning, filtrage de sensibilité.
- **Module IA** : abstraction sur le runtime LLM, segmentation, extraction, RAG, TTS/STT.
- **Interface** : UI Tauri (desktop) ; client léger envisagé pour mobile en phase ultérieure.

Chaque module expose une interface stable pour permettre le remplacement de son implémentation.

## Modèle de données

Le modèle est conçu en **quatre couches**. Le MVP n'implémente que L0 et L1 ; L2 et L3 sont planifiées.

### L0 — Transcripts bruts (MVP)

- Conversations, dictées et imports tels quels.
- **Immuables**, append-only.
- Champs minimaux : `id`, `timestamp`, `content`, `source` (`chat` / `voice` / `import_<type>` / `manual`), `session_id`, `sensitivity` (bool).
- Servent d'audit trail et de **source rejouable** pour les transformations en aval.

### L1 — Blocs segmentés (MVP)

- Issus d'une **transformation versionnée** de L0 (segmentation par sujet, timestamp, intention).
- Plusieurs versions de segmentation peuvent coexister pour un même transcript : on peut toujours re-segmenter ultérieurement avec un meilleur modèle.
- Granularité ciblée pour le RAG et la navigation humaine.

### L2 — Faits typés (Phase ultérieure)

- Records typés extraits par le LLM puis **validés par l'utilisateur** avant persistance.
- **Types non figés** : la liste émerge avec l'usage (transactions, mesures de santé, événements, recettes & ingrédients, listes de courses, etc.). Le design précis est différé.
- **Versioning complet** : chaque modification produit une nouvelle version, l'historique reste accessible.
- Référence vers le bloc L1 et le transcript L0 d'origine.
- Champ `sensitivity` hérité du contexte de session, surchargeable, défaut conservateur (`true`).

### L3 — Liens / graphe (Phase ultérieure)

- Arêtes entre blocs, faits et "pages" thématiques.
- Dérivées de l'extraction, éditables manuellement.
- Support des **dérivations entre types** (exemple cible : ajouter une recette ajoute automatiquement ses ingrédients à la liste de courses active).

### Politique de confidentialité

- Au démarrage d'une session, l'utilisateur choisit explicitement le mode :
  - **Privée** : aucun accès internet ni outil externe pour le LLM. Accès complet aux données.
  - **Connectée** : outils externes autorisés. Les entrées avec `sensitivity = true` sont **exclues du contexte** transmis au modèle.
- Le filtrage se fait à la lecture, en base, avant tout passage par le LLM. **Garantie technique, pas comportementale.**
- `sensitivity` est binaire (`true` / `false`).
- Toute classification proposée par le LLM doit être **validée par l'utilisateur** pour être persistée. Par défaut MVP : validation par fait, `sensitivity = true`.

## Capture conversationnelle

Le flux nominal :

1. L'utilisateur dialogue avec l'assistant (texte ou voix).
2. Le transcript brut est stocké tel quel en **L0** au fil de l'eau.
3. En fin de session, le LLM propose une **segmentation L1**.
4. Lorsque L2 sera disponible, le LLM proposera également des **faits typés** à valider un à un.
5. Une session interrompue avant validation reste en brouillon ; rien n'est perdu et rien n'est promu sans validation explicite.

**Enrichissement itératif** : lorsqu'une requête utilisateur ne peut pas être satisfaite faute de structuration suffisante, l'assistant propose à l'utilisateur de classifier / enrichir les éléments manquants en conversation, plutôt que de naviguer dans la donnée.

## Multi-appareil

- **MVP** : une seule machine.
- **Phases ultérieures** :
  - Client léger mobile (capture vocale + lecture) ; l'analyse lourde reste sur la machine principale, exécutée en différé.
  - Configuration LLM par profil de machine (les capacités matérielles varient fortement entre desktop, laptop et mobile).
  - Sync local entre machines via un mécanisme **adapté aux DB** (CRDT, log d'opérations, ou DB sync-aware) — un sync de fichiers brut type Syncthing sur SQLite corromprait la base.

**Note de faisabilité mobile** : Tauri 2 supporte iOS et Android avec le même cœur Rust ; SQLite chiffré, capture audio et STT (natif OS ou `whisper.cpp`) sont disponibles sur les deux plateformes. Le verrou n'est pas la plateforme mobile en soi, mais le mécanisme de sync — une fois celui-ci en place, le mobile devient un client de plus.

## Sécurité

- Chiffrement au repos (AES-256-GCM), clé dérivée du mot de passe (Argon2id).
- Authentification obligatoire à l'ouverture.
- Filtrage déterministe en base pour les données `sensitive`.
- Audit rigoureux et tests exhaustifs du module de filtrage.
- Sauvegarde chiffrée externe optionnelle.

## Non-objectifs

- **Pas de fine-tuning de modèle.** La personnalisation passe par le RAG et le contexte.
- **Pas de cloud sync** dans le périmètre initial. Sync local envisagé après évaluation.
- **Pas de types métier figés** dans le modèle de données : ils émergent et sont extensibles.
- **Pas de fonctionnalités à parité sur tous les appareils** : un téléphone ne fera pas tourner le pipeline d'extraction.

## Risques identifiés

| Risque | Mitigation |
|--------|-----------|
| Performance LLM insuffisante sur matériel modeste | Modèles quantifiés GGUF, configuration par profil de machine, capture déportable vers une machine principale |
| Hallucination du LLM à l'extraction | Validation utilisateur obligatoire avant persistance, transcript brut L0 conservé pour rejouer l'extraction |
| Fatigue de validation (per-fait) | UX dédiée ; validation par lot envisagée après MVP ; brouillons persistants |
| Sync multi-machine corrompant la DB | Mécanisme de réplication dédié, pas de sync de fichiers bruts |
| Fuite via erreur logique du filtrage | Filtrage en base (déterministe), mode session explicite, tests exhaustifs, audit dédié |
| Perte de données locales | Sauvegarde chiffrée externe optionnelle |

## Cas d'usage cibles

Exemples qui orientent la conception ; pas une liste exhaustive.

- **Journaling conversationnel** : raconter sa journée à l'assistant, valider à la fin.
- **Suivi finance** : "combien ai-je dépensé en alimentation le mois dernier ?"
- **Suivi santé** : retrouver le contenu d'une consultation, suivre une mesure dans le temps.
- **Gestion projet** : projets actifs, deadlines, blocages.
- **Cuisine** : ajouter une recette → ses ingrédients sont proposés à la liste de courses active.

## Roadmap (haute maille)

- **Phase 1 — Fondations** : Rust + Tauri, stockage chiffré, authentification, schéma L0, capture textuelle.
- **Phase 2 — Conversation** : intégration LLM modulaire, segmentation versionnée L0 → L1, mode session privée / connectée, filtrage déterministe.
- **Phase 3 — Faits typés** : extraction L1 → L2 avec validation par fait, premiers types métier, graphe L3, recherche sémantique.
- **Phase 4 — Multi-appareil & extensibilité** : client mobile léger, sync local, vocal complet, plugins, exports, sauvegarde externe.
