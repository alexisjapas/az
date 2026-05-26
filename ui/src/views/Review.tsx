import {
  batch,
  createResource,
  createSignal,
  For,
  Show,
  type Component,
} from "solid-js";
import { api, type Fact, type L0Entry } from "../api";

function prettyJson(s: string): string {
  try {
    return JSON.stringify(JSON.parse(s), null, 2);
  } catch {
    return s;
  }
}

const Review: Component = () => {
  const [drafts, { refetch: refetchDrafts }] = createResource<Fact[]>(
    api.listDrafts,
  );
  const [index, setIndex] = createSignal(0);

  const current = (): Fact | undefined => {
    const list = drafts();
    if (!list) return undefined;
    return list[index()];
  };

  const [sources] = createResource(current, async (f) => {
    if (!f) return [];
    return await api.factSources(f.id, f.version);
  });

  const [editing, setEditing] = createSignal(false);
  const [editPayload, setEditPayload] = createSignal("");
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);
  const [summaryCounts, setSummaryCounts] = createSignal({
    validated: 0,
    edited: 0,
    rejected: 0,
    skipped: 0,
  });

  const startEdit = () => {
    const f = current();
    if (!f) return;
    setEditPayload(prettyJson(f.payload));
    setEditing(true);
  };

  const advance = () => {
    setIndex(index() + 1);
    setEditing(false);
    setError(null);
  };

  const wrap = async (label: keyof ReturnType<typeof summaryCounts>, fn: () => Promise<void>) => {
    setBusy(true);
    setError(null);
    try {
      await fn();
      batch(() => {
        const c = summaryCounts();
        setSummaryCounts({ ...c, [label]: c[label] + 1 });
        advance();
      });
    } catch (err) {
      setError(typeof err === "string" ? err : String(err));
    } finally {
      setBusy(false);
    }
  };

  const validate = () => {
    const f = current();
    if (!f) return;
    wrap("validated", () => api.factValidate(f.id, f.version));
  };

  const reject = () => {
    const f = current();
    if (!f) return;
    if (!confirm("Supprimer définitivement ce draft ?")) return;
    wrap("rejected", () => api.factReject(f.id, f.version));
  };

  const validateEdit = () => {
    const f = current();
    if (!f) return;
    const payload = editPayload();
    wrap("edited", () => api.factUpdateAndValidate(f.id, f.version, payload));
  };

  const skip = () => {
    setSummaryCounts({
      ...summaryCounts(),
      skipped: summaryCounts().skipped + 1,
    });
    advance();
  };

  const reload = async () => {
    setIndex(0);
    setSummaryCounts({ validated: 0, edited: 0, rejected: 0, skipped: 0 });
    setEditing(false);
    setError(null);
    await refetchDrafts();
  };

  return (
    <div class="view">
      <h2>Validation L2</h2>

      <Show when={drafts.loading}><p>Chargement…</p></Show>
      <Show when={drafts.error}>
        <p class="error">{String(drafts.error)}</p>
      </Show>

      <Show when={drafts() && drafts()!.length === 0}>
        <section class="card">
          <p class="muted">Aucun draft à valider. Lance d'abord{" "}
            <code>cargo run --bin facts -- extract --segmentation &lt;id&gt;</code>{" "}
            pour générer des faits.
          </p>
        </section>
      </Show>

      <Show when={drafts() && drafts()!.length > 0 && !current()}>
        <section class="card">
          <h3>Fin de la passe</h3>
          <dl class="info">
            <dt>Validés</dt><dd>{summaryCounts().validated}</dd>
            <dt>Édités</dt><dd>{summaryCounts().edited}</dd>
            <dt>Rejetés</dt><dd>{summaryCounts().rejected}</dd>
            <dt>Passés</dt><dd>{summaryCounts().skipped}</dd>
          </dl>
          <button class="primary" onClick={reload}>
            Recharger la liste
          </button>
        </section>
      </Show>

      <Show when={current()}>
        {(f) => (
          <>
            <p class="muted small">
              Fait {index() + 1} / {drafts()!.length}
            </p>

            <section class="card fact">
              <header class="row between">
                <div>
                  <code>{f().fact_type}</code> · v{f().version}{" "}
                  {f().sensitivity ? <span class="tag">[s]</span> : null}
                </div>
                <div class="muted small">créé {f().created_at}</div>
              </header>

              <Show
                when={editing()}
                fallback={<pre>{prettyJson(f().payload)}</pre>}
              >
                <label class="field">
                  <span>Payload JSON</span>
                  <textarea
                    rows={10}
                    value={editPayload()}
                    onInput={(e) => setEditPayload(e.currentTarget.value)}
                    disabled={busy()}
                    spellcheck={false}
                  />
                </label>
              </Show>

              <Show when={error()}>
                {(msg) => <p class="error">{msg()}</p>}
              </Show>

              <div class="row gap wrap actions">
                <Show
                  when={!editing()}
                  fallback={
                    <>
                      <button
                        class="primary"
                        onClick={validateEdit}
                        disabled={busy()}
                      >
                        Valider l'édition
                      </button>
                      <button onClick={() => setEditing(false)} disabled={busy()}>
                        Annuler
                      </button>
                    </>
                  }
                >
                  <button class="primary" onClick={validate} disabled={busy()}>
                    Valider tel quel
                  </button>
                  <button onClick={startEdit} disabled={busy()}>
                    Éditer
                  </button>
                  <button class="danger" onClick={reject} disabled={busy()}>
                    Rejeter
                  </button>
                  <button onClick={skip} disabled={busy()}>
                    Passer
                  </button>
                </Show>
              </div>
            </section>

            <section class="card">
              <h3 class="muted small">Sources (transcripts L0)</h3>
              <Show
                when={sources() && sources()!.length > 0}
                fallback={<p class="muted small">(aucune source liée)</p>}
              >
                <For each={sources()}>
                  {(s: L0Entry) => (
                    <article class="hit">
                      <div class="muted small">
                        <code>{s.timestamp}</code> · <code>{s.source}</code>
                        {s.sensitivity ? " · [s]" : ""}
                      </div>
                      <div class="content">{s.content}</div>
                    </article>
                  )}
                </For>
              </Show>
            </section>
          </>
        )}
      </Show>
    </div>
  );
};

export default Review;
