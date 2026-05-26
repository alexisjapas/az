import { createSignal, For, Match, Show, Switch, type Component } from "solid-js";
import { api, type L0Entry, type SearchHit } from "../api";

type Mode = "plain" | "semantic";

const Search: Component = () => {
  const [mode, setMode] = createSignal<Mode>("plain");
  const [query, setQuery] = createSignal("");
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);
  const [plainResults, setPlainResults] = createSignal<L0Entry[]>([]);
  const [semResults, setSemResults] = createSignal<SearchHit[]>([]);

  const run = async (e?: SubmitEvent) => {
    e?.preventDefault();
    const q = query().trim();
    if (!q) return;
    setBusy(true);
    setError(null);
    try {
      if (mode() === "plain") {
        setPlainResults(await api.searchPlain(q, 20));
        setSemResults([]);
      } else {
        setSemResults(await api.searchSemantic(q, 10));
        setPlainResults([]);
      }
    } catch (err) {
      setError(typeof err === "string" ? err : String(err));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div class="view">
      <h2>Recherche</h2>

      <form class="card" onSubmit={run}>
        <div class="row gap wrap">
          <div class="row gap-sm">
            <button
              type="button"
              class={mode() === "plain" ? "primary small" : "small"}
              onClick={() => setMode("plain")}
              disabled={busy()}
            >
              Plein texte (FTS)
            </button>
            <button
              type="button"
              class={mode() === "semantic" ? "primary small" : "small"}
              onClick={() => setMode("semantic")}
              disabled={busy()}
            >
              Sémantique (Ollama)
            </button>
          </div>
        </div>
        <label class="field">
          <span>Requête</span>
          <input
            type="text"
            value={query()}
            onInput={(e) => setQuery(e.currentTarget.value)}
            placeholder={
              mode() === "plain"
                ? "mots-clés FTS5 (ex: pomme OR omelette)"
                : "intention en langage naturel"
            }
            disabled={busy()}
            autofocus
          />
        </label>
        <button type="submit" class="primary" disabled={busy() || !query().trim()}>
          {busy() ? "Recherche…" : "Lancer"}
        </button>
      </form>

      <Show when={error()}>
        {(msg) => <p class="error">{msg()}</p>}
      </Show>

      <Switch>
        <Match when={mode() === "plain" && plainResults().length > 0}>
          <section class="card">
            <h3 class="muted small">{plainResults().length} résultats FTS</h3>
            <For each={plainResults()}>
              {(e) => (
                <article class="hit">
                  <div class="muted small">
                    <code>{e.timestamp}</code> · <code>{e.source}</code>
                    {e.sensitivity ? " · [s]" : ""}
                  </div>
                  <div class="content">{e.content}</div>
                </article>
              )}
            </For>
          </section>
        </Match>
        <Match when={mode() === "semantic" && semResults().length > 0}>
          <section class="card">
            <h3 class="muted small">{semResults().length} hits sémantiques</h3>
            <For each={semResults()}>
              {(h) => (
                <article class="hit">
                  <div class="muted small">
                    <code>{h.target_type}</code> · score{" "}
                    {h.score.toFixed(3)}
                    {h.sensitivity ? " · [s]" : ""}
                  </div>
                  <div class="content">{h.content}</div>
                </article>
              )}
            </For>
          </section>
        </Match>
      </Switch>
    </div>
  );
};

export default Search;
