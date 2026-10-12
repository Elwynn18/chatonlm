// Raccourci global « Correct selected text » (propre à ChatonLM), dans la
// session virtuelle avec un faux bureau (e2e/lib/desktop-services.py :
// notifications, portail GlobalShortcuts, zone de notification) et une
// sélection primaire tenue par une autre application
// (e2e/lib/clipboard-writer.py, `holdSelection`) :
// - un seul ensemble lié par session : raccourci coupé par défaut (rien de
//   lié), activé (liaison de « proofread » seul, touche proposée
//   CTRL+ALT+C, description de la boîte du bureau), même réglage enregistré
//   de nouveau (pas de nouvelle liaison), dictée activée (nouvelle session
//   avec les deux raccourcis, l'ancienne fermée), touche changée par le
//   bureau pour un seul raccourci, dictée coupée (session avec la correction
//   seule), tout coupé (session fermée, « shortcuts_bind » refusé) ; statuts
//   par raccourci (`shortcuts_status`, évènement `shortcuts-status`, statut
//   de la page Dictation inchangé) ;
// - appui : sélection primaire lue (l'autre application la donne), corrigée
//   par le faux modèle `chatonlm-proofread`, copiée dans le presse-papiers,
//   notifications « Correcting the selected text… » puis « Corrected text
//   copied » avec l'action « Show » (fenêtre masquée remontrée, évènement
//   `navigate` vers /proofread?selection=1, dernière correction lue par
//   `proofread_last_selection`) ; progression affichée tant que la correction
//   dure (ni passagère, ni expirée) ; texte déjà correct (« No mistakes
//   found », copié quand même) ; second appui pendant une correction qui
//   l'arrête (`chatonlm-proofread-slow` : « Proofreading stopped », rien de
//   copié) ; sans sélection (« No text selected », presse-papiers d'une autre
//   application intact) ; sélection trop longue ; modèle absent du serveur
//   (« Proofreading failed », rien de copié) ; modèle du chat cloud ou
//   distant sans modèle choisi pour la correction (rien d'envoyé) ; aucun
//   modèle (« No model selected ») ; la dictée n'est jamais déclenchée ;
// - `--proofread` : premier lancement fenêtre masquée et sélection corrigée
//   une fois l'app prête, relance qui corrige de nouveau sans montrer la
//   fenêtre, relance sans argument qui la montre ; sans portail
//   (CHATONLM_NO_GLOBAL_SHORTCUTS=1) : statut « portail absent » et commande à
//   lier ;
// - refus de la boîte du bureau : les raccourcis voulus « Unavailable »,
//   nouvel essai par `shortcuts_bind` (liaison sans touche, page de réglage
//   du bureau ouverte) ;
// - dictée liée puis correction activée : pendant la boîte du bureau
//   (restée ouverte) puis après son refus, l'ancienne session reste et
//   Ctrl+Alt+D bascule encore la dictée, une seule fois par appui ; seule la
//   correction « Unavailable » ; correction coupée ensuite : rien à relier.
//
// Dépend du faux serveur de l'agent R1 : `chatonlm-proofread` (« teh » →
// « the », « recieve » → « receive »…) et `chatonlm-proofread-slow`.
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { openSync } from "node:fs";
import { ROOT, freePort, withApp } from "./lib/app.mjs";
import {
  holdClipboard,
  holdSelection,
  readClipboard,
} from "./lib/clipboard.mjs";
import { poll, startDesktopServices } from "./lib/desktop.mjs";
import { startFake } from "./lib/fake.mjs";
import { forWindows, runKWinScript } from "./lib/kwin.mjs";

const binary = process.argv[2];
const DATA = process.env.CHATONLM_DATA_DIR;
const delay = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

assert.ok(
  DATA,
  "CHATONLM_DATA_DIR absent : lancer dans scripts/e2e-session.sh",
);

const MODEL = "chatonlm-proofread";
const SLOW = "chatonlm-proofread-slow";
const CORRECTING = "Correcting the selected text…";
const CORRECTED = "Corrected text copied";
const STOPPED = "Proofreading stopped";
const PROGRESS = `Model: ${MODEL}\nRun the shortcut again to stop`;
const OTHER = "Held by another app";

const shortcuts = (driver) => driver.invoke("shortcuts_status");
const waitShortcuts = (driver, predicate, what, timeoutMs = 15000) =>
  poll(() => shortcuts(driver), predicate, { what, timeoutMs });
const dictationStatus = (driver) => driver.invoke("dictation_status");
const bound = (trigger) => ({ state: "bound", trigger });
const DISABLED = { state: "disabled" };
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
const visible = (driver) =>
  driver.invoke("plugin:window|is_visible", { label: "main" });
const waitVisible = (driver, expected) =>
  poll(
    () => visible(driver),
    (value) => value === expected,
    { what: `fenêtre ${expected ? "visible" : "masquée"}` },
  );
const closeWindow = () =>
  runKWinScript(forWindows("ChatonLM", "w.closeWindow();"));
const settings = (driver, patch) => driver.invoke("update_settings", { patch });

/**
 * Évènements `navigate` et `shortcuts-status` reçus par la page, notés dans
 * window.__events[nom].
 */
const listenEvents = (driver) =>
  driver.executeAsync(`const done = arguments[arguments.length - 1];
    window.__events = { navigate: [], "shortcuts-status": [] };
    const internals = window.__TAURI_INTERNALS__;
    Promise.all(Object.keys(window.__events).map((event) =>
      internals.invoke("plugin:event|listen", {
        event,
        target: { kind: "Any" },
        handler: internals.transformCallback((e) => window.__events[event].push(e.payload)),
      }),
    )).then(() => done(true), (error) => done(String(error)));`);
const events = (driver, name) =>
  driver.execute("return window.__events[arguments[0]]", [name]);

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

/** Évènements « bind » du faux portail. */
const binds = (desk) => desk.events.filter((e) => e.event === "bind");
/** Notifications reçues après la `since`-ième. */
const notifiedSince = (desk, since) => desk.notifications().slice(since);

/**
 * Appui bref sur le raccourci de la correction, puis attente de la
 * notification finale de cette correction (titre `summary`) ; renvoie
 * { progress, final } (progress : null si aucune notification de
 * progression n'est attendue).
 */
async function pressProofread(desk, summary, { progress = true } = {}) {
  const since = desk.notifications().length;
  await desk.command("tap", { id: "proofread" });
  const final = await desk.waitFor(
    (e) =>
      e.event === "notify" &&
      e.summary === summary &&
      desk.notifications().indexOf(e) >= since,
    { timeoutMs: 30000 },
  );
  const seen = notifiedSince(desk, since);
  const started = seen.filter((n) => n.summary === CORRECTING);
  if (!progress) {
    assert.deepEqual(started, [], "progression inattendue");
    return { progress: null, final };
  }
  assert.equal(started.length, 1, JSON.stringify(seen));
  return { progress: started[0], final };
}

const fake = await startFake("ollama");
const env = { OLLAMA_HOST: fake.url };

try {
  // 1. Raccourcis liés, appuis, notifications, « Show ».
  const desk = await startDesktopServices({
    notifications: true,
    watcher: true,
    portal: {
      version: 2,
      triggers: { dictation: "Ctrl+Alt+D", proofread: "Ctrl+Alt+C" },
    },
  });
  try {
    await withApp(
      binary,
      { name: "proofread-shortcut", env, quit: "command" },
      async (driver) => {
        await driver.waitFor("[data-testid=new-chat]");
        await desk.waitFor((e) => e.event === "item-registered");
        assert.equal(await listenEvents(driver), true);

        // Coupé par défaut : rien de lié.
        let s = await shortcuts(driver);
        assert.deepEqual(s.dictation, DISABLED);
        assert.deepEqual(s.proofread, DISABLED);
        assert.match(s.proofreadCommand, / --proofread$/);
        assert.match(s.dictateCommand, / --dictate$/);
        assert.deepEqual(binds(desk), []);

        // Activé : « proofread » seul, touche proposée, description.
        await settings(driver, {
          proofreadShortcut: true,
          proofreadModel: MODEL,
        });
        let bind = await desk.waitFor((e) => e.event === "bind");
        assert.deepEqual(bind.shortcuts, [
          [
            "proofread",
            {
              description: "Correct selected text",
              preferred_trigger: "CTRL+ALT+C",
            },
          ],
        ]);
        s = await waitShortcuts(
          driver,
          (x) => x.proofread.state === "bound",
          "correction liée",
        );
        assert.deepEqual(s.proofread, bound("Ctrl+Alt+C"));
        assert.deepEqual(s.dictation, DISABLED);
        assert.equal(s.canConfigure, true);
        // Page avertie ; statut de la page Dictation inchangé.
        await poll(
          () => events(driver, "shortcuts-status"),
          (list) => list.some((x) => x.proofread.state === "bound"),
          { what: "évènement shortcuts-status" },
        );
        assert.deepEqual((await dictationStatus(driver)).shortcut, DISABLED);
        const firstSession = bind.session;

        // Même réglage enregistré de nouveau : pas de nouvelle liaison.
        await settings(driver, { proofreadShortcut: true });
        await delay(500);
        assert.equal(binds(desk).length, 1);

        // Dictée activée : nouvelle session avec les deux raccourcis.
        await driver.invoke("update_dictation_settings", {
          patch: { enabled: true },
        });
        await poll(
          () => binds(desk),
          (list) => list.length === 2,
          { what: "seconde liaison" },
        );
        bind = binds(desk)[1];
        assert.deepEqual(bind.ids, ["dictation", "proofread"]);
        assert.deepEqual(bind.shortcuts[0], [
          "dictation",
          {
            description: "Start or stop dictation",
            preferred_trigger: "CTRL+ALT+D",
          },
        ]);
        assert.notEqual(bind.session, firstSession);
        await desk.waitFor(
          (e) => e.event === "close-session" && e.session === firstSession,
        );
        s = await waitShortcuts(
          driver,
          (x) => x.dictation.state === "bound" && x.proofread.state === "bound",
          "deux raccourcis liés",
        );
        assert.deepEqual(s.dictation, bound("Ctrl+Alt+D"));
        assert.deepEqual(s.proofread, bound("Ctrl+Alt+C"));
        assert.deepEqual(
          (await dictationStatus(driver)).shortcut,
          bound("Ctrl+Alt+D"),
        );

        // Appui : sélection primaire corrigée, copiée, notifiée.
        let held = await holdSelection("I recieve teh letter.");
        let pressed;
        try {
          pressed = await pressProofread(desk, CORRECTED);
          // La sélection primaire a bien été lue (texte demandé à l'autre
          // application), sans la lui prendre.
          assert.ok(
            held.requested.some((target) => /text|STRING/.test(target)),
            JSON.stringify(held.requested),
          );
          assert.ok(!held.requested.includes("cleared"), held.requested);
        } finally {
          await held.release();
        }
        const { progress, final } = pressed;
        assert.equal(progress.body, PROGRESS);
        // Affichée tant que la correction dure : ni passagère, ni expirée.
        assert.equal(progress.hints.transient, false);
        assert.equal(progress.timeout, 0);
        assert.equal(final.body, "I receive the letter.");
        assert.deepEqual(final.actions, [["show", "Show"]]);
        assert.equal(final.hints.transient, false);
        assert.equal(final.replacesId, progress.id);
        assert.equal(final.hints["desktop-entry"], "com.chatonlm.chatonlm");
        assert.equal((await readClipboard()).text, "I receive the letter.");
        const last = await driver.invoke("proofread_last_selection");
        assert.equal(last.model, MODEL);
        assert.equal(
          last.parts
            .map(
              (part, i) =>
                part.before + (last.results[i]?.text ?? part.text) + part.after,
            )
            .join(""),
          "I receive the letter.",
        );
        // La dictée n'a pas bougé.
        assert.equal((await dictationStatus(driver)).phase, "idle");

        // « Show » : fenêtre masquée remontrée, page amenée sur la revue.
        await closeWindow();
        await waitVisible(driver, false);
        await desk.command("notification-action", {
          id: final.id,
          key: "show",
        });
        await waitVisible(driver, true);
        await poll(
          () => events(driver, "navigate"),
          (list) => list.includes("/proofread?selection=1"),
          { what: "évènement navigate" },
        );
        await poll(
          () => driver.execute("return window.location.pathname"),
          (path) => path === "/proofread",
          { what: "page Proofread" },
        );

        // Texte déjà correct : copié quand même (Ctrl+V ne colle jamais un
        // vieux contenu).
        held = await holdSelection("The letter is here.");
        try {
          const { final: same } = await pressProofread(
            desk,
            "No mistakes found",
          );
          assert.equal(same.body, "The letter is here.");
          assert.deepEqual(same.actions, [["show", "Show"]]);
        } finally {
          await held.release();
        }
        assert.equal((await readClipboard()).text, "The letter is here.");

        // Second appui pendant une correction : elle s'arrête, rien n'est
        // copié (une progression, un arrêt qui la remplace).
        await settings(driver, { proofreadModel: SLOW });
        held = await holdSelection(
          "teh slow text that takes a while to correct, teh end",
        );
        try {
          const since = desk.notifications().length;
          await desk.command("tap", { id: "proofread" });
          const working = await desk.waitFor(
            (e) =>
              e.event === "notify" &&
              e.summary === CORRECTING &&
              desk.notifications().indexOf(e) >= since,
          );
          await desk.command("tap", { id: "proofread" });
          const stopped = await desk.waitFor(
            (e) =>
              e.event === "notify" &&
              e.summary === STOPPED &&
              desk.notifications().indexOf(e) >= since,
            { timeoutMs: 30000 },
          );
          assert.equal(stopped.body, "Nothing was copied.");
          assert.equal(stopped.replacesId, working.id);
          assert.deepEqual(stopped.actions, []);
          await delay(1500);
          const seen = notifiedSince(desk, since).map((n) => n.summary);
          assert.deepEqual(seen, [CORRECTING, STOPPED]);
        } finally {
          await held.release();
        }
        assert.equal((await readClipboard()).text, "The letter is here.");
        await settings(driver, { proofreadModel: MODEL });

        // Sans sélection : rien de copié, le presse-papiers d'une autre
        // application reste intact.
        held = await holdClipboard({ text: OTHER });
        try {
          const { final: none } = await pressProofread(
            desk,
            "No text selected",
            { progress: false },
          );
          assert.equal(
            none.body,
            "Select some text in any app, then use the shortcut again.",
          );
          assert.deepEqual(none.actions, []);
          assert.equal((await readClipboard()).text, OTHER);

          // Sélection trop longue.
          const long = await holdSelection("a".repeat(100_001));
          try {
            const { final: tooLong } = await pressProofread(
              desk,
              "The selected text is too long",
              { progress: false },
            );
            assert.equal(tooLong.body, "Select at most 100,000 characters.");
          } finally {
            await long.release();
          }

          // Modèle absent du serveur : échec, rien de copié.
          await settings(driver, { proofreadModel: "chatonlm-vanished" });
          const missing = await holdSelection("teh text");
          try {
            const { final: failed } = await pressProofread(
              desk,
              "Proofreading failed",
            );
            assert.match(failed.body, /chatonlm-vanished/);
            assert.deepEqual(failed.actions, []);
          } finally {
            await missing.release();
          }
          assert.equal((await readClipboard()).text, OTHER);

          // Modèle repris du chat (rien de choisi pour la correction), cloud
          // ou distant (`/api/show`) : rien n'est envoyé.
          for (const model of ["gpt-oss:20b-cloud", "chatonlm-polish-remote"]) {
            await settings(driver, {
              proofreadModel: "",
              selectedModel: model,
            });
            const secret = await holdSelection("teh confidential text");
            try {
              const { final: kept } = await pressProofread(
                desk,
                "Selected text not sent",
                { progress: false },
              );
              assert.equal(
                kept.body,
                `The chat model (${model}) runs outside this computer. To proofread with it, choose it on the Proofread page of ChatonLM.`,
              );
            } finally {
              await secret.release();
            }
            assert.equal((await readClipboard()).text, OTHER);
          }
          const chats = await (
            await fetch(new URL("/fake/requests", fake.url))
          ).json();
          assert.ok(
            !JSON.stringify(chats).includes("confidential"),
            "texte sélectionné envoyé",
          );

          // Aucun modèle, ni pour la correction ni dans le chat.
          await settings(driver, { proofreadModel: "", selectedModel: "" });
          const noModel = await holdSelection("teh text");
          try {
            const { final: unset } = await pressProofread(
              desk,
              "No model selected",
              { progress: false },
            );
            assert.equal(
              unset.body,
              "Choose a model on the Proofread page of ChatonLM.",
            );
          } finally {
            await noModel.release();
          }
          assert.equal((await readClipboard()).text, OTHER);
        } finally {
          await held.release();
        }
        await settings(driver, { proofreadModel: MODEL });
        // La dernière correction réussie reste celle montrée par « Show »
        // (pas celle qui a été arrêtée).
        assert.equal(
          (await driver.invoke("proofread_last_selection")).parts
            .map((part) => part.text)
            .join(""),
          "The letter is here.",
        );
        assert.equal((await dictationStatus(driver)).phase, "idle");

        // Touche changée par le bureau pour la correction seule.
        await desk.command("shortcuts-changed", {
          id: "proofread",
          trigger: "Meta+C",
        });
        s = await waitShortcuts(
          driver,
          (x) => x.proofread.trigger === "Meta+C",
          "touche changée",
        );
        assert.deepEqual(s.dictation, bound("Ctrl+Alt+D"));

        // Dictée coupée : nouvelle session, correction seule.
        const bothSession = bind.session;
        await driver.invoke("update_dictation_settings", {
          patch: { enabled: false },
        });
        await poll(
          () => binds(desk),
          (list) => list.length === 3,
          { what: "liaison de la correction seule" },
        );
        assert.deepEqual(binds(desk)[2].ids, ["proofread"]);
        await desk.waitFor(
          (e) => e.event === "close-session" && e.session === bothSession,
        );
        await waitShortcuts(
          driver,
          (x) =>
            x.proofread.state === "bound" && x.dictation.state === "disabled",
          "correction seule",
        );
        assert.deepEqual((await dictationStatus(driver)).shortcut, DISABLED);

        // Tout coupé : session fermée, plus rien à relier.
        const lastSession = binds(desk)[2].session;
        await settings(driver, { proofreadShortcut: false });
        await desk.waitFor(
          (e) => e.event === "close-session" && e.session === lastSession,
        );
        s = await waitShortcuts(
          driver,
          (x) => x.proofread.state === "disabled",
          "correction déliée",
        );
        assert.deepEqual(s.dictation, DISABLED);
        assert.deepEqual(await failureOf(driver, "shortcuts_bind"), {
          kind: "invalid",
          message: "Turn on a global shortcut first",
        });
        await delay(300);
        assert.equal(binds(desk).length, 3);

        // Pour la suite : raccourci demandé au prochain lancement.
        await settings(driver, { proofreadShortcut: true });
        await poll(
          () => binds(desk),
          (list) => list.length === 4,
          { what: "liaison rétablie" },
        );
      },
    );
  } finally {
    await desk.stop();
  }

  // 2. `--proofread` : fenêtre masquée, sélection corrigée une fois l'app
  // prête ; relance qui corrige de nouveau ; sans portail, statut et
  // commande à lier.
  const hidden = await startDesktopServices({
    notifications: true,
    watcher: true,
    portal: { bind: "ok" },
  });
  const held = await holdSelection("I recieve teh parcel.");
  try {
    await withApp(
      binary,
      {
        name: "proofread-cli",
        env: { ...env, CHATONLM_NO_GLOBAL_SHORTCUTS: "1" },
        args: ["--proofread"],
        quit: "command",
      },
      async (driver) => {
        await driver.waitFor("[data-testid=new-chat]");
        const first = await hidden.waitFor(
          (e) => e.event === "notify" && e.summary === CORRECTED,
          { timeoutMs: 30000 },
        );
        assert.equal(first.body, "I receive the parcel.");
        assert.equal((await readClipboard()).text, "I receive the parcel.");
        assert.equal(await visible(driver), false);

        // Relance avec --proofread : nouvelle correction, fenêtre masquée.
        assert.equal(await runInstance(["--proofread"], "proofread-second"), 0);
        await poll(
          () => hidden.notifications().filter((n) => n.summary === CORRECTED),
          (list) => list.length === 2,
          { what: "seconde correction", timeoutMs: 30000 },
        );
        assert.equal(await visible(driver), false);
        assert.equal((await dictationStatus(driver)).phase, "idle");
        // Relance sans argument : la fenêtre se montre.
        assert.equal(await runInstance([], "proofread-third"), 0);
        await waitVisible(driver, true);

        // Portail coupé par la variable : jamais contacté par ChatonLM
        // (GTK y enregistre seulement l'identifiant de l'app, depuis
        // enableGTKAppId), commande à lier.
        const s = await waitShortcuts(
          driver,
          (x) => x.proofread.state === "unavailable",
          "portail absent",
        );
        assert.deepEqual(s.proofread, {
          state: "unavailable",
          problem: "noPortal",
          reason: "Global shortcuts are not available on this desktop",
        });
        assert.deepEqual(s.dictation, DISABLED);
        assert.match(s.proofreadCommand, / --proofread$/);
        assert.deepEqual(
          hidden.events.filter((e) =>
            ["create-session", "bind"].includes(e.event),
          ),
          [],
          "portail contacté malgré la variable",
        );
      },
    );
  } finally {
    await held.release();
    await hidden.stop();
  }

  // 3. Refus de la boîte du bureau, puis nouvel essai (liaison sans touche,
  // page de réglage du bureau ouverte).
  const refusing = await startDesktopServices({
    notifications: true,
    watcher: false,
    portal: { bind: "cancel" },
  });
  try {
    await withApp(
      binary,
      { name: "proofread-refused", env },
      async (driver) => {
        await driver.waitFor("[data-testid=new-chat]");
        let s = await waitShortcuts(
          driver,
          (x) => x.proofread.state === "unavailable",
          "refus",
        );
        assert.deepEqual(s.proofread, {
          state: "unavailable",
          problem: "refused",
          reason: "The shortcut was not set",
        });
        assert.deepEqual(s.dictation, DISABLED);
        await driver.invoke("shortcuts_bind");
        s = await waitShortcuts(
          driver,
          (x) => x.proofread.state === "bound",
          "liaison sans touche",
        );
        assert.deepEqual(s.proofread, bound(""));
        assert.equal(binds(refusing).length, 2, "nouvel essai de liaison");
        await refusing.waitFor((e) => e.event === "configure");
        // La page Dictation : toujours rien (dictée coupée).
        assert.deepEqual(await failureOf(driver, "dictation_bind_shortcut"), {
          kind: "invalid",
          message: "Turn on dictation first",
        });
      },
    );
  } finally {
    await refusing.stop();
  }

  // 4. Dictée liée, puis correction activée : la boîte du bureau (pour la
  // correction seulement) reste ouverte, puis est refusée. L'ancienne
  // session reste : Ctrl+Alt+D bascule encore la dictée (sans modèle de
  // parole : « Dictation failed », une fois par appui), seule la correction
  // n'est pas liée.
  const partial = await startDesktopServices({
    notifications: true,
    watcher: false,
    portal: {
      version: 2,
      triggers: { dictation: "Ctrl+Alt+D", proofread: "Ctrl+Alt+C" },
    },
  });
  try {
    await withApp(
      binary,
      { name: "proofread-partial", env },
      async (driver) => {
        await driver.waitFor("[data-testid=new-chat]");
        // Raccourci de correction resté actif des lancements précédents :
        // lié au démarrage, puis coupé.
        await waitShortcuts(
          driver,
          (x) => x.proofread.state === "bound",
          "correction liée au démarrage",
        );
        await settings(driver, { proofreadShortcut: false });
        await waitShortcuts(
          driver,
          (x) => x.proofread.state === "disabled",
          "correction coupée au départ",
        );
        const before = binds(partial).length;
        await driver.invoke("update_dictation_settings", {
          patch: { enabled: true },
        });
        let s = await waitShortcuts(
          driver,
          (x) => x.dictation.state === "bound",
          "dictée liée",
        );
        assert.deepEqual(s.dictation, bound("Ctrl+Alt+D"));
        assert.equal(binds(partial).length, before + 1);
        assert.deepEqual(binds(partial)[before].ids, ["dictation"]);
        const dictationSession = binds(partial)[before].session;
        /** Appui bref sur Ctrl+Alt+D : la dictée bascule, une seule fois. */
        const tapDictation = async () => {
          const since = partial.notifications().length;
          await partial.command("tap", { id: "dictation" });
          await partial.waitFor(
            (e) =>
              e.event === "notify" &&
              e.summary === "Dictation failed" &&
              partial.notifications().indexOf(e) >= since,
          );
          await delay(1000);
          assert.deepEqual(
            notifiedSince(partial, since).map((n) => n.summary),
            ["Dictation failed"],
            "un seul déclenchement par appui",
          );
        };
        const closed = (session) =>
          partial.events.some(
            (e) => e.event === "close-session" && e.session === session,
          );

        // Boîte restée ouverte : la dictée reste liée et marche, la
        // correction attend ; un appui sur sa touche ne fait rien.
        await partial.command("portal-options", {
          bind: "hang",
          only: ["proofread"],
        });
        await settings(driver, {
          proofreadShortcut: true,
          proofreadModel: MODEL,
        });
        await poll(
          () => binds(partial),
          (list) => list.length === before + 2,
          { what: "liaison des deux" },
        );
        const hanging = binds(partial)[before + 1].session;
        assert.deepEqual(binds(partial)[before + 1].ids, [
          "dictation",
          "proofread",
        ]);
        s = await waitShortcuts(
          driver,
          (x) => x.proofread.state === "binding",
          "correction en attente",
        );
        assert.deepEqual(s.dictation, bound("Ctrl+Alt+D"));
        assert.ok(!closed(dictationSession), "ancienne session fermée");
        await tapDictation();
        const since = partial.notifications().length;
        await partial.command("tap", { id: "proofread" });
        await delay(1000);
        assert.deepEqual(notifiedSince(partial, since), []);

        // Correction coupée : boîte fermée, rien à relier.
        await settings(driver, { proofreadShortcut: false });
        await partial.waitFor(
          (e) => e.event === "close-session" && e.session === hanging,
        );
        s = await waitShortcuts(
          driver,
          (x) => x.proofread.state === "disabled",
          "correction coupée",
        );
        assert.deepEqual(s.dictation, bound("Ctrl+Alt+D"));
        await delay(500);
        assert.equal(binds(partial).length, before + 2, "liaison inutile");
        await tapDictation();

        // Refus de la boîte : seule la correction « Unavailable », la
        // dictée garde sa touche et son raccourci.
        await partial.command("portal-options", {
          bind: "cancel",
          only: ["proofread"],
        });
        await settings(driver, { proofreadShortcut: true });
        s = await waitShortcuts(
          driver,
          (x) => x.proofread.state === "unavailable",
          "refus de la correction",
        );
        assert.deepEqual(s.proofread, {
          state: "unavailable",
          problem: "refused",
          reason: "The shortcut was not set",
        });
        assert.deepEqual(s.dictation, bound("Ctrl+Alt+D"));
        assert.deepEqual(
          (await dictationStatus(driver)).shortcut,
          bound("Ctrl+Alt+D"),
        );
        const refusedSession = binds(partial)[before + 2].session;
        await partial.waitFor(
          (e) => e.event === "close-session" && e.session === refusedSession,
        );
        assert.ok(!closed(dictationSession), "ancienne session fermée");
        await tapDictation();

        // Nouvel essai : les deux liés (la correction sans touche), puis
        // l'ancienne session fermée.
        await driver.invoke("shortcuts_bind");
        s = await waitShortcuts(
          driver,
          (x) => x.proofread.state === "bound",
          "nouvel essai",
        );
        assert.deepEqual(s.proofread, bound(""));
        assert.deepEqual(s.dictation, bound("Ctrl+Alt+D"));
        await partial.waitFor(
          (e) => e.event === "close-session" && e.session === dictationSession,
        );
        await tapDictation();
      },
    );
  } finally {
    await partial.stop();
  }
} finally {
  await fake.stop();
}

console.log("✓ proofread-shortcut");
