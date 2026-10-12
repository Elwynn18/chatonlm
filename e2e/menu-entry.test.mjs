// Inscription au menu des applications d'une AppImage (étape 18a), dans la
// session virtuelle (XDG_DATA_HOME de l'essai). APPIMAGE est posé vers une
// fausse AppImage (script qui note comment il a été lancé), dans un dossier
// dont le nom a une espace, une apostrophe, un « $ » et des parenthèses :
// - premier lancement : entrée et icône écrites (marqueur, TryExec, Icon,
//   StartupWMClass, desktop-file-validate), Exec relu par GLib (`gio launch`
//   lance la fausse AppImage par ce chemin) ; ligne « Applications menu » de
//   Settings › General activée ;
// - interrupteur coupé : entrée et icône retirées, « Saved » ; relance :
//   rien n'est réécrit ; rallumé : écrites de nouveau ;
// - AppImage déplacée dans un dossier dont le nom a un « % » : chemin mis à
//   jour au lancement suivant, « %% » dans l'Exec (spécification ; GLib,
//   qui cherche le programme avant de remplacer « %% », ne sait pas lancer
//   un tel chemin : pas de `gio launch` ici) ; l'icône n'a pas changé : les
//   bureaux ne sont pas prévenus ;
// - icône d'une version précédente de ChatonLM : remplacée au lancement, thème
//   d'icônes marqué changé (date de `icons/hicolor`), signal `iconChanged` de
//   KIconLoader émis sur le bus de la session d'essai (sans lui, Plasma garde
//   l'ancienne icône dans sa barre des tâches) ;
// - fichier sans marqueur (copie de l'utilisateur) jamais écrasé ni retiré ;
// - sans APPIMAGE : ligne absente, rien d'écrit.
import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import {
  chmodSync,
  existsSync,
  mkdirSync,
  readFileSync,
  renameSync,
  rmSync,
  statSync,
  utimesSync,
  writeFileSync,
} from "node:fs";
import path from "node:path";
import { ROOT, withApp } from "./lib/app.mjs";
import { poll } from "./lib/desktop.mjs";
import { startFake } from "./lib/fake.mjs";

const binary = process.argv[2];
const IDENTIFIER = "com.chatonlm.chatonlm";
const DATA_HOME = process.env.XDG_DATA_HOME;
assert.ok(
  DATA_HOME?.startsWith(`${ROOT}.tools/`),
  "XDG_DATA_HOME hors de .tools : lancer dans scripts/e2e-session.sh",
);
const ENTRY = `${DATA_HOME}/applications/${IDENTIFIER}.desktop`;
const ICON = `${DATA_HOME}/icons/hicolor/256x256/apps/${IDENTIFIER}.png`;
const ICON_BYTES = readFileSync(`${ROOT}src-tauri/icons/256x256.png`);
const THEME = `${DATA_HOME}/icons/hicolor`;
const WORK = `${ROOT}.tools/tmp/menu-entry-${process.pid}`;
const LAUNCHED = `${WORK}/launched.txt`;
const FIRST = `${WORK}/My Apps/ChatonLM's $HOME (beta)/ChatonLM.AppImage`;
const MOVED = `${WORK}/100% sûr/ChatonLM-0.9.AppImage`;
const FOREIGN =
  "[Desktop Entry]\nType=Application\nName=My ChatonLM\nExec=/usr/bin/true\n";
const delay = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

/** Fausse AppImage : note son chemin et ses arguments. */
function fakeAppImage(file) {
  mkdirSync(path.dirname(file), { recursive: true });
  writeFileSync(file, `#!/bin/sh\nprintf '%s\\n' "$0" "$@" > '${LAUNCHED}'\n`);
  chmodSync(file, 0o755);
}

/** Valeurs d'une clé de l'entrée. */
const values = (text, key) =>
  text
    .split("\n")
    .filter((line) => line.startsWith(`${key}=`))
    .map((line) => line.slice(key.length + 1));

/**
 * Attend l'entrée écrite pour `appimage`, puis la vérifie entièrement ;
 * `launch` : la lancer par GLib.
 */
async function checkEntry(appimage, { launch: viaGlib = true } = {}) {
  const text = await poll(
    () => (existsSync(ENTRY) ? readFileSync(ENTRY, "utf8") : ""),
    (content) => values(content, "TryExec")[0] === appimage,
    { what: `entrée pour ${appimage}` },
  );
  assert.deepEqual(values(text, "X-ChatonLM-AppImage"), ["true"]);
  assert.deepEqual(values(text, "Icon"), [IDENTIFIER]);
  assert.deepEqual(values(text, "StartupWMClass"), [IDENTIFIER]);
  assert.deepEqual(values(text, "Name"), ["ChatonLM"]);
  assert.deepEqual(readFileSync(ICON), ICON_BYTES);
  const validate = spawnSync("desktop-file-validate", [ENTRY], {
    encoding: "utf8",
  });
  if (validate.error?.code === "ENOENT") {
    console.log("desktop-file-validate absent : validation sautée");
  } else {
    assert.equal(
      validate.status,
      0,
      `desktop-file-validate : ${validate.stdout}${validate.stderr}`,
    );
  }
  if (!viaGlib) return text;
  // Exec relu par GLib : la fausse AppImage est lancée par son chemin.
  rmSync(LAUNCHED, { force: true });
  const launch = spawnSync("gio", ["launch", ENTRY], {
    encoding: "utf8",
    timeout: 10000,
  });
  assert.equal(launch.status, 0, `gio launch : ${launch.stderr}`);
  const launched = await poll(
    () => (existsSync(LAUNCHED) ? readFileSync(LAUNCHED, "utf8") : ""),
    (content) => content.endsWith("\n"),
    { what: "fausse AppImage lancée" },
  );
  assert.equal(launched, `${appimage}\n`);
  return text;
}

/**
 * Écoute, sur le bus de la session d'essai, le signal par lequel KDE apprend
 * que des icônes ont changé ; `count()` : signaux reçus, `stop()` : fin.
 */
async function watchIconSignal() {
  const monitor = spawn(
    "dbus-monitor",
    [
      "--session",
      "type='signal',interface='org.kde.KIconLoader',member='iconChanged'",
    ],
    { stdio: ["ignore", "pipe", "inherit"] },
  );
  let output = "";
  monitor.stdout.on("data", (chunk) => (output += chunk));
  // Prêt : dbus-monitor a reçu ses propres signaux d'arrivée sur le bus.
  await poll(
    () => output,
    (text) => text.includes("NameAcquired"),
    { what: "dbus-monitor à l'écoute" },
  );
  return {
    count: () =>
      (
        output.match(
          /path=\/KIconLoader; interface=org\.kde\.KIconLoader; member=iconChanged/g,
        ) ?? []
      ).length,
    stop: () => monitor.kill(),
  };
}

async function openGeneral(driver) {
  await driver.waitFor("[data-testid=new-chat]");
  const show = await driver.findAll("button[aria-label='Show sidebar']");
  if (show.length > 0) await driver.click(show[0]);
  await driver.click(await driver.waitFor("nav a[href='/settings']"));
  await driver.waitFor("[data-testid=settings]");
}

const switchState = (driver) =>
  driver.execute(
    "return document.querySelector('[data-testid=menu-entry] [role=switch]')?.getAttribute('aria-checked') ?? null",
  );

const clickSwitch = async (driver) =>
  driver.click(await driver.waitFor("[data-testid=menu-entry] [role=switch]"));

const fake = await startFake("ollama");
const env = { OLLAMA_HOST: fake.url };
fakeAppImage(FIRST);

try {
  // 1. Premier lancement : entrée écrite ; retirée par l'interrupteur.
  await withApp(
    binary,
    { name: "menu-entry-1", env: { ...env, APPIMAGE: FIRST } },
    async (driver) => {
      await checkEntry(FIRST);
      assert.deepEqual(await driver.invoke("menu_entry_status"), {
        available: true,
        enabled: true,
      });
      await openGeneral(driver);
      await poll(
        () => switchState(driver),
        (state) => state === "true",
        { what: "ligne Applications menu activée" },
      );
      await driver.screenshot(".tools/shots/settings-menu-entry.png");
      await clickSwitch(driver);
      await poll(
        () => switchState(driver),
        (state) => state === "false",
        { what: "interrupteur coupé" },
      );
      assert.ok(!existsSync(ENTRY), "entrée encore là");
      assert.ok(!existsSync(ICON), "icône encore là");
      assert.deepEqual(await driver.invoke("menu_entry_status"), {
        available: true,
        enabled: false,
      });
    },
  );

  // 2. Relance : le retrait est respecté ; rallumé, l'entrée revient.
  await withApp(
    binary,
    { name: "menu-entry-2", env: { ...env, APPIMAGE: FIRST } },
    async (driver) => {
      await openGeneral(driver);
      await poll(
        () => switchState(driver),
        (state) => state === "false",
        { what: "ligne Applications menu coupée" },
      );
      // L'écriture du démarrage se ferait en quelques millisecondes.
      await delay(3000);
      assert.ok(!existsSync(ENTRY), "entrée réécrite malgré le retrait");
      assert.ok(!existsSync(ICON), "icône réécrite malgré le retrait");
      await clickSwitch(driver);
      await checkEntry(FIRST);
      assert.equal(await switchState(driver), "true");
    },
  );

  // 3. AppImage déplacée : chemin mis à jour au lancement ; l'icône n'a pas
  //    changé, les bureaux ne sont pas prévenus.
  mkdirSync(path.dirname(MOVED), { recursive: true });
  renameSync(FIRST, MOVED);
  const longAgo = new Date("2026-01-01T00:00:00Z");
  utimesSync(THEME, longAgo, longAgo);
  let signals = await watchIconSignal();
  try {
    await withApp(
      binary,
      { name: "menu-entry-3", env: { ...env, APPIMAGE: MOVED } },
      async () => {
        const text = await checkEntry(MOVED, { launch: false });
        assert.deepEqual(values(text, "Exec"), [
          `"${MOVED.replace("%", "%%")}"`,
        ]);
        await delay(1000);
      },
    );
    assert.equal(signals.count(), 0, "signal émis sans icône changée");
    assert.equal(statSync(THEME).mtimeMs, longAgo.getTime());
  } finally {
    signals.stop();
  }

  // 3 bis. Icône d'une version précédente : remplacée, bureaux prévenus.
  writeFileSync(ICON, "icon of an older ChatonLM");
  utimesSync(THEME, longAgo, longAgo);
  signals = await watchIconSignal();
  try {
    await withApp(
      binary,
      { name: "menu-entry-3-icon", env: { ...env, APPIMAGE: MOVED } },
      async () => {
        await poll(
          () => readFileSync(ICON),
          (bytes) => bytes.equals(ICON_BYTES),
          { what: "icône remplacée" },
        );
        await poll(
          () => signals.count(),
          (count) => count === 1,
          { what: "signal iconChanged de KIconLoader" },
        );
        assert.ok(
          statSync(THEME).mtimeMs > longAgo.getTime(),
          "thème d'icônes non marqué changé",
        );
      },
    );
    assert.equal(signals.count(), 1);
  } finally {
    signals.stop();
  }

  // 4. Fichier sans marqueur (copie de l'utilisateur) : jamais touché.
  writeFileSync(ENTRY, FOREIGN);
  await withApp(
    binary,
    { name: "menu-entry-4", env: { ...env, APPIMAGE: MOVED } },
    async (driver) => {
      await openGeneral(driver);
      await delay(3000);
      assert.equal(readFileSync(ENTRY, "utf8"), FOREIGN);
      await clickSwitch(driver);
      await poll(
        () => switchState(driver),
        (state) => state === "false",
        { what: "interrupteur coupé" },
      );
      assert.equal(readFileSync(ENTRY, "utf8"), FOREIGN);
    },
  );
  rmSync(ENTRY);

  // 5. Hors AppImage : ni ligne, ni fichier.
  await withApp(binary, { name: "menu-entry-5", env }, async (driver) => {
    await openGeneral(driver);
    await driver.waitFor("[data-testid=settings] [role=radiogroup]");
    assert.deepEqual(await driver.invoke("menu_entry_status"), {
      available: false,
      enabled: false,
    });
    await delay(1000);
    assert.equal(await switchState(driver), null);
    await driver.invoke("set_menu_entry", { enabled: true });
    assert.ok(!existsSync(ENTRY), "entrée écrite hors AppImage");
  });
  console.log("menu-entry : OK");
} finally {
  rmSync(WORK, { recursive: true, force: true });
  await fake.stop();
}
