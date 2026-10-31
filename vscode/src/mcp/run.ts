// Serveur MCP de ChatonLM tel que l'agent le lance (`dist/mcp.cjs --state
// <dossier> --session <séance>`) : justificatifs lus dans le fichier privé
// écrit par l'extension (jamais en argument ni en variable d'environnement :
// l'accès propre aux agents, que l'app limite elle-même aux cinq outils),
// connexion au pont de l'app comme l'extension (preuve de l'app vérifiée),
// appels notés pour le panneau, arrêts demandés par le panneau
// (`control-<séance>.json`) ou par l'effacement des justificatifs (réglage
// coupé, désappairage). Tout outil est refusé si aucune fenêtre de VS Code
// ne montre cette séance (`window-<séance>.json`). Sans dépendance à VS Code.
// Contrats : .tools/research/95-lot4b-contract.md (2), 97-lot4b-fix-contract.md.
import { watch as watchDir } from "node:fs";
import { promises as fs } from "node:fs";
import net from "node:net";
import path from "node:path";
import type { Duplex } from "node:stream";
import { bridgeDir, type Platform, readDiscovery } from "../bridge/discovery";
import { appRunning } from "../bridge/launcher";
import { Link, type LinkState } from "../bridge/link";
import {
  activityFile,
  type ActivityRecord,
  controlFile,
  CREDENTIALS,
  type McpCredentials,
  parseCredentials,
  readPrivate,
  SESSION,
  windowOpen,
} from "./files";
import { LineReader, McpServer } from "./server";
import type { BridgeLike } from "./tools";

/** Sondage des fichiers de l'extension pendant un appel (filet de `watch`). */
export const POLL_MS = 500;
/** Au repos, liaison ouverte : justificatifs relus un sondage sur IDLE_EVERY. */
export const IDLE_EVERY = 10;

export type McpOptions = {
  version: string;
  platform: Platform;
  env: NodeJS.ProcessEnv;
  input: NodeJS.ReadableStream;
  write(line: string): void;
  /** Connexion au point d'accès du pont (essais : faux pont). */
  connect?(endpoint: string): Promise<Duplex>;
  /** Lecture de `bridge.json` (essais : faux pont). */
  readDiscovery?: typeof readDiscovery;
  /** L'app tourne-t-elle (pont coupé ou app fermée) ? (essais) */
  appRunning?: () => Promise<boolean | null>;
  /** Le processus d'une fenêtre vit-il ? (essais) */
  alive?: (pid: number) => boolean;
  /** Fin de l'entrée (l'agent a fermé le serveur). */
  onEnd?(): void;
};

/** Arguments du script, ou null (alors le serveur répond, mais reste coupé). */
export function parseArgs(
  argv: string[],
): { stateDir: string; session: string } | null {
  const value = (flag: string) => {
    const i = argv.indexOf(flag);
    return i >= 0 ? argv[i + 1] : undefined;
  };
  const stateDir = value("--state");
  const session = value("--session");
  if (!stateDir || !path.isAbsolute(stateDir)) return null;
  if (!session || !SESSION.test(session)) return null;
  return { stateDir, session };
}

function connectSocket(endpoint: string): Promise<Duplex> {
  return new Promise((resolve, reject) => {
    const socket = net.connect({ path: endpoint });
    socket.once("error", reject);
    socket.once("connect", () => {
      socket.off("error", reject);
      resolve(socket);
    });
  });
}

/** Pourquoi le pont n'est pas joignable, pour l'agent. */
export function unreachableText(state: LinkState): string {
  switch (state.connection) {
    case "notPaired":
      // L'app ne connaît plus cet accès (`UNKNOWN_CLIENT`,
      // `UNAUTHENTICATED`) : retiré par la coupure du réglage, un nouvel
      // accès, ou l'éditeur retiré dans l'app.
      return REVOKED_TEXT;
    case "appClosed":
      return "ChatonLM is not open. Ask the user to open the ChatonLM app.";
    case "bridgeOff":
      return "The link with VS Code is off in ChatonLM (Settings › VS Code).";
    case "incompatible":
      return "This version of ChatonLM does not match the VS Code extension. Update both.";
    default:
      return "ChatonLM is not reachable.";
  }
}

export const REVOKED_TEXT =
  "ChatonLM access for agents was turned off. The user can turn it on again in VS Code (setting chatonlm.mcp.enabled).";

export const NO_WINDOW_TEXT =
  "No VS Code window shows this server's calls. ChatonLM's tools only work for a server started by an open VS Code window, where each call is shown.";

export const OFF_TEXT =
  "ChatonLM's tools for agents are turned off. The user can turn them on in VS Code (setting chatonlm.mcp.enabled), with VS Code paired with ChatonLM.";

const sameCredentials = (a: McpCredentials, b: McpCredentials) =>
  a.clientId === b.clientId &&
  a.secret === b.secret &&
  a.bridgeDir === b.bridgeDir;

/**
 * Démarre le serveur sur `input` ; rend de quoi l'arrêter. Sans arguments
 * valides (`args` null), il répond quand même (`initialize`, `tools/list`),
 * mais chaque outil est refusé.
 */
export function runMcp(
  options: McpOptions,
  args: { stateDir: string; session: string } | null,
) {
  const { platform } = options;
  let link: Link | null = null;
  let linked: McpCredentials | null = null;
  let stopped = false;
  /** Écritures du journal, dans l'ordre. */
  let journal = Promise.resolve();

  const readCredentials = async (): Promise<McpCredentials | null> => {
    if (!args) return null;
    const text = await readPrivate(
      path.join(args.stateDir, CREDENTIALS),
      platform,
    );
    return text === null ? null : parseCredentials(text);
  };

  /** Une fenêtre de VS Code montre-t-elle les appels de cette séance ? */
  const shown = async (): Promise<boolean> =>
    args !== null &&
    (await windowOpen(args.stateDir, args.session, platform, options.alive));

  const dropLink = () => {
    link?.dispose();
    link = null;
    linked = null;
  };

  const bridge = async (): Promise<BridgeLike | string> => {
    // Personne ne verrait l'appel : rien n'est demandé à l'app.
    if (!(await shown())) {
      dropLink();
      return NO_WINDOW_TEXT;
    }
    const credentials = await readCredentials();
    if (!credentials) {
      dropLink();
      return OFF_TEXT;
    }
    if (!link || !linked || !sameCredentials(linked, credentials)) {
      dropLink();
      const current = credentials;
      linked = current;
      link = new Link({
        dir: () =>
          current.bridgeDir ?? bridgeDir(platform, options.env) ?? null,
        readDiscovery: (dir) =>
          (options.readDiscovery ?? readDiscovery)(dir, platform),
        connect: options.connect ?? connectSocket,
        // Lus à chaque connexion ; l'extension seule les écrit et les efface.
        secrets: {
          get: async () => {
            const now = await readCredentials();
            return now && sameCredentials(now, current)
              ? { clientId: now.clientId, secret: now.secret }
              : null;
          },
          set: async () => {},
          delete: async () => {},
        },
        client: {
          name: "chatonlm-mcp",
          version: options.version,
          editor: server.agent ? `MCP: ${server.agent}` : "MCP",
        },
        watch: () => null,
        // Pont coupé dans l'app ≠ app fermée (texte pour l'agent).
        appRunning: options.appRunning ?? (() => appRunning(platform)),
      });
      await link.start();
    } else if (!link.connected) {
      await link.retryNow();
    }
    const current = link;
    if (!current) return OFF_TEXT;
    return current.connected ? current : unreachableText(current.state);
  };

  const record = (entry: ActivityRecord) => {
    if (!args) return;
    const file = path.join(args.stateDir, activityFile(args.session));
    journal = journal
      .then(() =>
        fs.appendFile(file, `${JSON.stringify(entry)}\n`, {
          encoding: "utf8",
          mode: 0o600,
        }),
      )
      .catch(() => {});
  };

  const server = new McpServer({
    version: options.version,
    bridge,
    record,
    write: options.write,
  });

  /** Arrêts demandés par le panneau, réglage coupé (justificatifs effacés)
   * et fenêtre fermée : vérifiés à chaque changement du dossier, toutes les
   * POLL_MS pendant un appel, et au repos (liaison encore ouverte) un
   * sondage sur IDLE_EVERY : une liaison sans justificatifs est lâchée même
   * sans appel en cours. */
  let checking = false;
  const check = async (idleToo = true) => {
    if (!args || checking) return;
    const idle = server.running.length === 0;
    if (idle && (!link || !idleToo)) return;
    checking = true;
    try {
      const credentials = await readCredentials();
      if (
        !credentials ||
        (linked && !sameCredentials(linked, credentials)) ||
        !(await shown())
      ) {
        server.stopAll();
        dropLink();
        return;
      }
      if (idle) return;
      const control = await readPrivate(
        path.join(args.stateDir, controlFile(args.session)),
        platform,
      );
      if (control === null) return;
      let stop: unknown;
      try {
        stop = (JSON.parse(control) as { stop?: unknown }).stop;
      } catch {
        return;
      }
      if (!Array.isArray(stop)) return;
      for (const id of stop) {
        if (typeof id === "string") server.stop(id);
      }
    } finally {
      checking = false;
    }
  };
  let watcher: { close(): void } | null = null;
  if (args) {
    try {
      const w = watchDir(args.stateDir, () => void check());
      w.on("error", () => w.close());
      watcher = w;
    } catch {
      // Dossier absent : le sondage suffit.
    }
  }
  let ticks = 0;
  const poll = setInterval(
    () => void check(++ticks % IDLE_EVERY === 0),
    POLL_MS,
  );
  poll.unref?.();

  const reader = new LineReader();
  const stop = () => {
    if (stopped) return;
    stopped = true;
    server.stopAll();
    clearInterval(poll);
    watcher?.close();
    dropLink();
  };
  options.input.on("data", (chunk: Buffer | string) => {
    for (const line of reader.push(chunk)) server.receive(line);
  });
  options.input.on("end", () => {
    // Appels arrêtés, réponses et journal écrits, puis fin.
    stop();
    void server
      .settled()
      .then(() => journal)
      .then(() => options.onEnd?.());
  });

  return {
    server,
    stop,
    /** Journal écrit (essais). */
    flushed: () => journal,
    check,
  };
}
