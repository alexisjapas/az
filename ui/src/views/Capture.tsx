import {
  createResource,
  createSignal,
  For,
  Show,
  type Component,
} from "solid-js";
import { api, type L0Entry } from "../api";

const Capture: Component = () => {
  const [sessionId, setSessionId] = createSignal<string>("");
  const [text, setText] = createSignal("");
  const [sensitive, setSensitive] = createSignal(true);
  const [entries, setEntries] = createSignal<L0Entry[]>([]);
  const [error, setError] = createSignal<string | null>(null);
  const [busy, setBusy] = createSignal(false);

  // Mode session global, juste pour rappel visuel.
  const [info] = createResource(api.appInfo);

  // Crée une session au montage si pas déjà fait.
  const ensureSession = async () => {
    if (!sessionId()) {
      const id = await api.sessionNew();
      setSessionId(id);
    }
  };
  ensureSession();

  const newSession = async () => {
    const id = await api.sessionNew();
    setSessionId(id);
    setEntries([]);
    setError(null);
  };

  const submit = async (e?: Event) => {
    e?.preventDefault();
    const content = text().trim();
    if (!content || !sessionId()) return;
    setBusy(true);
    setError(null);
    try {
      const entry = await api.transcriptAppend(
        sessionId(),
        content,
        sensitive(),
      );
      setEntries([entry, ...entries()]);
      setText("");
    } catch (err) {
      setError(typeof err === "string" ? err : String(err));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div class="view">
      <h2>Capture (L0 texte)</h2>

      <section class="card">
        <div class="row between wrap">
          <div>
            <div class="muted small">Session courante</div>
            <code>{sessionId() || "…"}</code>
          </div>
          <button class="small" onClick={newSession} disabled={busy()}>
            Nouvelle session
          </button>
        </div>
        <Show when={info()}>
          {(i) => (
            <p class="muted small hint">
              Mode session : <strong>{i().mode}</strong> — en mode privé tout est
              capturé tel quel, en connecté les entrées sensibles seront filtrées
              à la lecture par le LLM.
            </p>
          )}
        </Show>
      </section>

      <form class="card" onSubmit={submit}>
        <label class="field">
          <span>Énoncé</span>
          <textarea
            value={text()}
            onInput={(e) => setText(e.currentTarget.value)}
            placeholder="Une ligne ou plusieurs. Entrée + Maj pour saut de ligne, Entrée seul pour envoyer."
            rows={3}
            disabled={busy() || !sessionId()}
            onKeyDown={(e) => {
              if (e.key === "Enter" && !e.shiftKey) {
                e.preventDefault();
                submit();
              }
            }}
          />
        </label>
        <div class="row between wrap">
          <label class="row gap-sm checkbox">
            <input
              type="checkbox"
              checked={sensitive()}
              onChange={(e) => setSensitive(e.currentTarget.checked)}
              disabled={busy()}
            />
            <span>
              Sensible (équivalent <code>sensitivity = true</code>)
            </span>
          </label>
          <button
            type="submit"
            class="primary"
            disabled={busy() || !text().trim() || !sessionId()}
          >
            {busy() ? "..." : "Enregistrer"}
          </button>
        </div>
        <Show when={error()}>
          {(msg) => <p class="error">{msg()}</p>}
        </Show>
      </form>

      <Show when={entries().length > 0}>
        <section class="card">
          <h3 class="muted small">
            {entries().length} énoncé(s) dans cette session
          </h3>
          <For each={entries()}>
            {(e) => (
              <article class="hit">
                <div class="muted small">
                  <code>{e.timestamp}</code> ·{" "}
                  {e.sensitivity ? "[s]" : "ouvert"}
                </div>
                <div class="content">{e.content}</div>
              </article>
            )}
          </For>
        </section>
      </Show>
    </div>
  );
};

export default Capture;
