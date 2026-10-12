// Presse-papiers de la session virtuelle, lu ou rempli comme le ferait une
// autre application (e2e/lib/clipboard-reader.py et clipboard-writer.py,
// PyGObject du système) : vérifie ce qu'une copie de l'app a vraiment déposé,
// et ce que l'app fait d'un vrai collage. Jamais celui du bureau réel : refusé
// ici hors de la session, et les deux scripts le vérifient aussi de leur côté.
import { execFile, spawn } from "node:child_process";
import { existsSync } from "node:fs";
import { createInterface } from "node:readline";
import { fileURLToPath } from "node:url";

export const READER = fileURLToPath(
  new URL("./clipboard-reader.py", import.meta.url),
);

export const WRITER = fileURLToPath(
  new URL("./clipboard-writer.py", import.meta.url),
);

/** Python du système (PyGObject), pas celui d'un environnement virtuel. */
export const PYTHON = existsSync("/usr/bin/python3")
  ? "/usr/bin/python3"
  : "python3";

/** Codes de sortie du lecteur. */
const EXIT_REASONS = {
  2: "pas de focus clavier (fenêtre du lecteur non activée)",
  3: "hors de la session virtuelle",
};

/**
 * Lit le presse-papiers : { text, html, targets } ; `text` et `html` valent
 * null si le type manque, `targets` liste les types proposés. À appeler avant
 * de fermer l'app, qui reste propriétaire du contenu copié (pas de gestionnaire
 * de presse-papiers dans la session). Le lecteur prend le focus clavier le
 * temps de la lecture ; KWin le rend ensuite à la fenêtre précédente.
 * Rejette hors de la session, sans focus après `focusMs`, ou au-delà de
 * `timeoutMs` (lecteur tué).
 */
export function readClipboard({ focusMs = 3000, timeoutMs = 10000 } = {}) {
  if (process.env.CHATONLM_E2E_SESSION !== "1") {
    return Promise.reject(
      new Error(
        "Lecture du presse-papiers refusée hors de la session virtuelle.",
      ),
    );
  }
  return new Promise((resolve, reject) => {
    execFile(
      PYTHON,
      [READER, String(focusMs)],
      { encoding: "utf8", timeout: timeoutMs, killSignal: "SIGKILL" },
      (error, stdout, stderr) => {
        if (error) {
          const reason = error.killed
            ? `aucune réponse après ${timeoutMs} ms`
            : (EXIT_REASONS[error.code] ?? error.message);
          reject(
            new Error(
              `Lecture du presse-papiers : ${reason}\n${stderr}`.trim(),
            ),
          );
          return;
        }
        // Une seule ligne JSON ; GTK n'écrit que sur la sortie d'erreur.
        const line = stdout.trim().split("\n").at(-1) ?? "";
        try {
          const { text, html, targets } = JSON.parse(line);
          resolve({ text, html, targets });
        } catch {
          reject(
            new Error(`Lecture du presse-papiers : réponse illisible ${line}`),
          );
        }
      },
    );
  });
}

/**
 * Sélection primaire tenue par une autre application (voir `holdClipboard`) :
 * `text` surligné, comme un texte sélectionné à la souris dans un éditeur.
 */
export function holdSelection(text, options) {
  return holdClipboard({ text, primary: true }, options);
}

/** Codes de sortie de l'écrivain. */
const WRITER_EXIT_REASONS = {
  2: "pas de focus clavier (fenêtre de l'écrivain non activée)",
  3: "hors de la session virtuelle",
};

/**
 * Met `text` (types texte de GTK) et/ou `data` ({ type MIME: Buffer }) dans le
 * presse-papiers, comme une autre application. L'écrivain prend le focus
 * clavier le temps de poser la sélection, puis détruit sa fenêtre (KWin rend
 * le focus à la fenêtre précédente) et reste propriétaire du contenu jusqu'à
 * `release()`. Renvoie { targets, requested, release } : `targets` liste les
 * types proposés, `requested` (tableau tenu à jour) les types demandés par un
 * lecteur, dans l'ordre (« cleared » si un autre client a pris la sélection). Rejette hors de la session, sans focus après
 * `focusMs`, ou sans réponse après `timeoutMs` (écrivain tué).
 * `primary: true` : sélection primaire (texte surligné dans une application)
 * au lieu du presse-papiers ; le presse-papiers ne change pas.
 */
export function holdClipboard(
  { text = null, data = {}, image = null, primary = false },
  { focusMs = 3000, timeoutMs = 10000 } = {},
) {
  if (process.env.CHATONLM_E2E_SESSION !== "1") {
    return Promise.reject(
      new Error(
        "Écriture du presse-papiers refusée hors de la session virtuelle.",
      ),
    );
  }
  const spec = JSON.stringify({
    text,
    primary,
    image: image && Buffer.from(image).toString("base64"),
    data: Object.fromEntries(
      Object.entries(data).map(([mime, bytes]) => [
        mime,
        Buffer.from(bytes).toString("base64"),
      ]),
    ),
  });
  const writer = spawn(PYTHON, [WRITER, spec, String(focusMs)], {
    stdio: ["pipe", "pipe", "pipe"],
  });
  let stderr = "";
  writer.stderr.setEncoding("utf8");
  writer.stderr.on("data", (chunk) => (stderr += chunk));
  const exited = new Promise((resolve) => writer.once("exit", resolve));
  const requested = [];
  const release = async () => {
    writer.stdin.end();
    const timer = setTimeout(() => writer.kill("SIGKILL"), 3000);
    await exited;
    clearTimeout(timer);
  };
  return new Promise((resolve, reject) => {
    const fail = (reason) => {
      writer.kill("SIGKILL");
      reject(new Error(`Écriture du presse-papiers : ${reason}\n${stderr}`));
    };
    const timer = setTimeout(
      () => fail(`aucune réponse après ${timeoutMs} ms`),
      timeoutMs,
    );
    const early = (code) => {
      clearTimeout(timer);
      fail(WRITER_EXIT_REASONS[code] ?? `fin prématurée (code ${code})`);
    };
    writer.once("exit", early);
    // Une ligne JSON par évènement ; GTK n'écrit que sur la sortie d'erreur.
    createInterface({ input: writer.stdout }).on("line", (line) => {
      let event;
      try {
        event = JSON.parse(line);
      } catch {
        return;
      }
      if (event.event === "get") requested.push(event.target);
      if (event.event === "cleared") requested.push("cleared");
      if (event.event === "ready") {
        clearTimeout(timer);
        writer.removeListener("exit", early);
        resolve({ targets: event.targets, requested, release });
      }
    });
  });
}
