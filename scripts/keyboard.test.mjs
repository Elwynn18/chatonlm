// @vitest-environment node
// Client clavier des tests de l'app réelle (e2e/lib/keyboard.py, client libei
// de l'interface EIS de KWin) : il envoie de vraies touches, donc jamais hors
// de la session virtuelle (scripts/e2e-session.sh). Hors de la session, il
// refuse en code 3 AVANT toute connexion D-Bus et tout import de gi, dbus ou
// ctypes (libei). Chaque lancement se fait avec un environnement vide (comme
// env -i) complété à la main, et de faux paquets « gi », « dbus » et « ctypes »
// en tête de PYTHONPATH : ils notent leur import puis sortent en code 42. Même
// si un contrôle cédait, aucune connexion ne serait ouverte.
//
// Le dernier contrôle (le propriétaire de org.kde.KWin sur le bus doit avoir
// été lancé avec --virtual et la socket de la session) demande un bus : un
// dbus-daemon privé, sans activation de services, dont la socket a la forme
// de celle de dbus-run-session (/tmp/dbus-XXXXXXXXXX). Un faux KWin (Python
// et Gio, sans fenêtre) y prend le nom org.kde.KWin avec la ligne de commande
// voulue. Le vrai gi sert alors, mais seul ce bus est joignable, et le faux
// KWin note l'appel à connectToEIS puis renvoie une erreur au lieu d'un
// clavier ; les faux « dbus » et « ctypes » restent en place.
import { spawn, spawnSync } from "node:child_process";
import {
  existsSync,
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
import { PYTHON } from "../e2e/lib/clipboard.mjs";
import { KEYBOARD, pressKeys } from "../e2e/lib/keyboard.mjs";

const ROOT = path.resolve(import.meta.dirname, "..");
const RUNS = path.join(ROOT, ".tools/e2e-runs");
const REACHED = 42;
/** Bibliothèques qui mènent à un bus ou à KWin : jamais avant le refus. */
const GUARDED = ["gi", "dbus", "ctypes", "_ctypes"];
const fakeModule = (name) => `import json, sys
print(json.dumps({"reached": ${JSON.stringify(name)}}), flush=True)
sys.exit(${REACHED})
`;
const DISPLAY_NAME = `chatonlm-e2e-${process.pid}`;
const UID = process.getuid();
const DESKTOP_BUS = `/run/user/${UID}/bus`;
const GUID = "0123456789abcdef".repeat(2);

// Bus privé : pas de <servicedir>, donc aucun service activé à la demande
// (un appel à un nom sans propriétaire échoue au lieu de lancer quoi que ce
// soit). Socket dans /tmp, nommée comme celles de dbus-run-session.
const BUS_CONFIG = `<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN"
 "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
<busconfig>
  <type>session</type>
  <listen>unix:tmpdir=/tmp</listen>
  <auth>EXTERNAL</auth>
  <policy context="default">
    <allow send_destination="*" eavesdrop="true"/>
    <allow eavesdrop="true"/>
    <allow own="*"/>
  </policy>
</busconfig>
`;

// Faux KWin : prend org.kde.KWin sur le bus donné (argument 1) ; les autres
// arguments ne servent qu'à sa ligne de commande, lue par keyboard.py.
const FAKE_KWIN = `import json, sys
import gi
gi.require_version("Gio", "2.0")
from gi.repository import Gio, GLib

connection = Gio.DBusConnection.new_for_address_sync(
    sys.argv[1],
    Gio.DBusConnectionFlags.AUTHENTICATION_CLIENT
    | Gio.DBusConnectionFlags.MESSAGE_BUS_CONNECTION,
    None,
    None,
)
info = Gio.DBusNodeInfo.new_for_xml("""<node>
  <interface name="org.kde.KWin.EIS.RemoteDesktop">
    <method name="connectToEIS">
      <arg type="i" direction="in"/>
      <arg type="h" direction="out"/>
      <arg type="i" direction="out"/>
    </method>
  </interface>
</node>""").interfaces[0]


def on_call(connection, sender, path, interface, method, parameters, invocation):
    print(json.dumps({"called": method}), flush=True)
    invocation.return_dbus_error("org.chatonlm.Test.EisReached", "connectToEIS atteint")


# register_object est déprécié depuis GLib 2.84 au profit de cette variante.
register = getattr(connection, "register_object_with_closures2", None) or connection.register_object
register("/org/kde/KWin/EIS/RemoteDesktop", info, on_call, None, None)
# 4 : DO_NOT_QUEUE ; réponse 1 : propriétaire du nom.
owner = connection.call_sync(
    "org.freedesktop.DBus",
    "/org/freedesktop/DBus",
    "org.freedesktop.DBus",
    "RequestName",
    GLib.Variant("(su)", ("org.kde.KWin", 4)),
    GLib.VariantType.new("(u)"),
    Gio.DBusCallFlags.NONE,
    3000,
    None,
).unpack()[0]
if owner != 1:
    sys.exit(f"org.kde.KWin non obtenu : {owner}")
print("ready", flush=True)
GLib.MainLoop().run()
`;

let scratch;
let fakes;
let fakesWithoutGi;
let fakeKwin;
let runtime;
let fakeBus;
let busLink;
let homeLink;
const servers = [];

/** Socket Unix à `file` (un vrai S_ISSOCK, sans serveur derrière). */
async function socketAt(file) {
  const server = createServer((connection) => connection.destroy());
  servers.push(server);
  await new Promise((resolve, reject) => {
    server.once("error", reject);
    server.listen(file, resolve);
  });
}

/** Dossier de faux paquets Python (un par nom de `names`). */
function fakePackages(dir, names) {
  for (const name of names) {
    mkdirSync(path.join(dir, name), { recursive: true });
    writeFileSync(path.join(dir, name, "__init__.py"), fakeModule(name));
  }
  return dir;
}

beforeAll(async () => {
  const tmp = path.join(ROOT, ".tools/tmp");
  mkdirSync(tmp, { recursive: true });
  scratch = mkdtempSync(path.join(tmp, "keyboard-"));
  fakes = fakePackages(path.join(scratch, "fakes"), ["gi", "dbus", "ctypes"]);
  fakesWithoutGi = fakePackages(path.join(scratch, "fakes-without-gi"), [
    "dbus",
    "ctypes",
  ]);
  fakeKwin = path.join(scratch, "fake-kwin.py");
  writeFileSync(fakeKwin, FAKE_KWIN);
  // Dossier d'exécution de la forme de celui de la session (mktemp -d
  // /tmp/chatonlm-e2e.XXXXXX), avec sa socket et des leurres. Toutes les
  // sockets sont sous des chemins courts : celui d'une socket Unix est limité
  // à 108 octets, ce que le chemin du dépôt pourrait dépasser.
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
  // Bus de la session en apparence (/tmp/dbus-XXXX, comme dbus-run-session),
  // sans dbus-daemon derrière ; et un lien de même forme vers le bus du
  // bureau, jamais ouvert (le faux gi arrêterait tout avant).
  fakeBus = `/tmp/dbus-chatonlmkb${process.pid}`;
  await socketAt(fakeBus);
  busLink = `/tmp/dbus-chatonlmkblink${process.pid}`;
  symlinkSync(DESKTOP_BUS, busLink);
  // HOME sous .tools/e2e-runs en apparence, qui mène au vrai dossier personnel
  // (lien symbolique propre à ce processus, retiré en afterAll).
  mkdirSync(RUNS, { recursive: true });
  const link = path.join(RUNS, `keyboard-test-${process.pid}-home`);
  symlinkSync(homedir(), link);
  homeLink = link;
});

afterAll(async () => {
  // unlink retire les liens eux-mêmes, jamais ce vers quoi ils pointent.
  if (homeLink) unlinkSync(homeLink);
  if (busLink) unlinkSync(busLink);
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
  HOME: path.join(RUNS, "keyboard-test/home"),
  DBUS_SESSION_BUS_ADDRESS: `unix:path=${fakeBus},guid=${GUID}`,
});

/** Environnement de la session sans la variable `key`. */
function sessionWithout(key) {
  const env = session();
  delete env[key];
  return env;
}

/**
 * Lance le client (accord « tab ») avec seulement `env` (plus PATH et les
 * faux paquets) et la liste des modules importés (-X importtime) sur la
 * sortie d'erreur.
 */
function runKeyboard(env, packages = fakes) {
  const result = spawnSync(PYTHON, ["-X", "importtime", KEYBOARD, "tab"], {
    env: {
      PATH: "/usr/bin:/bin",
      PYTHONPATH: packages,
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
    imported,
    /** Modules gardés importés (paquet ou sous-module). */
    guarded: imported.filter((name) =>
      GUARDED.some((top) => name === top || name.startsWith(`${top}.`)),
    ),
  };
}

const REFUSED =
  "Touches refusées hors de la session virtuelle (scripts/e2e-session.sh) : ";

function expectRefused(env, reason) {
  const result = runKeyboard(env);
  expect(result.stderr).toContain(REFUSED);
  expect(result.stderr).toMatch(reason);
  expect(result.code).toBe(3);
  expect(result.stdout).toBe("");
  // Aucun faux paquet atteint, aucun module gi, dbus ou ctypes importé.
  expect(result.guarded).toEqual([]);
  // -X importtime fonctionne bien (sinon l'absence de gi ne prouverait rien).
  expect(result.imported).toContain("re");
}

describe("keyboard.py, before any D-Bus connection", () => {
  it("reaches gi in the session's environment, and nothing else before", () => {
    const result = runKeyboard(session());
    expect(result.code, result.stderr).toBe(REACHED);
    expect(JSON.parse(result.stdout)).toEqual({ reached: "gi" });
    expect(result.guarded).toEqual(["gi"]);
  });

  it("refuses an empty environment (env -i)", () => {
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
      { ...session(), XDG_RUNTIME_DIR: `/run/user/${UID}` },
      /n'est pas celui de la session/,
    );
    expectRefused(
      {
        ...session(),
        XDG_RUNTIME_DIR: `${runtime}/../${path.basename(runtime)}`,
      },
      /n'est pas celui de la session/,
    );
    expectRefused(
      sessionWithout("XDG_RUNTIME_DIR"),
      /XDG_RUNTIME_DIR='' n'est pas celui de la session/,
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

  it("refuses the desktop's D-Bus bus under /run/user, however written", () => {
    const notSession =
      /DBUS_SESSION_BUS_ADDRESS=.* n'est pas le bus de la session/;
    for (const address of [
      `unix:path=${DESKTOP_BUS}`,
      `unix:path=${DESKTOP_BUS},guid=${GUID}`,
      `unix:path=/tmp/../run/user/${UID}/bus`,
      `unix:path=/run//user/${UID}/bus`,
      `unix:path=/run/./user/${UID}/bus`,
      `unix:path=run/user/${UID}/bus`,
    ]) {
      expectRefused(
        { ...session(), DBUS_SESSION_BUS_ADDRESS: address },
        notSession,
      );
    }
  });

  it("refuses a link shaped like the session's bus to the desktop's bus", () => {
    expectRefused(
      { ...session(), DBUS_SESSION_BUS_ADDRESS: `unix:path=${busLink}` },
      /DBUS_SESSION_BUS_ADDRESS=.* n'est pas le bus de la session/,
    );
  });

  it("refuses any other bus address than the session's", () => {
    const notSession =
      /DBUS_SESSION_BUS_ADDRESS=.* n'est pas le bus de la session/;
    expectRefused(
      sessionWithout("DBUS_SESSION_BUS_ADDRESS"),
      /DBUS_SESSION_BUS_ADDRESS='' n'est pas le bus de la session/,
    );
    for (const address of [
      "",
      // Le bus de la session suivi d'un autre : les deux seraient essayés.
      `unix:path=${fakeBus};unix:path=${DESKTOP_BUS}`,
      `unix:path=${fakeBus},guid=${GUID};unix:path=${DESKTOP_BUS}`,
      // Socket abstraite, recherche par X11, bus d'un autre dossier.
      `unix:abstract=${fakeBus}`,
      "autolaunch:",
      "unix:runtime=yes",
      // Forme du bus de la session, mais pas de socket derrière.
      `unix:path=/tmp/dbus-chatonlmkbmissing${process.pid}`,
    ]) {
      expectRefused(
        { ...session(), DBUS_SESSION_BUS_ADDRESS: address },
        notSession,
      );
    }
  });
});

describe("keyboard.py, KWin on a private bus", { timeout: 30000 }, () => {
  let daemon;
  let busAddress;
  let busSocket;

  beforeAll(async () => {
    const config = path.join(scratch, "bus.conf");
    writeFileSync(config, BUS_CONFIG);
    daemon = spawn(
      "dbus-daemon",
      [
        `--config-file=${config}`,
        "--nofork",
        "--nopidfile",
        "--print-address=1",
      ],
      {
        env: { PATH: "/usr/bin:/bin" },
        cwd: scratch,
        stdio: ["ignore", "pipe", "pipe"],
      },
    );
    daemon.closed = new Promise((resolve) => daemon.once("close", resolve));
    busAddress = await new Promise((resolve, reject) => {
      let out = "";
      let err = "";
      const timer = setTimeout(
        () => reject(new Error(`dbus-daemon sans adresse : ${err}`)),
        10000,
      );
      daemon.stderr.setEncoding("utf8").on("data", (data) => (err += data));
      daemon.stdout.setEncoding("utf8").on("data", (data) => {
        out += data;
        if (out.includes("\n")) {
          clearTimeout(timer);
          resolve(out.split("\n")[0]);
        }
      });
      daemon.once("error", reject);
      daemon.once("exit", (code) =>
        reject(new Error(`dbus-daemon arrêté (${code}) : ${err}`)),
      );
    });
    // Même forme que le bus de dbus-run-session dans scripts/e2e-session.sh.
    const match = busAddress.match(
      /^unix:path=(\/tmp\/dbus-[A-Za-z0-9]+),guid=[0-9a-f]{32}$/,
    );
    expect(match, busAddress).not.toBeNull();
    busSocket = match[1];
  });

  afterAll(async () => {
    if (!daemon?.pid) return;
    daemon.kill("SIGTERM");
    await daemon.closed;
    // dbus-daemon retire sa socket en s'arrêtant ; au cas où.
    if (busSocket && existsSync(busSocket)) rmSync(busSocket, { force: true });
  });

  /**
   * Lance le faux KWin avec `args` sur sa ligne de commande, attend qu'il ait
   * le nom org.kde.KWin, lance le client (vrai gi, faux dbus et ctypes) avec
   * l'environnement de la session et ce bus, puis arrête le faux KWin.
   */
  async function againstKwin(args) {
    const kwin = spawn(PYTHON, [fakeKwin, busAddress, ...args], {
      env: { PATH: "/usr/bin:/bin", PYTHONDONTWRITEBYTECODE: "1" },
      stdio: ["ignore", "pipe", "pipe"],
    });
    const output = { stdout: "", stderr: "" };
    kwin.stdout
      .setEncoding("utf8")
      .on("data", (data) => (output.stdout += data));
    kwin.stderr
      .setEncoding("utf8")
      .on("data", (data) => (output.stderr += data));
    const closed = new Promise((resolve) => kwin.once("close", resolve));
    try {
      await new Promise((resolve, reject) => {
        const timer = setTimeout(
          () => reject(new Error(`faux KWin pas prêt : ${output.stderr}`)),
          10000,
        );
        kwin.stdout.on("data", () => {
          if (output.stdout.includes("ready\n")) {
            clearTimeout(timer);
            resolve();
          }
        });
        kwin.once("exit", (code) => {
          clearTimeout(timer);
          reject(new Error(`faux KWin arrêté (${code}) : ${output.stderr}`));
        });
      });
      const result = runKeyboard(
        { ...session(), DBUS_SESSION_BUS_ADDRESS: busAddress },
        fakesWithoutGi,
      );
      return { ...result, kwinPid: kwin.pid, kwin: output };
    } finally {
      kwin.kill("SIGKILL");
      await closed;
    }
  }

  /** Modules gardés autres que gi (le vrai, chargé dans ces essais). */
  const withoutGi = (names) =>
    names.filter((name) => name !== "gi" && !name.startsWith("gi."));

  async function expectKwinRefused(args, reason) {
    const result = await againstKwin(args);
    expect(result.stderr).toContain(REFUSED);
    expect(result.stderr).toContain(`KWin (PID ${result.kwinPid}) ${reason}`);
    expect(result.code).toBe(3);
    expect(result.stdout).toBe("");
    // Refusé avant l'interface EIS : le faux KWin n'a reçu aucun appel.
    expect(result.kwin.stdout).toBe("ready\n");
    expect(withoutGi(result.guarded)).toEqual([]);
  }

  it("goes on to KWin's EIS interface when KWin is virtual with the session's socket", async () => {
    const result = await againstKwin(["--virtual", "--socket", DISPLAY_NAME]);
    // Erreur renvoyée par le faux KWin à connectToEIS : contrôle passé.
    expect(result.stderr).toContain("org.chatonlm.Test.EisReached");
    expect(result.code).toBe(1);
    expect(result.kwin.stdout).toBe('ready\n{"called": "connectToEIS"}\n');
    // Le vrai gi (et ses sous-modules), mais toujours ni dbus ni ctypes.
    expect(result.guarded).toContain("gi");
    expect(withoutGi(result.guarded)).toEqual([]);
  });

  it("refuses a KWin that is not virtual", async () => {
    await expectKwinRefused(["--socket", DISPLAY_NAME], "n'est pas virtuel");
    // Ligne de commande du KWin d'un bureau réel.
    await expectKwinRefused(
      ["--wayland-fd", "7", "--socket", "wayland-0", "--xwayland"],
      "n'est pas virtuel",
    );
  });

  it("refuses a virtual KWin with another socket than the session's", async () => {
    const otherSocket = `n'a pas la socket ${DISPLAY_NAME}`;
    await expectKwinRefused(
      ["--virtual", "--socket", "wayland-0"],
      otherSocket,
    );
    await expectKwinRefused(
      ["--virtual", "--socket", `${DISPLAY_NAME}.desktop`],
      otherSocket,
    );
    await expectKwinRefused(
      ["--virtual", `--socket=${DISPLAY_NAME}`],
      otherSocket,
    );
    await expectKwinRefused(["--virtual", "--socket"], otherSocket);
    await expectKwinRefused(["--virtual"], otherSocket);
  });
});

describe("pressKeys (keyboard.mjs)", () => {
  it("refuses outside the session before running the client", async () => {
    // Si cette garde cédait, le client serait lancé avec l'environnement de
    // ce processus, sans la variable de la session et avec les faux paquets :
    // il refuserait à son tour (code 3), avec un autre message.
    const saved = {
      CHATONLM_E2E_SESSION: process.env.CHATONLM_E2E_SESSION,
      PYTHONPATH: process.env.PYTHONPATH,
    };
    process.env.PYTHONPATH = fakes;
    try {
      for (const value of [undefined, "0", "true"]) {
        if (value === undefined) delete process.env.CHATONLM_E2E_SESSION;
        else process.env.CHATONLM_E2E_SESSION = value;
        await expect(pressKeys("tab")).rejects.toThrow(
          /^Touches refusées hors de la session virtuelle\.$/,
        );
      }
    } finally {
      for (const [key, value] of Object.entries(saved)) {
        if (value === undefined) delete process.env[key];
        else process.env[key] = value;
      }
    }
  });
});
