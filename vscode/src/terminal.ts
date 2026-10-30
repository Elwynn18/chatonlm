// Dernière commande du terminal qui a échoué, pour le chaton en caoutchouc :
// intégration du shell de VS Code (stable depuis 1.93). `execution.read()`
// ne rend que ce qui est écrit après son premier appel : il est appelé dès le
// début de chaque commande, et la sortie gardée dans un tampon borné. Sans
// intégration du shell (cmd.exe, shell inconnu), rien. Suivi seulement une
// fois l'éditeur appairé (relecture 90, D5). Contrat :
// .tools/research/89-lot2-voice-contract.md (X, « Terminal »).
import * as vscode from "vscode";

/** Caractères de la sortie gardés pour le modèle (les derniers). */
export const TERMINAL_OUTPUT_CHARS = 4000;
/** Tampon d'une commande en cours (au-delà, le début est oublié). */
export const BUFFER_CHARS = 64_000;
/** Attente de la fin de la sortie après la fin de la commande. */
const DRAIN_MS = 500;

export type TerminalFailure = {
  command: string;
  exitCode: number;
  /** Fin de la sortie, sans séquences d'échappement. */
  output: string;
};

const ESC = String.fromCharCode(27);
const BEL = String.fromCharCode(7);
/** OSC (titre, intégration du shell…), terminé par BEL ou ESC \. */
const OSC = new RegExp(`${ESC}\\][^${BEL}${ESC}]*(?:${BEL}|${ESC}\\\\)?`, "g");
/** CSI (couleurs, curseur…). */
const CSI = new RegExp(`${ESC}\\[[0-?]*[ -/]*[@-~]`, "g");
/** Autres séquences à deux caractères (ESC suivi d'une lettre, de = ou >…). */
const SHORT = new RegExp(`${ESC}[=>@-Z\\\\-_]`, "g");
/** Choix d'un jeu de caractères (`ESC ( B`, émis par `tput sgr0`). */
const CHARSET = new RegExp(`${ESC}[()*+\\-./].`, "g");

/** Sortie lisible : séquences ANSI retirées ; une ligne réécrite par « \r »
 * (barre de progression) garde son dernier état. */
export function stripAnsi(text: string): string {
  return text
    .replace(OSC, "")
    .replace(CSI, "")
    .replace(CHARSET, "")
    .replace(SHORT, "")
    .replaceAll(ESC, "")
    .split("\n")
    .map((line) => {
      const clean = line.replace(/\r+$/, "");
      const cut = clean.lastIndexOf("\r");
      return cut === -1 ? clean : clean.slice(cut + 1);
    })
    .join("\n");
}

type Execution = vscode.TerminalShellExecution;

type Buffer = { chunks: string[]; size: number };
type Running = Buffer & { done: Promise<void> };

/** Sortie ajoutée au tampon borné (la fin est gardée, même d'un seul
 * morceau énorme). */
export function pushOutput(buffer: Buffer, data: string): void {
  const kept = data.length > BUFFER_CHARS ? data.slice(-BUFFER_CHARS) : data;
  buffer.chunks.push(kept);
  buffer.size += kept.length;
  while (buffer.size > BUFFER_CHARS && buffer.chunks.length > 1) {
    buffer.size -= buffer.chunks.shift()?.length ?? 0;
  }
}

/** Suit les commandes des terminaux ; garde la dernière qui a échoué. */
export class TerminalWatcher {
  #last: TerminalFailure | null = null;
  readonly #running = new Map<Execution, Running>();
  readonly #listeners = new Set<() => void>();
  #disposables: vscode.Disposable[] = [];
  /** Numéro de la dernière fin de commande vue : une fin plus ancienne
   * (sortie plus lente à finir) ne remplace pas une plus récente. */
  #ends = 0;
  #lastEnd = 0;

  /** Suit les terminaux (éditeur appairé), ou plus du tout (rien gardé). */
  watch(on: boolean): void {
    if (on === this.#disposables.length > 0) return;
    if (!on) {
      for (const disposable of this.#disposables) disposable.dispose();
      this.#disposables = [];
      this.#running.clear();
      if (this.#last) {
        this.#last = null;
        for (const listener of this.#listeners) listener();
      }
      return;
    }
    const { window } = vscode;
    // API détectée (éditeurs anciens, ou dérivés qui ne l'ont pas).
    if (
      typeof window.onDidStartTerminalShellExecution !== "function" ||
      typeof window.onDidEndTerminalShellExecution !== "function"
    ) {
      return;
    }
    this.#disposables = [
      window.onDidStartTerminalShellExecution((event) =>
        this.#started(event.execution),
      ),
      window.onDidEndTerminalShellExecution(
        (event) => void this.#ended(event.execution, event.exitCode),
      ),
    ];
  }

  /** Dernière commande échouée, ou null. */
  get last(): TerminalFailure | null {
    return this.#last;
  }

  onChange(listener: () => void): () => void {
    this.#listeners.add(listener);
    return () => this.#listeners.delete(listener);
  }

  dispose(): void {
    for (const disposable of this.#disposables) disposable.dispose();
    this.#disposables = [];
    this.#running.clear();
    this.#listeners.clear();
  }

  #started(execution: Execution): void {
    const running: Running = { chunks: [], size: 0, done: Promise.resolve() };
    let stream: AsyncIterable<string>;
    try {
      // Tout de suite : ce qui est écrit avant le premier appel est perdu.
      stream = execution.read();
    } catch {
      return;
    }
    running.done = (async () => {
      try {
        for await (const data of stream) pushOutput(running, data);
      } catch {
        // Terminal fermé pendant la lecture : on garde ce qui est lu.
      }
    })();
    this.#running.set(execution, running);
  }

  async #ended(
    execution: Execution,
    exitCode: number | undefined,
  ): Promise<void> {
    const running = this.#running.get(execution);
    this.#running.delete(execution);
    // Code inconnu (Ctrl+C, terminal fermé) ou réussite : rien à garder.
    if (exitCode === undefined || exitCode === 0) return;
    const end = ++this.#ends;
    if (running) {
      await Promise.race([
        running.done,
        new Promise((resolve) => setTimeout(resolve, DRAIN_MS)),
      ]);
    }
    // Une commande finie après celle-ci est déjà gardée (ou suivi arrêté).
    if (end < this.#lastEnd || this.#disposables.length === 0) return;
    this.#lastEnd = end;
    const output = stripAnsi((running?.chunks ?? []).join(""))
      .trimEnd()
      .slice(-TERMINAL_OUTPUT_CHARS);
    this.#last = {
      command: stripAnsi(execution.commandLine.value).trim(),
      exitCode,
      output,
    };
    for (const listener of this.#listeners) listener();
  }
}
