// ChatonLM comme serveur MCP pour les agents de l'éditeur (Copilot, Cline,
// Cursor…), côté extension : réglage `chatonlm.mcp.enabled` (COUPÉ par
// défaut, portée application, lu seulement dans les réglages de
// l'utilisateur : un dossier ouvert ne peut pas l'activer), déclaration du
// serveur stdio (`lm.registerMcpServerDefinitionProvider` de VS Code ≥ 1.101,
// `cursor.mcp.registerServer` de Cursor), et chaque appel d'un agent montré
// dans le panneau, arrêtable.
// Le serveur n'a jamais les justificatifs de l'éditeur : l'extension demande
// à l'app, par la liaison de l'éditeur, un accès propre aux agents
// (`agents.issue`), que l'app limite elle-même aux méthodes des cinq outils,
// et l'écrit dans un fichier privé (`credentials.json`). Réglage coupé :
// accès retiré dans l'app (`agents.revoke`, tout de suite si la liaison est
// là, sinon à la prochaine connexion), puis fichier effacé ; désappairage :
// fichier effacé (l'app retire l'accès agents avec l'éditeur).
// Chaque fenêtre écrit `window-<séance>.json` tant qu'elle est ouverte : le
// serveur d'une séance qu'aucune fenêtre ne montre refuse tout outil.
// Contrats : .tools/research/95-lot4b-contract.md (2), 97-lot4b-fix-contract.md.
import { randomBytes } from "node:crypto";
import { promises as fs, rmSync, watch as watchDir } from "node:fs";
import path from "node:path";
import * as vscode from "vscode";
import type {
  AgentCall,
  AgentsIssueResult,
  AgentsView,
  HostMessage,
} from "../shared/protocol";
import type { Credentials } from "./bridge/link";
import { Codes, RpcError } from "./bridge/rpc";
import { currentPlatform } from "./bridge/discovery";
import {
  activityFile,
  type ActivityRecord,
  controlFile,
  CREDENTIALS,
  ensurePrivateDir,
  eraseCredentials,
  type McpCredentials,
  parseActivity,
  parseCredentials,
  processAlive,
  readPrivate,
  removeFile,
  windowFile,
  windowRecord,
  writePrivate,
} from "./mcp/files";

/** Identifiant du fournisseur (contribution `mcpServerDefinitionProviders`). */
export const MCP_PROVIDER = "chatonlm.mcp";
/** Nom du serveur pour Cursor. */
export const CURSOR_SERVER = "chatonlm";
/** Appels gardés pour le panneau. */
export const MAX_CALLS = 20;
/** Sondage du journal pendant un appel (filet de `watch`). */
export const POLL_MS = 1000;
/** Journaux d'une séance plus anciens : effacés au démarrage. */
const STALE_MS = 24 * 60 * 60 * 1000;
/** Accès agents à retirer dans l'app à la prochaine connexion (réglage
 * coupé sans liaison) ; partagé par les fenêtres. */
export const REVOKE_PENDING = "revoke-pending";
/** Une seule fenêtre demande un accès à la fois (toutes reçoivent le
 * changement du réglage ; une seconde demande retirerait la première). */
export const ISSUE_LOCK = "issue.lock";
/** Verrou plus ancien : laissé par une fenêtre fermée brutalement. */
export const LOCK_STALE_MS = 30_000;
/** Attente du verrou tenu par une autre fenêtre. */
export const LOCK_WAIT_MS = 10_000;

/** Réglage activé dans les réglages de l'utilisateur (jamais ceux d'un
 * dossier ou d'un espace de travail). */
export function settingOn(name: string): boolean {
  const inspected = vscode.workspace
    .getConfiguration("chatonlm")
    .inspect<unknown>(name);
  return inspected?.globalValue === true;
}

export const mcpEnabled = () => settingOn("mcp.enabled");

type CursorMcp = {
  registerServer(config: {
    name: string;
    server: { command: string; args: string[]; env: Record<string, string> };
  }): void;
  unregisterServer(name: string): void;
};

/** API MCP de Cursor, si l'éditeur en est un. */
function cursorMcp(): CursorMcp | null {
  const mcp = (vscode as unknown as { cursor?: { mcp?: Partial<CursorMcp> } })
    .cursor?.mcp;
  return typeof mcp?.registerServer === "function" &&
    typeof mcp.unregisterServer === "function"
    ? (mcp as CursorMcp)
    : null;
}

/** Ce que l'accès agents demande à la liaison de l'éditeur. */
export type AgentsLink = {
  readonly connected: boolean;
  request<T>(method: string, params?: unknown): Promise<T>;
  onConnected(listener: () => void): () => void;
};

export type AgentsDeps = {
  post(message: HostMessage): void;
  reveal(): Promise<void>;
  /** Justificatifs de l'éditeur appairé (stockage secret), ou null : ils ne
   * vont jamais au serveur, seulement son identifiant (fichier rattaché à
   * l'éditeur). */
  credentials(): Promise<Credentials | null>;
  /** Liaison de l'éditeur : `agents.issue` et `agents.revoke`. */
  link: AgentsLink;
  /** Dossier de `bridge.json` vu par l'extension. */
  bridgeDir(): string | null;
  /** `<globalStorage>/mcp`, absolu. */
  storageDir: string;
  /** Chemin de `dist/mcp.cjs`. */
  script: string;
  /** Node de l'éditeur (`process.execPath`). */
  execPath: string;
  version: string;
  /** Demande l'appairage (au démarrage du serveur, éditeur pas appairé). */
  pair(): Promise<void>;
  /** Séance de cette fenêtre (essais). */
  session?: string;
};

export class Agents {
  readonly #deps: AgentsDeps;
  readonly session: string;
  readonly #changed = new vscode.EventEmitter<void>();
  readonly #disposables: { dispose(): void }[] = [];
  #calls: AgentCall[] = [];
  #offset = 0;
  #partial = "";
  #reading: Promise<void> | null = null;
  #readAgain = false;
  #stopping = new Set<string>();
  #writing: Promise<void> = Promise.resolve();
  #watcher: { close(): void } | null = null;
  #poll: ReturnType<typeof setInterval> | null = null;
  #status: vscode.StatusBarItem | null = null;
  #cursorRegistered = false;
  #enabled: boolean;
  /** Avertissements déjà montrés dans cette fenêtre. */
  #warned = new Set<string>();

  constructor(deps: AgentsDeps) {
    this.#deps = deps;
    this.session = deps.session ?? randomBytes(16).toString("hex");
    this.#enabled = mcpEnabled();
    this.#disposables.push(
      this.#changed,
      vscode.workspace.onDidChangeConfiguration((event) => {
        if (!event.affectsConfiguration("chatonlm.mcp")) return;
        const enabled = mcpEnabled();
        if (enabled === this.#enabled) return;
        this.#enabled = enabled;
        void this.sync();
        this.#changed.fire();
      }),
    );
    // Liaison (re)venue : accès demandé, ou retrait noté fait.
    const off = deps.link.onConnected(() => void this.sync());
    this.#disposables.push({ dispose: off });
  }

  get enabled(): boolean {
    return this.#enabled;
  }

  get #credentialsFile(): string {
    return path.join(this.#deps.storageDir, CREDENTIALS);
  }

  #file(name: string): string {
    return path.join(this.#deps.storageDir, name);
  }

  /** Arguments du serveur (aucun secret : seulement où lire). */
  get args(): string[] {
    return [
      this.#deps.script,
      "--state",
      this.#deps.storageDir,
      "--session",
      this.session,
    ];
  }

  /** Lance l'éditeur comme Node (Electron) pour le script. */
  static readonly ENV = { ELECTRON_RUN_AS_NODE: "1" };

  /** Déclare le serveur aux agents ; rend de quoi tout retirer. */
  register(): { dispose(): void }[] {
    const disposables: { dispose(): void }[] = [];
    const lm = vscode.lm as Partial<typeof vscode.lm> | undefined;
    if (typeof lm?.registerMcpServerDefinitionProvider === "function") {
      disposables.push(
        lm.registerMcpServerDefinitionProvider(MCP_PROVIDER, {
          onDidChangeMcpServerDefinitions: this.#changed.event,
          provideMcpServerDefinitions: () => this.definitions(),
          resolveMcpServerDefinition: (server) => this.resolve(server),
        }),
      );
    }
    if (cursorMcp()) {
      disposables.push(
        this.#changed.event(() => void this.#syncCursor()),
        { dispose: () => this.#unregisterCursor() },
      );
      void this.#syncCursor();
    }
    return disposables;
  }

  /** Serveur déclaré seulement si le réglage est activé. */
  definitions(): vscode.McpStdioServerDefinition[] {
    if (!this.#enabled) return [];
    return [
      new vscode.McpStdioServerDefinition(
        "ChatonLM",
        this.#deps.execPath,
        this.args,
        { ...Agents.ENV },
        this.#deps.version,
      ),
    ];
  }

  /** Au démarrage du serveur : réglage relu, appairage demandé au besoin,
   * justificatifs écrits. */
  async resolve(
    server: vscode.McpServerDefinition,
  ): Promise<vscode.McpServerDefinition | undefined> {
    if (!mcpEnabled()) return undefined;
    if (!(await this.#deps.credentials())) {
      const pair = vscode.l10n.t("Pair");
      const choice = await vscode.window.showWarningMessage(
        vscode.l10n.t(
          "Agents can use ChatonLM once VS Code is paired with it.",
        ),
        pair,
      );
      if (choice === pair) await this.#deps.pair();
      if (!(await this.#deps.credentials())) return undefined;
    }
    await this.sync();
    return server;
  }

  async #syncCursor(): Promise<void> {
    const cursor = cursorMcp();
    if (!cursor) return;
    if (this.#enabled && (await this.#deps.credentials())) {
      if (this.#cursorRegistered) return;
      cursor.registerServer({
        name: CURSOR_SERVER,
        server: {
          command: this.#deps.execPath,
          args: this.args,
          env: { ...Agents.ENV },
        },
      });
      this.#cursorRegistered = true;
    } else {
      this.#unregisterCursor();
    }
  }

  #unregisterCursor(): void {
    if (!this.#cursorRegistered) return;
    this.#cursorRegistered = false;
    try {
      cursorMcp()?.unregisterServer(CURSOR_SERVER);
    } catch {
      // Déjà retiré.
    }
  }

  /**
   * Accès du serveur : demandé et écrit si le réglage est activé, l'éditeur
   * appairé et la liaison là (sinon à la connexion) ; retiré dans l'app puis
   * effacé si le réglage est coupé (le serveur arrête alors ses appels).
   * Appelée au démarrage, au changement du réglage, à chaque connexion, à
   * l'appairage et au désappairage. Une à la fois.
   */
  sync(): Promise<void> {
    this.#writing = this.#writing
      .then(() => this.#syncOnce())
      .catch(() => {
        // Dossier de stockage inaccessible : le serveur reste coupé.
      });
    return this.#writing;
  }

  async #syncOnce(): Promise<void> {
    if (!this.#enabled) {
      // Un accès a pu être donné : fichier encore là, ou retrait en attente.
      const had =
        (await exists(this.#credentialsFile)) ||
        (await exists(this.#file(REVOKE_PENDING)));
      if (had) {
        if (await this.#revoke()) {
          await removeFile(this.#file(REVOKE_PENDING)).catch(() => {});
        } else {
          await writePrivate(this.#file(REVOKE_PENDING), "1").catch(() => {});
        }
      }
      await this.#erase();
    } else {
      const editor = await this.#deps.credentials();
      if (editor) await this.#ensureAccess(editor);
      else await this.#erase();
    }
    await this.#syncCursor();
  }

  /** Accès agents retiré dans l'app ; false : à refaire (pas de liaison). */
  async #revoke(): Promise<boolean> {
    if (!this.#deps.link.connected) return false;
    try {
      await this.#deps.link.request("agents.revoke");
      return true;
    } catch (error) {
      // L'app a répondu (version sans accès agents : rien à retirer).
      return error instanceof RpcError;
    }
  }

  /** Fichier effacé et vérifié ; resté sur le disque : erreur montrée. */
  async #erase(): Promise<void> {
    try {
      await eraseCredentials(this.#credentialsFile);
    } catch {
      this.#warnOnce(
        "erase",
        vscode.l10n.t(
          "ChatonLM could not delete the access file for agents: {0}. Delete it yourself.",
          this.#credentialsFile,
        ),
        "error",
      );
    }
  }

  #warnOnce(key: string, text: string, level: "warning" | "error"): void {
    if (this.#warned.has(key)) return;
    this.#warned.add(key);
    if (level === "error") void vscode.window.showErrorMessage(text);
    else void vscode.window.showWarningMessage(text);
  }

  /** Accès écrit, lisible et donné à cet éditeur, ou null. */
  async #current(editor: Credentials): Promise<McpCredentials | null> {
    const text = await readPrivate(this.#credentialsFile, currentPlatform());
    const current = text === null ? null : parseCredentials(text);
    return current?.editor === editor.clientId ? current : null;
  }

  async #write(access: AgentsIssueResult, editor: Credentials): Promise<void> {
    await writePrivate(
      this.#credentialsFile,
      JSON.stringify({
        clientId: access.clientId,
        secret: access.secret,
        bridgeDir: this.#deps.bridgeDir(),
        editor: editor.clientId,
      }),
    );
  }

  async #ensureAccess(editor: Credentials): Promise<void> {
    const current = await this.#current(editor);
    if (current) {
      // Dossier du pont changé : même accès, réécrit.
      if (current.bridgeDir !== this.#deps.bridgeDir()) {
        await this.#write(current, editor);
      }
      this.#watch();
      return;
    }
    // Fichier illisible, ou d'un éditeur d'avant (nouvel appairage) : il ne
    // sert plus.
    await this.#erase();
    // Pas de liaison : demandé à la connexion (`onConnected`).
    if (!this.#deps.link.connected) return;
    await this.#issue(editor);
    this.#watch();
  }

  /** Nouvel accès demandé à l'app, une fenêtre à la fois (verrou). */
  async #issue(editor: Credentials): Promise<void> {
    const lock = this.#file(ISSUE_LOCK);
    const held = await takeLock(lock);
    if (!held) {
      // Une autre fenêtre le demande : son fichier sert à toutes.
      await waitForLock(lock);
      return;
    }
    try {
      // Écrit par une autre fenêtre entre-temps.
      if (await this.#current(editor)) return;
      let access: AgentsIssueResult;
      try {
        access =
          await this.#deps.link.request<AgentsIssueResult>("agents.issue");
      } catch (error) {
        if (error instanceof RpcError && error.code === Codes.methodNotFound) {
          this.#warnOnce(
            "issue",
            vscode.l10n.t(
              "This version of ChatonLM can't give agents their own access. Update ChatonLM.",
            ),
            "warning",
          );
        }
        return;
      }
      if (
        typeof access?.clientId !== "string" ||
        !access.clientId ||
        typeof access.secret !== "string" ||
        !/^[0-9a-f]{64}$/i.test(access.secret)
      ) {
        return;
      }
      await this.#write(access, editor);
      // Un nouvel accès remplace l'ancien dans l'app : rien à retirer.
      await removeFile(this.#file(REVOKE_PENDING)).catch(() => {});
    } finally {
      await held.close().catch(() => {});
      await removeFile(lock).catch(() => {});
    }
  }

  /** Désappairage : accès retiré dans l'app si la liaison est là, fichier
   * effacé tout de suite. À appeler avant `link.unpair()`. */
  async forget(): Promise<void> {
    this.#writing = this.#writing
      .then(async () => {
        if (await exists(this.#credentialsFile)) await this.#revoke();
        await this.#erase();
      })
      .catch(() => {});
    await this.#writing;
    this.#unregisterCursor();
  }

  /** Au démarrage : dossier privé, journaux d'anciennes séances effacés. */
  async start(): Promise<void> {
    try {
      await ensurePrivateDir(this.#deps.storageDir);
      const now = Date.now();
      for (const name of await fs.readdir(this.#deps.storageDir)) {
        const file = path.join(this.#deps.storageDir, name);
        const window = /^window-([0-9a-f]{32})\.json$/.exec(name);
        if (window) {
          // Fenêtre fermée brutalement : son serveur ne doit plus servir.
          if (window[1] !== this.session && !(await windowAlive(file))) {
            await removeFile(file);
          }
          continue;
        }
        if (!/^(activity-.*\.jsonl|control-.*\.json|.*\.tmp)$/.test(name)) {
          continue;
        }
        const stat = await fs.stat(file).catch(() => null);
        if (stat && now - stat.mtimeMs > STALE_MS) await removeFile(file);
      }
    } catch {
      // Rien à nettoyer.
    }
    try {
      // Cette fenêtre montre les appels de sa séance.
      await writePrivate(this.#file(windowFile(this.session)), windowRecord());
    } catch {
      // Dossier inaccessible : le serveur de cette séance refusera.
    }
    await this.sync();
    this.#watch();
  }

  #watch(): void {
    if (this.#watcher) return;
    try {
      const watcher = watchDir(this.#deps.storageDir, (_event, name) => {
        if (!name || name === activityFile(this.session)) void this.read();
      });
      watcher.on("error", () => {
        watcher.close();
        if (this.#watcher === watcher) this.#watcher = null;
      });
      this.#watcher = watcher;
    } catch {
      // Dossier absent pour l'instant : relu plus tard.
    }
  }

  /** Nouvelles lignes du journal de cette fenêtre. */
  read(): Promise<void> {
    if (this.#reading) {
      // Lecture en cours : elle relira, et l'appelant attend sa fin.
      this.#readAgain = true;
      return this.#reading;
    }
    this.#reading = (async () => {
      try {
        do {
          this.#readAgain = false;
          await this.#readOnce();
        } while (this.#readAgain);
      } finally {
        this.#reading = null;
      }
    })();
    return this.#reading;
  }

  async #readOnce(): Promise<void> {
    const file = path.join(this.#deps.storageDir, activityFile(this.session));
    let handle: fs.FileHandle;
    try {
      handle = await fs.open(file, "r");
    } catch {
      return;
    }
    let text: string;
    try {
      const { size } = await handle.stat();
      if (size < this.#offset) {
        this.#offset = 0;
        this.#partial = "";
      }
      if (size === this.#offset) return;
      const length = Math.min(size - this.#offset, 1024 * 1024);
      const buffer = Buffer.alloc(length);
      await handle.read(buffer, 0, length, this.#offset);
      this.#offset += length;
      text = this.#partial + buffer.toString("utf8");
    } finally {
      await handle.close();
    }
    const lines = text.split("\n");
    this.#partial = lines.pop() ?? "";
    let changed = false;
    for (const line of lines) {
      const record = parseActivity(line);
      if (record) changed = this.#apply(record) || changed;
    }
    if (changed) this.#publish();
  }

  #apply(record: ActivityRecord): boolean {
    const index = this.#calls.findIndex((call) => call.id === record.id);
    if (index >= 0) {
      const known = this.#calls[index];
      // Un appel fini ne revient jamais « en cours ».
      if (known.state !== "running") return false;
      this.#calls[index] = {
        ...known,
        state: record.state,
        endedAt: record.at,
      };
      if (record.state !== "running") this.#stopping.delete(record.id);
      return true;
    }
    this.#calls = [
      {
        id: record.id,
        tool: record.tool,
        agent: record.agent,
        detail: record.detail,
        state: record.state,
        startedAt: record.at,
        endedAt: record.state === "running" ? null : record.at,
      },
      ...this.#calls,
    ].slice(0, MAX_CALLS);
    return true;
  }

  get view(): AgentsView | null {
    return this.#calls.length > 0
      ? { calls: this.#calls, enabled: this.#enabled }
      : null;
  }

  get running(): number {
    return this.#calls.filter((call) => call.state === "running").length;
  }

  #publish(): void {
    this.#deps.post({ type: "agents", agents: this.view });
    this.#statusBar();
    // Sondage seulement pendant un appel.
    if (this.running > 0 && !this.#poll) {
      this.#poll = setInterval(() => void this.read(), POLL_MS);
    } else if (this.running === 0 && this.#poll) {
      clearInterval(this.#poll);
      this.#poll = null;
    }
  }

  postView(): void {
    this.#deps.post({ type: "agents", agents: this.view });
  }

  /** Un agent se sert de ChatonLM : visible même panneau replié. */
  #statusBar(): void {
    const running = this.running;
    if (running === 0) {
      this.#status?.hide();
      return;
    }
    this.#status ??= vscode.window.createStatusBarItem(
      "chatonlm.agents",
      vscode.StatusBarAlignment.Left,
      98,
    );
    this.#status.name = vscode.l10n.t("ChatonLM used by an agent");
    this.#status.text = `$(plug) ${vscode.l10n.t("An agent is using ChatonLM")}`;
    this.#status.tooltip = vscode.l10n.t(
      "An agent is using ChatonLM. Click to see the call and stop it.",
    );
    this.#status.command = "chatonlm.chat.focus";
    this.#status.show();
  }

  /** Commande du panneau. */
  async command(
    command: "stop" | "turnOff" | "clear",
    callId?: string,
  ): Promise<void> {
    switch (command) {
      case "stop":
        if (callId) await this.stop(callId);
        return;
      case "turnOff":
        await this.turnOff();
        return;
      case "clear":
        this.#calls = this.#calls.filter((call) => call.state === "running");
        this.#publish();
        return;
    }
  }

  /** Arrête un appel en cours (lu par le serveur dans le fichier de contrôle). */
  async stop(callId: string): Promise<void> {
    const call = this.#calls.find((c) => c.id === callId);
    if (!call || call.state !== "running") return;
    this.#stopping.add(callId);
    try {
      await writePrivate(
        path.join(this.#deps.storageDir, controlFile(this.session)),
        JSON.stringify({ stop: [...this.#stopping] }),
      );
    } catch {
      // Dossier inaccessible : couper le réglage reste possible.
    }
  }

  /** Coupe le serveur pour tous les agents (réglage de l'utilisateur) : ses
   * justificatifs sont effacés, ses appels en cours s'arrêtent. */
  async turnOff(): Promise<void> {
    await vscode.workspace
      .getConfiguration("chatonlm")
      .update("mcp.enabled", false, vscode.ConfigurationTarget.Global);
    this.#enabled = false;
    await this.sync();
    this.#changed.fire();
  }

  dispose(): void {
    for (const disposable of this.#disposables) disposable.dispose();
    this.#watcher?.close();
    this.#watcher = null;
    if (this.#poll) clearInterval(this.#poll);
    this.#poll = null;
    this.#status?.dispose();
    this.#unregisterCursor();
    // Plus de fenêtre : le serveur de cette séance refuse tout (effacé tout
    // de suite, l'hôte des extensions peut s'arrêter juste après).
    try {
      rmSync(this.#file(windowFile(this.session)), { force: true });
    } catch {
      // Le processus mort suffit au serveur.
    }
    // Journaux de cette fenêtre : plus personne ne les lira.
    void removeFile(
      path.join(this.#deps.storageDir, activityFile(this.session)),
    ).catch(() => {});
    void removeFile(
      path.join(this.#deps.storageDir, controlFile(this.session)),
    ).catch(() => {});
  }
}

async function exists(file: string): Promise<boolean> {
  return fs.lstat(file).then(
    () => true,
    () => false,
  );
}

/** Le fichier d'une fenêtre nomme-t-il un processus vivant ? */
async function windowAlive(file: string): Promise<boolean> {
  try {
    const { pid } = JSON.parse(await fs.readFile(file, "utf8")) as {
      pid?: unknown;
    };
    return typeof pid === "number" && pid > 0 && processAlive(pid);
  } catch {
    return false;
  }
}

/** Verrou pris (fichier créé seul), ou null s'il est tenu ailleurs ; un
 * verrou trop ancien (fenêtre fermée brutalement) est repris. */
async function takeLock(file: string): Promise<fs.FileHandle | null> {
  for (let attempt = 0; attempt < 2; attempt++) {
    try {
      return await fs.open(file, "wx", 0o600);
    } catch (error) {
      if ((error as NodeJS.ErrnoException).code !== "EEXIST") throw error;
      const stat = await fs.stat(file).catch(() => null);
      if (stat && Date.now() - stat.mtimeMs <= LOCK_STALE_MS) return null;
      await removeFile(file);
    }
  }
  return null;
}

/** Attend la fin du verrou d'une autre fenêtre (au plus LOCK_WAIT_MS). */
async function waitForLock(file: string): Promise<void> {
  const deadline = Date.now() + LOCK_WAIT_MS;
  while (Date.now() < deadline && (await exists(file))) {
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
}
