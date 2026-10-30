// Chaton en caoutchouc dans un vrai éditeur (VSCodium, e2e/lib/vscodium.mjs)
// face à l'app réelle (build e2e), au faux Ollama (chatonlm-voice), à la vraie
// transcription (Parakeet) et au faux micro de l'app :
// - « Talk it through » dans le panneau : l'app ouvre son micro pour VS Code
//   (séance marquée « editor ») ; la phrase du faux micro est transcrite,
//   envoyée avec la consigne socratique et le passage ouvert dans l'éditeur ;
//   la conversation, marquée « VS Code », reçoit la réponse ; capture
//   (.tools/shots/vscode-duck.png) ;
// - VSCodium fermé : la séance vocale de l'app s'arrête (micro libéré).
// Prérequis : builds de l'extension (vscode/dist), Parakeet et voix de
// lecture (.tools), ORT_DYLIB_PATH.
import assert from "node:assert/strict";
import { existsSync } from "node:fs";
import { withApp } from "./lib/app.mjs";
import { findDiscovery } from "./lib/bridge.mjs";
import { startFake } from "./lib/fake.mjs";
import {
  PARAKEET,
  PHRASES,
  linkParakeet,
  linkVoice,
  writePhrase,
} from "./lib/voice.mjs";
import { ROOT, prepare, shot, waitUntil } from "./lib/vscodium.mjs";

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
const delay = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

const fake = await startFake("ollama");
const codium = prepare("vscode-duck", {
  "sample.js": "function add(a, b) {\n  return a - b;\n}\n",
});
linkParakeet(DATA);
for (const voice of ["pocket-tts-en", "pocket-tts-fr", "pocket-tts-es"]) {
  linkVoice(DATA, voice);
}
const phrase = writePhrase(
  PHRASES.first,
  `${ROOT}.tools/tmp/vscode-duck-${process.pid}`,
);

try {
  await withApp(
    binary,
    {
      name: "vscode-duck",
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
      await driver.invoke("bridge_set_enabled", { enabled: true });
      const { pairing } = await driver.invoke("bridge_pair");
      const found = await waitUntil(findDiscovery, "bridge.json");
      const exited = codium.start({
        bridgeDir: found.dir,
        scenario: "duck",
        env: { CHATONLM_TEST_PAIR_CODE: pairing.code },
      });

      let verdict = "ok";
      try {
        await waitUntil(
          () => codium.step() === "talking",
          "séance lancée",
          90000,
        );
        await waitUntil(
          async () => (await driver.invoke("voice_mode_state")).editor,
          "séance tenue par VS Code",
        );
        const chat = await waitUntil(
          async () => {
            const chats = await driver.invoke("list_chats");
            return chats.find((c) => c.source === "vscode");
          },
          "conversation marquée VS Code",
          90000,
        );
        const read = await waitUntil(
          async () => {
            const full = await driver.invoke("get_chat", { id: chat.id });
            return full.messages.length >= 2 && full.messages[1].content
              ? full
              : null;
          },
          "réponse du modèle enregistrée",
          90000,
        );
        assert.ok(read.messages[0].content.length > 0, "phrase transcrite");
        const sent = (
          await (await fetch(new URL("/fake/requests", fake.url))).json()
        )
          .filter((request) => request.model === "chatonlm-voice")
          .at(-1);
        assert.equal(sent.messages[0].role, "system");
        // Passage ouvert dans l'éditeur, joint en contexte.
        assert.match(sent.messages[0].content, /return a - b/);
        await delay(3000);
        await shot("vscode-duck");
      } catch (error) {
        verdict = String(error?.stack ?? error);
      }
      codium.done(verdict);
      const code = await exited();
      assert.equal(verdict, "ok");
      assert.equal(code, 0, "VSCodium exited with an error (see its log)");
      // Éditeur fermé : micro libéré.
      await waitUntil(
        async () => (await driver.invoke("voice_mode_state")).phase === "off",
        "séance arrêtée à la fermeture de l'éditeur",
        20000,
      );
    },
  );
  console.log("✓ vscode-duck");
} finally {
  await fake.stop();
  codium.finish();
}
