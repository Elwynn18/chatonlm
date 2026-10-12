// Processeur sans les instructions dont whisper.cpp a besoin (étape 18a :
// ggml compilé pour x86-64-v3), imposé par CHATONLM_E2E_CPU_MISSING=avx2,fma
// (fonctionnalité e2e) :
// - catalogue : modèles Whisper marqués (`missingCpuFeatures`), Parakeet non ;
// - téléchargement d'un modèle Whisper refusé, rien en file ;
// - dictée avec Whisper Tiny installé : refusée aussitôt (avant d'écouter),
//   avec le message clair (aussi dans la notification, faux serveur de
//   notifications de e2e/lib/desktop-services.py), sans appel à
//   whisper.cpp ; l'app reste vivante ;
// - carte Dictation : modèle grisé dans la liste et expliqué ; ModelHub :
//   carte Whisper sans Download, avec l'explication.
import assert from "node:assert/strict";
import { mkdirSync, symlinkSync } from "node:fs";
import { ROOT, withApp } from "./lib/app.mjs";
import { poll, startDesktopServices } from "./lib/desktop.mjs";
import { startFake } from "./lib/fake.mjs";

const binary = process.argv[2];
const DATA = process.env.CHATONLM_DATA_DIR;
const JFK = `${ROOT}.tools/speech-samples/jfk.wav`;
const TINY = `${ROOT}.tools/speech-models/whisper/ggml-tiny.bin`;
const REFUSAL =
  "This processor cannot run Whisper models (missing: AVX2, FMA). Choose a Parakeet model in Settings.";

assert.ok(
  DATA,
  "CHATONLM_DATA_DIR absent : lancer dans scripts/e2e-session.sh",
);

/** Erreur renvoyée par une commande (null si elle réussit). */
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
    "return document.querySelector(arguments[0])?.textContent ?? ''",
    [css],
  );

async function openSettingsTab(driver, tab) {
  const show = await driver.findAll("button[aria-label='Show sidebar']");
  if (show.length > 0) await driver.click(show[0]);
  await driver.click(await driver.waitFor("nav a[href='/settings']"));
  await driver.click(await driver.waitFor(`[data-testid=settings-tab-${tab}]`));
}

// Whisper Tiny « installé » (lien vers la copie de l'essai).
mkdirSync(`${DATA}/speech-models/whisper-tiny`, { recursive: true });
symlinkSync(TINY, `${DATA}/speech-models/whisper-tiny/ggml-tiny.bin`);
const fake = await startFake("ollama");
// Notifications seulement : ni zone de notification, ni portail.
const desk = await startDesktopServices({
  notifications: true,
  watcher: false,
  portal: null,
});

try {
  await withApp(
    binary,
    {
      name: "whisper-cpu",
      env: {
        OLLAMA_HOST: fake.url,
        CHATONLM_E2E_MICROPHONE: JFK,
        CHATONLM_E2E_CPU_MISSING: "avx2,fma",
        // Pas de portail des raccourcis dans cette session.
        CHATONLM_NO_GLOBAL_SHORTCUTS: "1",
      },
    },
    async (driver) => {
      await driver.waitFor("[data-testid=new-chat]");
      const models = await driver.invoke("speech_models");
      for (const model of models) {
        assert.deepEqual(
          model.missingCpuFeatures,
          model.engine === "whisper" ? ["AVX2", "FMA"] : [],
          model.id,
        );
      }
      assert.ok(models.find((model) => model.id === "whisper-tiny").installed);

      // Téléchargement refusé.
      const refused = await failureOf(driver, "speech_download", {
        id: "whisper-base",
      });
      assert.equal(refused?.message, REFUSAL);
      assert.deepEqual(await driver.invoke("speech_downloads"), []);

      // Dictée : refusée sans écouter (un seul appui suffit à la finir).
      await driver.invoke("update_dictation_settings", {
        patch: { enabled: true, speechModel: "whisper-tiny", language: "en" },
      });
      await driver.invoke("dictation_toggle");
      const status = await poll(
        () => driver.invoke("dictation_status"),
        (s) => s.phase === "idle" && s.lastResult !== null,
        { what: "dictée refusée", timeoutMs: 15000 },
      );
      assert.equal(status.lastResult.text, "");
      assert.equal(status.lastResult.error, REFUSAL);
      await poll(
        () => desk.notifications(),
        (list) => list.some((n) => `${n.summary} ${n.body}`.includes(REFUSAL)),
        { what: "notification de l'échec" },
      );

      // Carte Dictation : modèle grisé et expliqué.
      await openSettingsTab(driver, "dictation");
      const note = await poll(
        () => textOf(driver, "[data-testid=dictation]"),
        (text) => text.includes("can't run on this processor"),
        { what: "explication de la carte Dictation" },
      );
      assert.ok(
        note.includes(
          "Whisper Tiny can't run on this processor (missing: AVX2, FMA). Choose a Parakeet model.",
        ),
      );
      const option = await driver.execute(
        `const select = document.querySelector("[data-testid=dictation] select");
         const option = [...select.options].find((o) => o.value === "whisper-tiny");
         return [option.text, option.disabled];`,
      );
      assert.deepEqual(option, [
        "Whisper Tiny (not supported by this processor)",
        true,
      ]);

      // ModelHub : carte Whisper sans Download, avec l'explication.
      await driver.click(await driver.waitFor("a[href='/hub/transcription']"));
      const card = await poll(
        () =>
          driver.execute(
            `const card = [...document.querySelectorAll("[data-testid=speech-model]")]
               .find((c) => c.querySelector("h3")?.textContent === "Whisper Base");
             return card ? [card.textContent, [...card.querySelectorAll("button")].map((b) => b.textContent)] : null;`,
          ),
        (found) => found !== null,
        { what: "carte Whisper Base" },
      );
      assert.ok(
        card[0].includes(
          "Not available on this computer: Whisper needs instructions its processor lacks (AVX2, FMA).",
        ),
        card[0],
      );
      assert.ok(!card[1].includes("Download"), JSON.stringify(card[1]));
      await driver.screenshot(".tools/shots/hub-whisper-cpu.png");
    },
  );
  console.log("whisper-cpu : OK");
} finally {
  await desk.stop();
  await fake.stop();
}
