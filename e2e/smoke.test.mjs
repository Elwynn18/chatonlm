// Test de fumée : avec le moteur intégré (faux llama-server) et un modèle
// installé, l'app s'ouvre sur une nouvelle conversation (logo et zone de
// saisie) dans une fenêtre visible.
import assert from "node:assert/strict";
import { withApp } from "./lib/app.mjs";
import { fakeModel, startEngine } from "./lib/fake.mjs";

const binary = process.argv[2];
const MODEL = fakeModel("simple");
const engine = startEngine([MODEL]);
await withApp(binary, { name: "smoke", env: engine.env }, async (driver) => {
  await driver.waitFor("[data-testid=new-chat]");
  const placeholder = await driver.execute(
    "return document.querySelector('textarea')?.placeholder",
  );
  assert.equal(placeholder, "Send a message");
  const visible = await driver.execute(
    "return document.visibilityState === 'visible'",
  );
  assert.equal(visible, true, "la fenêtre devrait être affichée");

  const status = await driver.invoke("engine_status");
  assert.equal(status.available, true);
  const models = await driver.invoke("list_models");
  assert.deepEqual(
    models.map((model) => model.id),
    [MODEL],
  );
  await driver.screenshot(".tools/shots/smoke.png");
});
