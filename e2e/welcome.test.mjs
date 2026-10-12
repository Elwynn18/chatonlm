// Écran d'accueil, port local non standard : Ollama installé (faits de la
// machine imposés), rien n'écoute ; l'écran donne la commande avec
// OLLAMA_HOST, puis bascule tout seul vers l'app dès qu'un serveur Ollama
// démarre à cette adresse. Une fois l'app affichée, elle y reste.
import assert from "node:assert/strict";
import { freePort, withApp } from "./lib/app.mjs";
import { startFake } from "./lib/fake.mjs";

const binary = process.argv[2];
const port = await freePort();

await withApp(
  binary,
  {
    name: "welcome",
    env: {
      OLLAMA_HOST: `127.0.0.1:${port}`,
      CHATONLM_E2E_HOST_FACTS: "binary=yes;unit=none",
    },
  },
  async (driver) => {
    const title = await driver.waitFor("h1");
    assert.equal(
      await driver.text(title),
      "Ollama is not running at this address",
    );
    const commands = await driver.execute(
      "return [...document.querySelectorAll('code')].map((c) => c.textContent)",
    );
    assert.deepEqual(commands, [`OLLAMA_HOST=127.0.0.1:${port} ollama serve`]);
    const placeholder = await driver.execute(
      "return document.querySelector('#server-url').placeholder",
    );
    assert.equal(placeholder, `http://127.0.0.1:${port}`);
    await driver.screenshot(".tools/shots/welcome-not-running.png");

    // Revérification automatique : pas de clic sur Retry.
    const fake = await startFake("ollama", port);
    try {
      await driver.waitFor("[data-testid=new-chat]", { timeoutMs: 10000 });
      const settings = await driver.invoke("get_settings");
      assert.equal(settings.effectiveServerUrl, `http://127.0.0.1:${port}`);
    } finally {
      await fake.stop();
    }

    // Serveur arrêté après coup : l'app reste affichée.
    await new Promise((resolve) => setTimeout(resolve, 3500));
    await driver.find("[data-testid=new-chat]");
  },
);
