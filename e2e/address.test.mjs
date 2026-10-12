// Écran d'accueil : l'adresse du serveur se corrige sur place. Une adresse
// invalide est refusée avec un message ; une bonne adresse connecte l'app et
// reste enregistrée.
import assert from "node:assert/strict";
import { freePort, withApp } from "./lib/app.mjs";
import { startFake } from "./lib/fake.mjs";

const binary = process.argv[2];
const closed = await freePort();
const fake = await startFake("ollama");

try {
  await withApp(
    binary,
    { name: "address", env: { OLLAMA_HOST: `127.0.0.1:${closed}` } },
    async (driver) => {
      const input = await driver.waitFor("#server-url");
      const save = await driver.find("form button[type=submit]");

      await driver.clear(input);
      await driver.type(input, "ftp://example.com");
      await driver.click(save);
      const alert = await driver.waitFor("[role=alert]");
      assert.match(await driver.text(alert), /unsupported scheme ftp/);

      await driver.clear(input);
      await driver.type(input, fake.url);
      await driver.click(save);
      await driver.waitFor("[data-testid=new-chat]", { timeoutMs: 10000 });
      const settings = await driver.invoke("get_settings");
      assert.equal(settings.serverUrl, fake.url);
      assert.equal(settings.effectiveServerUrl, fake.url);
    },
  );
} finally {
  await fake.stop();
}
