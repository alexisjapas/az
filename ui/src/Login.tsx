import { createSignal, Show, type Component } from "solid-js";
import { authCreate, authUnlock, type AuthStatus } from "./auth";

type Props = {
  status: AuthStatus;
  onUnlocked: () => void;
};

const Login: Component<Props> = (props) => {
  const isFirstTime = () => !props.status.salt_exists;
  const [password, setPassword] = createSignal("");
  const [confirm, setConfirm] = createSignal("");
  const [error, setError] = createSignal<string | null>(null);
  const [busy, setBusy] = createSignal(false);

  const submit = async (e: SubmitEvent) => {
    e.preventDefault();
    setError(null);
    setBusy(true);
    try {
      if (isFirstTime()) {
        await authCreate(password(), confirm());
      } else {
        await authUnlock(password());
      }
      setPassword("");
      setConfirm("");
      props.onUnlocked();
    } catch (err) {
      setError(typeof err === "string" ? err : String(err));
    } finally {
      setBusy(false);
    }
  };

  return (
    <main class="app login">
      <header class="app-header">
        <h1>AZ</h1>
        <p class="tagline">
          {isFirstTime()
            ? "Création d'une nouvelle base chiffrée"
            : "Déverrouillage de la base"}
        </p>
      </header>

      <form class="card" onSubmit={submit}>
        <label class="field">
          <span>Mot de passe</span>
          <input
            type="password"
            autocomplete={isFirstTime() ? "new-password" : "current-password"}
            value={password()}
            onInput={(e) => setPassword(e.currentTarget.value)}
            disabled={busy()}
            autofocus
            required
          />
        </label>

        <Show when={isFirstTime()}>
          <label class="field">
            <span>Confirmation</span>
            <input
              type="password"
              autocomplete="new-password"
              value={confirm()}
              onInput={(e) => setConfirm(e.currentTarget.value)}
              disabled={busy()}
              required
            />
          </label>
        </Show>

        <Show when={error()}>
          {(msg) => <p class="error">{msg()}</p>}
        </Show>

        <button type="submit" class="primary" disabled={busy()}>
          {busy()
            ? "..."
            : isFirstTime()
              ? "Créer la base"
              : "Déverrouiller"}
        </button>

        <p class="hint">
          Base : <code>{props.status.db_path}</code>
        </p>
      </form>
    </main>
  );
};

export default Login;
