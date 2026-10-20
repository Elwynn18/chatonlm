// Fin de l'app, réglage « Unload models when quitting » (onglet Models,
// activé par défaut) : les modèles encore en mémoire dans l'Ollama de la
// machine (ici le faux serveur) sont déchargés quand l'app quitte (Ctrl+Q,
// « Quit ChatonLM »), et restent chargés quand le réglage est coupé.
// Les modèles sont chargés directement sur le faux serveur, comme par un
// autre programme ou par une conversation précédente (« Always »).
import assert from "node:assert/strict";
import { withApp } from "./lib/app.mjs";
import { startFake } from "./lib/fake.mjs";

const binary = process.argv[2];
const MODELS = ["qwen3:0.6b", "chatonlm-vision:latest"];
const fake = await startFake("ollama");
const delay = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

/** Charge `model` sur le faux serveur, sans échéance. */
async function load(model) {
  const response = await fetch(new URL("/api/generate", fake.url), {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ model, keep_alive: -1 }),
  });
  assert.equal(response.status, 200, `chargement de ${model}`);
}

/** Noms des modèles en mémoire (`/api/ps`), triés. */
async function loaded() {
  const response = await fetch(new URL("/api/ps", fake.url));
  return (await response.json()).models.map((model) => model.name).sort();
}

/** Déchargements (`keep_alive: 0`) reçus par le faux serveur. */
async function unloads() {
  const response = await fetch(new URL("/fake/loads", fake.url));
  const body = await response.json();
  return (Array.isArray(body) ? body : body.loads)
    .filter((entry) => entry.kind === "unload")
    .map((entry) => entry.model);
}

/**
 * Lance l'app avec les modèles chargés, règle `unloadOnQuit` s'il est donné,
 * la fait quitter (`quit_app`, comme Ctrl+Q) et attend sa fin.
 */
async function quitWith(name, unloadOnQuit) {
  for (const model of MODELS) await load(model);
  assert.deepEqual(await loaded(), [...MODELS].sort());
  const before = (await unloads()).length;
  await withApp(
    binary,
    { name, env: { OLLAMA_HOST: fake.url } },
    async (driver, app) => {
      await driver.waitFor("[data-testid=new-chat]");
      const settings = await driver.invoke("get_settings");
      assert.equal(settings.unloadOnQuit, true, "activé par défaut");
      if (unloadOnQuit !== undefined) {
        await driver.invoke("update_settings", { patch: { unloadOnQuit } });
      }
      // Rien n'est déchargé avant la fin de l'app.
      assert.deepEqual(await loaded(), [...MODELS].sort());
      await driver
        .execute("window.__TAURI_INTERNALS__.invoke('quit_app'); return true")
        .catch(() => {});
      const code = await Promise.race([app.exited, delay(15000)]);
      assert.equal(code, 0, "l'app n'a pas quitté proprement");
    },
  );
  return (await unloads()).slice(before);
}

try {
  const unloaded = await quitWith("unload-on-quit-default");
  assert.deepEqual(await loaded(), [], "modèles encore en mémoire");
  assert.deepEqual(unloaded.sort(), [...MODELS].sort());
  console.log(`réglage par défaut : ${unloaded.join(", ")} déchargés`);

  const kept = await quitWith("unload-on-quit-off", false);
  assert.deepEqual(kept, [], "déchargement malgré le réglage coupé");
  assert.deepEqual(await loaded(), [...MODELS].sort());
  console.log("réglage coupé : modèles gardés en mémoire");
  console.log("unload-on-quit : OK");
} finally {
  await fake.stop();
}
