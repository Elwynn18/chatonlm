// Sélecteur de modèles, avec les modèles enregistrés d'une vraie instance
// (faux serveur) : modèles recommandés par le serveur (gemma4, qwen3.5) en
// tête, puis les modèles installés ; modèle par défaut = le plus récemment
// installé, jamais un recommandé ; recherche, choix mémorisé, modèle d'une
// conversation restauré à son ouverture, modèle choisi mais non installé en
// fin de liste, serveur sans aucun modèle installé (recommandés seulement,
// aucun choisi).
import assert from "node:assert/strict";
import { withApp } from "./lib/app.mjs";
import { startFake } from "./lib/fake.mjs";

const binary = process.argv[2];

const label = (driver) =>
  driver.execute(
    "return document.querySelector('[title=\"Select model\"]')?.textContent ?? null",
  );
const options = (driver) =>
  driver.execute(
    "return [...document.querySelectorAll('[role=option]')].map((o) => o.textContent)",
  );
/** Pour chaque ligne du sélecteur : porte-t-elle l'icône « non téléchargé » ? */
const missing = (driver) =>
  driver.execute(
    "return [...document.querySelectorAll('[role=option]')].map((o) => o.querySelector('[aria-label=\"Not downloaded\"]') !== null)",
  );
/** Modèles recommandés par le faux serveur (liste par défaut d'Ollama). */
const RECOMMENDED = ["gemma4", "qwen3.5"];

async function waitUntil(check, what, timeoutMs = 5000) {
  const deadline = Date.now() + timeoutMs;
  while (!(await check())) {
    if (Date.now() > deadline) throw new Error(`Délai dépassé : ${what}`);
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
}

// Serveur sans aucun modèle (en premier : la base de la session est neuve).
const empty = await startFake("no-models");
try {
  await withApp(
    binary,
    { name: "models-none", env: { OLLAMA_HOST: empty.url } },
    async (driver) => {
      await driver.waitFor("[data-testid=new-chat]");
      await waitUntil(
        async () => (await label(driver)) === "Select a model",
        "aucun modèle",
      );
      await driver.click(await driver.find("[title='Select model']"));
      await driver.waitFor("[role=listbox]");
      // Recommandés seulement, non installés ; aucun n'est choisi.
      await waitUntil(
        async () =>
          JSON.stringify(await options(driver)) === JSON.stringify(RECOMMENDED),
        "modèles recommandés",
      );
      assert.deepEqual(await missing(driver), [true, true]);
      assert.equal(await label(driver), "Select a model");
      assert.equal((await driver.invoke("get_settings")).selectedModel, "");
    },
  );
} finally {
  await empty.stop();
}

// Modèles enregistrés.
const fake = await startFake("ollama");
try {
  await withApp(
    binary,
    {
      name: "models",
      env: { OLLAMA_HOST: fake.url, CHATONLM_SEED_DEMO: "1" },
    },
    async (driver) => {
      await driver.waitFor("[data-testid=new-chat]");
      // Aucun modèle choisi : le plus récemment installé.
      await waitUntil(
        async () => (await label(driver)) === "qwen3:0.6b",
        "modèle par défaut",
      );
      assert.equal(
        (await driver.invoke("get_settings")).selectedModel,
        "qwen3:0.6b",
      );

      await driver.click(await driver.find("[title='Select model']"));
      await driver.waitFor("[role=listbox]");
      await waitUntil(
        async () => (await options(driver))[0] === "gemma4",
        "modèles recommandés en tête",
      );
      assert.deepEqual(await options(driver), [
        ...RECOMMENDED,
        "qwen3:0.6b",
        "qwen3.5:0.8b",
      ]);
      assert.deepEqual(await missing(driver), [true, true, false, false]);
      await driver.screenshot(".tools/shots/models-open.png");
      await driver.type(await driver.find("[aria-label='Find model']"), "3.5");
      assert.deepEqual(await options(driver), ["qwen3.5", "qwen3.5:0.8b"]);
      await driver.click((await driver.findAll("[role=option]"))[1]);
      await waitUntil(
        async () => (await label(driver)) === "qwen3.5:0.8b",
        "modèle choisi",
      );
      assert.equal(
        (await driver.invoke("get_settings")).selectedModel,
        "qwen3.5:0.8b",
      );

      // Conversation de démonstration (réponses de qwen3:0.6b) : son modèle
      // redevient le modèle choisi.
      await driver.click(
        await driver.find("button[aria-label='Show sidebar']"),
      );
      await driver.click(await driver.waitFor("[data-chat-id] a"));
      await driver.waitFor("[data-testid=chat]");
      await waitUntil(
        async () => (await label(driver)) === "qwen3:0.6b",
        "modèle de la conversation",
      );

      // Modèle choisi mais absent du serveur (et pas recommandé), sur une
      // nouvelle conversation (une conversation ouverte remettrait son propre
      // modèle) : en fin de liste, avec l'icône.
      await driver.click((await driver.findAll("nav a"))[0]);
      await driver.waitFor("[data-testid=new-chat]");
      await driver.invoke("update_settings", {
        patch: { selectedModel: "mistral" },
      });
      // Rechargement de la page : un marqueur posé sur l'ancienne page dit
      // quand la nouvelle est là.
      await driver.execute("window.__oldPage = true; location.reload()");
      await waitUntil(
        async () => !(await driver.execute("return window.__oldPage === true")),
        "rechargement",
        10000,
      );
      await driver.waitFor("[data-testid=new-chat] [title='Select model']", {
        timeoutMs: 10000,
      });
      await waitUntil(
        async () => (await label(driver)) === "mistral",
        "modèle non installé",
      );
      await driver.click(await driver.find("[title='Select model']"));
      await driver.waitFor("[role=listbox]");
      await waitUntil(
        async () => (await options(driver))[0] === "gemma4",
        "modèles recommandés en tête",
      );
      assert.deepEqual(await options(driver), [
        ...RECOMMENDED,
        "qwen3:0.6b",
        "qwen3.5:0.8b",
        "mistral",
      ]);
      assert.deepEqual(await missing(driver), [true, true, false, false, true]);
      await driver.screenshot(".tools/shots/models-missing.png");
    },
  );
} finally {
  await fake.stop();
}
