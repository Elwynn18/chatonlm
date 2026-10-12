// ONNX Runtime de Parakeet chargé à l'exécution (étape 18a : fonctionnalité
// load-dynamic d'ort, build officiel de Microsoft) :
// - la session des essais transmet ORT_DYLIB_PATH (activate.sh) à l'app ;
// - vraie bibliothèque : Parakeet disponible, dictée de jfk.wav transcrite
//   par le vrai modèle (lien vers .tools/speech-models) ;
// - ORT_DYLIB_PATH vers un fichier absent : Parakeet indisponible avec la
//   raison (catalogue `runtimeProblem`, téléchargement refusé, dictée
//   refusée avant d'écouter, carte Dictation et ModelHub qui l'expliquent),
//   l'app reste vivante et Whisper n'est pas concerné ;
// - ORT_DYLIB_PATH vide : bibliothèque cherchée dans le dossier des
//   ressources de Tauri (/usr/lib/chatonlm pour ce binaire).
import assert from "node:assert/strict";
import { existsSync, mkdirSync, rmSync, symlinkSync } from "node:fs";
import { ROOT, withApp } from "./lib/app.mjs";
import { poll, startDesktopServices } from "./lib/desktop.mjs";
import { startFake } from "./lib/fake.mjs";

const binary = process.argv[2];
const DATA = process.env.CHATONLM_DATA_DIR;
const RUNTIME = process.env.ORT_DYLIB_PATH;
const JFK = `${ROOT}.tools/speech-samples/jfk.wav`;
const PARAKEET_FILES = `${ROOT}.tools/speech-models/parakeet-tdt-0.6b-v3-int8`;
const PARAKEET = "parakeet-tdt-0.6b-v3";
const ABSENT = `${ROOT}.tools/tmp/onnx-absent-${process.pid}/libonnxruntime.so.1`;
const REFUSAL = `Parakeet can't run: ONNX Runtime could not be loaded (${ABSENT}). Reinstall ChatonLM, or choose a Whisper model in Settings.`;

assert.ok(
  DATA,
  "CHATONLM_DATA_DIR absent : lancer dans scripts/e2e-session.sh",
);
assert.ok(
  RUNTIME && existsSync(RUNTIME),
  `ORT_DYLIB_PATH non transmis ou absent (${RUNTIME}) : scripts/fetch-onnxruntime.sh`,
);
assert.ok(!existsSync(ABSENT));

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

const parakeetOf = async (driver) =>
  (await driver.invoke("speech_models")).find((model) => model.id === PARAKEET);

/** Dictée avec Parakeet, sans nettoyage ni copie automatique. */
const useParakeet = (driver) =>
  driver.invoke("update_dictation_settings", {
    patch: {
      enabled: true,
      speechModel: PARAKEET,
      polishModel: "",
      autoCopy: false,
    },
  });

async function openSettingsTab(driver, tab) {
  const show = await driver.findAll("button[aria-label='Show sidebar']");
  if (show.length > 0) await driver.click(show[0]);
  await driver.click(await driver.waitFor("nav a[href='/settings']"));
  await driver.click(await driver.waitFor(`[data-testid=settings-tab-${tab}]`));
}

// Parakeet « installé » : liens vers la copie de l'essai (670 Mo).
mkdirSync(`${DATA}/speech-models/${PARAKEET}`, { recursive: true });
for (const file of [
  "encoder-model.int8.onnx",
  "decoder_joint-model.int8.onnx",
  "nemo128.onnx",
  "vocab.txt",
]) {
  symlinkSync(
    `${PARAKEET_FILES}/${file}`,
    `${DATA}/speech-models/${PARAKEET}/${file}`,
  );
}
const fake = await startFake("ollama");
// Notifications seulement : ni zone de notification, ni portail.
const desk = await startDesktopServices({
  notifications: true,
  watcher: false,
  portal: null,
});
const env = {
  OLLAMA_HOST: fake.url,
  CHATONLM_E2E_MICROPHONE: JFK,
  CHATONLM_NO_GLOBAL_SHORTCUTS: "1",
};

try {
  // 1. Vraie bibliothèque : Parakeet transcrit.
  await withApp(binary, { name: "onnx-real", env }, async (driver) => {
    await driver.waitFor("[data-testid=new-chat]");
    const parakeet = await parakeetOf(driver);
    assert.equal(parakeet.installed, true);
    assert.equal(parakeet.runtimeProblem, null);
    await useParakeet(driver);
    await driver.invoke("dictation_toggle");
    await poll(
      () => driver.invoke("dictation_status"),
      (s) => s.phase === "recording",
      { what: "écoute" },
    );
    await driver.invoke("dictation_toggle");
    const status = await poll(
      () => driver.invoke("dictation_status"),
      (s) => s.phase === "idle" && s.lastResult !== null,
      { what: "transcription par Parakeet", timeoutMs: 90000 },
    );
    assert.equal(status.lastResult.error, null, status.lastResult.error);
    assert.match(
      status.lastResult.text.toLowerCase(),
      /ask not what your country can do for you/,
    );
  });

  // 2. Bibliothèque absente : Parakeet indisponible, l'app continue.
  await withApp(
    binary,
    { name: "onnx-absent", env: { ...env, ORT_DYLIB_PATH: ABSENT } },
    async (driver) => {
      await driver.waitFor("[data-testid=new-chat]");
      const parakeet = await parakeetOf(driver);
      assert.deepEqual(parakeet.runtimeProblem, {
        kind: "missing",
        detail: ABSENT,
      });
      const whisper = (await driver.invoke("speech_models")).find(
        (model) => model.id === "whisper-tiny",
      );
      assert.equal(whisper.runtimeProblem, null);

      const refused = await failureOf(driver, "speech_download", {
        id: PARAKEET,
      });
      assert.equal(refused?.message, REFUSAL);
      assert.deepEqual(await driver.invoke("speech_downloads"), []);

      // Dictée refusée sans écouter : un seul appui suffit à la finir.
      await useParakeet(driver);
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

      // Carte Dictation : Parakeet grisé et expliqué.
      await openSettingsTab(driver, "dictation");
      const note = await poll(
        () => textOf(driver, "[data-testid=dictation]"),
        (text) => text.includes("ONNX Runtime"),
        { what: "explication de la carte Dictation" },
      );
      assert.ok(
        note.includes(
          `Parakeet TDT 0.6B v3 can't run: ONNX Runtime is missing (${ABSENT}). Reinstall ChatonLM, or choose a Whisper model.`,
        ),
        note,
      );
      const option = await driver.execute(
        `const select = document.querySelector("[data-testid=dictation] select");
         const option = [...select.options].find((o) => o.value === arguments[0]);
         return [option.text, option.disabled];`,
        [PARAKEET],
      );
      assert.deepEqual(option, ["Parakeet TDT 0.6B v3 (unavailable)", true]);

      // ModelHub : carte Parakeet sans Use ni Download, avec la raison.
      await driver.click(await driver.waitFor("a[href='/hub/transcription']"));
      const card = await poll(
        () =>
          driver.execute(
            `const card = [...document.querySelectorAll("[data-testid=speech-model]")]
               .find((c) => c.querySelector("h3")?.textContent === "Parakeet TDT 0.6B v3");
             return card ? [card.textContent, [...card.querySelectorAll("button")].map((b) => b.textContent)] : null;`,
          ),
        (found) => found !== null,
        { what: "carte Parakeet" },
      );
      assert.ok(
        card[0].includes(
          `Not available: ONNX Runtime, which Parakeet needs, is missing (${ABSENT}). Reinstall ChatonLM.`,
        ),
        card[0],
      );
      assert.ok(
        !card[1].includes("Download") && !card[1].includes("Use"),
        JSON.stringify(card[1]),
      );
      await driver.screenshot(".tools/shots/hub-onnx-missing.png");
    },
  );

  // 3. Sans variable : dossier des ressources de Tauri, celui d'un binaire
  // installé ici (dossier cible qui ne s'appelle pas « target »).
  const INSTALLED = "/usr/lib/chatonlm/libonnxruntime.so.1";
  await withApp(
    binary,
    { name: "onnx-resources", env: { ...env, ORT_DYLIB_PATH: "" } },
    async (driver) => {
      await driver.waitFor("[data-testid=new-chat]");
      const parakeet = await parakeetOf(driver);
      if (existsSync(INSTALLED)) {
        assert.equal(parakeet.runtimeProblem, null);
      } else {
        assert.deepEqual(parakeet.runtimeProblem, {
          kind: "missing",
          detail: INSTALLED,
        });
      }
    },
  );
  console.log("onnx-runtime : OK");
} finally {
  rmSync(`${ROOT}.tools/tmp/onnx-absent-${process.pid}`, {
    recursive: true,
    force: true,
  });
  await desk.stop();
  await fake.stop();
}
