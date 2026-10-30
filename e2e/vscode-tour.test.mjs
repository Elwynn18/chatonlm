// Visite guidée parlée dans un vrai éditeur (VSCodium, e2e/lib/vscodium.mjs)
// face à l'app réelle (build e2e), au faux Ollama (modèle chatonlm-tour : plan
// en JSON) et à la vraie lecture à voix haute (voix de .tools/voice-models,
// sortie factice au rythme réel) :
// - « ChatonLM: Walk me through this » sur un fichier : l'app produit le plan,
//   le lit segment par segment ; capture de la première partie (lignes
//   surlignées, barre d'état, carte de la visite dans le panneau :
//   .tools/shots/vscode-tour.png) ;
// - pause sur la première partie (la lecture de l'app s'arrête, capture),
//   partie suivante, puis arrêt (la lecture de l'app s'arrête) ;
// - le plan est bien parti au modèle avec le code et ses lignes.
// Prérequis : builds de l'extension (vscode/dist), voix de lecture.
import assert from "node:assert/strict";
import { withApp } from "./lib/app.mjs";
import { findDiscovery } from "./lib/bridge.mjs";
import { startFake } from "./lib/fake.mjs";
import { linkVoice } from "./lib/voice.mjs";
import { chooseModel, prepare, shot, waitUntil } from "./lib/vscodium.mjs";

const binary = process.argv[2];
const DATA = process.env.CHATONLM_DATA_DIR;
assert.ok(
  DATA,
  "CHATONLM_DATA_DIR absent : lancer dans scripts/e2e-session.sh",
);
const delay = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

const fake = await startFake("ollama");
const codium = prepare("vscode-tour", {
  "sample.rs":
    'use std::io;\nfn main() {\n    println!("bonjour");\n    let mut line = String::new();\n    io::stdin().read_line(&mut line).unwrap();\n}\n',
});
for (const voice of ["pocket-tts-en", "pocket-tts-fr", "pocket-tts-es"]) {
  linkVoice(DATA, voice);
}

try {
  await withApp(
    binary,
    { name: "vscode-tour", env: { OLLAMA_HOST: fake.url } },
    async (driver) => {
      await driver.waitFor("[data-testid=new-chat]");
      await chooseModel(driver, "chatonlm-tour");
      await driver.invoke("bridge_set_enabled", { enabled: true });
      const { pairing } = await driver.invoke("bridge_pair");
      const found = await waitUntil(findDiscovery, "bridge.json");
      const exited = codium.start({
        bridgeDir: found.dir,
        scenario: "tour",
        env: { CHATONLM_TEST_PAIR_CODE: pairing.code },
      });

      let verdict = "ok";
      try {
        await waitUntil(
          () => codium.step() === "touring",
          "visite lancée",
          90000,
        );
        // Le plan est parti au modèle avec le code.
        const requests = await (
          await fetch(new URL("/fake/requests", fake.url))
        ).json();
        const plan = requests.find((r) => r.model === "chatonlm-tour");
        assert.ok(plan, "plan demandé au modèle");
        assert.match(JSON.stringify(plan.messages), /read_line/);
        assert.ok(plan.format, "sortie JSON contrainte");
        // L'app lit la visite (première partie), puis pause.
        await waitUntil(
          async () =>
            (await driver.invoke("read_aloud_state"))?.phase === "playing",
          "lecture en cours dans l'app",
          30000,
        );
        codium.ask("pause");
        await waitUntil(() => codium.step() === "paused", "visite en pause");
        await waitUntil(
          async () =>
            (await driver.invoke("read_aloud_state"))?.phase !== "playing",
          "lecture suspendue dans l'app",
          15000,
        );
        // Capture : partie 1 sur 2 surlignée, carte de la visite, barre d'état.
        await delay(1500);
        await shot("vscode-tour");
        codium.ask("next");
        await waitUntil(() => codium.step() === "next", "partie suivante");
        await delay(1500);
        codium.ask("stop");
        await waitUntil(() => codium.step() === "stopped", "visite arrêtée");
        await waitUntil(
          async () =>
            (await driver.invoke("read_aloud_state"))?.phase !== "playing",
          "lecture arrêtée dans l'app",
          15000,
        );
      } catch (error) {
        verdict = String(error?.stack ?? error);
      }
      codium.done(verdict);
      const code = await exited();
      assert.equal(verdict, "ok");
      assert.equal(code, 0, "VSCodium exited with an error (see its log)");
    },
  );
  console.log("✓ vscode-tour");
} finally {
  await fake.stop();
  codium.finish();
}
