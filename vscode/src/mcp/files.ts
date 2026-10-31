// Fichiers partagés entre l'extension et son serveur MCP (`dist/mcp.cjs`,
// lancé par un agent : Copilot, Cline…), dans le dossier de stockage de
// l'extension (`<globalStorage>/mcp/`, 0700 sous Linux ; sous Windows, le
// dossier du profil de l'utilisateur, privé par ses droits) :
// - `credentials.json` (0600) : accès propre aux agents (`agents.issue`,
//   contrat 97 § 1 : un second client du pont, que l'app limite elle-même aux
//   méthodes des cinq outils), JAMAIS celui de l'éditeur ; écrit par
//   l'extension seulement quand le réglage `chatonlm.mcp.enabled` est activé
//   et l'éditeur appairé, effacé (et l'accès retiré dans l'app) au
//   désappairage et à la coupure du réglage (jamais passé en argument ni en
//   variable d'environnement) ; son absence coupe le serveur ;
// - `window-<séance>.json` (0600) : la fenêtre de VS Code de cette séance est
//   ouverte (processus de son hôte d'extensions) ; sans lui, le serveur refuse
//   tout outil : un appel que personne ne montre n'est pas servi ;
// - `activity-<séance>.jsonl` : appels des agents, ajoutés par le serveur,
//   lus par l'extension (panneau) ;
// - `control-<séance>.json` (0600) : appels à arrêter, écrit par l'extension.
// Sans dépendance à VS Code : ce module est aussi dans `dist/mcp.cjs`.
// Contrats : .tools/research/95-lot4b-contract.md (2), 97-lot4b-fix-contract.md.
import { promises as fs } from "node:fs";
import path from "node:path";
import type { AgentTool } from "../../shared/protocol";
import type { Platform } from "../bridge/discovery";

export const CREDENTIALS = "credentials.json";
/** Sous-dossier du stockage de l'extension. */
export const MCP_DIR = "mcp";

/** Séance d'une fenêtre de VS Code (128 bits en hexadécimal). */
export const SESSION = /^[0-9a-f]{32}$/;

export const activityFile = (session: string) => `activity-${session}.jsonl`;
export const controlFile = (session: string) => `control-${session}.json`;
export const windowFile = (session: string) => `window-${session}.json`;

/** Justificatifs donnés au serveur MCP. */
export type McpCredentials = {
  clientId: string;
  secret: string;
  /** Dossier de `bridge.json` vu par l'extension (null : le serveur le
   * calcule comme elle). */
  bridgeDir: string | null;
  /** Éditeur à qui l'app a donné cet accès (pour le redemander après un
   * nouvel appairage) ; absent pour le serveur, qui ne s'en sert pas. */
  editor?: string | null;
};

/** Outils exposés aux agents (liste blanche). */
export const TOOL_NAMES = [
  "chatonlm_ask",
  "chatonlm_generate_image",
  "chatonlm_speak",
  "chatonlm_proofread",
  "chatonlm_models",
] as const satisfies readonly AgentTool[];
export type ToolName = AgentTool;

export const isToolName = (name: unknown): name is ToolName =>
  typeof name === "string" && (TOOL_NAMES as readonly string[]).includes(name);

/** Appel d'un agent, tel que le serveur le note. */
export type ActivityRecord = {
  /** Identifiant de l'appel (aléatoire, unique). */
  id: string;
  tool: ToolName;
  /** Nom de l'agent (`clientInfo` de MCP), ou null. */
  agent: string | null;
  state: "running" | "done" | "failed" | "stopped";
  /** Heure en millisecondes. */
  at: number;
  /** Début de la demande (une ligne, 80 caractères), ou null. */
  detail: string | null;
};

/** Marques et enchâssements bidirectionnels (LRM, RLM, LRE…RLO, LRI…PDI) :
 * un texte pourrait s'afficher à l'envers dans le panneau et y cacher sa
 * nature. */
const BIDI = /[\u200e\u200f\u202a-\u202e\u2066-\u2069]/g;

/** Texte d'un tiers ramené à une ligne courte, sans caractères de contrôle
 * ni bidirectionnels. */
export function oneLine(value: unknown, max: number): string | null {
  if (typeof value !== "string") return null;
  const text = value
    .replace(BIDI, "")
    // eslint-disable-next-line no-control-regex
    .replace(/[\u0000-\u001f\u007f-\u009f\s]+/g, " ")
    .trim();
  if (!text) return null;
  return text.length > max ? `${text.slice(0, max - 1)}…` : text;
}

/** Ligne du journal d'activité, sans confiance (écrite par un autre
 * processus) : ce qui ne répond pas au format est ignoré. */
export function parseActivity(line: string): ActivityRecord | null {
  let value: unknown;
  try {
    value = JSON.parse(line);
  } catch {
    return null;
  }
  const v = value as Partial<ActivityRecord> | null;
  if (typeof v !== "object" || v === null) return null;
  if (typeof v.id !== "string" || !/^[0-9a-f]{8,64}$/.test(v.id)) return null;
  if (!isToolName(v.tool)) return null;
  if (!["running", "done", "failed", "stopped"].includes(String(v.state))) {
    return null;
  }
  if (typeof v.at !== "number" || !Number.isFinite(v.at)) return null;
  return {
    id: v.id,
    tool: v.tool,
    agent: oneLine(v.agent, 60),
    state: v.state as ActivityRecord["state"],
    at: v.at,
    detail: oneLine(v.detail, 80),
  };
}

/** Justificatifs lus sans confiance. */
export function parseCredentials(text: string): McpCredentials | null {
  let value: unknown;
  try {
    value = JSON.parse(text);
  } catch {
    return null;
  }
  const v = value as Partial<McpCredentials> | null;
  if (typeof v !== "object" || v === null) return null;
  if (typeof v.clientId !== "string" || !v.clientId) return null;
  if (typeof v.secret !== "string" || !/^([0-9a-f]{2})+$/i.test(v.secret)) {
    return null;
  }
  const bridgeDir =
    typeof v.bridgeDir === "string" && path.isAbsolute(v.bridgeDir)
      ? v.bridgeDir
      : null;
  const editor = typeof v.editor === "string" && v.editor ? v.editor : null;
  return { clientId: v.clientId, secret: v.secret, bridgeDir, editor };
}

/** Le processus vit-il (pour cet utilisateur ou un autre) ? */
export function processAlive(pid: number): boolean {
  try {
    process.kill(pid, 0);
    return true;
  } catch (error) {
    // EPERM : il existe, mais n'est pas à nous.
    return (error as NodeJS.ErrnoException).code === "EPERM";
  }
}

/** Contenu de `window-<séance>.json` : processus de l'hôte d'extensions. */
export function windowRecord(pid = process.pid): string {
  return JSON.stringify({ pid });
}

/**
 * La fenêtre de VS Code de cette séance est-elle ouverte ? Son fichier existe
 * (privé, comme les justificatifs) et le processus qu'il nomme vit encore
 * (fenêtre fermée brutalement : fichier resté, processus mort).
 */
export async function windowOpen(
  stateDir: string,
  session: string,
  platform: Platform,
  alive: (pid: number) => boolean = processAlive,
): Promise<boolean> {
  const text = await readPrivate(
    path.join(stateDir, windowFile(session)),
    platform,
  );
  if (text === null) return false;
  let pid: unknown;
  try {
    pid = (JSON.parse(text) as { pid?: unknown } | null)?.pid;
  } catch {
    return false;
  }
  return typeof pid === "number" && Number.isInteger(pid) && pid > 0
    ? alive(pid)
    : false;
}

/** Dossier privé (0700 sous Linux), créé au besoin. */
export async function ensurePrivateDir(dir: string): Promise<void> {
  await fs.mkdir(dir, { recursive: true, mode: 0o700 });
  if (process.platform !== "win32") await fs.chmod(dir, 0o700);
}

/**
 * Écrit un fichier privé d'un coup : fichier temporaire créé en 0600 (jamais
 * lisible par un autre, même un instant), puis renommé à sa place.
 */
export async function writePrivate(file: string, content: string) {
  await ensurePrivateDir(path.dirname(file));
  const temp = `${file}.${process.pid}.${Date.now()}.tmp`;
  const handle = await fs.open(temp, "wx", 0o600);
  try {
    await handle.writeFile(content, "utf8");
  } finally {
    await handle.close();
  }
  try {
    await fs.rename(temp, file);
  } catch (error) {
    await fs.rm(temp, { force: true });
    throw error;
  }
}

/**
 * Contenu d'un fichier privé, ou null s'il manque ou si, sous Linux, ce
 * n'est pas un fichier ordinaire à nous en 0600 (comme `bridge.json`).
 */
export async function readPrivate(
  file: string,
  platform: Platform,
  uid: number | undefined = process.getuid?.(),
): Promise<string | null> {
  try {
    if (platform === "linux") {
      const stat = await fs.lstat(file);
      if (!stat.isFile()) return null;
      if (uid !== undefined && stat.uid !== uid) return null;
      if ((stat.mode & 0o077) !== 0) return null;
    }
    return await fs.readFile(file, "utf8");
  } catch {
    return null;
  }
}

export async function removeFile(file: string): Promise<void> {
  await fs.rm(file, { force: true });
}

/** Opérations de `eraseCredentials` (essais : échecs simulés). */
export type EraseOps = {
  rm(file: string): Promise<void>;
  overwrite(file: string, content: string): Promise<void>;
  read(file: string): Promise<string | null>;
};

const ERASE_OPS: EraseOps = {
  rm: removeFile,
  overwrite: (file, content) => fs.writeFile(file, content, "utf8"),
  read: (file) => fs.readFile(file, "utf8").catch(() => null),
};

/**
 * Justificatifs effacés, vérifiés après coup : sous Windows, un fichier tenu
 * par un antivirus ou un indexeur ne se supprime pas toujours. Resté là, il
 * est réécrit vide (`{}`, refusé par `parseCredentials`). Lève une erreur si
 * des justificatifs lisibles restent sur le disque.
 */
export async function eraseCredentials(
  file: string,
  ops: EraseOps = ERASE_OPS,
): Promise<void> {
  await ops.rm(file).catch(() => {});
  if ((await ops.read(file)) === null) return;
  await ops.overwrite(file, "{}").catch(() => {});
  const left = await ops.read(file);
  if (left !== null && parseCredentials(left) !== null) {
    throw new Error("credentials left on disk");
  }
}
