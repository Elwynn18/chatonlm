// Test de fumée : avec un serveur Ollama qui répond, l'app s'ouvre sur une
// nouvelle conversation (logo et zone de saisie) dans une fenêtre visible.
import assert from "node:assert/strict";
import { withApp } from "./lib/app.mjs";
import { startFake } from "./lib/fake.mjs";

const binary = process.argv[2];
const fake = await startFake("ollama");
try {
  await withApp(
    binary,
    { name: "smoke", env: { OLLAMA_HOST: fake.url } },
    async (driver) => {
      await driver.waitFor("[data-testid=new-chat]");
      const placeholder = await driver.execute(
        "return document.querySelector('textarea')?.placeholder",
      );
      assert.equal(placeholder, "Send a message");
      const visible = await driver.execute(
        "return document.visibilityState === 'visible'",
      );
      assert.equal(visible, true, "la fenêtre devrait être affichée");

      const status = await driver.invoke("server_status");
      assert.equal(status.state, "running");
      assert.equal(status.version, "0.35.1");
      await driver.screenshot(".tools/shots/smoke.png");
    },
  );
} finally {
  await fake.stop();
}
