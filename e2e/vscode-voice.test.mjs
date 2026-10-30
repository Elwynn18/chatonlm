// Voix pilotée par l'extension VS Code (lot 2) dans l'app réelle : faux
// serveur Ollama, vrai Parakeet et vraie voix française (liens vers .tools),
// sortie audio factice au rythme réel (CHATONLM_E2E_AUDIO_OUT=null), faux micro
// (CHATONLM_E2E_VOICE_MIC), et un client Node qui fait ce que fera
// l'extension (e2e/lib/bridge.mjs) :
// 1. tour.plan (chatonlm-tour) : segments liés aux lignes du code donné ;
// 2. speech.read : début et fin de chaque segment dans l'ordre, calés sur le
//    son joué (la fin du dernier segment vient au moins le temps de dire les
//    deux textes après le premier début) ; speech.stop : « stopped » ;
// 3. séance « duck » tenue par VS Code : la page la voit (bouton inactif « Voice
//    is in use by VS Code ») sans pouvoir l'arrêter ni en lancer une ; la
//    phrase du faux micro est transcrite, envoyée par le moteur (consigne du
//    profil et contexte de l'éditeur en message système), la réponse est lue
//    (Speaking), la conversation est marquée « VS Code » ;
// 4. bandeau « VS Code is using your microphone » sur la page pendant la
//    séance ; `voice.resume` servi (sans effet hors pause) ; Stop du bandeau →
//    séance arrêtée, l'extension reçoit `off` ;
// 5. `voice.stop` envoyé juste après `voice.start` : démarrage arrêté
//    (-32800), le micro ne s'ouvre pas ; une nouvelle séance reprend la
//    numérotation des tours de la connexion ; fin de la connexion → séance
//    arrêtée (micro fermé).
import assert from "node:assert/strict";
import { existsSync } from "node:fs";
import { setTimeout as delay } from "node:timers/promises";
import { ROOT, withApp } from "./lib/app.mjs";
import { BridgeClient, readDiscovery } from "./lib/bridge.mjs";
import { startFake } from "./lib/fake.mjs";
import {
  PARAKEET,
  PHRASES,
  heard,
  linkParakeet,
  linkVoice,
  writePhrase,
} from "./lib/voice.mjs";

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
const TMP = `${ROOT}.tools/tmp/vscode-voice-${process.pid}`;
/** Textes français de chatonlm-tour (TOUR_TEXTS de fake-ollama). */
const TOUR_TEXTS = [
  "Cette première ligne plante le décor.",
  "La suite du code fait le vrai travail.",
];
const CODE = 'use std::io;\nfn main() {\n    println!("bonjour");\n}';

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

/** Notifications `method` reçues, avec l'heure de leur arrivée. */
function record(client) {
  const seen = [];
  const push = client.notifications.push.bind(client.notifications);
  client.notifications.push = (...messages) => {
    for (const message of messages) {
      seen.push({ at: Date.now(), message });
    }
    return push(...messages);
  };
  return (method) =>
    seen
      .filter(({ message }) => message.method === method)
      .map(({ at, message }) => ({ at, ...message.params }));
}

const fake = await startFake("ollama");
const requests = async () =>
  (await fetch(new URL("/fake/requests", fake.url))).json();

try {
  linkParakeet(DATA);
  linkVoice(DATA, "pocket-tts-fr");
  const phrase = writePhrase(PHRASES.first, TMP);
  await withApp(
    binary,
    {
      name: "vscode-voice",
      env: {
        OLLAMA_HOST: fake.url,
        CHATONLM_NO_GLOBAL_SHORTCUTS: "1",
        CHATONLM_E2E_VOICE_MIC: JSON.stringify([
          { on: "listening", wav: phrase.wav, delayMs: 300 },
        ]),
      },
    },
    async (driver) => {
      await driver.waitFor("[data-testid=new-chat]");
      await driver.invoke("update_dictation_settings", {
        patch: { speechModel: PARAKEET, language: "fr" },
      });
      await driver.invoke("update_settings", {
        patch: { selectedModel: "chatonlm-voice" },
      });

      // Pont ouvert, éditeur appairé (code demandé au moteur), connexion
      // authentifiée.
      await driver.invoke("bridge_set_enabled", { enabled: true });
      const { pairing } = await driver.invoke("bridge_pair");
      const discovery = await waitUntil(readDiscovery, "bridge.json");
      const pairer = await BridgeClient.connect(discovery.endpoint);
      await pairer.initialize();
      const { clientId, secret } = await pairer.pair(pairing.code, "VS Code");
      await pairer.close();
      const client = await BridgeClient.connect(discovery.endpoint);
      const events = record(client);
      await client.initialize();
      assert.ok((await client.auth(clientId, secret)).result);

      // 1. Plan de la visite.
      const plan = await client.request("tour.plan", {
        model: "chatonlm-tour",
        locale: "fr",
        path: "src/main.rs",
        language: "rust",
        code: CODE,
        firstLine: 20,
        kind: "file",
      });
      assert.deepEqual(
        plan.result,
        {
          segments: [
            { lines: [20, 20], text: TOUR_TEXTS[0] },
            { lines: [21, 23], text: TOUR_TEXTS[1] },
          ],
        },
        JSON.stringify(plan),
      );

      // 2. Lecture par segments, calée sur le son joué.
      const segments = plan.result.segments.map((segment, index) => ({
        id: `s${index}`,
        text: segment.text,
      }));
      const asked = Date.now();
      const read = await client.request("speech.read", {
        segments,
        language: "fr",
      });
      const readId = read.result?.readId;
      assert.ok(readId, JSON.stringify(read));
      await waitUntil(
        () =>
          events("speech.event").some(
            (event) => event.type === "finished" && event.segmentId === "s1",
          ) || events("speech.event").some((e) => e.type === "error"),
        "lecture des deux segments",
        60000,
      );
      const spoken = events("speech.event").filter((e) => e.readId === readId);
      assert.deepEqual(
        spoken.map((event) => `${event.type} ${event.segmentId}`),
        ["started s0", "finished s0", "started s1", "finished s1"],
        JSON.stringify(spoken),
      );
      // Deux phrases dites au rythme réel : plus de 2 s entre le premier son
      // et la fin (la synthèse, elle, va bien plus vite).
      const played = spoken[3].at - spoken[0].at;
      assert.ok(played > 2000, `segments joués en ${played} ms`);
      assert.ok(spoken[2].at - spoken[1].at < 500, "s1 suit s0");
      assert.ok(spoken[0].at - asked > 200, "début après le chargement");
      // Arrêt d'une lecture en cours.
      const long = await client.request("speech.read", {
        segments: [{ id: "long", text: TOUR_TEXTS.join(" ").repeat(4) }],
        language: "fr",
      });
      await waitUntil(
        () =>
          events("speech.event").some(
            (e) => e.readId === long.result.readId && e.type === "started",
          ),
        "début de la longue lecture",
      );
      assert.deepEqual(
        (await client.request("speech.stop", { readId: long.result.readId }))
          .result,
        {},
      );
      await waitUntil(
        () =>
          events("speech.event").some(
            (e) => e.readId === long.result.readId && e.type === "stopped",
          ),
        "lecture arrêtée",
      );

      // 3. Séance « duck » tenue par VS Code.
      const started = await client.request("voice.start", {
        profile: "duck",
        model: "chatonlm-voice",
        context: "src/main.rs\nfn main() { panic!() }",
      });
      assert.ok(started.result?.sessionId, JSON.stringify(started));
      assert.equal(started.result.chatId, null);
      await waitUntil(
        async () => (await driver.invoke("voice_mode_state")).editor,
        "séance tenue par VS Code, vue par la page",
      );
      // La page : bouton inactif qui le dit ; ni arrêt ni lancement.
      await waitUntil(
        () =>
          driver.execute(
            `const b = document.querySelector("[data-voice-mode-start]");
             return b && b.disabled && b.getAttribute("aria-label") === "Voice is in use by VS Code";`,
          ),
        "bouton du mode vocal inactif",
      );
      await driver.invoke("voice_mode_stop");
      assert.notEqual((await driver.invoke("voice_mode_state")).phase, "off");
      const refused = await driver.executeAsync(
        `const done = arguments[arguments.length - 1];
         window.__TAURI_INTERNALS__.invoke("voice_mode_start", {})
           .then(() => done(null), (failure) => done(JSON.stringify(failure)));`,
      );
      assert.match(refused ?? "", /VS Code/);

      // Phrase du faux micro : transcrite, envoyée, réponse lue.
      const done = await waitUntil(
        () =>
          events("voice.chat").find(
            (event) => event.turn === 1 && event.event.type === "done",
          ),
        "réponse du tour 1",
        90000,
      );
      assert.ok(done);
      const turn = events("voice.chat").filter((event) => event.turn === 1);
      const created = turn.find((event) => event.event.type === "chatCreated");
      const user = turn.find((event) => event.event.type === "userMessage");
      assert.ok(created && user, JSON.stringify(turn.map((e) => e.event.type)));
      assert.ok(
        heard(user.event.message.content, PHRASES.first),
        user.event.message.content,
      );
      await waitUntil(
        () =>
          events("voice.state").some(
            (event) => event.state.phase === "speaking",
          ),
        "réponse lue à voix haute",
      );
      await waitUntil(
        () => {
          const states = events("voice.state");
          const speaking = states.findIndex(
            (event) => event.state.phase === "speaking",
          );
          return states
            .slice(speaking)
            .some((event) => event.state.phase === "listening");
        },
        "retour à l'écoute",
        60000,
      );
      assert.ok(events("voice.level").length > 0, "niveaux du micro");
      const sent = (await requests())
        .filter((request) => request.model === "chatonlm-voice")
        .at(-1);
      const system = sent.messages[0];
      assert.equal(system.role, "system");
      assert.match(system.content, /rubber duck/);
      assert.match(system.content, /panic!\(\)/);
      assert.equal(sent.think, false);
      const listed = await client.request("chat.list");
      assert.ok(
        listed.result.some((chat) => chat.id === created.event.chatId),
        "conversation marquée VS Code",
      );

      // 4. Bandeau de la page, reprise sans pause, Stop du bandeau.
      await waitUntil(
        () =>
          driver.execute(
            `const b = document.querySelector("[data-testid=editor-voice-banner]");
             return !!b && b.textContent.includes("VS Code is using your microphone");`,
          ),
        "bandeau de la séance de VS Code",
      );
      assert.deepEqual(
        (await client.request("voice.resume", {})).result,
        {},
        "voice.resume servi",
      );
      assert.notEqual((await driver.invoke("voice_mode_state")).phase, "off");
      const offBefore = events("voice.state").filter(
        (event) => event.state.phase === "off",
      ).length;
      await driver.execute(
        `document.querySelector("[data-testid=editor-voice-banner] button").click();`,
      );
      await waitUntil(
        async () => (await driver.invoke("voice_mode_state")).phase === "off",
        "séance arrêtée par le bandeau",
        10000,
      );
      await waitUntil(
        () =>
          events("voice.state").filter((event) => event.state.phase === "off")
            .length > offBefore,
        "l'extension apprend la fin",
      );
      await waitUntil(
        () =>
          driver.execute(
            `return !document.querySelector("[data-testid=editor-voice-banner]");`,
          ),
        "bandeau retiré",
      );

      // 5. Arrêt pendant le démarrage : rien ne s'ouvre.
      const starting = client.request("voice.start", {
        profile: "duck",
        model: "chatonlm-voice",
      });
      const stopping = client.request("voice.stop", {});
      const cancelled = await starting;
      assert.equal(cancelled.error?.code, -32800, JSON.stringify(cancelled));
      assert.deepEqual((await stopping).result, {});
      await delay(1500);
      assert.equal((await driver.invoke("voice_mode_state")).phase, "off");
      // Nouvelle séance : ses tours suivent ceux de la connexion.
      const statesBefore = events("voice.state").length;
      const again = await client.request("voice.start", {
        profile: "duck",
        model: "chatonlm-voice",
      });
      assert.ok(again.result?.sessionId, JSON.stringify(again));
      const restarted = events("voice.state")
        .slice(statesBefore)
        .find((event) => event.state.phase === "starting");
      assert.equal(restarted?.state.turn, 1, JSON.stringify(restarted));

      // Fin de la connexion : séance arrêtée.
      await client.close();
      await waitUntil(
        async () => (await driver.invoke("voice_mode_state")).phase === "off",
        "séance arrêtée à la fin de la connexion",
        10000,
      );
      assert.equal((await driver.invoke("voice_mode_state")).editor, false);
    },
  );
  console.log("✓ voix pilotée par VS Code");
} finally {
  await fake.stop();
}
