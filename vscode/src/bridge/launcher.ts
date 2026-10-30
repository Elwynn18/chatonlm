// Lancement de l'app quand elle est fermée : programme trouvé par
// `launcher.json` (écrit par l'app), puis par les emplacements d'installation
// connus ; démarrage masqué (`--background`) dans un environnement nettoyé de
// celui d'un VS Code en Snap ou en AppImage. Contrat :
// .tools/research/87-lot1b-contract.md (B).
import { execFile, spawn } from "node:child_process";
import { promises as fs } from "node:fs";
import path from "node:path";
import { IDENTIFIER, type Platform, runtimeDir } from "./discovery";

/** Argument de l'app : fenêtre masquée (zone de notification). */
export const BACKGROUND_ARG = "--background";
/** Attente de `bridge.json` après un lancement. */
export const LAUNCH_TIMEOUT_MS = 30_000;

/** Ce que la recherche lit du système (remplacé dans les essais). */
export type SystemProbe = {
  readFile(file: string): Promise<string>;
  isFile(file: string): Promise<boolean>;
  /** Valeurs d'une clé du registre de l'utilisateur (Windows). */
  registry(key: string): Promise<Record<string, string>>;
};

export const UNINSTALL_KEY =
  "HKCU\\Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\ChatonLM";

const SNAP_ORIGINAL = "_VSCODE_SNAP_ORIG";

/**
 * Environnement de la session, tel qu'il était avant le Snap de VS Code :
 * valeurs d'origine rendues (`X_VSCODE_SNAP_ORIG`), $XDG_RUNTIME_DIR ramené
 * au dossier de la session (snapd en donne un sous-dossier au Snap), et
 * $XDG_DATA_HOME du Snap (sous `…/snap/…`) oublié. Sans Snap : inchangé.
 */
export function sessionEnv(env: NodeJS.ProcessEnv): NodeJS.ProcessEnv {
  const session: NodeJS.ProcessEnv = {};
  for (const [name, value] of Object.entries(env)) {
    if (value !== undefined && !name.endsWith(SNAP_ORIGINAL)) {
      session[name] = value;
    }
  }
  for (const [name, value] of Object.entries(env)) {
    if (!name.endsWith(SNAP_ORIGINAL)) continue;
    const original = name.slice(0, -SNAP_ORIGINAL.length);
    if (value) session[original] = value;
    else delete session[original];
  }
  const runtime = runtimeDir(session);
  if (runtime) session.XDG_RUNTIME_DIR = runtime;
  if (session.XDG_DATA_HOME && /(^|\/)snap\//.test(session.XDG_DATA_HOME)) {
    delete session.XDG_DATA_HOME;
  }
  return session;
}

/** Dossier des données de l'app (là où elle écrit `launcher.json`). */
export function dataDir(
  platform: Platform,
  hostEnv: NodeJS.ProcessEnv,
  home: string,
): string | null {
  if (platform === "windows") {
    return hostEnv.LOCALAPPDATA
      ? path.win32.join(hostEnv.LOCALAPPDATA, IDENTIFIER)
      : null;
  }
  const env = sessionEnv(hostEnv);
  const base =
    env.XDG_DATA_HOME && path.posix.isAbsolute(env.XDG_DATA_HOME)
      ? env.XDG_DATA_HOME
      : home
        ? path.posix.join(home, ".local", "share")
        : null;
  return base ? path.posix.join(base, IDENTIFIER) : null;
}

/** Valeurs lues dans la sortie de `reg.exe query <clé>`. */
export function parseRegQuery(output: string): Record<string, string> {
  const values: Record<string, string> = {};
  for (const line of output.split(/\r?\n/)) {
    const match = /^\s+(\S.*?)\s{2,}REG_\w+\s{2,}(.*?)\s*$/.exec(line);
    if (match) values[match[1]] = match[2];
  }
  return values;
}

/** `TryExec` (sinon le premier mot d'`Exec`) d'une entrée de menu. */
export function desktopExec(content: string): string | null {
  const entry = /^\[Desktop Entry\]$([\s\S]*?)(?=^\[|(?![\s\S]))/m.exec(
    content,
  );
  const body = entry ? entry[1] : content;
  const tryExec = /^TryExec=(.+)$/m.exec(body)?.[1].trim();
  if (tryExec) return tryExec;
  const exec = /^Exec=(.+)$/m.exec(body)?.[1].trim();
  if (!exec) return null;
  const quoted = /^"([^"]+)"/.exec(exec);
  return quoted ? quoted[1] : exec.split(/\s+/)[0];
}

const unquote = (value: string) => value.trim().replace(/^"(.*)"$/, "$1");

/** Programme de l'app, ou null s'il n'est pas trouvé. */
export async function findApp(
  platform: Platform,
  env: NodeJS.ProcessEnv,
  home: string,
  probe: SystemProbe,
): Promise<string | null> {
  const join = platform === "windows" ? path.win32.join : path.posix.join;
  const absolute =
    platform === "windows" ? path.win32.isAbsolute : path.posix.isAbsolute;
  const data = dataDir(platform, env, home);
  if (data) {
    try {
      const launcher: unknown = JSON.parse(
        await probe.readFile(join(data, "launcher.json")),
      );
      const exe = (launcher as { path?: unknown } | null)?.path;
      if (typeof exe === "string" && absolute(exe) && (await probe.isFile(exe)))
        return exe;
    } catch {
      // Pas encore écrit (app jamais lancée avec le pont) : on cherche ailleurs.
    }
  }
  if (platform === "windows") {
    try {
      const values = await probe.registry(UNINSTALL_KEY);
      const dir = values.InstallLocation && unquote(values.InstallLocation);
      if (dir) {
        let binary = unquote(values.MainBinaryName || "chatonlm.exe");
        if (!/\.exe$/i.test(binary)) binary += ".exe";
        const exe = join(dir, binary);
        if (await probe.isFile(exe)) return exe;
      }
    } catch {
      // Pas installé par l'installateur pour cet utilisateur.
    }
    if (env.LOCALAPPDATA) {
      const exe = join(env.LOCALAPPDATA, "ChatonLM", "chatonlm.exe");
      if (await probe.isFile(exe)) return exe;
    }
    return null;
  }
  if (await probe.isFile("/usr/bin/chatonlm")) return "/usr/bin/chatonlm";
  if (home) {
    try {
      const exe = desktopExec(
        await probe.readFile(
          join(
            home,
            ".local",
            "share",
            "applications",
            `${IDENTIFIER}.desktop`,
          ),
        ),
      );
      if (exe && absolute(exe) && (await probe.isFile(exe))) return exe;
    } catch {
      // Pas d'entrée de menu d'une AppImage.
    }
  }
  return null;
}

/** Variables d'un VS Code en Snap ou en AppImage, ou de son hôte d'extensions. */
const DROPPED = new Set([
  "APPDIR",
  "APPIMAGE",
  "ARGV0",
  "OWD",
  "GTK_PATH",
  "GTK_EXE_PREFIX",
  "GTK_DATA_PREFIX",
  "GTK_IM_MODULE_FILE",
  "GIO_MODULE_DIR",
  "GIO_EXTRA_MODULES",
  "GDK_PIXBUF_MODULE_FILE",
  "GDK_PIXBUF_MODULEDIR",
  "GSETTINGS_SCHEMA_DIR",
  "LD_LIBRARY_PATH",
  "LD_PRELOAD",
  "LOCPATH",
  "PYTHONHOME",
  "QT_PLUGIN_PATH",
  "ELECTRON_RUN_AS_NODE",
  "ELECTRON_NO_ATTACH_CONSOLE",
]);
const DROPPED_PREFIXES = ["SNAP", "VSCODE_", "ELECTRON_"];

/**
 * Environnement pour l'app : celui de la session d'avant le Snap de VS Code
 * (`sessionEnv` : valeurs d'origine, $XDG_RUNTIME_DIR de la session, pour que
 * l'app ouvre son pont là où toute fenêtre le cherche), sans les ajouts du
 * Snap ni ceux d'une AppImage.
 */
export function cleanEnv(env: NodeJS.ProcessEnv): NodeJS.ProcessEnv {
  const session = sessionEnv(env);
  const clean: NodeJS.ProcessEnv = {};
  for (const [name, value] of Object.entries(session)) {
    if (value === undefined || DROPPED.has(name)) continue;
    if (DROPPED_PREFIXES.some((prefix) => name.startsWith(prefix))) continue;
    clean[name] = value;
  }
  return clean;
}

/** Programme du système de Windows, par son chemin complet (jamais cherché
 * dans le dossier courant). */
export function system32(env: NodeJS.ProcessEnv, exe: string): string {
  const root =
    env.SystemRoot && path.win32.isAbsolute(env.SystemRoot)
      ? env.SystemRoot
      : "C:\\Windows";
  return path.win32.join(root, "System32", exe);
}

/** Lance l'app détachée de VS Code ; `background` : sans montrer sa fenêtre. */
export function launch(
  exe: string,
  background: boolean,
  env: NodeJS.ProcessEnv,
  spawnFn: typeof spawn = spawn,
): Promise<void> {
  return new Promise((resolve, reject) => {
    const child = spawnFn(exe, background ? [BACKGROUND_ARG] : [], {
      detached: true,
      stdio: "ignore",
      env: cleanEnv(env),
      cwd: path.dirname(exe),
    });
    child.once("error", reject);
    child.once("spawn", () => {
      child.unref();
      resolve();
    });
  });
}

/** Recherche réelle, sur le système. */
export const systemProbe: SystemProbe = {
  readFile: (file) => fs.readFile(file, "utf8"),
  isFile: async (file) => {
    try {
      return (await fs.stat(file)).isFile();
    } catch {
      return false;
    }
  },
  registry: (key) =>
    new Promise((resolve, reject) => {
      execFile(
        system32(process.env, "reg.exe"),
        ["query", key],
        { windowsHide: true, timeout: 5000 },
        (error, stdout) =>
          error ? reject(error) : resolve(parseRegQuery(stdout)),
      );
    }),
};

/** Noms du processus de l'app. */
const PROCESS_NAMES = new Set(["chatonlm", "chatonlm.exe"]);

/**
 * L'app tourne-t-elle pour cet utilisateur (pont coupé plutôt qu'app
 * fermée) ? null si on ne sait pas le dire.
 */
export async function appRunning(platform: Platform): Promise<boolean | null> {
  try {
    if (platform === "windows") {
      const output = await new Promise<string>((resolve, reject) =>
        execFile(
          system32(process.env, "tasklist.exe"),
          ["/FI", "IMAGENAME eq chatonlm.exe", "/FO", "CSV", "/NH"],
          { windowsHide: true, timeout: 5000 },
          (error, stdout) => (error ? reject(error) : resolve(stdout)),
        ),
      );
      return /^"chatonlm\.exe"/im.test(output);
    }
    const uid = process.getuid?.();
    for (const entry of await fs.readdir("/proc")) {
      if (!/^\d+$/.test(entry)) continue;
      try {
        const dir = `/proc/${entry}`;
        if (uid !== undefined && (await fs.stat(dir)).uid !== uid) continue;
        const comm = (await fs.readFile(`${dir}/comm`, "utf8")).trim();
        if (PROCESS_NAMES.has(comm)) return true;
      } catch {
        // Processus fini entre-temps.
      }
    }
    return false;
  } catch {
    return null;
  }
}

/** Attend que `ready()` réponde vrai (sondé toutes les `everyMs`), ou false à l'échéance. */
export async function waitFor(
  ready: () => Promise<boolean>,
  timeoutMs = LAUNCH_TIMEOUT_MS,
  everyMs = 250,
): Promise<boolean> {
  const deadline = Date.now() + timeoutMs;
  for (;;) {
    if (await ready()) return true;
    if (Date.now() >= deadline) return false;
    await new Promise((resolve) => setTimeout(resolve, everyMs));
  }
}

/**
 * Après un lancement : attend `bridge.json` (`ready`), sauf si l'app tournait
 * déjà sans pont (lien coupé dans ses réglages) : rien n'apparaîtra, la
 * réponse est immédiate.
 */
export function awaitBridge(
  wasRunning: boolean,
  ready: () => Promise<boolean>,
  timeoutMs = LAUNCH_TIMEOUT_MS,
): Promise<boolean> {
  return wasRunning ? ready() : waitFor(ready, timeoutMs);
}
