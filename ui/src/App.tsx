import {
  createResource,
  createSignal,
  Match,
  Show,
  Switch,
  type Component,
} from "solid-js";
import { api, type AppInfo, type SessionMode } from "./api";
import { authLock, authStatus } from "./auth";
import { draftsCount, refreshDraftsCount, resetDraftsCount } from "./store";
import Login from "./Login";
import Dashboard from "./views/Dashboard";
import Capture from "./views/Capture";
import Captures from "./views/Captures";
import Facts from "./views/Facts";
import Review from "./views/Review";
import Search from "./views/Search";

type ViewKey = "dashboard" | "capture" | "captures" | "facts" | "review" | "search";

const VIEWS: { key: ViewKey; label: string }[] = [
  { key: "dashboard", label: "Tableau de bord" },
  { key: "capture", label: "Capture" },
  { key: "captures", label: "Captures (L0)" },
  { key: "facts", label: "Faits (L2)" },
  { key: "review", label: "Valider drafts" },
  { key: "search", label: "Recherche" },
];

const App: Component = () => {
  const [status, { refetch: refetchStatus }] = createResource(authStatus);
  const [info, { refetch: refetchInfo }] = createResource<AppInfo>(api.appInfo);
  const [view, setView] = createSignal<ViewKey>("dashboard");

  const onUnlocked = async () => {
    await refetchStatus();
    await refetchInfo();
    refreshDraftsCount();
  };

  const onLock = async () => {
    await authLock();
    resetDraftsCount();
    await refetchStatus();
    await refetchInfo();
  };

  const onSetMode = async (m: SessionMode) => {
    await api.sessionSetMode(m);
    await refetchInfo();
  };

  return (
    <Switch fallback={<main class="app"><p>Chargement…</p></main>}>
      <Match when={status.loading || info.loading}>
        <main class="app"><p>Chargement…</p></main>
      </Match>
      <Match when={status.error}>
        <main class="app">
          <h1>AZ</h1>
          <p class="error">Erreur : {String(status.error)}</p>
        </main>
      </Match>
      <Match when={status() && !status()!.unlocked}>
        <Login status={status()!} onUnlocked={onUnlocked} />
      </Match>
      <Match when={status() && status()!.unlocked && info()}>
        <div class="shell">
          <aside class="sidebar">
            <div class="brand">
              <h1>AZ</h1>
              <p class="muted small">v{info()!.version}</p>
            </div>
            <nav>
              {VIEWS.map((v) => (
                <button
                  classList={{ nav: true, active: view() === v.key }}
                  onClick={() => setView(v.key)}
                >
                  {v.label}
                  <Show when={v.key === "review" && draftsCount() > 0}>
                    <span class="badge">{draftsCount()}</span>
                  </Show>
                </button>
              ))}
            </nav>
            <div class="sidebar-footer">
              <div class="field inline">
                <span class="muted small">Mode session</span>
                <div class="row gap-sm">
                  <button
                    class={info()!.mode === "private" ? "primary small" : "small"}
                    onClick={() => onSetMode("private")}
                  >
                    Privé
                  </button>
                  <button
                    class={info()!.mode === "connected" ? "primary small" : "small"}
                    onClick={() => onSetMode("connected")}
                  >
                    Connecté
                  </button>
                </div>
              </div>
              <button class="ghost small full" onClick={onLock}>
                Verrouiller
              </button>
            </div>
          </aside>

          <main class="main">
            <Show when={view() === "dashboard"}><Dashboard /></Show>
            <Show when={view() === "capture"}>
              <Capture onGoToReview={() => setView("review")} />
            </Show>
            <Show when={view() === "captures"}><Captures /></Show>
            <Show when={view() === "facts"}><Facts /></Show>
            <Show when={view() === "review"}><Review /></Show>
            <Show when={view() === "search"}><Search /></Show>
          </main>
        </div>
      </Match>
    </Switch>
  );
};

export default App;
