// Dictée et intégration au bureau (étape 17), dans la session virtuelle avec
// un faux bureau (e2e/lib/desktop-services.py : notifications, portail
// GlobalShortcuts, zone de notification) :
// - zone de notification : icône inscrite, infobulle, pastille pendant
//   l'écoute, menu selon la phase (Stop d'un menu périmé sans effet), clic
//   gauche qui montre ou masque la fenêtre (montre une fenêtre réduite),
//   « Dictation settings… » (évènement `navigate`), zone qui disparaît
//   (fenêtre masquée remontrée) puis revient, « Quit ChatonLM » ;
// - raccourci : application enregistrée auprès du portail, touche proposée,
//   statut lié, bascule (répétitions et relâchement ignorés), push-to-talk
//   (appuis brefs : appui et relâchement dans l'ordre), touche changée, boîte
//   de réglage, refus puis nouvel essai (page de réglage du bureau ouverte),
//   liaison en attente abandonnée quand la dictée est coupée ;
// - dictée de bout en bout (micro simulé par un WAV, Whisper tiny, nettoyage
//   par le faux modèle `chatonlm-polish`) : presse-papiers, notifications,
//   silence (« No speech detected ») ; copie automatique coupée par
//   l'interrupteur de la page : presse-papiers inchangé, « Transcription
//   ready » avec l'action « Copy » qui copie, bouton « Copy » du dernier
//   résultat, « Nothing to copy » sans dictée ;
// - fermeture qui masque (notification la première fois seulement), sans
//   zone de notification fermeture qui quitte ;
// - `--dictate` : premier lancement fenêtre masquée et dictée commencée,
//   relance qui l'arrête sans montrer la fenêtre, relance sans argument qui
//   la montre, relance arrivée pendant le setup (bascule faite une fois
//   l'app prête) ;
// - taille de la fenêtre gardée par « Quit ChatonLM » et par Ctrl+Q.
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import {
  mkdirSync,
  openSync,
  rmSync,
  symlinkSync,
  writeFileSync,
} from "node:fs";
import { ROOT, freePort, withApp } from "./lib/app.mjs";
import { holdClipboard, readClipboard } from "./lib/clipboard.mjs";
import { poll, startDesktopServices } from "./lib/desktop.mjs";
import { startFake } from "./lib/fake.mjs";
import { forWindows, runKWinScript } from "./lib/kwin.mjs";

const binary = process.argv[2];
const IDENTIFIER = "com.chatonlm.chatonlm";
const DATA = process.env.CHATONLM_DATA_DIR;
const JFK = `${ROOT}.tools/speech-samples/jfk.wav`;
const TINY = `${ROOT}.tools/speech-models/whisper/ggml-tiny.bin`;
const SILENCE = `${ROOT}.tools/tmp/silence-${process.pid}.wav`;
const HINT = "ChatonLM is still running";
const delay = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

assert.ok(
  DATA,
  "CHATONLM_DATA_DIR absent : lancer dans scripts/e2e-session.sh",
);

/** WAV PCM 16 bits mono 16 kHz de `seconds` secondes de silence. */
function silenceWav(seconds) {
  const samples = Math.round(16000 * seconds);
  const buffer = Buffer.alloc(44 + samples * 2);
  buffer.write("RIFF", 0);
  buffer.writeUInt32LE(36 + samples * 2, 4);
  buffer.write("WAVEfmt ", 8);
  buffer.writeUInt32LE(16, 16);
  buffer.writeUInt16LE(1, 20);
  buffer.writeUInt16LE(1, 22);
  buffer.writeUInt32LE(16000, 24);
  buffer.writeUInt32LE(32000, 28);
  buffer.writeUInt16LE(2, 32);
  buffer.writeUInt16LE(16, 34);
  buffer.write("data", 36);
  buffer.writeUInt32LE(samples * 2, 40);
  return buffer;
}

const status = (driver) => driver.invoke("dictation_status");
/**
 * Erreur renvoyée par une commande (null si elle réussit) : `driver.invoke`
 * prendrait la réponse `{ error }` pour une erreur du serveur WebDriver.
 */
const failureOf = (driver, command, args = {}) =>
  driver
    .executeAsync(
      `const done = arguments[arguments.length - 1];
       window.__TAURI_INTERNALS__.invoke(arguments[0], arguments[1])
         .then(() => done(null), (failure) => done({ failure }));`,
      [command, args],
    )
    .then((result) => result?.failure ?? null);
const textOf = (driver, css) =>
  driver.execute(
    "return document.querySelector(arguments[0])?.textContent.trim() ?? null",
    [css],
  );
const inner = (driver) =>
  driver.execute("return [window.innerWidth, window.innerHeight]");
const near = (a, b) => Math.abs(a - b) <= 2;
const waitStatus = (driver, predicate, what, timeoutMs = 15000) =>
  poll(() => status(driver), predicate, { what, timeoutMs });
const visible = (driver) =>
  driver.invoke("plugin:window|is_visible", { label: "main" });
const waitVisible = (driver, expected) =>
  poll(
    () => visible(driver),
    (value) => value === expected,
    {
      what: `fenêtre ${expected ? "visible" : "masquée"}`,
    },
  );
const closeWindow = () =>
  runKWinScript(forWindows("ChatonLM", "w.closeWindow();"));
const entry = (menu, label) => {
  const found = menu.find((item) => item.label === label);
  assert.ok(found, `entrée absente : ${label} dans ${JSON.stringify(menu)}`);
  return found;
};
const labels = (menu) =>
  menu.map((item) => (item.type === "separator" ? "-" : item.label));

/** Dictée finie après `previous` (horodatage du résultat précédent). */
const finished = (driver, previous = 0) =>
  waitStatus(
    driver,
    (s) => s.phase === "idle" && s.lastResult && s.lastResult.at > previous,
    "fin de la dictée",
    90000,
  );

/** Seconde instance (rappel de l'instance unique) ; code de sortie. */
async function runInstance(args, name) {
  const log = openSync(`${ROOT}.tools/logs/e2e-${name}.log`, "w");
  const child = spawn(binary, args, {
    env: { ...process.env, TAURI_WEBDRIVER_PORT: String(await freePort()) },
    stdio: ["ignore", log, log],
  });
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => {
      child.kill("SIGKILL");
      reject(new Error(`seconde instance ${args} pas arrêtée`));
    }, 10000);
    child.once("exit", (code) => {
      clearTimeout(timer);
      resolve(code);
    });
  });
}

/** Évènements `navigate` reçus par la page, notés dans window.__navigations. */
const listenNavigate = (driver) =>
  driver.executeAsync(`const done = arguments[arguments.length - 1];
    window.__navigations = [];
    const internals = window.__TAURI_INTERNALS__;
    internals.invoke("plugin:event|listen", {
      event: "navigate",
      target: { kind: "Any" },
      handler: internals.transformCallback((event) => window.__navigations.push(event.payload)),
    }).then(() => done(true), (error) => done(String(error)));`);

// Modèle Whisper tiny « installé » (lien vers la copie de l'essai, 77 Mo).
mkdirSync(`${DATA}/speech-models/whisper-tiny`, { recursive: true });
symlinkSync(TINY, `${DATA}/speech-models/whisper-tiny/ggml-tiny.bin`);
mkdirSync(`${ROOT}.tools/tmp`, { recursive: true });
writeFileSync(SILENCE, silenceWav(1.5));

const fake = await startFake("ollama");
const env = { OLLAMA_HOST: fake.url, CHATONLM_E2E_MICROPHONE: JFK };

/** Taille de la fenêtre au Quit du lancement 1, à retrouver ensuite. */
let quitSize;
/** Taille au Ctrl+Q (quit_app) du lancement 2. */
let commandSize;

try {
  // 1. Zone de notification, raccourci, dictée, fermeture qui masque, Quit.
  const desk = await startDesktopServices({
    notifications: true,
    watcher: true,
    portal: {
      version: 2,
      trigger: "Ctrl+Alt+D",
      configureTrigger: "Ctrl+Alt+F",
    },
  });
  try {
    await withApp(
      binary,
      { name: "dictation-desktop", env, quit: "command" },
      async (driver, app) => {
        await driver.waitFor("[data-testid=new-chat]");
        await desk.waitFor((e) => e.event === "item-registered");
        let s = await waitStatus(driver, (x) => x.trayAvailable, "zone");
        assert.deepEqual(s.shortcut, { state: "disabled" });

        let props = await desk.command("item-props");
        assert.equal(props.title, "ChatonLM");
        assert.deepEqual(props.tooltip, {
          title: "ChatonLM",
          description: "Ready",
        });
        assert.deepEqual(
          props.icons.map((icon) => [icon.width, icon.height]),
          [
            [32, 32],
            [64, 64],
          ],
        );
        const idle = props.icons.map((icon) => icon.sha1);
        let menu = await desk.command("menu");
        assert.deepEqual(labels(menu), [
          "Show ChatonLM",
          "-",
          "Start dictation",
          "Stop dictation",
          "Cancel dictation",
          "Dictation settings…",
          "-",
          "Quit ChatonLM",
        ]);
        // Dictée coupée : rien à commencer, rien de lié.
        assert.equal(entry(menu, "Start dictation").enabled, false);
        assert.equal(entry(menu, "Stop dictation").visible, false);
        assert.equal(entry(menu, "Cancel dictation").visible, false);
        assert.ok(!desk.events.some((e) => e.event === "bind"));

        // Dictée activée avec un modèle installé : raccourci lié.
        await driver.invoke("update_dictation_settings", {
          patch: {
            enabled: true,
            speechModel: "whisper-tiny",
            language: "en",
            polishModel: "chatonlm-polish",
          },
        });
        const bind = await desk.waitFor((e) => e.event === "bind");
        assert.deepEqual(bind.shortcuts, [
          [
            "dictation",
            {
              description: "Start or stop dictation",
              preferred_trigger: "CTRL+ALT+D",
            },
          ],
        ]);
        // Application enregistrée auprès du portail, avant toute session,
        // par la connexion qui ouvre la session (GTK enregistre aussi
        // l'identifiant par la sienne depuis enableGTKAppId, étape 18a).
        const session = desk.events.findIndex(
          (e) => e.event === "create-session",
        );
        const registered = desk.events.findIndex(
          (e) =>
            e.event === "register-app" &&
            e.sender === desk.events[session].sender,
        );
        assert.ok(registered >= 0, "register-app absent");
        assert.equal(desk.events[registered].appId, IDENTIFIER);
        assert.ok(registered < session);
        s = await waitStatus(
          driver,
          (x) => x.shortcut.state === "bound",
          "raccourci lié",
        );
        assert.deepEqual(s.shortcut, { state: "bound", trigger: "Ctrl+Alt+D" });
        assert.equal(s.canConfigureShortcut, true);
        props = await poll(
          () => desk.command("item-props"),
          (p) => p.tooltip.description !== "Ready",
          { what: "infobulle avec la touche" },
        );
        assert.equal(props.tooltip.description, "Ready — Ctrl+Alt+D");
        menu = await desk.command("menu");
        assert.equal(entry(menu, "Start dictation").enabled, true);

        // Menu : Start dictation, puis Cancel dictation (rien de copié).
        await desk.command("menu-click", { label: "Start dictation" });
        await waitStatus(driver, (x) => x.phase === "recording", "écoute");
        props = await poll(
          () => desk.command("item-props"),
          (p) => p.tooltip.description.startsWith("Listening"),
          { what: "infobulle d'écoute" },
        );
        assert.equal(props.tooltip.description, "Listening… — Ctrl+Alt+D");
        // Pastille rouge en bas à droite (32 et 64 px).
        assert.deepEqual(props.icons[0].badge, [255, 0xef, 0x44, 0x44]);
        assert.deepEqual(props.icons[1].badge, [255, 0xef, 0x44, 0x44]);
        menu = await desk.command("menu");
        assert.equal(entry(menu, "Stop dictation").visible, true);
        assert.equal(entry(menu, "Start dictation").visible, false);
        assert.equal(entry(menu, "Cancel dictation").visible, true);
        const staleStop = entry(menu, "Stop dictation").id;
        await desk.command("menu-click", { label: "Cancel dictation" });
        s = await waitStatus(driver, (x) => x.phase === "idle", "abandon");
        assert.equal(s.lastResult, null);
        props = await poll(
          () => desk.command("item-props"),
          (p) => p.tooltip.description.startsWith("Ready"),
          { what: "icône au repos" },
        );
        assert.deepEqual(
          props.icons.map((icon) => icon.sha1),
          idle,
        );
        // Menu resté ouvert : son « Stop dictation » ne relance rien.
        await desk.command("menu-event", { id: staleStop });
        await delay(500);
        assert.equal((await status(driver)).phase, "idle");

        // Raccourci en bascule : répétition (touche maintenue) et
        // relâchement ignorés, second appui qui arrête.
        await desk.command("activated", { id: "dictation" });
        await waitStatus(driver, (x) => x.phase === "recording", "écoute");
        await desk.command("activated", { id: "dictation" });
        await desk.command("deactivated", { id: "dictation" });
        await delay(400);
        assert.equal((await status(driver)).phase, "recording");
        await desk.command("activated", { id: "dictation" });
        await desk.command("deactivated", { id: "dictation" });
        s = await finished(driver);
        assert.equal(s.lastResult.error, null);
        assert.equal(s.lastResult.polished, true);
        assert.match(s.lastResult.text, /^Polished: .*country/is);
        assert.equal((await readClipboard()).text, s.lastResult.text);
        const copied = await desk.waitFor(
          (e) => e.event === "notify" && e.summary === "Copied to clipboard",
        );
        assert.match(copied.body, /country/);
        assert.equal(copied.hints["desktop-entry"], IDENTIFIER);
        // Déjà copié : pas d'action « Copy ».
        assert.deepEqual(copied.actions, []);
        await desk.waitFor(
          (e) =>
            e.event === "notify" &&
            e.summary === "Listening…" &&
            e.body === "Press Ctrl+Alt+D again to stop",
        );

        // Push-to-talk : appui → écoute, relâchement → transcription.
        await driver.invoke("update_dictation_settings", {
          patch: { mode: "hold" },
        });
        const first = s.lastResult.at;
        await desk.command("activated", { id: "dictation" });
        await waitStatus(driver, (x) => x.phase === "recording", "écoute");
        await desk.waitFor(
          (e) =>
            e.event === "notify" &&
            e.summary === "Listening…" &&
            e.body === "Release Ctrl+Alt+D to stop",
        );
        await desk.command("deactivated", { id: "dictation" });
        s = await finished(driver, first);
        assert.match(s.lastResult.text, /^Polished: /);
        // Touche à peine effleurée : appui et relâchement arrivent ensemble,
        // traités dans l'ordre (sinon l'écoute ne s'arrêterait pas).
        for (let tap = 0; tap < 8; tap++) {
          await desk.command("tap", { id: "dictation" });
          await waitStatus(
            driver,
            (x) => x.phase === "idle",
            `écoute arrêtée après l'appui bref ${tap + 1}`,
            3000,
          );
          await delay(150);
          assert.equal(
            (await status(driver)).phase,
            "idle",
            `appui ${tap + 1}`,
          );
        }
        await driver.invoke("update_dictation_settings", {
          patch: { mode: "toggle" },
        });

        // Copie automatique coupée par l'interrupteur de la page : le
        // presse-papiers (tenu par une autre app) ne change pas ; la
        // notification « Transcription ready » propose « Copy ».
        await desk.command("menu-click", { label: "Dictation settings…" });
        const autoCopy =
          '[data-testid=dictation] [role=switch][aria-label="Copy to clipboard automatically"]';
        await driver.click(await driver.waitFor(autoCopy));
        await poll(
          () => driver.invoke("get_dictation_settings"),
          (settings) => settings.autoCopy === false,
          { what: "copie automatique coupée" },
        );
        const other = "Held by another app";
        let held = await holdClipboard({ text: other });
        try {
          const before = s.lastResult.at;
          await desk.command("activated", { id: "dictation" });
          await waitStatus(driver, (x) => x.phase === "recording", "écoute");
          await desk.command("deactivated", { id: "dictation" });
          await desk.command("activated", { id: "dictation" });
          await desk.command("deactivated", { id: "dictation" });
          s = await finished(driver, before);
          assert.equal(s.lastResult.copied, false);
          assert.equal(s.lastResult.error, null);
          assert.match(s.lastResult.text, /^Polished: .*country/is);
          assert.equal((await readClipboard()).text, other);
          const ready = await desk.waitFor(
            (e) => e.event === "notify" && e.summary === "Transcription ready",
          );
          assert.deepEqual(ready.actions, [["copy", "Copy"]]);
          assert.equal(ready.hints.transient, false);
          assert.match(ready.body, /country/);
          // Clic sur « Copy » dans la notification.
          await desk.command("notification-action", {
            id: ready.id,
            key: "copy",
          });
          s = await waitStatus(
            driver,
            (x) => x.lastResult?.copied === true,
            "copie depuis la notification",
          );
          assert.equal((await readClipboard()).text, s.lastResult.text);
          assert.ok(held.requested.includes("cleared"), held.requested);
        } finally {
          await held.release();
        }
        // Presse-papiers changé depuis : « Copy » du dernier résultat.
        held = await holdClipboard({ text: other });
        try {
          const copyLast = "[data-testid=last-dictation] button";
          assert.equal(await textOf(driver, copyLast), "Copy");
          await driver.click(await driver.find(copyLast));
          await poll(
            () => textOf(driver, copyLast),
            (text) => text === "Copied",
            { what: "bouton Copied" },
          );
          assert.equal((await readClipboard()).text, s.lastResult.text);
        } finally {
          await held.release();
        }
        await driver.click(await driver.find(autoCopy));
        await poll(
          () => driver.invoke("get_dictation_settings"),
          (settings) => settings.autoCopy === true,
          { what: "copie automatique rétablie" },
        );

        // Touche changée par le bureau, puis par sa boîte de réglage.
        await desk.command("shortcuts-changed", { trigger: "Meta+D" });
        await waitStatus(
          driver,
          (x) => x.shortcut.trigger === "Meta+D",
          "touche changée",
        );
        await driver.invoke("dictation_configure_shortcut");
        await desk.waitFor((e) => e.event === "configure");
        await waitStatus(
          driver,
          (x) => x.shortcut.trigger === "Ctrl+Alt+F",
          "touche réglée",
        );

        // Fermeture : la fenêtre n'est que masquée, notification une fois.
        assert.equal(await listenNavigate(driver), true);
        await closeWindow();
        await waitVisible(driver, false);
        const hint = await desk.waitFor(
          (e) => e.event === "notify" && e.summary === HINT,
        );
        assert.match(hint.body, /system tray so your shortcuts stay available/);
        assert.equal(hint.hints["desktop-entry"], IDENTIFIER);
        // Clic gauche : montrée ; de nouveau : masquée ; menu : montrée.
        await desk.command("item-activate");
        await waitVisible(driver, true);
        await desk.command("item-activate");
        await waitVisible(driver, false);
        await desk.command("menu-click", { label: "Show ChatonLM" });
        await waitVisible(driver, true);
        // Réduite (sous Wayland, toujours « visible » et jamais signalée
        // réduite) : le clic la montre au lieu de la masquer.
        // Script relancé tant que la fenêtre garde le focus : juste remontrée,
        // elle peut manquer encore à KWin (script sans effet, suite chargée).
        let minimizeTries = 0;
        await poll(
          async () => {
            if (minimizeTries++ % 20 === 0) {
              await runKWinScript(
                forWindows("ChatonLM", "w.minimized = true;"),
              );
            }
            return driver.invoke("plugin:window|is_focused", { label: "main" });
          },
          (focused) => focused === false,
          { what: "fenêtre réduite sans focus", timeoutMs: 20000 },
        );
        await desk.command("item-activate");
        await poll(
          () => driver.invoke("plugin:window|is_focused", { label: "main" }),
          (focused) => focused === true,
          { what: "fenêtre réduite remontrée" },
        );
        assert.equal(await visible(driver), true);
        await closeWindow();
        await waitVisible(driver, false);
        await delay(500);
        assert.equal(
          desk.notifications().filter((n) => n.summary === HINT).length,
          1,
          "notification de fermeture répétée",
        );
        // Réglages de la dictée : fenêtre montrée, page avertie.
        await desk.command("menu-click", { label: "Dictation settings…" });
        await waitVisible(driver, true);
        await poll(
          () => driver.execute("return window.__navigations"),
          (list) => list.includes("/settings?section=dictation"),
          { what: "évènement navigate" },
        );

        // Zone de notification qui disparaît pendant que la fenêtre est
        // masquée : la fenêtre revient (sinon introuvable), puis la zone.
        await closeWindow();
        await waitVisible(driver, false);
        await desk.command("watcher-off");
        await waitStatus(driver, (x) => !x.trayAvailable, "zone disparue");
        await waitVisible(driver, true);
        await desk.command("watcher-on");
        await poll(
          () => desk.events.filter((e) => e.event === "item-registered"),
          (list) => list.length === 2,
          { what: "icône réinscrite" },
        );
        await waitStatus(driver, (x) => x.trayAvailable, "zone revenue");

        // Quit ChatonLM : la taille de la fenêtre est gardée, l'app se termine
        // normalement.
        await runKWinScript(
          forWindows(
            "ChatonLM",
            "w.frameGeometry = { x: 30, y: 20, width: 1010, height: 730 };",
          ),
        );
        quitSize = await inner(driver);
        await desk.command("menu-click", { label: "Quit ChatonLM" });
        assert.equal(await app.exited, 0);
      },
    );
  } finally {
    await desk.stop();
  }

  // 2. `--dictate` : fenêtre masquée, dictée commencée puis arrêtée par une
  // relance ; liaison du raccourci en attente (boîte du bureau ouverte)
  // abandonnée quand la dictée est coupée.
  const dictate = await startDesktopServices({
    notifications: true,
    watcher: true,
    portal: { bind: "hang" },
  });
  try {
    await withApp(
      binary,
      { name: "dictation-dictate", env, args: ["--dictate"], quit: "command" },
      async (driver) => {
        await driver.waitFor("[data-testid=new-chat]");
        const saved = await driver.invoke("get_settings");
        assert.ok(near(saved.windowWidth, quitSize[0]), JSON.stringify(saved));
        assert.ok(near(saved.windowHeight, quitSize[1]), JSON.stringify(saved));
        await waitStatus(driver, (x) => x.phase === "recording", "écoute");
        assert.equal(await visible(driver), false);
        assert.equal(
          (await status(driver)).shortcut.state,
          "binding",
          "liaison en attente",
        );
        assert.equal(await runInstance(["--dictate"], "dictation-second"), 0);
        const s = await finished(driver);
        assert.match(s.lastResult.text, /^Polished: .*country/is);
        assert.equal((await readClipboard()).text, s.lastResult.text);
        assert.equal(await visible(driver), false);
        // Relance sans argument : la fenêtre se montre.
        assert.equal(await runInstance([], "dictation-third"), 0);
        await waitVisible(driver, true);

        await driver.invoke("update_dictation_settings", {
          patch: { enabled: false },
        });
        await dictate.waitFor((e) => e.event === "close-session");
        await waitStatus(
          driver,
          (x) => x.shortcut.state === "disabled",
          "raccourci délié",
        );
        // Ctrl+Q (quit_app) garde aussi la taille.
        await runKWinScript(
          forWindows(
            "ChatonLM",
            "w.frameGeometry = { x: 50, y: 40, width: 1080, height: 700 };",
          ),
        );
        commandSize = await inner(driver);
      },
    );
  } finally {
    await dictate.stop();
  }

  // 3. Sans zone de notification : refus du raccourci et nouvel essai (page
  // de réglage du bureau), silence, fermeture qui quitte.
  const plain = await startDesktopServices({
    notifications: true,
    watcher: false,
    portal: { bind: "cancel" },
  });
  try {
    await withApp(
      binary,
      {
        name: "dictation-silence",
        env: { ...env, CHATONLM_E2E_MICROPHONE: SILENCE },
      },
      async (driver, app) => {
        await driver.waitFor("[data-testid=new-chat]");
        // Aucune dictée encore : rien à copier.
        assert.deepEqual(await failureOf(driver, "dictation_copy_last"), {
          kind: "invalid",
          message: "Nothing to copy",
        });
        const saved = await driver.invoke("get_settings");
        assert.ok(
          near(saved.windowWidth, commandSize[0]),
          JSON.stringify(saved),
        );
        assert.ok(
          near(saved.windowHeight, commandSize[1]),
          JSON.stringify(saved),
        );
        await driver.invoke("update_dictation_settings", {
          patch: { enabled: true },
        });
        let s = await waitStatus(
          driver,
          (x) => x.shortcut.state === "unavailable",
          "refus",
        );
        assert.deepEqual(s.shortcut, {
          state: "unavailable",
          problem: "refused",
          reason: "The shortcut was not set",
        });
        assert.equal(s.trayAvailable, false);
        // Nouvel essai : KDE lie aussitôt, sans touche ; la page de réglage
        // du bureau s'ouvre pour en choisir une.
        await driver.invoke("dictation_bind_shortcut");
        s = await waitStatus(
          driver,
          (x) => x.shortcut.state === "bound",
          "liaison sans touche",
        );
        assert.deepEqual(s.shortcut, { state: "bound", trigger: "" });
        assert.equal(
          plain.events.filter((e) => e.event === "bind").length,
          2,
          "nouvel essai de liaison",
        );
        await plain.waitFor((e) => e.event === "configure");

        await driver.invoke("dictation_toggle");
        await waitStatus(driver, (x) => x.phase === "recording", "écoute");
        await driver.invoke("dictation_toggle");
        s = await finished(driver);
        assert.equal(s.lastResult.error, "No speech detected");
        assert.equal(s.lastResult.text, "");
        await plain.waitFor(
          (e) =>
            e.event === "notify" &&
            e.summary === "Dictation failed" &&
            e.body.includes("No speech detected"),
        );

        // Fermer la fenêtre quitte l'app, sans notification.
        await closeWindow();
        assert.equal(await app.exited, 0);
        assert.ok(!plain.notifications().some((n) => n.summary === HINT));
      },
    );
  } finally {
    await plain.stop();
  }

  // 4. Relance `--dictate` arrivée pendant le setup (base pas encore
  // ouverte) : la bascule attend que l'app soit prête. Depuis
  // enableGTKAppId (étape 18a), la relance attend d'abord que la boucle de
  // la première instance tourne (GApplication de la relance : appel
  // DescribeAll synchrone à l'instance principale), donc la fin du setup ;
  // elle n'est pas perdue. Avec CHATONLM_NO_GLOBAL_SHORTCUTS=1 (recette avant
  // l'empaquetage), ChatonLM ne contacte jamais le portail des raccourcis :
  // statut « portail absent », commande à lier (GTK y enregistre seulement
  // l'identifiant de l'app).
  const early = await startDesktopServices({
    notifications: true,
    watcher: false,
    portal: { bind: "ok" },
  });
  try {
    const started = Date.now();
    // Relance lancée pendant le setup ralenti de la première instance (nom
    // D-Bus de l'instance unique déjà pris, base pas encore gérée).
    const second = delay(2000).then(async () => ({
      code: await runInstance(["--dictate"], "dictation-early-second"),
      at: Date.now() - started,
    }));
    await withApp(
      binary,
      {
        name: "dictation-early",
        env: {
          ...env,
          CHATONLM_E2E_SETUP_DELAY_MS: "6000",
          CHATONLM_NO_GLOBAL_SHORTCUTS: "1",
        },
      },
      async (driver) => {
        await driver.waitFor("[data-testid=new-chat]", { timeoutMs: 20000 });
        const secondExit = await second;
        assert.equal(secondExit.code, 0);
        assert.ok(
          secondExit.at < 6000 + 4000,
          `relance finie longtemps après le setup : ${secondExit.at} ms`,
        );
        const s = await waitStatus(
          driver,
          (x) => x.phase !== "idle" || x.lastResult,
          "bascule différée",
        );
        assert.equal(s.lastResult, null, JSON.stringify(s.lastResult));
        assert.equal(s.phase, "recording");
        await driver.invoke("dictation_cancel");
        const off = await waitStatus(
          driver,
          (x) => x.shortcut.state !== "binding",
          "portail coupé",
        );
        assert.deepEqual(off.shortcut, {
          state: "unavailable",
          problem: "noPortal",
          reason: "Global shortcuts are not available on this desktop",
        });
        const portalCalls = early.events.filter((e) =>
          ["create-session", "bind"].includes(e.event),
        );
        assert.deepEqual(
          portalCalls,
          [],
          "portail contacté malgré la variable",
        );
      },
    );
  } finally {
    await early.stop();
  }
} finally {
  await fake.stop();
  rmSync(SILENCE, { force: true });
}
