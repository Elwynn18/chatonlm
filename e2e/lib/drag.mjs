// Vrai glisser-déposer natif dans la session virtuelle : une source GTK 3
// (e2e/lib/drag-source.py, comme un gestionnaire de fichiers) offre des
// fichiers en text/uri-list ; le pointeur de KWin (actions pointer:, button:
// de e2e/lib/keyboard.py) la saisit et la dépose sur l'app. Jamais sur le
// bureau réel : refusé ici hors de la session, et les deux scripts le
// vérifient aussi de leur côté.
import { spawn } from "node:child_process";
import { createInterface } from "node:readline";
import { fileURLToPath } from "node:url";
import { PYTHON } from "./clipboard.mjs";

export const DRAG_SOURCE = fileURLToPath(
  new URL("./drag-source.py", import.meta.url),
);

/** Titre de la fenêtre de la source (placée par un script KWin). */
export const DRAG_SOURCE_CAPTION = "chatonlm-e2e-drag-source";

/**
 * Affiche la source de glisser des fichiers `paths`. Renvoie { events, stop } :
 * `events` (tenu à jour) liste les évènements de la source (« drag-begin »,
 * « data-get », « drag-end »), `stop()` la ferme. Rejette hors de la session
 * ou si la fenêtre n'est pas affichée après `timeoutMs`.
 */
export function startDragSource(paths, { timeoutMs = 10000 } = {}) {
  if (process.env.CHATONLM_E2E_SESSION !== "1") {
    return Promise.reject(
      new Error("Source de glisser refusée hors de la session virtuelle."),
    );
  }
  const source = spawn(PYTHON, [DRAG_SOURCE, ...paths], {
    stdio: ["pipe", "pipe", "pipe"],
  });
  let stderr = "";
  source.stderr.setEncoding("utf8");
  source.stderr.on("data", (chunk) => (stderr += chunk));
  const exited = new Promise((resolve) => source.once("exit", resolve));
  const events = [];
  const stop = async () => {
    source.stdin.end();
    const timer = setTimeout(() => source.kill("SIGKILL"), 3000);
    await exited;
    clearTimeout(timer);
  };
  const errors = () => stderr;
  return new Promise((resolve, reject) => {
    const fail = (reason) => {
      source.kill("SIGKILL");
      reject(new Error(`Source de glisser : ${reason}\n${stderr}`));
    };
    const timer = setTimeout(
      () => fail(`pas affichée après ${timeoutMs} ms`),
      timeoutMs,
    );
    const early = (code) => {
      clearTimeout(timer);
      fail(code === 3 ? "hors de la session virtuelle" : `code ${code}`);
    };
    source.once("exit", early);
    // Une ligne JSON par évènement ; GTK n'écrit que sur la sortie d'erreur.
    createInterface({ input: source.stdout }).on("line", (line) => {
      let event;
      try {
        event = JSON.parse(line);
      } catch {
        return;
      }
      if (event.event !== "shown") {
        events.push(event.event);
        return;
      }
      clearTimeout(timer);
      source.removeListener("exit", early);
      resolve({ events, stop, errors });
    });
  });
}
