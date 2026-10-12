// Mode conversation vocale en conditions réelles : un tour avec qwen3:0.6b
// sur l'instance Ollama de test (scripts/test-ollama.sh, 127.0.0.1:11435,
// démarrée par scripts/check.sh --full), jamais sur le service du système ;
// vrai Parakeet, vraies voix (.tools/voice-models : la française, et
// l'anglaise et l'espagnole si le petit modèle répond dans une autre langue),
// sortie audio factice au rythme réel, faux micro (début de fr0.wav, voir
// e2e/lib/voice.mjs), mesures du moteur (CHATONLM_VOICE_TIMINGS=1).
// Le modèle est d'abord chargé par l'app (load_model : contexte et durée de
// ses réglages), comme après le préchargement : latence d'un échange courant.
// Vérifie : phrase transcrite (mots de fr0), réponse écrite sans
// raisonnement (think coupé) et lue, retour à l'écoute, historique ; mesure
// de bout en bout fin de la parole → premier son (et fin du WAV → premier
// son), affichée sur la sortie d'erreur (check.sh n'affiche qu'elle) et
// bornée largement (≤ 8 s) pour ne pas dépendre de la charge de la machine.
// Les réponses d'un vrai modèle varient : seule leur forme est vérifiée.
import assert from "node:assert/strict";
import { existsSync, rmSync } from "node:fs";
import { setTimeout as delay } from "node:timers/promises";
import { ROOT, withApp } from "./lib/app.mjs";
import { watchLog } from "./lib/log.mjs";
import {
  PARAKEET,
  PHRASES,
  barState,
  describeStates,
  firstSoundDelay,
  heard,
  linkParakeet,
  linkVoice,
  recordVoiceEvents,
  stateAfter,
  voiceTimings,
  waitVoice,
  wavEnds,
  writePhrase,
} from "./lib/voice.mjs";

const binary = process.argv[2];
const OLLAMA = "http://127.0.0.1:11435";
const MODEL = "qwen3:0.6b";
/** Borne de la latence (fin de la parole → premier son), en ms. */
const LIMIT_MS = 8000;
const DATA = process.env.CHATONLM_DATA_DIR;
const TMP = `${ROOT}.tools/tmp/real-voice-mode-${process.pid}`;
assert.ok(
  DATA,
  "CHATONLM_DATA_DIR absent : lancer dans scripts/e2e-session.sh",
);
assert.ok(
  process.env.ORT_DYLIB_PATH && existsSync(process.env.ORT_DYLIB_PATH),
  "ORT_DYLIB_PATH non transmis ou absent : scripts/fetch-onnxruntime.sh",
);

async function waitUntil(check, what, timeoutMs = 10000) {
  const deadline = Date.now() + timeoutMs;
  for (;;) {
    const value = await check();
    if (value) return value;
    if (Date.now() > deadline) throw new Error(`Délai dépassé : ${what}`);
    await delay(100);
  }
}

const response = await fetch(`${OLLAMA}/api/tags`).catch(() => null);
if (!response?.ok) {
  throw new Error(
    "Instance Ollama de test injoignable : scripts/test-ollama.sh start",
  );
}
const installed = (await response.json()).models.map((m) => m.name);
if (!installed.includes(MODEL)) {
  throw new Error(`${MODEL} absent : scripts/test-ollama.sh pull ${MODEL}`);
}

linkParakeet(DATA);
for (const voice of ["pocket-tts-fr", "pocket-tts-en", "pocket-tts-es"]) {
  linkVoice(DATA, voice);
}
const first = writePhrase(PHRASES.first, TMP);

try {
  await withApp(
    binary,
    {
      name: "real-voice-mode",
      env: {
        OLLAMA_HOST: OLLAMA,
        CHATONLM_NO_GLOBAL_SHORTCUTS: "1",
        CHATONLM_VOICE_TIMINGS: "1",
        CHATONLM_E2E_VOICE_MIC: JSON.stringify([
          { on: "listening", wav: first.wav, delayMs: 300 },
        ]),
      },
    },
    async (driver) => {
      const log = watchLog("real-voice-mode");
      try {
        await driver.waitFor("[data-testid=new-chat]");
        await driver.invoke("update_dictation_settings", {
          patch: { speechModel: PARAKEET, language: "fr" },
        });
        await driver.invoke("update_settings", {
          patch: { selectedModel: MODEL },
        });
        await driver.execute("window.__oldPage = true; location.reload()");
        await waitUntil(
          async () =>
            !(await driver.execute("return window.__oldPage === true")),
          "rechargement",
        );
        await driver.tauriReady();
        await driver.waitFor("[data-testid=new-chat]");
        // Modèle en mémoire avant le tour (rend la main une fois chargé).
        await driver.invoke("load_model", { model: MODEL });
        assert.ok(
          (await driver.invoke("running_models")).some((m) => m.name === MODEL),
          `${MODEL} pas en mémoire`,
        );
        await recordVoiceEvents(driver);

        await driver.click(await driver.waitFor("[data-voice-mode-start]"));
        await waitUntil(
          async () =>
            ["listening", "hearing"].includes((await barState(driver))?.phase),
          "barre vocale à l'écoute",
          60000,
        );
        const utterance = await waitVoice(
          driver,
          (e) => e.utterances.find((u) => u.turn === 1),
          "phrase transcrite",
          40000,
        );
        assert.ok(
          heard(utterance.text, PHRASES.first),
          `phrase : ${utterance.text}`,
        );
        // Premier son, ou avis (réponse dans une autre langue, voix
        // absente, échec) : la réponse n'est alors pas lue.
        const spoken = await waitVoice(
          driver,
          (e) =>
            stateAfter(
              e.states,
              (s) =>
                s.turn === 1 && (s.phase === "speaking" || s.notice !== null),
            ),
          "lecture de la réponse",
          90000,
        );
        if (spoken.state.phase !== "speaking") {
          const reply = await driver.execute(
            "return document.querySelector('[data-role=assistant]')?.textContent ?? ''",
          );
          throw new Error(
            `Réponse non lue (avis ${JSON.stringify(spoken.state.notice)}) : ${reply}`,
          );
        }
        const end = wavEnds(log)[0];
        assert.ok(end, "ligne « wav <n> ended » du faux micro absente");
        const { wav, speech } = firstSoundDelay(
          end,
          spoken.state,
          first.afterSpeech,
        );
        const timing = voiceTimings(log).find((t) => t.turn === 1);
        console.error(
          `  mode vocal, ${MODEL} : fin de la parole → premier son ${speech} ms` +
            ` (fin du WAV → premier son ${wav} ms) ; moteur : ${JSON.stringify(timing ?? null)}`,
        );
        assert.ok(
          speech <= LIMIT_MS,
          `premier son ${speech} ms après la fin de la parole (borne ${LIMIT_MS} ms)`,
        );
        assert.equal(typeof timing?.first_sound_ms, "number");

        // Lecture jusqu'au bout, retour à l'écoute, puis End.
        const back = await waitVoice(
          driver,
          (e) =>
            stateAfter(
              e.states,
              (s) => s.phase === "listening" && s.turn === 1,
              spoken.index,
            ),
          "retour à l'écoute après la réponse",
          120000,
        );
        assert.equal(back.state.notice, null, describeStates([back.state]));
        await driver.click(await driver.find("[data-voice-mode-end]"));
        await waitUntil(
          async () =>
            (await driver.invoke("voice_mode_state")).phase === "off" &&
            (await barState(driver)) === null,
          "fin de la séance",
        );

        // Historique : la phrase et une vraie réponse, sans raisonnement.
        const chat = (await driver.execute("return location.pathname")).slice(
          3,
        );
        const saved = await driver.invoke("get_chat", { id: chat });
        assert.deepEqual(
          saved.messages.map((m) => m.role),
          ["user", "assistant"],
        );
        assert.equal(saved.messages[0].content.trim(), utterance.text.trim());
        assert.ok(saved.messages[1].content.trim().length > 0, "réponse vide");
        assert.equal(saved.messages[1].thinking, "", "raisonnement non coupé");
      } finally {
        log.stop();
      }
    },
  );
} finally {
  rmSync(TMP, { recursive: true, force: true });
}
console.log("✓ real-voice-mode");
