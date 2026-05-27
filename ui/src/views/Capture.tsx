import {
  createResource,
  createSignal,
  For,
  onCleanup,
  onMount,
  Show,
  type Component,
} from "solid-js";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import {
  api,
  type AudioConfig,
  type L0Entry,
  type VoiceErrorEvent,
  type VoiceLevelEvent,
} from "../api";
import { refreshDraftsCount } from "../store";

type Props = {
  onGoToReview?: () => void;
};

const Capture: Component<Props> = (props) => {
  const [sessionId, setSessionId] = createSignal<string>("");
  const [text, setText] = createSignal("");
  const [sensitive, setSensitive] = createSignal(true);
  const [entries, setEntries] = createSignal<L0Entry[]>([]);
  const [error, setError] = createSignal<string | null>(null);
  const [busy, setBusy] = createSignal(false);

  // État spécifique à la capture vocale.
  const [audioCfg, setAudioCfg] = createSignal<AudioConfig | null>(null);
  const [recording, setRecording] = createSignal(false);
  const [level, setLevel] = createSignal(0);
  const [voiceError, setVoiceError] = createSignal<string | null>(null);

  // État du pipeline "Traiter cette session" (segment → extract → review).
  type ProcessStep = "idle" | "segmenting" | "extracting" | "done";
  const [processStep, setProcessStep] = createSignal<ProcessStep>("idle");
  const [processResult, setProcessResult] = createSignal<string | null>(null);
  const [processError, setProcessError] = createSignal<string | null>(null);

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
    if (recording()) {
      // Évite de scinder un enregistrement entre deux sessions sans s'en
      // rendre compte. L'utilisateur arrête lui-même puis recommence.
      setError("arrête d'abord l'enregistrement vocal");
      return;
    }
    const id = await api.sessionNew();
    setSessionId(id);
    setEntries([]);
    setError(null);
    setVoiceError(null);
  };

  // Abonnements aux événements Tauri. Une seule fois au mount, sans gating sur
  // `recording` : le backend n'émet rien quand aucune capture n'est active.
  let unlistens: UnlistenFn[] = [];
  onMount(async () => {
    try {
      const cfg = await api.audioCheckConfig();
      setAudioCfg(cfg);
    } catch (e) {
      setVoiceError(typeof e === "string" ? e : String(e));
    }
    unlistens = await Promise.all([
      listen<L0Entry>("voice/transcript", (ev) => {
        setEntries([ev.payload, ...entries()]);
      }),
      listen<VoiceLevelEvent>("voice/level", (ev) => {
        setLevel(ev.payload.rms);
      }),
      listen<VoiceErrorEvent>("voice/error", (ev) => {
        setVoiceError(ev.payload.message);
      }),
    ]);
  });

  onCleanup(() => {
    for (const u of unlistens) u();
    if (recording()) {
      api.audioStopRecording().catch(() => {});
    }
  });

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

  const startVoice = async () => {
    if (!sessionId() || recording()) return;
    setVoiceError(null);
    try {
      await api.audioStartRecording(sessionId(), sensitive());
      setRecording(true);
    } catch (err) {
      setVoiceError(typeof err === "string" ? err : String(err));
    }
  };

  const processSession = async () => {
    if (!sessionId() || entries().length === 0) return;
    if (processStep() !== "idle" && processStep() !== "done") return;
    setProcessError(null);
    setProcessResult(null);
    try {
      setProcessStep("segmenting");
      const seg = await api.segmentRun(sessionId());
      setProcessStep("extracting");
      const ext = await api.extractFacts(seg.segmentation_id);
      setProcessStep("done");
      setProcessResult(
        `${seg.blocks_count} bloc(s) segmenté(s), ${ext.drafts_count} draft(s) extrait(s).`,
      );
      refreshDraftsCount();
      // CH21 : embeddings en arrière-plan, sans bloquer la navigation.
      api.embeddingsRun().catch(() => {});
      props.onGoToReview?.();
    } catch (err) {
      setProcessStep("idle");
      setProcessError(typeof err === "string" ? err : String(err));
    }
  };

  const stopVoice = async () => {
    if (!recording()) return;
    try {
      await api.audioStopRecording();
    } catch (err) {
      setVoiceError(typeof err === "string" ? err : String(err));
    } finally {
      setRecording(false);
      setLevel(0);
    }
  };

  // Le niveau RMS plafonne en pratique vers ~0.2 sur une voix normale.
  // On amplifie pour avoir une jauge utile à l'œil.
  const levelPct = () => Math.min(100, Math.round(level() * 400));

  return (
    <div class="view">
      <h2>Capture (L0)</h2>

      <section class="card">
        <div class="row between wrap">
          <div>
            <div class="muted small">Session courante</div>
            <code>{sessionId() || "…"}</code>
          </div>
          <button
            class="small"
            onClick={newSession}
            disabled={busy() || recording()}
          >
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

      <section class="card">
        <div class="row between wrap gap-sm">
          <div>
            <h3 class="muted small" style="margin:0">Pipeline L0 → L2</h3>
            <p class="muted small hint" style="margin:0.25rem 0 0">
              Segmente cette session, extrait les drafts L2, puis ouvre la vue
              Valider. Les embeddings sont recalculés en tâche de fond.
            </p>
          </div>
          <button
            type="button"
            class="primary"
            onClick={processSession}
            disabled={
              !sessionId() ||
              entries().length === 0 ||
              recording() ||
              (processStep() !== "idle" && processStep() !== "done")
            }
          >
            {processStep() === "segmenting"
              ? "Segmentation…"
              : processStep() === "extracting"
                ? "Extraction…"
                : "Traiter cette session"}
          </button>
        </div>
        <Show when={processResult()}>
          {(msg) => <p class="muted small hint">{msg()}</p>}
        </Show>
        <Show when={processError()}>
          {(msg) => <p class="error">{msg()}</p>}
        </Show>
      </section>

      <section class="card">
        <h3 class="muted small">Voix (whisper.cpp + VAD)</h3>
        <Show
          when={audioCfg()?.model_set}
          fallback={
            <p class="muted small hint">
              Modèle whisper non configuré. Définir <code>AZ_WHISPER_MODEL</code>{" "}
              (chemin vers un fichier <code>ggml-*.bin</code>) avant de lancer
              l'UI, puis relancer.
            </p>
          }
        >
          <div class="row between wrap gap-sm">
            <Show
              when={recording()}
              fallback={
                <button
                  type="button"
                  class="primary"
                  onClick={startVoice}
                  disabled={!sessionId()}
                >
                  Enregistrer
                </button>
              }
            >
              <button type="button" class="danger" onClick={stopVoice}>
                Arrêter
              </button>
            </Show>
            <div class="muted small">
              Langue : <code>{audioCfg()?.language ?? "auto"}</code>
              {recording() ? " · en écoute…" : ""}
            </div>
          </div>
          <div class="rms-bar" aria-hidden="true">
            <span style={{ width: `${levelPct()}%` }} />
          </div>
        </Show>
        <Show when={voiceError()}>
          {(msg) => <p class="error">{msg()}</p>}
        </Show>
      </section>

      <form class="card" onSubmit={submit}>
        <label class="field">
          <span>Énoncé (texte)</span>
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
            {busy() ? "..." : "Envoyer"}
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
                  <code>{e.timestamp}</code> · {e.source}
                  {" · "}
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
