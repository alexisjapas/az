import {
  createResource,
  createSignal,
  For,
  Show,
  type Component,
} from "solid-js";
import { api, type SegmentRunResult, type ExtractFactsResult } from "../api";
import { refreshDraftsCount } from "../store";

const Captures: Component = () => {
  const [sessionFilter, setSessionFilter] = createSignal<string>("");
  const [sessions, { refetch: refetchSessions }] = createResource(api.listSessions);
  const [entries, { refetch: refetchEntries }] = createResource(sessionFilter, (sid) =>
    api.listTranscripts(sid || null, 200),
  );
  const [segmentations, { refetch: refetchSegmentations }] = createResource(
    sessionFilter,
    (sid) => (sid ? api.listSegmentations(sid) : Promise.resolve([])),
  );

  // Etat des actions en cours (par session pour segmenter, par segmentation pour
  // extraire). Une seule action active à la fois côté UX (boutons disabled).
  const [busy, setBusy] = createSignal<string | null>(null);
  const [statusMsg, setStatusMsg] = createSignal<string | null>(null);
  const [errMsg, setErrMsg] = createSignal<string | null>(null);

  const onSegment = async () => {
    const sid = sessionFilter();
    if (!sid) return;
    setBusy(`segment:${sid}`);
    setStatusMsg(null);
    setErrMsg(null);
    try {
      const r: SegmentRunResult = await api.segmentRun(sid);
      setStatusMsg(
        `Segmentation creee : ${r.blocks_count} bloc(s) en ${r.elapsed_ms} ms (modele ${r.model}, mode ${r.mode}).`,
      );
      refetchSegmentations();
      // CH21 : embeddings en arrière-plan (nouveaux blocs L1).
      api.embeddingsRun().catch(() => {});
    } catch (e) {
      setErrMsg(String(e));
    } finally {
      setBusy(null);
    }
  };

  const onExtract = async (segId: string) => {
    setBusy(`extract:${segId}`);
    setStatusMsg(null);
    setErrMsg(null);
    try {
      const r: ExtractFactsResult = await api.extractFacts(segId);
      setStatusMsg(
        `Extraction terminee : ${r.drafts_count} draft(s) en ${r.elapsed_ms} ms (modele ${r.model}, mode ${r.mode}). A valider dans "Valider drafts".`,
      );
      refreshDraftsCount();
      // CH21 : embeddings en arrière-plan (les nouveaux blocs/transcripts ne
      // sont indexés que si on les recalcule explicitement).
      api.embeddingsRun().catch(() => {});
    } catch (e) {
      setErrMsg(String(e));
    } finally {
      setBusy(null);
    }
  };

  const refreshAll = () => {
    refetchSessions();
    refetchEntries();
    refetchSegmentations();
  };

  return (
    <div class="view">
      <h2>Captures (L0)</h2>

      <section class="card">
        <div class="row gap">
          <label class="field inline">
            <span>Session</span>
            <select
              value={sessionFilter()}
              onChange={(e) => setSessionFilter(e.currentTarget.value)}
            >
              <option value="">Toutes</option>
              <For each={sessions()}>
                {(s) => (
                  <option value={s.id}>
                    {s.id.slice(0, 8)} ({s.transcripts})
                  </option>
                )}
              </For>
            </select>
          </label>
          <button class="small" onClick={refreshAll} disabled={busy() !== null}>
            Rafraichir
          </button>
        </div>
      </section>

      <Show when={errMsg()}>
        <p class="error">{errMsg()}</p>
      </Show>
      <Show when={statusMsg()}>
        <p class="muted small">{statusMsg()}</p>
      </Show>

      <Show when={sessionFilter()}>
        <section class="card">
          <div class="row between">
            <h3>Pipeline L0 -&gt; L1 -&gt; L2</h3>
            <button
              class="primary small"
              onClick={onSegment}
              disabled={busy() !== null}
            >
              {busy() === `segment:${sessionFilter()}`
                ? "Segmentation..."
                : "Segmenter cette session"}
            </button>
          </div>
          <p class="muted small">
            Cree une nouvelle segmentation (L0 -&gt; L1) via Ollama. Operation longue,
            la fenetre reste reactive.
          </p>

          <Show when={segmentations.loading}>
            <p class="muted">Chargement des segmentations...</p>
          </Show>
          <Show when={segmentations() && segmentations()!.length === 0}>
            <p class="muted small">Aucune segmentation pour cette session.</p>
          </Show>
          <Show when={segmentations() && segmentations()!.length > 0}>
            <table class="rows">
              <thead>
                <tr>
                  <th>Cree</th>
                  <th>Id</th>
                  <th>Modele</th>
                  <th>Actions</th>
                </tr>
              </thead>
              <tbody>
                <For each={segmentations()}>
                  {(s) => (
                    <tr>
                      <td class="ts"><code>{s.created_at}</code></td>
                      <td><code>{s.id.slice(0, 8)}</code></td>
                      <td>{s.model}</td>
                      <td>
                        <button
                          class="small"
                          onClick={() => onExtract(s.id)}
                          disabled={busy() !== null}
                        >
                          {busy() === `extract:${s.id}`
                            ? "Extraction..."
                            : "Extraire faits"}
                        </button>
                      </td>
                    </tr>
                  )}
                </For>
              </tbody>
            </table>
          </Show>
        </section>
      </Show>

      <Show when={entries.loading}><p>Chargement...</p></Show>
      <Show when={entries.error}>
        <p class="error">{String(entries.error)}</p>
      </Show>
      <Show when={entries() && entries()!.length === 0}>
        <p class="muted">Aucune capture (mode connecte ? les sensibles sont masquees).</p>
      </Show>
      <Show when={entries() && entries()!.length > 0}>
        <section class="card">
          <table class="rows">
            <thead>
              <tr>
                <th>Quand</th>
                <th>Src</th>
                <th>Contenu</th>
                <th>Sens.</th>
              </tr>
            </thead>
            <tbody>
              <For each={entries()}>
                {(e) => (
                  <tr>
                    <td class="ts"><code>{e.timestamp}</code></td>
                    <td><code>{e.source}</code></td>
                    <td class="content">{e.content}</td>
                    <td>{e.sensitivity ? "[s]" : ""}</td>
                  </tr>
                )}
              </For>
            </tbody>
          </table>
        </section>
      </Show>
    </div>
  );
};

export default Captures;
