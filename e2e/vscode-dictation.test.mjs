// Dictée tenue par l'extension VS Code (lot 4b) dans l'app réelle : micro
// simulé par un WAV (CHATONLM_E2E_MICROPHONE), vrai Whisper tiny, nettoyage
// par le faux modèle `chatonlm-polish`, et un client Node qui fait ce que fera
// l'extension (e2e/lib/bridge.mjs) :
// 1. `dictation.start` avec les noms du projet, interrupteur de la dictée
//    coupé : `recording` puis la réponse ; bandeau « VS Code is using your
//    microphone » sur la page ; l'essai de la page ne l'arrête pas ; une
//    autre connexion est refusée (-32032 `bridge`) ;
// 2. `dictation.stop` : `transcribing`, `polishing`, le texte (`dictation.text`),
//    puis `idle` ; glossaire de la séance dans la consigne du nettoyage, après
//    le dictionnaire de l'utilisateur, qui reste inchangé en base ; ni
//    presse-papiers (le contenu tenu par une autre application reste), ni
//    « Last dictation » ;
// 3. Stop du bandeau : dictée abandonnée, l'extension apprend `idle` ;
// 4. `dictation.stop` juste après `dictation.start` : -32800 ; fin de la
//    connexion pendant l'écoute : micro fermé (dictée finie dans l'app).
import assert from "node:assert/strict";
import { mkdirSync, symlinkSync } from "node:fs";
import { setTimeout as delay } from "node:timers/promises";
import { ROOT, withApp } from "./lib/app.mjs";
import { BridgeClient, readDiscovery } from "./lib/bridge.mjs";
import { holdClipboard, readClipboard } from "./lib/clipboard.mjs";
import { startFake } from "./lib/fake.mjs";

const binary = process.argv[2];
const DATA = process.env.CHATONLM_DATA_DIR;
assert.ok(
  DATA,
  "CHATONLM_DATA_DIR absent : lancer dans scripts/e2e-session.sh",
);
const JFK = `${ROOT}.tools/speech-samples/jfk.wav`;
const TINY = `${ROOT}.tools/speech-models/whisper/ggml-tiny.bin`;
const SENTINEL = "copied by another application";

async function waitUntil(check, what, timeoutMs = 30000) {
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

/** Messages reçus hors réponses (notifications), dans l'ordre. */
const notes = (client, method) =>
  client.notifications.filter((message) => message.method === method);
const states = (client) =>
  notes(client, "dictation.state").map((message) => message.params.state);

mkdirSync(`${DATA}/speech-models/whisper-tiny`, { recursive: true });
symlinkSync(TINY, `${DATA}/speech-models/whisper-tiny/ggml-tiny.bin`);
const fake = await startFake("ollama");
const requests = async () =>
  (await fetch(new URL("/fake/requests", fake.url))).json();

try {
  await withApp(
    binary,
    {
      name: "vscode-dictation",
      env: {
        OLLAMA_HOST: fake.url,
        CHATONLM_E2E_MICROPHONE: JFK,
        CHATONLM_NO_GLOBAL_SHORTCUTS: "1",
      },
    },
    async (driver) => {
      await driver.waitFor("[data-testid=new-chat]");
      // Dictée coupée (raccourci) : l'éditeur peut quand même dicter.
      const settings = await driver.invoke("update_dictation_settings", {
        patch: {
          speechModel: "whisper-tiny",
          polishModel: "chatonlm-polish",
          dictionary: ["ChatonLM"],
        },
      });
      assert.equal(settings.enabled, false);
      // Copie et collage automatiques (réglages par défaut) : jamais pour
      // l'éditeur.
      assert.equal(settings.autoCopy, true);
      assert.equal(settings.autoPaste, true);
      const held = await holdClipboard({ text: SENTINEL });

      await driver.invoke("bridge_set_enabled", { enabled: true });
      const { pairing } = await driver.invoke("bridge_pair");
      const discovery = await waitUntil(readDiscovery, "bridge.json");
      const pairer = await BridgeClient.connect(discovery.endpoint);
      await pairer.initialize();
      const { clientId, secret } = await pairer.pair(pairing.code, "VS Code");
      await pairer.close();
      const connect = async () => {
        const client = await BridgeClient.connect(discovery.endpoint);
        await client.initialize();
        assert.ok((await client.auth(clientId, secret)).result);
        return client;
      };
      const client = await connect();
      const other = await connect();

      // 1. Dictée tenue par VS Code.
      const started = await client.request("dictation.start", {
        glossary: ["GpuChief", " chatonlm ", "fellow_americans"],
      });
      assert.deepEqual(started.result, {}, JSON.stringify(started));
      assert.deepEqual(states(client), ["recording"]);
      const status = await driver.invoke("dictation_status");
      assert.equal(status.editor, true);
      assert.equal(status.phase, "recording");
      await waitUntil(
        () =>
          driver.execute(
            `const b = document.querySelector("[data-testid=editor-voice-banner]");
             return !!b && b.textContent.includes("VS Code is using your microphone");`,
          ),
        "bandeau de la dictée de VS Code",
      );
      // L'essai de la page ne l'arrête pas.
      await driver.invoke("dictation_toggle");
      await driver.invoke("dictation_cancel");
      await delay(300);
      assert.equal(
        (await driver.invoke("dictation_status")).phase,
        "recording",
      );
      assert.deepEqual(states(client), ["recording"]);
      // Une seule dictée dans l'app.
      const busy = await other.request("dictation.start", {});
      assert.equal(busy.error?.code, -32032, JSON.stringify(busy));
      assert.equal(busy.error.data.owner, "bridge");
      assert.equal(busy.error.data.message, "VS Code is already dictating");

      // 2. Fin de l'enregistrement : le texte revient à l'éditeur.
      assert.deepEqual((await client.request("dictation.stop", {})).result, {});
      await waitUntil(
        () => states(client).includes("idle"),
        () => `fin de la dictée : ${JSON.stringify(client.notifications)}`,
        90000,
      );
      assert.deepEqual(states(client), [
        "recording",
        "transcribing",
        "polishing",
        "idle",
      ]);
      const texts = notes(client, "dictation.text");
      assert.equal(texts.length, 1, JSON.stringify(client.notifications));
      assert.match(texts[0].params.text, /^Polished: .*country/is);
      const order = client.notifications
        .filter((m) => m.method?.startsWith("dictation."))
        .map((m) => m.method);
      assert.equal(order.at(-1), "dictation.state", "idle comes last");
      assert.equal(order.at(-2), "dictation.text");
      assert.equal(notes(client, "dictation.error").length, 0);
      const polish = (await requests())
        .filter((request) => request.model === "chatonlm-polish")
        .at(-1);
      assert.match(
        polish.messages[0].content,
        /Spell these words exactly as written: ChatonLM, GpuChief, fellow_americans\./,
      );
      const after = await driver.invoke("get_dictation_settings");
      assert.deepEqual(after.dictionary, ["ChatonLM"], "dictionary untouched");
      const done = await driver.invoke("dictation_status");
      assert.equal(done.editor, false);
      assert.equal(done.phase, "idle");
      assert.equal(done.lastResult, null, "no Last dictation");
      assert.equal((await readClipboard()).text, SENTINEL, "never copied");
      await held.release();

      // 3. Stop du bandeau.
      client.notifications.length = 0;
      assert.deepEqual(
        (await client.request("dictation.start", {})).result,
        {},
      );
      await waitUntil(
        () =>
          driver.execute(
            `return !!document.querySelector("[data-testid=editor-voice-banner] button");`,
          ),
        "bouton Stop du bandeau",
      );
      await driver.execute(
        `document.querySelector("[data-testid=editor-voice-banner] button").click();`,
      );
      await waitUntil(
        () => states(client).includes("idle"),
        "l'extension apprend la fin",
      );
      assert.equal(notes(client, "dictation.text").length, 0);
      await waitUntil(
        () =>
          driver.execute(
            `return !document.querySelector("[data-testid=editor-voice-banner]");`,
          ),
        "bandeau retiré",
      );
      assert.equal((await driver.invoke("dictation_status")).phase, "idle");

      // 4. Arrêt pendant le démarrage, puis fin de la connexion.
      client.notifications.length = 0;
      const [early, stopped] = await Promise.all([
        client.request("dictation.start", {}),
        client.request("dictation.stop", {}),
      ]);
      assert.deepEqual(stopped.result, {});
      assert.equal(early.error?.code, -32800, JSON.stringify(early));
      await delay(500);
      assert.equal((await driver.invoke("dictation_status")).phase, "idle");
      assert.deepEqual(states(client), [], "nothing told");

      assert.deepEqual((await other.request("dictation.start", {})).result, {});
      await other.close();
      await waitUntil(
        async () => (await driver.invoke("dictation_status")).phase === "idle",
        "micro fermé à la fin de la connexion",
        10000,
      );
      await client.close();
    },
  );
} finally {
  await fake.stop();
}
console.log("vscode-dictation : OK");
