// Vraies touches du clavier dans la session virtuelle (e2e/lib/keyboard.py,
// client libei de l'interface EIS de KWin) : Tab déplace vraiment le focus,
// Entrée et Espace activent vraiment un bouton, contrairement aux touches du
// plugin WebDriver (KeyboardEvent simulés en JavaScript). Jamais sur le bureau
// réel : refusé ici hors de la session, et le client vérifie de son côté la
// session et le KWin visé (lancé avec --virtual et la socket de la session).
import { execFile } from "node:child_process";
import { fileURLToPath } from "node:url";
import { PYTHON } from "./clipboard.mjs";

export const KEYBOARD = fileURLToPath(
  new URL("./keyboard.py", import.meta.url),
);

/** Codes de sortie du client. */
const EXIT_REASONS = {
  2: "pas de clavier EIS dans le délai",
  3: "hors de la session virtuelle",
};

/** Touche (sans Maj) de chaque caractère non alphanumérique connu. */
const CHARACTER_KEYS = {
  " ": "space",
  "/": "slash",
  ".": "period",
  "-": "minus",
  _: "shift+minus",
};

/**
 * Accords qui tapent `text` (disposition qwerty de la session : lettres,
 * chiffres, espace, « / », « . », « - », « _ ») ; autre caractère : erreur.
 */
export function chordsOf(text) {
  return [...text].map((character) => {
    if (/^[a-z0-9]$/.test(character)) return character;
    if (/^[A-Z]$/.test(character)) return `shift+${character.toLowerCase()}`;
    const key = CHARACTER_KEYS[character];
    if (!key) throw new Error(`Caractère sans touche connue : ${character}`);
    return key;
  });
}

/**
 * Envoie des accords (« tab », « shift+tab », « enter », « escape »...) à la
 * fenêtre qui a le focus clavier, un à la fois, puis attend `settleMs` que la
 * page les traite. Accepte aussi les actions du pointeur de keyboard.py
 * (« pointer:x,y », « button:down », « button:up », « wait:ms ») : un geste
 * entier en un seul appel, KWin relâchant le bouton à la fin du client.
 */
export function pressKeys(chords, { settleMs = 150, timeoutMs = 10000 } = {}) {
  if (process.env.CHATONLM_E2E_SESSION !== "1") {
    return Promise.reject(
      new Error("Touches refusées hors de la session virtuelle."),
    );
  }
  const list = Array.isArray(chords) ? chords : [chords];
  return new Promise((resolve, reject) => {
    execFile(
      PYTHON,
      [KEYBOARD, ...list],
      { encoding: "utf8", timeout: timeoutMs, killSignal: "SIGKILL" },
      (error, stdout, stderr) => {
        if (error) {
          const reason = error.killed
            ? `aucune réponse après ${timeoutMs} ms`
            : (EXIT_REASONS[error.code] ?? error.message);
          reject(new Error(`Touches ${list.join(" ")} : ${reason}\n${stderr}`));
          return;
        }
        const line = stdout.trim().split("\n").at(-1) ?? "";
        if (JSON.parse(line).sent !== list.length) {
          reject(new Error(`Touches ${list.join(" ")} : réponse ${line}`));
          return;
        }
        setTimeout(resolve, settleMs);
      },
    );
  });
}
