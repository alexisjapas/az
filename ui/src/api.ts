import { invoke } from "@tauri-apps/api/core";

export type AppInfo = {
  name: string;
  version: string;
  db_path: string;
  db_exists: boolean;
  salt_exists: boolean;
  unlocked: boolean;
  mode: SessionMode;
};

export type SessionMode = "private" | "connected";

export type Summary = {
  transcripts: number;
  segmentations: number;
  facts_total: number;
  facts_drafts: number;
  embeddings: number;
  pages: number;
  links: number;
};

export type SessionInfo = {
  id: string;
  transcripts: number;
  first_at: string;
  last_at: string;
};

export type L0Entry = {
  id: string;
  timestamp: string;
  content: string;
  source: string;
  session_id: string;
  sensitivity: boolean;
};

export type Segmentation = {
  id: string;
  created_at: string;
  session_id: string;
  model: string;
  prompt_version: string;
  notes: string | null;
};

export type Block = {
  id: string;
  segmentation_id: string;
  seq: number;
  topic: string | null;
  content: string;
  sensitivity: boolean;
};

export type Fact = {
  id: string;
  version: number;
  fact_type: string;
  payload: string;
  block_id: string | null;
  sensitivity: boolean;
  created_at: string;
  validated_at: string | null;
};

export type Page = {
  id: string;
  title: string;
  description: string | null;
  is_active: boolean;
  created_at: string;
  archived_at: string | null;
};

export type Link = {
  id: string;
  src_kind: string;
  src_id: string;
  dst_kind: string;
  dst_id: string;
  rel_type: string;
  derived_by: string;
  metadata: string | null;
  created_at: string;
};

export type SearchHit = {
  target_type: string;
  target_id: string;
  score: number;
  content: string;
  sensitivity: boolean;
};

export type SegmentRunResult = {
  segmentation_id: string;
  blocks_count: number;
  elapsed_ms: number;
  model: string;
  mode: SessionMode;
};

export type ExtractFactsResult = {
  drafts_count: number;
  elapsed_ms: number;
  model: string;
  mode: SessionMode;
};

export type EmbedTargetReport = {
  target: string;
  candidates: number;
  added: number;
  skipped: number;
};

export type EmbeddingsRunResult = {
  model: string;
  added: number;
  skipped: number;
  elapsed_ms: number;
  per_target: EmbedTargetReport[];
};

export const api = {
  appInfo: () => invoke<AppInfo>("app_info"),

  sessionGetMode: () => invoke<SessionMode>("session_get_mode"),
  sessionSetMode: (mode: SessionMode) => invoke<void>("session_set_mode", { mode }),

  summary: () => invoke<Summary>("summary"),
  listSessions: () => invoke<SessionInfo[]>("list_sessions"),
  listTranscripts: (sessionId: string | null, limit: number | null) =>
    invoke<L0Entry[]>("list_transcripts", { sessionId, limit }),
  listSegmentations: (sessionId: string | null) =>
    invoke<Segmentation[]>("list_segmentations", { sessionId }),
  listBlocks: (segmentationId: string) =>
    invoke<Block[]>("list_blocks", { segmentationId }),
  listFacts: (factType: string | null, validatedOnly: boolean) =>
    invoke<Fact[]>("list_facts", { factType, validatedOnly }),
  listDrafts: () => invoke<Fact[]>("list_drafts"),
  listPages: () => invoke<Page[]>("list_pages"),
  listLinks: (filter: {
    srcKind?: string;
    srcId?: string;
    dstKind?: string;
    dstId?: string;
  }) => invoke<Link[]>("list_links", filter),

  searchPlain: (query: string, limit: number) =>
    invoke<L0Entry[]>("search_plain", { query, limit }),
  searchSemantic: (query: string, limit: number) =>
    invoke<SearchHit[]>("search_semantic", { query, limit }),

  sessionNew: () => invoke<string>("session_new"),
  transcriptAppend: (sessionId: string, content: string, sensitive: boolean) =>
    invoke<L0Entry>("transcript_append", { sessionId, content, sensitive }),

  factSources: (id: string, version: number) =>
    invoke<L0Entry[]>("fact_sources", { id, version }),
  factValidate: (id: string, version: number) =>
    invoke<void>("fact_validate", { id, version }),
  factUpdateAndValidate: (id: string, version: number, payload: string) =>
    invoke<void>("fact_update_and_validate", { id, version, payload }),
  factReject: (id: string, version: number) =>
    invoke<void>("fact_reject", { id, version }),

  segmentRun: (sessionId: string, model?: string) =>
    invoke<SegmentRunResult>("segment_run", { sessionId, model: model ?? null }),
  extractFacts: (segmentationId: string, model?: string) =>
    invoke<ExtractFactsResult>("extract_facts", {
      segmentationId,
      model: model ?? null,
    }),
  embeddingsRun: (targets?: string[], model?: string) =>
    invoke<EmbeddingsRunResult>("embeddings_run", {
      targets: targets ?? null,
      model: model ?? null,
    }),
};
