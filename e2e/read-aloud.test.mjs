// Lecture à voix haute dans l'app réelle, avec le vrai moteur (voix de
// .tools/voice-models liées dans le dossier des données, sortie audio factice
// CHATONLM_E2E_AUDIO_OUT=null) et le faux serveur Ollama :
// 1. bouton sous une réponse finie : lecture (synthèse réelle), puis Arrêter ;
// 2. voix absente : accord demandé avant le téléchargement ; Annuler ne
//    télécharge rien ; accepté, le téléchargement (hôtes fermés) échoue et
//    l'erreur reste sous la réponse ;
// 3. lecture automatique (réglage) : la réponse (chatonlm-markdown-long : une
//    phrase de 150 caractères, puis 4,6 s de code) est lue pendant qu'elle
//    s'écrit, Arrêter sur la réponse en cours ; un nouvel envoi passe à la
//    nouvelle réponse.
import assert from "node:assert/strict";
import {
  existsSync,
  mkdirSync,
  readdirSync,
  rmSync,
  symlinkSync,
} from "node:fs";
import { ROOT, withApp } from "./lib/app.mjs";
import { startFake } from "./lib/fake.mjs";

const binary = process.argv[2];
const DATA = process.env.CHATONLM_DATA_DIR;
const VOICES = `${ROOT}.tools/voice-models`;
const ENGLISH = "pocket-tts-en";
if (!existsSync(`${VOICES}/${ENGLISH}`)) {
  throw new Error(
    `Voix absente : ${VOICES}/${ENGLISH} (scripts/tts/README.md, assemble.py)`,
  );
}

/** Bouton Copier d'une réponse finie (ceux des blocs de code n'ont pas ce titre). */
const MESSAGE_COPY = "button.copy-button[title='Copy']";

/** Lie la voix `id` de .tools/voice-models dans le dossier des données. */
function linkVoice(id) {
  const dir = `${DATA}/voice-models/${id}`;
  mkdirSync(dir, { recursive: true });
  for (const file of readdirSync(`${VOICES}/${id}`)) {
    symlinkSync(`${VOICES}/${id}/${file}`, `${dir}/${file}`);
  }
}

async function waitUntil(check, what, timeoutMs = 15000) {
  const deadline = Date.now() + timeoutMs;
  for (;;) {
    const value = await check();
    if (value) return value;
    if (Date.now() > deadline) throw new Error(`Délai dépassé : ${what}`);
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
}

/** Phases des boutons de lecture, dans l'ordre des réponses. */
const phases = (driver) =>
  driver.execute(
    "return [...document.querySelectorAll('button[data-read-aloud]')].map((b) => b.dataset.readAloud)",
  );

const clickLast = (driver, css) =>
  driver.execute(
    "const all = document.querySelectorAll(arguments[0]); all[all.length - 1].click()",
    [css],
  );

async function send(driver, text) {
  const textarea = await driver.waitFor("textarea[aria-label='Message']");
  await driver.type(textarea, text);
  await driver.execute(
    `document.querySelector("textarea[aria-label='Message']").dispatchEvent(
       new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true }));`,
  );
}

/** Choisit un modèle du faux serveur, puis recharge la page. */
async function useModel(driver, model) {
  await driver.invoke("update_settings", { patch: { selectedModel: model } });
  await driver.execute("window.__oldPage = true; location.reload()");
  await waitUntil(
    async () => !(await driver.execute("return window.__oldPage === true")),
    "rechargement",
  );
  await driver.tauriReady();
}

const fake = await startFake("ollama");
const env = { OLLAMA_HOST: fake.url };
try {
  // 1. Bouton, voix anglaise installée.
  linkVoice(ENGLISH);
  await withApp(binary, { name: "read-aloud-button", env }, async (driver) => {
    await driver.tauriReady();
    await useModel(driver, "chatonlm-slow");
    await send(driver, "Say something.");
    await waitUntil(
      async () => (await phases(driver)).length === 1,
      "bouton sous la réponse finie",
    );
    assert.deepEqual(await phases(driver), ["idle"]);
    await clickLast(driver, "button[data-read-aloud]");
    await waitUntil(
      async () => (await phases(driver))[0] === "playing",
      "lecture (synthèse réelle)",
    );
    const state = await driver.invoke("read_aloud_state");
    assert.equal(state.phase, "playing");
    // L'état nomme le texte lu : ici une réponse de la conversation.
    assert.equal(state.source?.kind, "message");
    assert.equal(typeof state.source.messageId, "number");
    await clickLast(driver, "button[data-read-aloud]");
    await waitUntil(
      async () => (await phases(driver))[0] === "idle",
      "arrêt par le bouton",
    );
    assert.equal((await driver.invoke("read_aloud_state")).phase, "idle");
  });

  // 2. Voix absente : accord demandé, puis téléchargement en échec.
  rmSync(`${DATA}/voice-models`, { recursive: true, force: true });
  await withApp(
    binary,
    { name: "read-aloud-download", env },
    async (driver) => {
      await driver.tauriReady();
      await useModel(driver, "chatonlm-slow");
      await send(driver, "Say something.");
      await waitUntil(
        async () => (await phases(driver)).length === 1,
        "bouton sous la réponse finie",
      );
      await clickLast(driver, "button[data-read-aloud]");
      const dialog = await driver.waitFor(
        "[role='dialog'], [role='alertdialog']",
      );
      assert.ok(dialog);
      const text = await driver.execute(
        "return document.querySelector(\"[role='dialog'], [role='alertdialog']\").textContent",
      );
      assert.match(text, /Download the voice\?/);
      assert.match(text, /English voice \(\d+(\.\d+)? MB\)/);
      // Annuler : rien n'est téléchargé.
      await driver.execute(
        `[...document.querySelectorAll("[role='dialog'] button, [role='alertdialog'] button")]
       .find((b) => b.textContent.trim() === "Cancel").click()`,
      );
      await waitUntil(
        async () =>
          !(await driver.execute(
            "return !!document.querySelector(\"[role='dialog'], [role='alertdialog']\")",
          )),
        "dialogue fermé",
      );
      assert.deepEqual(await driver.invoke("voice_downloads"), []);
      // Accepté : téléchargement (hôtes fermés) en échec, erreur sous la réponse.
      await clickLast(driver, "button[data-read-aloud]");
      await driver.waitFor("[role='dialog'], [role='alertdialog']");
      await driver.execute(
        `[...document.querySelectorAll("[role='dialog'] button, [role='alertdialog'] button")]
       .find((b) => b.textContent.trim() === "Download and read").click()`,
      );
      await waitUntil(async () => {
        const state = await driver.invoke("read_aloud_state");
        return state.phase === "idle" && state.error === "downloadFailed";
      }, "téléchargement en échec");
      await waitUntil(
        async () =>
          await driver.execute(
            'return document.body.textContent.includes("The voice couldn\'t be downloaded. Try again.")',
          ),
        "erreur affichée sous la réponse",
      );
    },
  );

  // 3. Lecture automatique.
  linkVoice(ENGLISH);
  await withApp(binary, { name: "read-aloud-auto", env }, async (driver) => {
    await driver.tauriReady();
    await driver.invoke("update_settings", { patch: { autoRead: true } });
    await useModel(driver, "chatonlm-markdown-long");
    await send(driver, "Explain something.");
    // Lue pendant qu'elle s'écrit : pas encore de bouton Copier de la réponse
    // (réponse finie ; ceux des blocs de code n'ont pas ce titre), mais Arrêter
    // sur la réponse en cours.
    await waitUntil(
      async () => (await phases(driver)).includes("playing"),
      "lecture automatique pendant l'écriture",
      30000,
    );
    const copies = await driver.execute(
      "return document.querySelectorAll(arguments[0]).length",
      [MESSAGE_COPY],
    );
    assert.equal(copies, 0, "la réponse s'écrit encore");
    const first = (await driver.invoke("read_aloud_state")).source?.messageId;
    assert.equal(typeof first, "number");
    // Nouvel envoi (après la fin de la réponse) : la lecture passe à la
    // nouvelle réponse.
    await waitUntil(
      async () =>
        (await driver.execute(
          "return document.querySelectorAll(arguments[0]).length",
          [MESSAGE_COPY],
        )) === 1,
      "fin de la première réponse",
      60000,
    );
    await send(driver, "Another question.");
    await waitUntil(
      async () => {
        const state = await driver.invoke("read_aloud_state");
        return state.phase !== "idle" && state.source?.messageId > first;
      },
      "lecture de la nouvelle réponse",
      30000,
    );
    await driver.invoke("read_aloud_stop");
    await waitUntil(
      async () => (await driver.invoke("read_aloud_state")).phase === "idle",
      "arrêt",
    );
  });
} finally {
  await fake.stop();
}
console.log("✓ read-aloud");
