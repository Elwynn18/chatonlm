// Faux bureau de la session virtuelle (e2e/lib/desktop-services.py, PyGObject
// du système) : notifications, portail GlobalShortcuts (dictée, correction du
// texte sélectionné) et zone de notification, pris sur le bus de la session
// avant le lancement de l'app.
// Le test lit ce que l'app leur a envoyé et déclenche ce que ferait
// l'utilisateur (raccourci appuyé, clic sur l'icône, entrée du menu). Jamais
// sur le bus du bureau réel : refusé ici hors de la session, et le script le
// vérifie aussi de son côté.
import { spawn } from "node:child_process";
import { createInterface } from "node:readline";
import { fileURLToPath } from "node:url";
import { PYTHON } from "./clipboard.mjs";

export const SERVICES = fileURLToPath(
  new URL("./desktop-services.py", import.meta.url),
);

const delay = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

/**
 * Démarre les faux services choisis :
 *   { notifications: bool, watcher: bool,
 *     portal: null | { version, bind: "ok"|"cancel"|"hang"|"empty", trigger,
 *                      triggers: { [id]: touche }, configureTrigger } }
 * Le portail lie un ou plusieurs raccourcis par `BindShortcuts` (évènement
 * « bind » : `ids`, `shortcuts` avec description et touche proposée,
 * `answered` : touches répondues) ; `command("tap", { id: "proofread" })`
 * appuie sur l'un d'eux, `command("shortcuts-changed", { id, trigger })`
 * change sa touche (toutes sans `id`).
 * Renvoie { events, command, waitFor, notifications, stop } :
 * - `events` : tous les évènements reçus, dans l'ordre ({ event, ... }) ;
 * - `command(cmd, args)` : commande du script (voir son en-tête), résultat ou
 *   rejet ;
 * - `waitFor(predicate, { timeoutMs })` : premier évènement (déjà reçu ou à
 *   venir) qui satisfait `predicate` ;
 * - `notifications()` : notifications reçues ({ id, summary, body, actions,
 *   hints, ... }) ; `command("notification-action", { id, key })` simule un
 *   clic sur une action.
 */
export async function startDesktopServices(
  options = { notifications: true, watcher: true, portal: {} },
  { timeoutMs = 10000 } = {},
) {
  if (process.env.CHATONLM_E2E_SESSION !== "1") {
    throw new Error(
      "Faux services du bureau refusés hors de la session virtuelle.",
    );
  }
  const child = spawn(PYTHON, [SERVICES, JSON.stringify(options)], {
    stdio: ["pipe", "pipe", "pipe"],
  });
  let stderr = "";
  child.stderr.setEncoding("utf8");
  child.stderr.on("data", (chunk) => (stderr += chunk));
  const exited = new Promise((resolve) => child.once("exit", resolve));
  const events = [];
  const replies = new Map();
  const listeners = new Set();
  createInterface({ input: child.stdout }).on("line", (line) => {
    let event;
    try {
      event = JSON.parse(line);
    } catch {
      return;
    }
    events.push(event);
    if (event.event === "reply" && replies.has(event.seq)) {
      const { resolve, reject } = replies.get(event.seq);
      replies.delete(event.seq);
      if ("error" in event) reject(new Error(`${event.error}`));
      else resolve(event.result);
    }
    for (const listener of listeners) listener(event);
  });

  const waitFor = (predicate, { timeoutMs: limit = 10000 } = {}) => {
    const found = events.find(predicate);
    if (found) return Promise.resolve(found);
    return new Promise((resolve, reject) => {
      const listener = (event) => {
        if (!predicate(event)) return;
        clearTimeout(timer);
        listeners.delete(listener);
        resolve(event);
      };
      const timer = setTimeout(() => {
        listeners.delete(listener);
        reject(
          new Error(
            `Évènement du faux bureau attendu en vain (${limit} ms) ; reçus : ${JSON.stringify(events.slice(-8))}\n${stderr}`,
          ),
        );
      }, limit);
      listeners.add(listener);
    });
  };

  await Promise.race([
    waitFor((event) => event.event === "ready", { timeoutMs }),
    exited.then((code) => {
      throw new Error(`Faux services arrêtés (code ${code}) :\n${stderr}`);
    }),
  ]);

  let seq = 0;
  const command = (cmd, args = {}, { timeoutMs: limit = 10000 } = {}) =>
    new Promise((resolve, reject) => {
      const id = ++seq;
      const timer = setTimeout(() => {
        replies.delete(id);
        reject(new Error(`Commande ${cmd} sans réponse après ${limit} ms`));
      }, limit);
      replies.set(id, {
        resolve: (value) => {
          clearTimeout(timer);
          resolve(value);
        },
        reject: (error) => {
          clearTimeout(timer);
          reject(new Error(`${cmd} : ${error.message}`));
        },
      });
      child.stdin.write(`${JSON.stringify({ seq: id, cmd, ...args })}\n`);
    });

  return {
    events,
    command,
    waitFor,
    notifications: () => events.filter((event) => event.event === "notify"),
    async stop() {
      child.stdin.end();
      const timer = setTimeout(() => child.kill("SIGKILL"), 3000);
      await exited;
      clearTimeout(timer);
    },
  };
}

/**
 * Attend que `read()` renvoie une valeur qui satisfait `predicate` (sondage
 * toutes les 100 ms), puis la renvoie ; échoue après `timeoutMs` avec la
 * dernière valeur lue.
 */
export async function poll(
  read,
  predicate,
  { timeoutMs = 10000, what = "" } = {},
) {
  const deadline = Date.now() + timeoutMs;
  let last;
  for (;;) {
    last = await read();
    if (predicate(last)) return last;
    if (Date.now() > deadline) {
      throw new Error(
        `Attente vaine ${what} ; dernière valeur : ${JSON.stringify(last)}`,
      );
    }
    await delay(100);
  }
}
