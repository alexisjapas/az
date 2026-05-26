import {
  createResource,
  createSignal,
  For,
  Show,
  type Component,
} from "solid-js";
import { api, type Fact } from "../api";

function prettyPayload(s: string): string {
  try {
    return JSON.stringify(JSON.parse(s), null, 2);
  } catch {
    return s;
  }
}

const Facts: Component = () => {
  const [typeFilter, setTypeFilter] = createSignal<string>("");
  const [validatedOnly, setValidatedOnly] = createSignal<boolean>(true);
  const [facts] = createResource(
    () => [typeFilter(), validatedOnly()] as const,
    ([t, v]) => api.listFacts(t || null, v),
  );

  return (
    <div class="view">
      <h2>Faits (L2)</h2>

      <section class="card">
        <div class="row gap wrap">
          <label class="field inline">
            <span>Type</span>
            <input
              type="text"
              placeholder="ex: expense, recipe, …"
              value={typeFilter()}
              onInput={(e) => setTypeFilter(e.currentTarget.value)}
            />
          </label>
          <label class="row gap-sm checkbox">
            <input
              type="checkbox"
              checked={validatedOnly()}
              onChange={(e) => setValidatedOnly(e.currentTarget.checked)}
            />
            <span>Validés uniquement</span>
          </label>
        </div>
      </section>

      <Show when={facts.loading}><p>Chargement…</p></Show>
      <Show when={facts.error}>
        <p class="error">{String(facts.error)}</p>
      </Show>
      <Show when={facts() && facts()!.length === 0}>
        <p class="muted">Aucun fait ne correspond.</p>
      </Show>
      <For each={facts()}>
        {(f: Fact) => (
          <section class="card fact">
            <header class="row between">
              <div>
                <code>{f.fact_type}</code> · v{f.version}{" "}
                {f.sensitivity ? <span class="tag">[s]</span> : null}
              </div>
              <div class="muted">
                {f.validated_at
                  ? `validé ${f.validated_at}`
                  : "draft"}
              </div>
            </header>
            <pre>{prettyPayload(f.payload)}</pre>
            <footer class="muted small">id <code>{f.id.slice(0, 8)}</code></footer>
          </section>
        )}
      </For>
    </div>
  );
};

export default Facts;
