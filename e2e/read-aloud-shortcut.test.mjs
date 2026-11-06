// Raccourci global « Read selected text aloud » (propre à ChatonLM), dans la
// session virtuelle avec un faux bureau (e2e/lib/desktop-services.py :
// notifications, portail GlobalShortcuts), une sélection primaire tenue par
// une autre application (e2e/lib/clipboard-writer.py, `holdSelection`), le
// vrai moteur de lecture (voix anglaise de .tools/voice-models liée dans le
// dossier des données) et la sortie audio factice de `withApp` :
// 1. coupé par défaut (rien de lié, commande de repli « --read-aloud ») ;
//    activé : liaison de « readAloud » seul, touche proposée CTRL+ALT+R et
//    description de la boîte du bureau ; appui : la sélection est lue
//    (source « selection » de l'état, synthèse réelle), sans notification ;
//    nouvel appui : la lecture s'arrête ; sans sélection : « No text
//    selected », rien n'est lu ; une lecture d'une autre source n'est pas
//    arrêtée par le raccourci, elle est remplacée ;
// 2. `--read-aloud` : premier lancement fenêtre masquée et sélection lue une
//    fois l'app prête ; relance avec `--read-aloud` : la lecture s'arrête,
//    fenêtre toujours masquée ; voix absente : « The English voice is not
//    downloaded », rien n'est lu.
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import {
  existsSync,
  mkdirSync,
  openSync,
  readdirSync,
  rmSync,
  symlinkSync,
} from "node:fs";
import { ROOT, freePort, withApp } from "./lib/app.mjs";
import { holdClipboard, holdSelection } from "./lib/clipboard.mjs";
import { poll, startDesktopServices } from "./lib/desktop.mjs";
import { startFake } from "./lib/fake.mjs";

const binary = process.argv[2];
const DATA = process.env.CHATONLM_DATA_DIR;
const VOICES = `${ROOT}.tools/voice-models`;
const ENGLISH = "pocket-tts-en";
const delay = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

assert.ok(
  DATA,
  "CHATONLM_DATA_DIR absent : lancer dans scripts/e2e-session.sh",
);
if (!existsSync(`${VOICES}/${ENGLISH}`)) {
  throw new Error(
    `Voix absente : ${VOICES}/${ENGLISH} (scripts/tts/README.md, assemble.py)`,
  );
}

/** Lie la voix `id` de .tools/voice-models dans le dossier des données. */
function linkVoice(id) {
  const dir = `${DATA}/voice-models/${id}`;
  mkdirSync(dir, { recursive: true });
  for (const file of readdirSync(`${VOICES}/${id}`)) {
    symlinkSync(`${VOICES}/${id}/${file}`, `${dir}/${file}`);
  }
}

/** Assez long pour que la lecture dure pendant les vérifications. */
const TEXT =
  "The meeting moves to Friday morning. Please bring the quarterly report, " +
  "the budget for next year, and your questions about the new office. " +
  "We will start at nine and finish before lunch.";
const OTHER = "Held by another app";

const shortcuts = (driver) => driver.invoke("shortcuts_status");
const readState = (driver) => driver.invoke("read_aloud_state");
const settings = (driver, patch) => driver.invoke("update_settings", { patch });
const bound = (trigger) => ({ state: "bound", trigger });
const DISABLED = { state: "disabled" };
const binds = (desk) => desk.events.filter((e) => e.event === "bind");
const visible = (driver) =>
  driver.invoke("plugin:window|is_visible", { label: "main" });

/** Attend la lecture de la sélection jusqu'au son (synthèse réelle). */
const waitReadingSelection = (driver, what) =>
  poll(
    () => readState(driver),
    (state) => state.source?.kind === "selection" && state.phase === "playing",
    { what, timeoutMs: 30000 },
  );
const waitIdle = (driver, what) =>
  poll(
    () => readState(driver),
    (state) => state.phase === "idle",
    { what },
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

const fake = await startFake("ollama");
const env = { OLLAMA_HOST: fake.url };
linkVoice(ENGLISH);

try {
  // 1. Raccourci lié par le portail, appuis.
  const desk = await startDesktopServices({
    notifications: true,
    watcher: true,
    portal: { version: 2, triggers: { readAloud: "Ctrl+Alt+R" } },
  });
  try {
    await withApp(
      binary,
      { name: "read-aloud-shortcut", env, quit: "command" },
      async (driver) => {
        await driver.waitFor("[data-testid=new-chat]");
        await desk.waitFor((e) => e.event === "item-registered");

        // Coupé par défaut : rien de lié.
        let s = await shortcuts(driver);
        assert.deepEqual(s.readAloud, DISABLED);
        assert.match(s.readAloudCommand, / --read-aloud$/);
        assert.equal(
          (await driver.invoke("get_settings")).readAloudShortcut,
          false,
        );
        assert.deepEqual(binds(desk), []);

        // Activé : « readAloud » seul, touche proposée, description.
        await settings(driver, { readAloudShortcut: true });
        const bind = await desk.waitFor((e) => e.event === "bind");
        assert.deepEqual(bind.shortcuts, [
          [
            "readAloud",
            {
              description: "Read selected text aloud",
              preferred_trigger: "CTRL+ALT+R",
            },
          ],
        ]);
        s = await poll(
          () => shortcuts(driver),
          (x) => x.readAloud.state === "bound",
          { what: "lecture liée" },
        );
        assert.deepEqual(s.readAloud, bound("Ctrl+Alt+R"));
        assert.deepEqual(s.proofread, DISABLED);
        assert.deepEqual(s.dictation, DISABLED);

        // Appui : la sélection est lue ; nouvel appui : arrêt.
        const notified = desk.notifications().length;
        let held = await holdSelection(TEXT);
        try {
          await desk.command("tap", { id: "readAloud" });
          await waitReadingSelection(driver, "sélection lue");
          assert.ok(
            held.requested.some((target) => /text|STRING/.test(target)),
            JSON.stringify(held.requested),
          );
          await desk.command("tap", { id: "readAloud" });
          await waitIdle(driver, "lecture arrêtée par un nouvel appui");
          // Pas relancée par le second appui.
          await delay(1500);
          assert.equal((await readState(driver)).phase, "idle");
        } finally {
          await held.release();
        }
        // Tout s'est bien passé : aucune notification.
        assert.deepEqual(desk.notifications().slice(notified), []);

        // Sans sélection : notifiée, rien n'est lu.
        held = await holdClipboard({ text: OTHER });
        try {
          await desk.command("tap", { id: "readAloud" });
          const none = await desk.waitFor(
            (e) => e.event === "notify" && e.summary === "No text selected",
            { timeoutMs: 15000 },
          );
          assert.equal(
            none.body,
            "Select some text in any app, then use the shortcut again.",
          );
          assert.equal((await readState(driver)).phase, "idle");
        } finally {
          await held.release();
        }
      },
    );
  } finally {
    await desk.stop();
  }

  // 2. `--read-aloud` : premier lancement masqué, relance qui arrête ;
  // voix absente.
  const hidden = await startDesktopServices({
    notifications: true,
    watcher: true,
    portal: { bind: "ok" },
  });
  const held = await holdSelection(TEXT);
  try {
    await withApp(
      binary,
      {
        name: "read-aloud-cli",
        env: { ...env, CHATONLM_NO_GLOBAL_SHORTCUTS: "1" },
        args: ["--read-aloud"],
        quit: "command",
      },
      async (driver) => {
        await driver.waitFor("[data-testid=new-chat]");
        await waitReadingSelection(driver, "sélection lue au lancement");
        assert.equal(await visible(driver), false);

        // Relance avec --read-aloud pendant la lecture : arrêt, fenêtre
        // toujours masquée.
        assert.equal(
          await runInstance(["--read-aloud"], "read-aloud-second"),
          0,
        );
        await waitIdle(driver, "lecture arrêtée par la relance");
        assert.equal(await visible(driver), false);

        // Voix absente : rien n'est lu, la notification le dit.
        rmSync(`${DATA}/voice-models`, { recursive: true, force: true });
        assert.equal(
          await runInstance(["--read-aloud"], "read-aloud-novoice"),
          0,
        );
        await hidden.waitFor(
          (e) =>
            e.event === "notify" &&
            e.summary === "The English voice is not downloaded",
          { timeoutMs: 15000 },
        );
        assert.equal((await readState(driver)).phase, "idle");
      },
    );
  } finally {
    await held.release();
    await hidden.stop();
  }
} finally {
  await fake.stop();
}
