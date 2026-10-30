// L'extension dans un vrai éditeur (VSCodium, e2e/lib/vscodium.mjs) face à
// l'app réelle (build e2e) et au faux Ollama :
// - l'app ouvre le pont et montre un code ; la commande « ChatonLM: Pair »
//   l'emploie, l'éditeur apparaît connecté dans l'app ;
// - « Explain » sur la sélection d'un fichier du dossier ouvert : la
//   conversation arrive dans l'app, marquée « VS Code », avec la réponse du
//   modèle ; capture du panneau (.tools/shots/vscode-panel.png) ;
// - VSCodium se ferme sans erreur (l'essai intérieur rend son verdict).
// Prérequis : builds de l'extension (vscode/dist).
import assert from "node:assert/strict";
import { withApp } from "./lib/app.mjs";
import { findDiscovery } from "./lib/bridge.mjs";
import { startFake } from "./lib/fake.mjs";
import { prepare, shot, waitUntil } from "./lib/vscodium.mjs";

const binary = process.argv[2];
const MODEL = "qwen3:0.6b";
const delay = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

const fake = await startFake("ollama");
const codium = prepare("vscode-extension", {
  "sample.js": "function add(a, b) {\n  return a + b;\n}\n",
});

try {
  await withApp(
    binary,
    { name: "vscode-extension", env: { OLLAMA_HOST: fake.url } },
    async (driver) => {
      await driver.waitFor("[data-testid=new-chat]");
      await driver.invoke("update_settings", {
        patch: { selectedModel: MODEL },
      });
      await driver.invoke("bridge_set_enabled", { enabled: true });
      const { pairing } = await driver.invoke("bridge_pair");
      const found = await waitUntil(findDiscovery, "bridge.json");
      const exited = codium.start({
        bridgeDir: found.dir,
        scenario: "explain",
        env: { CHATONLM_TEST_PAIR_CODE: pairing.code },
      });

      let verdict = "ok";
      try {
        await waitUntil(
          async () =>
            (await driver.invoke("bridge_state")).clients.some(
              (client) => client.connected,
            ),
          "éditeur connecté dans l'app",
        );
        const chat = await waitUntil(async () => {
          const chats = await driver.invoke("list_chats");
          return chats.find((c) => c.source === "vscode");
        }, "conversation marquée VS Code");
        assert.match(chat.userExcerpt, /add/);
        const read = await waitUntil(async () => {
          const full = await driver.invoke("get_chat", { id: chat.id });
          return full.messages.length === 2 && full.messages[1].content
            ? full
            : null;
        }, "réponse du modèle enregistrée");
        assert.match(read.messages[0].content, /return a \+ b/);
        assert.match(read.messages[0].content, /sample\.js/);
        await waitUntil(() => codium.step() === "explained", "panneau ouvert");
        // Panneau ouvert, réponse affichée.
        await delay(8000);
        await shot("vscode-panel");
      } catch (error) {
        verdict = String(error?.stack ?? error);
      }
      codium.done(verdict);
      const code = await exited();
      assert.equal(verdict, "ok");
      assert.equal(code, 0, "VSCodium exited with an error (see its log)");
    },
  );
  console.log("✓ vscode-extension");
} finally {
  await fake.stop();
  codium.finish();
}
