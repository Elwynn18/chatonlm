// Quitter par Ctrl+Q (propre à ChatonLM) : ce qui n'est pas encore enregistré
// (brouillon des consignes de la dictée...) l'est d'abord, deux secondes au
// plus, puis l'app quitte.

import { quitApp } from "@/lib/ipc";

/** Attente maximale des enregistrements avant de quitter. */
export const BEFORE_QUIT_LIMIT_MS = 2000;

type Task = () => Promise<unknown>;

const tasks = new Set<Task>();

/** Tâche lancée avant de quitter ; renvoie de quoi la retirer. */
export function onBeforeQuit(task: Task): () => void {
  tasks.add(task);
  return () => {
    tasks.delete(task);
  };
}

/** Enregistre ce qui attend (échecs ignorés), puis quitte. */
export async function quit(): Promise<void> {
  const pending = Promise.allSettled(
    [...tasks].map((task) => Promise.resolve().then(task)),
  );
  await Promise.race([
    pending,
    new Promise((resolve) => setTimeout(resolve, BEFORE_QUIT_LIMIT_MS)),
  ]);
  await quitApp();
}
