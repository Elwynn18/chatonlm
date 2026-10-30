// Easy Link prévient à la fin d'une tâche longue (lot 4a) : fin d'une
// commande du terminal (intégration du shell), d'une tâche de VS Code, d'une
// réponse de ChatonLM, au-delà d'un seuil (réglage, 60 s), ou de la commande
// « armée » par « Notify me when this finishes ». Coupé par défaut
// (`chatonlm.notify.enabled`). Rien d'autre que le titre (nom de la commande,
// sans arguments ni chemin), la durée, l'issue et, si l'utilisateur l'a
// permis, le nom du dossier ouvert ne part vers l'app (`notify.send`) ; les
// tâches en cours depuis plus de 10 s sont tenues à jour pour « /status »
// d'Easy Link (`notify.task`). Contrat : .tools/research/93-lot4a-contract.md (1).
import path from "node:path";
import * as vscode from "vscode";
import { RpcError } from "./bridge/rpc";
import type { LinkLike } from "./controller";
import { hasSecretValue } from "./receipt";
import type { CommandEnd, CommandStart } from "./terminal";

/** Codes d'erreur du lot 4a (contrat 93). */
export const LinkCodes = {
  /** Easy Link non relié, ou aucun compte lié. */
  notLinked: -32034,
  /** Réglage de l'app « Let VS Code notify me » coupé. */
  notifyOff: -32035,
  /** Aucun modèle d'images installé. */
  noImageModel: -32036,
} as const;

/** Une tâche n'est signalée à « /status » qu'après ce délai (pas `ls`). */
export const REPORT_AFTER_MS = 10_000;
/** Une commande du terminal et une tâche de VS Code commencées et finies à
 * moins de cet écart sont la même (tâche lancée dans un terminal à
 * intégration du shell) : une seule notification. */
export const SAME_JOB_MS = 2_000;
/** Attente avant de prévenir d'une commande du terminal (la tâche de VS Code
 * qui l'a lancée finit au même moment). */
const DEDUPE_WAIT_MS = 1_000;
/** Titre envoyé, en caractères. */
export const MAX_TITLE_CHARS = 80;

export type Outcome = "success" | "failure" | "done";

export function outcomeOf(exitCode: number | undefined): Outcome {
  if (exitCode === undefined) return "done";
  return exitCode === 0 ? "success" : "failure";
}

/** Affectation d'environnement en tête (`FOO=bar make`). */
const ENV_ASSIGNMENT = /^[A-Za-z_]\w*=/;
/** Préfixes qui lancent la vraie commande (`sudo make`, `time cargo test`). */
const WRAPPERS = new Set([
  "sudo",
  "doas",
  "time",
  "env",
  "nice",
  "nohup",
  "command",
  "exec",
]);
/** Sous-commande gardée : mot court en minuscules, sans point, barre ni
 * capitale (`build`, `build:prod`, `run-script`). Un hôte, un jeton, un mot
 * de passe ou un chemin n'y ressemblent pas, et un outil inconnu n'en garde
 * aucune. */
const SUBCOMMAND = /^[a-z][a-z0-9-]{0,19}(?::[a-z0-9-]{1,20}){0,2}$/;

/**
 * Outils dont le mot qui suit le programme est une sous-commande (et non un
 * argument libre), avec les sous-commandes qui en prennent une seconde
 * (`npm run build`, `docker compose up`, `deno task dev`). Tout autre
 * programme (script, `ssh`, `openssl`, `vault`…) ne donne que son nom.
 */
const SUBCOMMAND_TOOLS: Record<string, readonly string[]> = {
  npm: ["run", "run-script"],
  pnpm: ["run"],
  yarn: ["run"],
  bun: ["run"],
  deno: ["task"],
  cargo: [],
  git: [],
  make: [],
  gmake: [],
  just: [],
  docker: ["compose", "buildx", "container", "image", "network", "volume"],
  podman: ["compose", "container", "image", "network", "volume"],
  "docker-compose": [],
  go: ["mod", "work"],
  mvn: [],
  mvnw: [],
  gradle: [],
  gradlew: [],
  dotnet: [],
  kubectl: [],
  helm: [],
  terraform: [],
  tofu: [],
  poetry: [],
  uv: [],
  pip: [],
  pip3: [],
  rake: [],
  bundle: [],
  composer: [],
  mix: [],
  flutter: [],
  dart: [],
  swift: [],
  zig: [],
  ninja: [],
  bazel: [],
  nx: [],
  turbo: [],
  vite: [],
  vitest: [],
};

/**
 * Titre d'une commande du terminal : le nom du programme (sans son chemin),
 * plus ses sous-commandes seulement pour un outil connu à sous-commandes
 * (`cargo build`, `npm run build`, `docker compose up`) ; jamais d'argument
 * libre, d'option, d'hôte, de chemin ni de valeur (rapport 94, S1). Une
 * commande qui ressemble à un secret donne le programme seul. Vide si rien de
 * sûr.
 */
export function commandTitle(commandLine: string): string {
  const words = commandLine.trim().split(/\s+/).filter(Boolean);
  for (;;) {
    while (words.length > 0 && ENV_ASSIGNMENT.test(words[0])) words.shift();
    if (words.length < 2 || !WRAPPERS.has(words[0])) break;
    // `sudo -u nom make` : l'option prend peut-être une valeur, qu'on
    // prendrait pour le programme ; le préfixe seul.
    if (words[1].startsWith("-")) return words[0];
    words.shift();
  }
  if (words.length === 0) return "";
  const program = path.posix.basename(words[0].replaceAll("\\", "/"));
  if (!/^[\w.+-]+$/.test(program) || program.startsWith("-")) return "";
  const name = program.slice(0, MAX_TITLE_CHARS);
  if (hasSecretValue(commandLine)) return name;
  const tool = program.toLowerCase().replace(/\.(exe|cmd|bat)$/, "");
  const nested = Object.hasOwn(SUBCOMMAND_TOOLS, tool)
    ? SUBCOMMAND_TOOLS[tool]
    : null;
  if (!nested) return name;
  const title = [program];
  const first = words[1];
  if (first !== undefined && SUBCOMMAND.test(first)) {
    title.push(first);
    const second = words[2];
    if (
      nested.includes(first) &&
      second !== undefined &&
      SUBCOMMAND.test(second)
    ) {
      title.push(second);
    }
  }
  return title.join(" ").slice(0, MAX_TITLE_CHARS);
}

/** Titre d'une tâche de VS Code (nom donné par l'utilisateur), borné. */
export function taskTitle(name: string, source: string | undefined): string {
  const base = name.replace(/\s+/g, " ").trim();
  const full =
    source && source !== "Workspace" && !base.startsWith(`${source}:`)
      ? `${source}: ${base}`
      : base;
  if (!full || hasSecretValue(full)) return "";
  return full.slice(0, MAX_TITLE_CHARS);
}

export type NotifySettings = {
  enabled: boolean;
  /** Seuil, en millisecondes. */
  minMs: number;
  /** Le nom du dossier ouvert peut partir avec. */
  includeFolder: boolean;
};

export function readSettings(): NotifySettings {
  const config = vscode.workspace.getConfiguration("chatonlm.notify");
  const seconds = config.get<number>("minDurationSeconds", 60);
  return {
    enabled: config.get<boolean>("enabled", false) === true,
    minMs:
      typeof seconds === "number" && Number.isFinite(seconds) && seconds >= 0
        ? seconds * 1000
        : 60_000,
    includeFolder: config.get<boolean>("includeFolderName", false) === true,
  };
}

/** Ce que le notificateur attend du suivi du terminal. */
export type TerminalEvents = {
  onCommandStart(listener: (start: CommandStart) => void): () => void;
  onCommandEnd(listener: (end: CommandEnd) => void): () => void;
  runningIn(terminal: unknown): object | null;
};

export type NotifierDeps = {
  link: LinkLike;
  terminal?: TerminalEvents;
  /** Ouvre l'app sur une page (`app.show`). */
  show(route: string): Promise<void>;
  /** Heure (essais). */
  now?(): number;
  /** Délais raccourcis (essais). */
  delays?: { reportMs?: number; dedupeMs?: number };
};

type Source = "terminal" | "task" | "reply";

type Job = {
  id: string;
  source: Source;
  title: string;
  startedAt: number;
  /** Signalée à « /status » (`notify.task running`). */
  reported: boolean;
  /** Demandée par « Notify me when this finishes » : seuil ignoré, et
   * prévenue même si les notifications automatiques sont coupées. */
  armed: boolean;
  timer: ReturnType<typeof setTimeout> | null;
};

/** Page de l'app où se règle Easy Link. */
export const EASY_LINK_ROUTE = "/settings?section=easyLink";

export class Notifier {
  readonly #deps: NotifierDeps;
  readonly #jobs = new Map<object, Job>();
  #ids = 0;
  /** Terminal dont la prochaine commande est armée. */
  #armedTerminal: unknown = null;
  #status: vscode.StatusBarItem | null = null;
  #disposers: (() => void)[] = [];
  /** Tâches de VS Code finies récemment (doublon d'une commande). */
  #taskEnds: { startedAt: number; endedAt: number }[] = [];
  /** Refus de l'app déjà dit (notifications automatiques : une fois). */
  #warned = false;

  constructor(deps: NotifierDeps) {
    this.#deps = deps;
    const terminal = deps.terminal;
    if (terminal) {
      this.#disposers.push(
        terminal.onCommandStart((start) => this.#commandStarted(start)),
        terminal.onCommandEnd((end) => void this.#commandEnded(end)),
      );
    }
    // Terminal fermé : sa notification armée ne viendrait jamais (rapport 94,
    // B1) ; ses commandes en cours sont finies par le suivi du terminal.
    const closed = vscode.window.onDidCloseTerminal as
      typeof vscode.window.onDidCloseTerminal | undefined;
    if (typeof closed === "function") {
      const subscription = closed((terminal) => {
        if (this.#armedTerminal === terminal) {
          this.#armedTerminal = null;
          this.#updateStatus();
        }
      });
      this.#disposers.push(() => subscription.dispose());
    }
    // Tâches de VS Code (`tasks.json`, scripts npm…), API détectée.
    const tasks = vscode.tasks as typeof vscode.tasks | undefined;
    if (
      typeof tasks?.onDidStartTaskProcess === "function" &&
      typeof tasks.onDidEndTaskProcess === "function"
    ) {
      const started = tasks.onDidStartTaskProcess((event) => {
        const task = event.execution.task;
        this.start(
          event.execution,
          "task",
          taskTitle(task.name, task.source) ||
            vscode.l10n.t("A task in VS Code"),
        );
      });
      const ended = tasks.onDidEndTaskProcess((event) => {
        const job = this.#jobs.get(event.execution);
        if (job) {
          const now = this.#now();
          this.#taskEnds = this.#taskEnds.filter(
            (end) => now - end.endedAt < 10 * SAME_JOB_MS,
          );
          this.#taskEnds.push({
            startedAt: job.startedAt,
            endedAt: this.#now(),
          });
        }
        void this.end(event.execution, outcomeOf(event.exitCode));
      });
      this.#disposers.push(
        () => started.dispose(),
        () => ended.dispose(),
      );
    }
  }

  dispose(): void {
    for (const dispose of this.#disposers) dispose();
    this.#disposers = [];
    for (const job of this.#jobs.values()) {
      if (job.timer) clearTimeout(job.timer);
    }
    this.#jobs.clear();
    this.#status?.dispose();
    this.#status = null;
  }

  #now(): number {
    return this.#deps.now?.() ?? Date.now();
  }

  /** Une notification est armée (commande ou prochaine commande). */
  get armed(): boolean {
    if (this.#armedTerminal !== null) return true;
    for (const job of this.#jobs.values()) if (job.armed) return true;
    return false;
  }

  /** Tâche commencée ; signalée à « /status » si elle dure (ou armée). */
  start(key: object, source: Source, title: string, armed = false): void {
    if (this.#jobs.has(key)) return;
    const job: Job = {
      id: `vscode-${++this.#ids}`,
      source,
      title,
      startedAt: this.#now(),
      reported: false,
      armed,
      timer: null,
    };
    this.#jobs.set(key, job);
    if (armed) {
      void this.#report(job);
    } else {
      job.timer = setTimeout(() => {
        job.timer = null;
        if (this.#jobs.get(key) === job && this.#duplicate(job, null)) return;
        if (this.#jobs.get(key) === job) void this.#report(job);
      }, this.#deps.delays?.reportMs ?? REPORT_AFTER_MS);
    }
    this.#updateStatus();
  }

  /** Tâche finie : « /status » mis à jour, et notification si elle a duré
   * au moins le seuil (ou si elle était armée). */
  async end(key: object, outcome: Outcome | null): Promise<void> {
    const job = this.#jobs.get(key);
    if (!job) return;
    this.#jobs.delete(key);
    if (job.timer) clearTimeout(job.timer);
    this.#updateStatus();
    const durationMs = Math.max(0, this.#now() - job.startedAt);
    // Réponse arrêtée par l'utilisateur : il est là, rien à dire.
    if (outcome === null) {
      if (job.reported) await this.#task(job, "done");
      return;
    }
    if (job.reported) await this.#task(job, outcome);
    const settings = readSettings();
    if (!job.armed && (!settings.enabled || durationMs < settings.minMs)) {
      return;
    }
    await this.#send(job, outcome, durationMs, settings);
  }

  /** Vrai si une tâche de VS Code est la même que cette commande. */
  #duplicate(job: Job, endedAt: number | null): boolean {
    if (job.source !== "terminal") return false;
    for (const other of this.#jobs.values()) {
      if (
        other.source === "task" &&
        Math.abs(other.startedAt - job.startedAt) <= SAME_JOB_MS
      ) {
        return true;
      }
    }
    if (endedAt === null) return false;
    return this.#taskEnds.some(
      (end) =>
        Math.abs(end.startedAt - job.startedAt) <= SAME_JOB_MS &&
        Math.abs(end.endedAt - endedAt) <= SAME_JOB_MS,
    );
  }

  #commandStarted(start: CommandStart): void {
    const armed =
      this.#armedTerminal !== null && this.#armedTerminal === start.terminal;
    if (armed) this.#armedTerminal = null;
    const title = commandTitle(start.commandLine);
    this.start(
      start.key,
      "terminal",
      title || vscode.l10n.t("A command in the terminal"),
      armed,
    );
  }

  async #commandEnded(end: CommandEnd): Promise<void> {
    const job = this.#jobs.get(end.key);
    if (!job) return;
    if (!job.armed) {
      const endedAt = this.#now();
      // La tâche de VS Code qui a lancé cette commande finit au même moment.
      await new Promise((resolve) =>
        setTimeout(resolve, this.#deps.delays?.dedupeMs ?? DEDUPE_WAIT_MS),
      );
      if (this.#duplicate(job, endedAt)) {
        this.#jobs.delete(end.key);
        if (job.timer) clearTimeout(job.timer);
        return;
      }
    }
    await this.end(end.key, outcomeOf(end.exitCode));
  }

  /** « Notify me when this finishes » : commande en cours du terminal
   * actif, sinon sa prochaine commande. */
  async armTerminal(): Promise<void> {
    if (!this.#deps.link.connected) {
      void vscode.window.showWarningMessage(
        vscode.l10n.t("ChatonLM is not connected. See the ChatonLM panel."),
      );
      return;
    }
    const terminal = vscode.window.activeTerminal;
    if (!terminal) {
      void vscode.window.showInformationMessage(
        vscode.l10n.t("Open a terminal first, then run this command again."),
      );
      return;
    }
    const running = this.#deps.terminal?.runningIn(terminal) ?? null;
    const job = running ? this.#jobs.get(running) : undefined;
    if (job) {
      job.armed = true;
      if (job.timer) clearTimeout(job.timer);
      job.timer = null;
      if (!job.reported) void this.#report(job);
      this.#updateStatus();
      void vscode.window.showInformationMessage(
        vscode.l10n.t(
          "ChatonLM will message you through Easy Link when this command finishes.",
        ),
      );
      return;
    }
    // Sans intégration du shell (cmd.exe…), la fin d'une commande ne se voit pas.
    if (!this.#deps.terminal || terminal.shellIntegration === undefined) {
      void vscode.window.showWarningMessage(
        vscode.l10n.t(
          "VS Code can't tell when commands end in this terminal (shell integration is off). Use bash, zsh, fish or PowerShell.",
        ),
      );
      return;
    }
    this.#armedTerminal = terminal;
    this.#updateStatus();
    void vscode.window.showInformationMessage(
      vscode.l10n.t(
        "ChatonLM will message you through Easy Link when the next command in this terminal finishes.",
      ),
    );
  }

  /** Retire la notification armée (clic sur la barre d'état). */
  disarm(): void {
    this.#armedTerminal = null;
    for (const job of this.#jobs.values()) job.armed = false;
    this.#updateStatus();
  }

  /** `notify.task` : pour « /status » d'Easy Link (titre, début, état ;
   * une fin sans échec connu, Ctrl+C ou réponse arrêtée, compte comme finie
   * sans erreur : le contrat n'a que trois états). */
  async #task(job: Job, state: "running" | Outcome): Promise<void> {
    if (!this.#deps.link.connected) return;
    try {
      await this.#deps.link.request("notify.task", {
        id: job.id,
        title: job.title,
        state: state === "done" ? "success" : state,
        startedAt: job.startedAt,
      });
    } catch {
      // Refus (réglage coupé, Easy Link absent) : /status n'aura rien.
    }
  }

  async #report(job: Job): Promise<void> {
    if (!job.armed && !readSettings().enabled) return;
    job.reported = true;
    await this.#task(job, "running");
  }

  async #send(
    job: Job,
    outcome: Outcome,
    durationMs: number,
    settings: NotifySettings,
  ): Promise<void> {
    if (!this.#deps.link.connected) {
      // Demandée par l'utilisateur : il attend ce message, on dit qu'il ne
      // viendra pas (rapport 94, B2).
      if (job.armed) {
        void vscode.window.showWarningMessage(
          vscode.l10n.t(
            "ChatonLM is not connected: no message was sent when {0} finished.",
            job.title,
          ),
        );
      }
      return;
    }
    const params: Record<string, unknown> = {
      title: job.title,
      outcome,
      durationMs,
    };
    const folder = vscode.workspace.name;
    if (settings.includeFolder && folder) params.workspace = folder;
    try {
      const result = await this.#deps.link.request<{ sent?: unknown }>(
        "notify.send",
        params,
      );
      // Aucun message parti (un autre il y a moins de 30 s, ou service de
      // messagerie injoignable) : dit seulement pour une tâche armée.
      if (job.armed && result?.sent === 0) {
        void vscode.window.showWarningMessage(
          vscode.l10n.t(
            "Easy Link sent no message when {0} finished: a message was sent less than 30 seconds ago, or the messaging service could not be reached.",
            job.title,
          ),
        );
      }
    } catch (error) {
      // Automatique : dit une fois par session ; armée : à chaque fois.
      if (!job.armed && this.#warned) return;
      this.#warned = true;
      await this.#refused(error);
    }
  }

  async #refused(error: unknown): Promise<void> {
    if (!(error instanceof RpcError)) return;
    let text: string;
    if (error.code === LinkCodes.notifyOff) {
      text = vscode.l10n.t(
        "ChatonLM did not send the notification: turn on Let VS Code notify me through Easy Link, in Settings › Easy Link.",
      );
    } else if (error.code === LinkCodes.notLinked) {
      text = vscode.l10n.t(
        "ChatonLM did not send the notification: set up Easy Link and link your account first.",
      );
    } else {
      text = vscode.l10n.t(
        "ChatonLM did not send the notification: {0}",
        error.appMessage ?? error.message,
      );
    }
    const open = vscode.l10n.t("Open ChatonLM");
    const choice = await vscode.window.showWarningMessage(text, open);
    if (choice === open) await this.#deps.show(EASY_LINK_ROUTE);
  }

  #updateStatus(): void {
    const armed = this.armed;
    if (!armed) {
      this.#status?.hide();
      return;
    }
    if (!this.#status) {
      this.#status = vscode.window.createStatusBarItem(
        "chatonlm.notify",
        vscode.StatusBarAlignment.Right,
        99,
      );
      this.#status.name = vscode.l10n.t("ChatonLM notification");
      this.#status.command = "chatonlm.notifyCancel";
    }
    this.#status.text = "$(bell) ChatonLM";
    this.#status.tooltip = vscode.l10n.t(
      "ChatonLM will message you through Easy Link when the command finishes. Click to cancel.",
    );
    this.#status.accessibilityInformation = {
      label: vscode.l10n.t(
        "ChatonLM will message you through Easy Link when the command finishes. Click to cancel.",
      ),
    };
    this.#status.show();
  }
}
