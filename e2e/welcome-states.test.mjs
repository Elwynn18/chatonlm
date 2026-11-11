// États du moteur au démarrage (moteur intégré, plus d'écran d'installation
// d'Ollama) : l'app s'ouvre toujours sur le chat.
// - Moteur absent de l'installation (`CHATONLM_LLAMA_SERVER_BIN` vide : aucun
//   binaire) : un envoi finit par une erreur lisible (la zone de saisie ne
//   reste pas bloquée), et Settings › Engine dit de réinstaller.
// - Moteur présent, aucun modèle : rien d'inscrit, l'écran du moteur dit
//   l'accélération, sans alerte.
// - Moteur présent et un modèle : la réponse arrive du faux llama-server
//   (chaîne complète app → moteur → processus → flux → page).
import assert from "node:assert/strict";
import { dirname, join } from "node:path";
import { withApp } from "./lib/app.mjs";
import { fakeModel, startEngine } from "./lib/fake.mjs";
import { pressKeys } from "./lib/keyboard.mjs";

const binary = process.argv[2];
const MODEL = fakeModel("simple");
const engine = startEngine([MODEL]);

async function waitUntil(check, what, timeoutMs = 10000) {
  const deadline = Date.now() + timeoutMs;
  for (;;) {
    const value = await check();
    if (value) return value;
    if (Date.now() > deadline) throw new Error(`Délai dépassé : ${what}`);
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
}

const texts = (driver, css) =>
  driver.execute(
    "return [...document.querySelectorAll(arguments[0])].map((e) => e.textContent)",
    [css],
  );

/** Tape un message et l'envoie avec Entrée. */
async function send(driver, text) {
  const textarea = await driver.waitFor("textarea[aria-label='Message']");
  await driver.type(textarea, text);
  await driver.execute(
    `document.querySelector("textarea[aria-label='Message']").dispatchEvent(
       new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true }));`,
  );
}

/**
 * Ouvre les réglages (Ctrl+,) et rend le texte de la ligne Engine (Row de
 * settings/Engine.tsx, autour de son bouton « Open logs »).
 */
async function engineRow(driver) {
  await pressKeys("ctrl+comma");
  await driver.waitFor("[data-testid=settings]");
  return waitUntil(
    () =>
      driver.execute(
        `const button = [...document.querySelectorAll("[data-testid=settings] button")]
           .find((b) => b.textContent === "Open logs");
         return button?.closest("div.w-full")?.textContent ?? null;`,
      ),
    "ligne Engine des réglages",
  );
}

// Moteur absent : un modèle est inscrit, mais aucun binaire.
await withApp(
  binary,
  {
    name: "welcome-engine-missing",
    env: { ...engine.env, CHATONLM_LLAMA_SERVER_BIN: "" },
  },
  async (driver) => {
    await driver.waitFor("[data-testid=new-chat]");
    const status = await driver.invoke("engine_status");
    assert.equal(status.available, false);
    await driver.invoke("update_settings", { patch: { selectedModel: MODEL } });
    await send(driver, "hi");
    await waitUntil(
      async () =>
        (await texts(driver, "[role=alert]")).some((text) => text.trim()),
      "erreur du moteur absent",
    );
    await waitUntil(
      () =>
        driver.execute(
          "return document.querySelector(\"textarea[aria-label='Message']\")?.disabled === false",
        ),
      "zone de saisie de nouveau active",
    );
    const row = await engineRow(driver);
    assert.match(
      row,
      /The engine is missing from this installation\. Reinstall ChatonLM\./,
    );
    await driver.screenshot(".tools/shots/welcome-engine-missing.png");
  },
);
console.log(
  "moteur absent : erreur à l'envoi, réglages qui disent de réinstaller",
);

// Moteur présent, aucun modèle inscrit : dossier de données neuf (celui de
// la session garde le modèle inscrit par le lancement précédent).
const emptyData = join(
  dirname(process.env.CHATONLM_DATA_DIR ?? "."),
  "data-no-model",
);
await withApp(
  binary,
  { name: "welcome-no-model", env: { CHATONLM_DATA_DIR: emptyData } },
  async (driver) => {
    await driver.waitFor("[data-testid=new-chat]");
    const status = await driver.invoke("engine_status");
    assert.equal(status.available, true);
    assert.deepEqual(await driver.invoke("list_models"), []);
    await engineRow(driver);
    assert.ok(await driver.find("[data-testid=engine-acceleration]"));
    assert.deepEqual(
      (await texts(driver, "[data-testid=settings] [role=alert]")).filter(
        Boolean,
      ),
      [],
    );
    await driver.screenshot(".tools/shots/welcome-no-model.png");
  },
);
console.log("moteur présent, aucun modèle : accélération dite, pas d'alerte");

// Moteur présent et un modèle : réponse du faux llama-server.
await withApp(
  binary,
  { name: "welcome-model", env: engine.env },
  async (driver) => {
    await driver.waitFor("[data-testid=new-chat]");
    const models = await driver.invoke("list_models");
    assert.deepEqual(
      models.map((model) => [model.id, model.origin]),
      [[MODEL, "hf"]],
    );
    await driver.invoke("update_settings", { patch: { selectedModel: MODEL } });
    await send(driver, "hi");
    await waitUntil(
      async () =>
        (await texts(driver, "[data-role=assistant]")).some((text) =>
          text.includes("Hello! You said: hi"),
        ),
      "réponse du faux moteur",
    );
    const requests = await engine.chatRequests(MODEL);
    assert.equal(requests.length, 1);
    assert.equal(requests[0].stream, true);
  },
);
console.log("moteur présent et un modèle : réponse reçue");
console.log("welcome-states : OK");
