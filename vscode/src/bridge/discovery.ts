// Découverte du pont : `bridge.json`, écrit par l'app à côté du point d'accès
// (Linux : $XDG_RUNTIME_DIR/<identifiant>/ ; Windows : dossier des données) et
// supprimé à sa fermeture. Contrat : .tools/research/87-lot1a-contract.md.
import { promises as fs } from "node:fs";
import path from "node:path";

export const IDENTIFIER = "com.chatonlm.chatonlm";
export const DISCOVERY = "bridge.json";
/** Point d'accès sous Linux, à côté de `bridge.json`. */
export const SOCKET = "bridge.sock";
/** Tube nommé de l'app sous Windows (128 bits aléatoires, en hexadécimal). */
const PIPE = /^\\\\\.\\pipe\\chatonlm-[0-9a-f]{32}$/;
/** Versions du protocole que l'extension parle. */
export const PROTOCOL: [number, number] = [1, 1];

export type Discovery = {
  protocol: [number, number];
  endpoint: string;
  pid: number;
  app: string;
  instance: string;
  startedAt: number;
};

export type Platform = "linux" | "windows";

export function currentPlatform(): Platform {
  return process.platform === "win32" ? "windows" : "linux";
}

/**
 * $XDG_RUNTIME_DIR de la session, ou null. Un VS Code en Snap reçoit de snapd
 * un sous-dossier à lui (`/run/user/<uid>/snap.code`) : l'app, lancée hors du
 * Snap, écrit dans le dossier de la session, son parent.
 */
export function runtimeDir(env: NodeJS.ProcessEnv): string | null {
  const runtime = env.XDG_RUNTIME_DIR;
  if (!runtime || !path.posix.isAbsolute(runtime)) return null;
  const dir = path.posix.normalize(runtime).replace(/(.)\/+$/, "$1");
  return /\/snap\.[^/]+$/.test(dir) ? path.posix.dirname(dir) : dir;
}

/**
 * Dossier du fichier de découverte, ou null (Linux sans $XDG_RUNTIME_DIR).
 * `override` : réglage caché `chatonlm.bridgeDir` (essais, Linux seulement ;
 * l'extension ne le lit que dans les réglages de l'utilisateur, jamais dans
 * ceux d'un dossier ouvert). Sous Windows, le pont est toujours cherché dans
 * le dossier des données de l'app.
 */
export function bridgeDir(
  platform: Platform,
  env: NodeJS.ProcessEnv,
  override?: string | null,
): string | null {
  if (platform === "windows") {
    const local = env.LOCALAPPDATA;
    return local && path.win32.isAbsolute(local)
      ? path.win32.join(local, IDENTIFIER)
      : null;
  }
  if (override && path.posix.isAbsolute(override)) return override;
  const runtime = runtimeDir(env);
  return runtime ? path.posix.join(runtime, IDENTIFIER) : null;
}

/**
 * Point d'accès acceptable : sous Windows, un tube local de l'app
 * (`\\.\pipe\chatonlm-…`, jamais `\\hôte\…`, qui ouvrirait une session
 * réseau) ; sous Linux, exactement `<dossier>/bridge.sock` (ni ailleurs, ni
 * socket abstrait).
 */
export function validEndpoint(
  endpoint: string,
  dir: string,
  platform: Platform,
): boolean {
  if (endpoint.includes("\0")) return false;
  if (platform === "windows") return PIPE.test(endpoint);
  return (
    path.posix.isAbsolute(endpoint) &&
    path.posix.normalize(endpoint) === path.posix.join(dir, SOCKET)
  );
}

function isDiscovery(value: unknown): value is Discovery {
  const v = value as Partial<Discovery> | null;
  return (
    typeof v === "object" &&
    v !== null &&
    Array.isArray(v.protocol) &&
    v.protocol.length === 2 &&
    v.protocol.every((n) => Number.isInteger(n)) &&
    typeof v.endpoint === "string" &&
    v.endpoint.length > 0 &&
    typeof v.instance === "string"
  );
}

/**
 * Contenu de `bridge.json`, ou null s'il manque, s'il est illisible, si son
 * point d'accès n'est pas celui de l'app ou, sous Linux, s'il n'est pas un
 * fichier ordinaire à nous en 0600 (sinon ignoré).
 */
export async function readDiscovery(
  dir: string,
  platform: Platform,
  uid: number | undefined = process.getuid?.(),
): Promise<Discovery | null> {
  const file = path.join(dir, DISCOVERY);
  try {
    if (platform === "linux") {
      const stat = await fs.lstat(file);
      if (!stat.isFile()) return null;
      if (uid !== undefined && stat.uid !== uid) return null;
      if ((stat.mode & 0o077) !== 0) return null;
    }
    const value: unknown = JSON.parse(await fs.readFile(file, "utf8"));
    return isDiscovery(value) && validEndpoint(value.endpoint, dir, platform)
      ? value
      : null;
  } catch {
    return null;
  }
}

/** Les versions annoncées par l'app recoupent les nôtres. */
export function compatible(discovery: Discovery): boolean {
  const [min, max] = discovery.protocol;
  return max >= PROTOCOL[0] && min <= PROTOCOL[1];
}
