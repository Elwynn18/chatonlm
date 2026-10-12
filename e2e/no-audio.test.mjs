// Machine sans carte son (conteneur de la CI, serveur, poste sans micro) :
// la lecture à voix haute et le mode conversation vocale échouent proprement,
// avec leur message, et l'app reste utilisable. App réelle, VRAI chemin audio
// (cpal sur ALSA), sans la sortie factice (CHATONLM_E2E_AUDIO_OUT) ni le faux
// micro (CHATONLM_E2E_VOICE_MIC) des autres essais.
//
// Aucun périphérique de la machine n'est jamais ouvert : ALSA_CONFIG_PATH
// remplace toute la configuration d'ALSA (ni ~/.asoundrc ni conf.d, donc ni
// PipeWire ni PulseAudio) par un fichier dont l'entrée et la sortie « default »
// visent une carte qui n'existe pas. L'ouverture échoue alors comme sur une
// machine sans carte (carte introuvable). ALSA découpe ALSA_CONFIG_PATH aux
// espaces : le fichier va dans le dossier d'exécution de la session
// (/tmp/chatonlm-e2e.*), pas dans le projet, dont le chemin a une espace ; le
// journal de l'app prouve à la fin qu'il a été lu.
// 1. lecture : voix installée, synthèse prête, sortie impossible à ouvrir →
//    « no audio output device was found » sous la réponse, bouton de nouveau
//    prêt ; un second clic redonne la même erreur ;
// 2. mode vocal : modèle de transcription et voix installés, micro impossible
//    à ouvrir → message du micro montré, pas de séance, zone de saisie rendue ;
//    un message tapé reçoit toujours sa réponse.
import assert from "node:assert/strict";
import { existsSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { setTimeout as delay } from "node:timers/promises";
import { ROOT, withApp } from "./lib/app.mjs";
import { startFake } from "./lib/fake.mjs";
import { PARAKEET, barState, linkParakeet, linkVoice } from "./lib/voice.mjs";

const binary = process.argv[2];
const DATA = process.env.CHATONLM_DATA_DIR;
assert.ok(
  DATA,
  "CHATONLM_DATA_DIR absent : lancer dans scripts/e2e-session.sh",
);
assert.ok(
  process.env.ORT_DYLIB_PATH && existsSync(process.env.ORT_DYLIB_PATH),
  "ORT_DYLIB_PATH non transmis ou absent : scripts/fetch-onnxruntime.sh",
);

const RUNTIME = process.env.XDG_RUNTIME_DIR;
assert.ok(
  RUNTIME && !/\s/.test(RUNTIME),
  `XDG_RUNTIME_DIR absent ou avec une espace : ${RUNTIME}`,
);
const ENGLISH = "pocket-tts-en";
const TEXTAREA = "textarea[aria-label='Message']";
const PROBLEM = "[data-testid=read-aloud-problem]";
const NO_OUTPUT = "Couldn't read aloud: no audio output device was found.";
/**
 * Échec du vrai micro : cpal sur ALSA donne toujours un périphérique
 * « default », c'est son ouverture qui échoue. « No microphone found » serait
 * le faux micro des autres essais (CHATONLM_E2E_VOICE_MIC=none), pas ce chemin.
 */
const MICROPHONE = /Could not open the microphone: \S[^\n]*/;

async function waitUntil(check, what, timeoutMs = 15000) {
  const deadline = Date.now() + timeoutMs;
  for (;;) {
    const value = await check();
    if (value) return value;
    if (Date.now() > deadline) {
      throw new Error(
        `Délai dépassé : ${typeof what === "function" ? await what() : what}`,
      );
    }
    await delay(100);
  }
}

const exists = (driver, css) =>
  driver.execute("return document.querySelector(arguments[0]) !== null", [css]);
const texts = (driver, css) =>
  driver.execute(
    "return [...document.querySelectorAll(arguments[0])].map((e) => e.textContent)",
    [css],
  );
/** Texte visible de la page (ce que l'utilisateur peut lire). */
const pageText = (driver) => driver.execute("return document.body.innerText");
const phases = (driver) =>
  driver.execute(
    "return [...document.querySelectorAll('button[data-read-aloud]')].map((b) => b.dataset.readAloud)",
  );
const clickLast = (driver, css) =>
  driver.execute(
    "const all = document.querySelectorAll(arguments[0]); all[all.length - 1].click()",
    [css],
  );

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

/** Tape un message et l'envoie (Entrée), puis attend `count` réponses finies. */
async function send(driver, text, count) {
  await driver.type(await driver.waitFor(TEXTAREA), text);
  await driver.execute(
    `document.querySelector(arguments[0]).dispatchEvent(
       new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true }));`,
    [TEXTAREA],
  );
  await waitUntil(
    async () =>
      (await texts(driver, "[data-role=assistant]")).length === count &&
      !(await exists(driver, "button[aria-label='Stop']")),
    `réponse ${count} au message tapé`,
  );
}

/** Lance `fn`, avec une capture de la page si elle échoue (diagnostic). */
async function shotOnFailure(driver, name, fn) {
  try {
    await fn();
  } catch (error) {
    const file = `${ROOT}.tools/shots/e2e-${name}-failure.png`;
    await driver.screenshot(file).catch(() => {});
    console.error(`Capture de la page : ${file}`);
    throw error;
  }
}

const asound = `${RUNTIME}/chatonlm-no-audio-asound.conf`;
writeFileSync(
  asound,
  "pcm.!default { type hw card 99 }\nctl.!default { type hw card 99 }\n",
);

const fake = await startFake("ollama");
const env = {
  OLLAMA_HOST: fake.url,
  CHATONLM_NO_GLOBAL_SHORTCUTS: "1",
  ALSA_CONFIG_PATH: asound,
  // Vrais périphériques (ceux de la configuration ci-dessus : aucun).
  CHATONLM_E2E_AUDIO_OUT: undefined,
  CHATONLM_E2E_VOICE_MIC: undefined,
};

try {
  linkParakeet(DATA);
  rmSync(`${DATA}/voice-models`, { recursive: true, force: true });
  linkVoice(DATA, ENGLISH);

  await withApp(binary, { name: "no-audio", env }, async (driver) => {
    await driver.tauriReady();
    await driver.invoke("update_dictation_settings", {
      patch: { speechModel: PARAKEET, language: "en" },
    });
    await useModel(driver, "chatonlm-slow");

    // 1. Lecture à voix haute sans sortie audio.
    await shotOnFailure(driver, "no-audio-read-aloud", async () => {
      await send(driver, "Say something.", 1);
      await waitUntil(
        async () => (await phases(driver)).length === 1,
        "bouton sous la réponse finie",
      );
      for (const attempt of [1, 2]) {
        await clickLast(driver, "button[data-read-aloud]");
        // La synthèse se prépare (vraie voix), puis la sortie est refusée.
        await waitUntil(
          async () =>
            (await texts(driver, PROBLEM)).includes(NO_OUTPUT) &&
            (await phases(driver))[0] === "idle",
          async () =>
            `erreur de sortie sous la réponse (essai ${attempt}) : ${JSON.stringify(
              [await phases(driver), await texts(driver, PROBLEM)],
            )}`,
          60000,
        );
        assert.equal((await driver.invoke("read_aloud_state")).phase, "idle");
      }
    });

    // 2. Mode conversation vocale sans micro.
    await shotOnFailure(driver, "no-audio-voice-mode", async () => {
      const requirements = await driver.invoke("voice_mode_requirements");
      assert.equal(requirements.unavailable, null);
      assert.equal(requirements.speechModelInstalled, true);
      assert.equal(requirements.voiceInstalled, true);
      await driver.click(await driver.waitFor("[data-voice-mode-start]"));
      // Le message du micro est montré (modèles chargés d'abord).
      await waitUntil(
        async () => MICROPHONE.test(await pageText(driver)),
        async () =>
          `message du micro : ${JSON.stringify([
            await driver.invoke("voice_mode_state"),
            await barState(driver),
          ])}`,
        90000,
      );
      console.log(
        `Message du micro : ${MICROPHONE.exec(await pageText(driver))?.[0]}`,
      );
      const state = await driver.invoke("voice_mode_state");
      assert.equal(state.phase, "off", JSON.stringify(state));
      // La zone de saisie est rendue, le bouton du mode vocal aussi.
      await waitUntil(
        async () =>
          (await exists(driver, TEXTAREA)) &&
          (await exists(driver, "[data-voice-mode-start]")),
        "zone de saisie rendue",
      );
      // L'app reste utilisable.
      await send(driver, "Say something else.", 2);
      assert.equal(
        await exists(driver, "[data-testid=chat] [role=alert]"),
        false,
        "erreur affichée dans la conversation",
      );
    });
  });
  // La configuration a bien été lue, et la sortie a été refusée par ALSA.
  const log = readFileSync(`${ROOT}.tools/logs/e2e-no-audio.log`, "utf8");
  assert.doesNotMatch(log, /Cannot access file/);
  assert.match(log, /cannot open the audio output/);
} finally {
  await fake.stop();
  rmSync(asound, { force: true });
}
