// Sélecteur de modèles sur le moteur intégré : modèles recommandés de
// ChatonLM (hub/curation.json, identifiants de Hugging Face) en tête, puis
// les modèles installés (base locale, du plus récent au plus ancien) ; modèle
// par défaut = le plus récemment installé, jamais un recommandé ; recherche,
// choix mémorisé, conversation d'avant la 0.10 dont l'ancien nom d'Ollama
// n'a pas été repris (src/lib/models.ts, `restoredModel` : « legacy ») : le
// modèle choisi reste, l'ancien nom n'est pas proposé ; aucun modèle
// installé (recommandés seulement, aucun choisi) ; mémoire demandée et
// pastille des recommandés non téléchargés (ancien e2e cloud-models). Lignes
// repérées par leur identifiant (`data-model-id`).
import assert from "node:assert/strict";
import { withApp } from "./lib/app.mjs";
import { fakeModel, startEngine } from "./lib/fake.mjs";

const binary = process.argv[2];

/** Recommandés (hub/curation.json, dans cet ordre). */
const RECOMMENDED = [
  "unsloth/Qwen3.5-9B-GGUF:Q4_K_M",
  "ggml-org/gemma-3-12b-it-GGUF:Q4_K_M",
  "ggml-org/gpt-oss-20b-GGUF:MXFP4",
  "unsloth/Qwen3.5-4B-GGUF:Q4_K_M",
];
/** Installés, du plus récent au plus ancien (ordre de `startEngine`). */
const NEWEST = fakeModel("think");
const OLDER = fakeModel("simple");

/** Identifiants des lignes du sélecteur ouvert. */
const options = (driver) =>
  driver.execute(
    "return [...document.querySelectorAll('[role=option]')].map((o) => o.dataset.modelId)",
  );
/** Pour chaque ligne : porte-t-elle l'icône « non téléchargé » ? */
const missing = (driver) =>
  driver.execute(
    "return [...document.querySelectorAll('[role=option]')].map((o) => o.querySelector('[aria-label=\"Not downloaded\"]') !== null)",
  );
/**
 * Identifiants des lignes qui montrent la mémoire demandée et une pastille
 * d'adéquation (recommandés non téléchargés ; machine injectée par withApp).
 */
const needs = (driver) =>
  driver.execute(
    `return [...document.querySelectorAll("[role=option]")]
       .filter((o) => o.querySelector("[title$=' of memory to run']") && o.querySelector("[data-fit]"))
       .map((o) => o.dataset.modelId);`,
  );
/** Identifiant de la ligne choisie. */
const chosen = (driver) =>
  driver.execute(
    "return document.querySelector('[role=option][aria-selected=true]')?.dataset.modelId ?? null",
  );
const buttonText = (driver) =>
  driver.execute(
    "return document.querySelector('[title=\"Select model\"]')?.textContent ?? null",
  );

async function waitUntil(check, what, timeoutMs = 8000) {
  const deadline = Date.now() + timeoutMs;
  while (!(await check())) {
    if (Date.now() > deadline) throw new Error(`Délai dépassé : ${what}`);
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
}

async function openPicker(driver) {
  if (
    !(await driver.execute(
      "return document.querySelector('[role=listbox]') !== null",
    ))
  ) {
    await driver.click(await driver.find("[title='Select model']"));
  }
  await driver.waitFor("[role=listbox]");
}

async function closePicker(driver) {
  await driver.execute(
    "document.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }))",
  );
}

// Aucun modèle installé (en premier : la base de la session est neuve).
await withApp(binary, { name: "models-none" }, async (driver) => {
  await driver.waitFor("[data-testid=new-chat]");
  await waitUntil(
    async () => (await buttonText(driver)) === "Select a model",
    "aucun modèle choisi",
  );
  await openPicker(driver);
  await waitUntil(
    async () =>
      JSON.stringify(await options(driver)) === JSON.stringify(RECOMMENDED),
    "recommandés seulement",
  );
  assert.deepEqual(
    await missing(driver),
    RECOMMENDED.map(() => true),
  );
  assert.equal(await chosen(driver), null);
  assert.equal((await driver.invoke("get_settings")).selectedModel, "");
});
console.log("aucun modèle : recommandés seulement, rien de choisi");

const engine = startEngine([NEWEST, OLDER]);
await withApp(
  binary,
  { name: "models", env: { ...engine.env, CHATONLM_SEED_DEMO: "1" } },
  async (driver) => {
    await driver.waitFor("[data-testid=new-chat]");
    // Aucun modèle choisi : le plus récemment installé.
    await waitUntil(
      async () =>
        (await driver.invoke("get_settings")).selectedModel === NEWEST,
      "modèle par défaut",
    );
    await openPicker(driver);
    await waitUntil(
      async () => (await options(driver)).length === RECOMMENDED.length + 2,
      "recommandés et installés",
    );
    assert.deepEqual(await options(driver), [...RECOMMENDED, NEWEST, OLDER]);
    assert.deepEqual(await missing(driver), [
      true,
      true,
      true,
      true,
      false,
      false,
    ]);
    assert.equal(await chosen(driver), NEWEST);
    // Recommandés non téléchargés : mémoire demandée et pastille ; jamais
    // pour un modèle installé.
    assert.deepEqual(await needs(driver), RECOMMENDED);
    await driver.screenshot(".tools/shots/models-open.png");

    // Recherche sur l'identifiant et le nom montré.
    await driver.type(
      await driver.find("[aria-label='Find model']"),
      "chatonlm-simple",
    );
    await waitUntil(
      async () =>
        JSON.stringify(await options(driver)) === JSON.stringify([OLDER]),
      "recherche",
    );
    await driver.click((await driver.findAll("[role=option]"))[0]);
    await waitUntil(
      async () => (await driver.invoke("get_settings")).selectedModel === OLDER,
      "modèle choisi mémorisé",
    );

    // Conversation de démonstration (réponses de qwen3:0.6b, ancien nom
    // d'Ollama, ni installé ni repris) : il ne désigne aucun modèle du moteur
    // intégré (l'envoi échouerait) ; le modèle choisi reste, l'ancien nom
    // n'est pas proposé.
    await driver.click(await driver.find("button[aria-label='Show sidebar']"));
    await driver.click(await driver.waitFor("[data-chat-id] a"));
    await driver.waitFor("[data-testid=chat]");
    // Le temps que la remise du modèle de la conversation passe (elle
    // enregistre l'ancien nom, puis remet le modèle d'avant).
    await new Promise((resolve) => setTimeout(resolve, 1500));
    await waitUntil(
      async () => (await driver.invoke("get_settings")).selectedModel === OLDER,
      "modèle choisi gardé",
    );
    await openPicker(driver);
    await waitUntil(
      async () => (await chosen(driver)) === OLDER,
      "modèle choisi gardé dans le sélecteur",
    );
    assert.deepEqual(await options(driver), [...RECOMMENDED, NEWEST, OLDER]);
    await driver.screenshot(".tools/shots/models-missing.png");
    await closePicker(driver);
  },
);
console.log("modèles installés : ordre, défaut, recherche, conversation");
console.log("models : OK");
