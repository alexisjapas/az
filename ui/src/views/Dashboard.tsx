import { createResource, createSignal, For, Show, type Component } from "solid-js";
import { api, type EmbeddingsRunResult } from "../api";

const Dashboard: Component = () => {
  const [summary, { refetch: refetchSummary }] = createResource(api.summary);
  const [sessions] = createResource(api.listSessions);

  const [running, setRunning] = createSignal(false);
  const [result, setResult] = createSignal<EmbeddingsRunResult | null>(null);
  const [err, setErr] = createSignal<string | null>(null);

  const onRunEmbeddings = async () => {
    setRunning(true);
    setErr(null);
    setResult(null);
    try {
      const r = await api.embeddingsRun();
      setResult(r);
      refetchSummary();
    } catch (e) {
      setErr(String(e));
    } finally {
      setRunning(false);
    }
  };

  return (
    <div class="view">
      <h2>Tableau de bord</h2>

      <section class="card">
        <h3>Volumes</h3>
        <Show when={summary()} fallback={<p>Chargement...</p>}>
          {(s) => (
            <dl class="info grid-2">
              <dt>Transcripts</dt><dd>{s().transcripts}</dd>
              <dt>Segmentations L1</dt><dd>{s().segmentations}</dd>
              <dt>Faits L2 (courants)</dt><dd>{s().facts_total}</dd>
              <dt>Drafts non valides</dt><dd>{s().facts_drafts}</dd>
              <dt>Embeddings</dt><dd>{s().embeddings}</dd>
              <dt>Pages L3</dt><dd>{s().pages}</dd>
              <dt>Liens L3</dt><dd>{s().links}</dd>
            </dl>
          )}
        </Show>
      </section>

      <section class="card">
        <div class="row between">
          <h3>Embeddings</h3>
          <button
            class="primary small"
            onClick={onRunEmbeddings}
            disabled={running()}
          >
            {running() ? "Calcul en cours..." : "Recalculer embeddings"}
          </button>
        </div>
        <p class="muted small">
          Idempotent : ne recalcule que les transcripts et blocs non encore
          embeddes (modele par defaut nomic-embed-text via Ollama).
        </p>
        <Show when={err()}>
          <p class="error">{err()}</p>
        </Show>
        <Show when={result()}>
          {(r) => (
            <dl class="info grid-2">
              <dt>Modele</dt><dd><code>{r().model}</code></dd>
              <dt>Duree</dt><dd>{r().elapsed_ms} ms</dd>
              <dt>Ajoutes</dt><dd>{r().added}</dd>
              <dt>Sautes</dt><dd>{r().skipped}</dd>
              <For each={r().per_target}>
                {(t) => (
                  <>
                    <dt>{t.target}</dt>
                    <dd>
                      {t.added} ajoute(s), {t.skipped} saute(s) /{" "}
                      {t.candidates} candidat(s)
                    </dd>
                  </>
                )}
              </For>
            </dl>
          )}
        </Show>
      </section>

      <section class="card">
        <h3>Sessions</h3>
        <Show
          when={sessions() && sessions()!.length > 0}
          fallback={<p class="muted">Aucune session pour l'instant.</p>}
        >
          <table class="rows">
            <thead>
              <tr>
                <th>ID</th>
                <th>Captures</th>
                <th>Premiere</th>
                <th>Derniere</th>
              </tr>
            </thead>
            <tbody>
              <For each={sessions()}>
                {(s) => (
                  <tr>
                    <td><code>{s.id.slice(0, 8)}</code></td>
                    <td>{s.transcripts}</td>
                    <td>{s.first_at}</td>
                    <td>{s.last_at}</td>
                  </tr>
                )}
              </For>
            </tbody>
          </table>
        </Show>
      </section>
    </div>
  );
};

export default Dashboard;
