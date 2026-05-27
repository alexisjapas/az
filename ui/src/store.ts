import { createSignal } from "solid-js";
import { api } from "./api";

// Compteur global des drafts L2 en attente de validation. Permet d'afficher
// un badge dans la sidebar et de signaler du travail en cours sans imposer
// un refetch dans chaque vue.
const [draftsCount, setDraftsCount] = createSignal(0);

export { draftsCount };

export async function refreshDraftsCount(): Promise<void> {
  try {
    const s = await api.summary();
    setDraftsCount(s.facts_drafts);
  } catch {
    // Silencieux : un échec de summary (DB verrouillée, etc.) ne doit pas
    // casser l'UI. La prochaine action de l'utilisateur re-déclenchera un
    // refresh quand le contexte sera valide.
  }
}

export function resetDraftsCount(): void {
  setDraftsCount(0);
}
