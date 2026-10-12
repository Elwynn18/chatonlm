// Journal de l'app lancée par withApp (sorties standard et d'erreur, dans
// .tools/logs/e2e-<nom>.log) lu au fil de l'eau : chaque ligne est datée à sa
// lecture (toutes les 20 ms), pour mesurer le délai entre une ligne du moteur
// et un évènement noté par la page (même horloge, celle de la machine).
import { closeSync, fstatSync, openSync, readSync } from "node:fs";
import { StringDecoder } from "node:string_decoder";
import { ROOT } from "./app.mjs";

const delay = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

/** Chemin du journal de l'app lancée par withApp sous ce nom. */
export const appLogPath = (name) => `${ROOT}.tools/logs/e2e-${name}.log`;

/**
 * Suit le journal du lancement `name` (à appeler une fois l'app lancée, dans
 * la fonction de withApp). Renvoie { lines, matching, waitFor, stop } :
 * `lines` ({ at, text }, `at` = Date.now() de la lecture), `matching(regex)`
 * (lignes qui correspondent, journal relu d'abord), `waitFor(regex, what,
 * timeoutMs)` (première ligne qui correspond, déjà lue ou à venir), `stop()`
 * (à appeler en fin d'essai : arrête la lecture périodique).
 */
export function watchLog(name, { intervalMs = 20 } = {}) {
  const file = appLogPath(name);
  const lines = [];
  const decoder = new StringDecoder("utf8");
  let offset = 0;
  let rest = "";

  const read = () => {
    let fd;
    try {
      fd = openSync(file, "r");
    } catch {
      return;
    }
    try {
      const size = fstatSync(fd).size;
      if (size <= offset) return;
      const buffer = Buffer.alloc(size - offset);
      const count = readSync(fd, buffer, 0, buffer.length, offset);
      offset += count;
      const parts = (rest + decoder.write(buffer.subarray(0, count))).split(
        "\n",
      );
      rest = parts.pop();
      const at = Date.now();
      for (const text of parts) lines.push({ at, text });
    } finally {
      closeSync(fd);
    }
  };

  const timer = setInterval(read, intervalMs);
  read();

  const matching = (pattern) => {
    read();
    return lines.filter((line) => pattern.test(line.text));
  };

  return {
    lines,
    matching,
    async waitFor(pattern, what, timeoutMs = 30000) {
      const deadline = Date.now() + timeoutMs;
      for (;;) {
        const found = matching(pattern)[0];
        if (found) return found;
        if (Date.now() > deadline) {
          throw new Error(
            `Ligne attendue en vain dans ${file} (${what}) : ${pattern}`,
          );
        }
        await delay(intervalMs);
      }
    },
    stop() {
      clearInterval(timer);
      read();
    },
  };
}
