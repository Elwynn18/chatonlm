// @vitest-environment node
// Lecteur et écrivain du presse-papiers, source de glisser des tests de l'app
// réelle (e2e/lib/clipboard-reader.py, clipboard-writer.py, drag-source.py,
// mêmes contrôles) :
// hors de la session virtuelle (scripts/e2e-session.sh), chacun refuse en code 3
// AVANT d'importer gi (aucun accès à un affichage, donc jamais de fenêtre sur
// le bureau réel) ; dans la session, il retire DISPLAY et WAYLAND_SOCKET
// avant l'import. Chaque lancement se fait avec un environnement vide (comme
// env -i) complété à la main, et un faux paquet « gi » en tête de PYTHONPATH :
// il note l'environnement vu à l'import puis sort en code 42. Même si un
// contrôle cédait, aucune bibliothèque graphique ne serait chargée.
import { spawnSync } from "node:child_process";
import {
  mkdirSync,
  mkdtempSync,
  rmSync,
  symlinkSync,
  unlinkSync,
  writeFileSync,
} from "node:fs";
import { createServer } from "node:net";
import { homedir } from "node:os";
import path from "node:path";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { PYTHON, READER, WRITER } from "../e2e/lib/clipboard.mjs";
import { DRAG_SOURCE } from "../e2e/lib/drag.mjs";

const ROOT = path.resolve(import.meta.dirname, "..");
const RUNS = path.join(ROOT, ".tools/e2e-runs");
const REACHED_GI = 42;
const FAKE_GI = `import json, os, sys
keys = ("DISPLAY", "WAYLAND_SOCKET", "WAYLAND_DISPLAY", "GDK_BACKEND")
print(json.dumps({k: os.environ.get(k) for k in keys}), flush=True)
sys.exit(${REACHED_GI})
`;

let fakeGi;
let scratch;
let runtime;
let homeLink;
const servers = [];
const DISPLAY_NAME = `chatonlm-e2e-${process.pid}`;

/** Socket Unix à `file` (un vrai S_ISSOCK, sans serveur Wayland derrière). */
async function socketAt(file) {
  const server = createServer((connection) => connection.destroy());
  servers.push(server);
  await new Promise((resolve, reject) => {
    server.once("error", reject);
    server.listen(file, resolve);
  });
}

beforeAll(async () => {
  const tmp = path.join(ROOT, ".tools/tmp");
  mkdirSync(tmp, { recursive: true });
  scratch = mkdtempSync(path.join(tmp, "clipboard-reader-"));
  fakeGi = path.join(scratch, "fake-gi");
  mkdirSync(path.join(fakeGi, "gi"), { recursive: true });
  writeFileSync(path.join(fakeGi, "gi/__init__.py"), FAKE_GI);
  // Dossier d'exécution de la forme de celui de la session (mktemp -d
  // /tmp/chatonlm-e2e.XXXXXX), avec sa socket et un leurre au nom du bureau.
  // Toutes les sockets sont sous ce chemin court : celui d'une socket Unix est
  // limité à 108 octets, ce que le chemin du dépôt pourrait dépasser.
  runtime = mkdtempSync("/tmp/chatonlm-e2e.");
  await socketAt(path.join(runtime, DISPLAY_NAME));
  await socketAt(path.join(runtime, "wayland-0"));
  // Vraie socket au nom de la session suivi d'un suffixe : seul le nom,
  // correspondant au début du motif mais pas en entier, doit la faire refuser.
  await socketAt(path.join(runtime, `${DISPLAY_NAME}.desktop`));
  writeFileSync(path.join(runtime, "chatonlm-e2e-1"), "pas une socket");
  // Bonne socket, mauvais dossier d'exécution : un sous-dossier, qui commence
  // comme /tmp/chatonlm-e2e.XXXXXX sans lui correspondre en entier.
  mkdirSync(path.join(runtime, "run"));
  await socketAt(path.join(runtime, "run", DISPLAY_NAME));
  // HOME sous .tools/e2e-runs en apparence, qui mène au vrai dossier personnel
  // (lien symbolique propre à ce processus, retiré en afterAll).
  mkdirSync(RUNS, { recursive: true });
  const link = path.join(RUNS, `clipboard-reader-test-${process.pid}-home`);
  symlinkSync(homedir(), link);
  homeLink = link;
});

afterAll(async () => {
  // unlink retire le lien lui-même, jamais le dossier vers lequel il pointe.
  if (homeLink) unlinkSync(homeLink);
  await Promise.all(
    servers.map((server) => new Promise((resolve) => server.close(resolve))),
  );
  if (runtime) rmSync(runtime, { recursive: true, force: true });
  if (scratch) rmSync(scratch, { recursive: true, force: true });
});

/** Environnement de la session virtuelle (HOME sous .tools/e2e-runs, jamais créé). */
const session = () => ({
  CHATONLM_E2E_SESSION: "1",
  WAYLAND_DISPLAY: DISPLAY_NAME,
  XDG_RUNTIME_DIR: runtime,
  HOME: path.join(RUNS, "clipboard-reader-test/home"),
});

/** Environnement de la session sans la variable `key`. */
function sessionWithout(key) {
  const env = session();
  delete env[key];
  return env;
}

/** Scripts vérifiés : chemin, arguments, début du message de refus. */
const SCRIPTS = [
  {
    name: "clipboard-reader.py",
    script: READER,
    args: ["100"],
    refusal: "Lecture du presse-papiers refusée hors de la session virtuelle",
  },
  {
    name: "clipboard-writer.py",
    script: WRITER,
    args: [JSON.stringify({ text: "x" }), "100"],
    refusal: "Écriture du presse-papiers refusée hors de la session virtuelle",
  },
  {
    name: "drag-source.py",
    script: DRAG_SOURCE,
    args: ["/tmp/x"],
    refusal: "Source de glisser refusée hors de la session virtuelle",
  },
];

/**
 * Lance le script avec seulement `env` (plus PATH et le faux gi) et la liste
 * des modules importés (-X importtime) sur la sortie d'erreur.
 */
function runScript({ script, args }, env) {
  const result = spawnSync(PYTHON, ["-X", "importtime", script, ...args], {
    env: {
      PATH: "/usr/bin:/bin",
      PYTHONPATH: fakeGi,
      PYTHONDONTWRITEBYTECODE: "1",
      ...env,
    },
    encoding: "utf8",
    timeout: 10000,
    killSignal: "SIGKILL",
  });
  if (result.error) throw result.error;
  const imported = result.stderr
    .split("\n")
    .filter((line) => line.startsWith("import time:"))
    .map((line) => line.split("|").at(-1).trim());
  return {
    code: result.status,
    stdout: result.stdout,
    stderr: result.stderr,
    importedGi: imported.some(
      (name) => name === "gi" || name.startsWith("gi."),
    ),
    imported,
  };
}

describe.each(SCRIPTS)("$name", (tested) => {
  const runReader = (env) => runScript(tested, env);

  function expectRefused(env, reason) {
    const result = runReader(env);
    expect(result.stderr).toContain(tested.refusal);
    expect(result.stderr).toMatch(reason);
    expect(result.code).toBe(3);
    expect(result.stdout).toBe("");
    // Le faux gi n'a pas été atteint, et aucun module gi n'a été importé.
    expect(result.importedGi).toBe(false);
    // -X importtime fonctionne bien (sinon l'absence de gi ne prouverait rien).
    expect(result.imported).toContain("re");
  }

  it("reaches gi in the session's environment, without DISPLAY or WAYLAND_SOCKET", () => {
    // Valeurs sans serveur derrière : même sans le faux gi, rien du bureau.
    const result = runReader({
      ...session(),
      DISPLAY: ":987",
      WAYLAND_SOCKET: "987",
    });
    expect(result.code, result.stderr).toBe(REACHED_GI);
    expect(result.importedGi).toBe(true);
    expect(JSON.parse(result.stdout)).toEqual({
      DISPLAY: null,
      WAYLAND_SOCKET: null,
      WAYLAND_DISPLAY: DISPLAY_NAME,
      GDK_BACKEND: "wayland",
    });
  });

  it("refuses an empty environment (env -i) before importing gi", () => {
    expectRefused({}, /CHATONLM_E2E_SESSION absent/);
  });

  it("refuses without the session variable", () => {
    expectRefused(
      sessionWithout("CHATONLM_E2E_SESSION"),
      /CHATONLM_E2E_SESSION absent/,
    );
    expectRefused(
      { ...session(), CHATONLM_E2E_SESSION: "0" },
      /CHATONLM_E2E_SESSION absent/,
    );
  });

  it("refuses another Wayland socket than the session's", () => {
    // wayland-0 existe bien dans le dossier : seul son nom est refusé.
    expectRefused(
      { ...session(), WAYLAND_DISPLAY: "wayland-0" },
      /WAYLAND_DISPLAY='wayland-0' n'est pas la socket de la session/,
    );
    expectRefused(
      { ...session(), WAYLAND_DISPLAY: path.join(runtime, DISPLAY_NAME) },
      /n'est pas la socket de la session/,
    );
    // La socket existe aussi : le nom doit correspondre en entier au motif.
    expectRefused(
      { ...session(), WAYLAND_DISPLAY: `${DISPLAY_NAME}.desktop` },
      /WAYLAND_DISPLAY='chatonlm-e2e-[0-9]+\.desktop' n'est pas la socket de la session/,
    );
    expectRefused(
      sessionWithout("WAYLAND_DISPLAY"),
      /WAYLAND_DISPLAY='' n'est pas la socket de la session/,
    );
  });

  it("refuses another runtime directory than the session's", () => {
    // La socket y existe : seul le dossier est refusé (il ne correspond pas en
    // entier à /tmp/chatonlm-e2e.XXXXXX).
    expectRefused(
      { ...session(), XDG_RUNTIME_DIR: path.join(runtime, "run") },
      /XDG_RUNTIME_DIR=.* n'est pas celui de la session/,
    );
    expectRefused(
      { ...session(), XDG_RUNTIME_DIR: `/run/user/${process.getuid()}` },
      /n'est pas celui de la session/,
    );
    expectRefused(
      {
        ...session(),
        XDG_RUNTIME_DIR: `${runtime}/../${path.basename(runtime)}`,
      },
      /n'est pas celui de la session/,
    );
  });

  it("refuses a missing socket or a file that is not a socket", () => {
    expectRefused(
      { ...session(), WAYLAND_DISPLAY: "chatonlm-e2e-2" },
      /socket chatonlm-e2e-2 introuvable/,
    );
    expectRefused(
      { ...session(), WAYLAND_DISPLAY: "chatonlm-e2e-1" },
      /socket chatonlm-e2e-1 introuvable/,
    );
  });

  it("refuses the real HOME or a HOME outside .tools/e2e-runs", () => {
    expectRefused(
      { ...session(), HOME: homedir() },
      /HOME=.* hors de \.tools\/e2e-runs\//,
    );
    expectRefused({ ...session(), HOME: RUNS }, /hors de \.tools\/e2e-runs\//);
    expectRefused(
      sessionWithout("HOME"),
      /HOME='\/' hors de \.tools\/e2e-runs\//,
    );
  });

  it("refuses a HOME that only starts with .tools/e2e-runs/ (.. not normalized)", () => {
    // Écrit tel quel (path.join résoudrait les « .. ») : il commence par
    // RUNS/ mais désigne la racine du dépôt, que seul realpath fait voir.
    expectRefused(
      { ...session(), HOME: `${RUNS}/x/../../..` },
      /HOME=.* hors de \.tools\/e2e-runs\//,
    );
  });

  it("refuses a symbolic link under .tools/e2e-runs to the real HOME", () => {
    expectRefused(
      { ...session(), HOME: homeLink },
      /HOME=.* hors de \.tools\/e2e-runs\//,
    );
  });
});
