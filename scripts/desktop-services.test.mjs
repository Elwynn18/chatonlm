// @vitest-environment node
// Faux bureau D-Bus des essais de la dictée (e2e/lib/desktop-services.py) :
// hors de la session virtuelle (scripts/e2e-session.sh), il refuse en code 3
// AVANT d'importer gi, donc avant toute connexion au bus : jamais de faux
// portail ni de fausse zone de notification sur le bus du bureau réel. Mêmes
// contrôles que clipboard-reader.py (voir clipboard-reader.test.mjs), plus le
// bus de la session. Chaque lancement se fait avec un environnement vide
// complété à la main, et un faux paquet « gi » en tête de PYTHONPATH (sortie
// en code 42 s'il est atteint).
import { spawnSync } from "node:child_process";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { createServer } from "node:net";
import path from "node:path";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { PYTHON } from "../e2e/lib/clipboard.mjs";
import { SERVICES } from "../e2e/lib/desktop.mjs";

const ROOT = path.resolve(import.meta.dirname, "..");
const RUNS = path.join(ROOT, ".tools/e2e-runs");
const REACHED_GI = 42;
const FAKE_GI = `import json, os, sys
print(json.dumps({"bus": os.environ.get("DBUS_SESSION_BUS_ADDRESS")}), flush=True)
sys.exit(${REACHED_GI})
`;
const DISPLAY_NAME = `chatonlm-e2e-${process.pid}`;

let scratch;
let fakeGi;
let runtime;
let server;

beforeAll(async () => {
  const tmp = path.join(ROOT, ".tools/tmp");
  mkdirSync(tmp, { recursive: true });
  scratch = mkdtempSync(path.join(tmp, "desktop-services-"));
  fakeGi = path.join(scratch, "fake-gi");
  mkdirSync(path.join(fakeGi, "gi"), { recursive: true });
  writeFileSync(path.join(fakeGi, "gi/__init__.py"), FAKE_GI);
  // Dossier d'exécution de la forme de celui de la session, avec sa socket
  // Wayland (sans serveur derrière).
  runtime = mkdtempSync("/tmp/chatonlm-e2e.");
  server = createServer((connection) => connection.destroy());
  await new Promise((resolve, reject) => {
    server.once("error", reject);
    server.listen(path.join(runtime, DISPLAY_NAME), resolve);
  });
});

afterAll(async () => {
  if (server) await new Promise((resolve) => server.close(resolve));
  if (runtime) rmSync(runtime, { recursive: true, force: true });
  if (scratch) rmSync(scratch, { recursive: true, force: true });
});

/** Environnement de la session virtuelle (bus de dbus-run-session). */
const session = () => ({
  CHATONLM_E2E_SESSION: "1",
  WAYLAND_DISPLAY: DISPLAY_NAME,
  XDG_RUNTIME_DIR: runtime,
  HOME: path.join(RUNS, "desktop-services-test/home"),
  DBUS_SESSION_BUS_ADDRESS: "unix:path=/tmp/dbus-AbCdEf,guid=0123",
});

function run(env) {
  const result = spawnSync(
    PYTHON,
    ["-X", "importtime", SERVICES, JSON.stringify({ watcher: true })],
    {
      env: {
        PATH: "/usr/bin:/bin",
        PYTHONPATH: fakeGi,
        PYTHONDONTWRITEBYTECODE: "1",
        ...env,
      },
      encoding: "utf8",
      timeout: 10000,
      killSignal: "SIGKILL",
    },
  );
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

function expectRefused(env, reason) {
  const result = run(env);
  expect(result.stderr).toContain(
    "Faux services du bureau refusés hors de la session virtuelle",
  );
  expect(result.stderr).toMatch(reason);
  expect(result.code).toBe(3);
  expect(result.stdout).toBe("");
  expect(result.importedGi).toBe(false);
  // -X importtime fonctionne (sinon l'absence de gi ne prouverait rien).
  expect(result.imported).toContain("re");
}

describe("desktop-services.py", () => {
  it("reaches gi in the session's environment", () => {
    const result = run(session());
    expect(result.code, result.stderr).toBe(REACHED_GI);
    expect(JSON.parse(result.stdout)).toEqual({
      bus: "unix:path=/tmp/dbus-AbCdEf,guid=0123",
    });
  });

  it("refuses an empty environment (env -i) before importing gi", () => {
    expectRefused({}, /CHATONLM_E2E_SESSION absent/);
  });

  it("refuses the desktop's own bus", () => {
    expectRefused(
      {
        ...session(),
        DBUS_SESSION_BUS_ADDRESS: `unix:path=/run/user/${process.getuid()}/bus`,
      },
      /n'est pas le bus de la session/,
    );
    const withoutBus = session();
    delete withoutBus.DBUS_SESSION_BUS_ADDRESS;
    expectRefused(withoutBus, /DBUS_SESSION_BUS_ADDRESS='' n'est pas le bus/);
  });

  it("refuses another runtime directory or HOME than the session's", () => {
    expectRefused(
      { ...session(), XDG_RUNTIME_DIR: `/run/user/${process.getuid()}` },
      /n'est pas celui de la session/,
    );
    expectRefused(
      { ...session(), WAYLAND_DISPLAY: "wayland-0" },
      /n'est pas la socket de la session/,
    );
    expectRefused(
      { ...session(), HOME: ROOT },
      /HOME=.* hors de \.tools\/e2e-runs\//,
    );
  });
});
