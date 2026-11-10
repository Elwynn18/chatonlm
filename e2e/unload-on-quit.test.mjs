// Fin de l'app : les modèles chargés sont des processus llama-server de l'app
// (moteur intégré) ; quand l'app quitte (Ctrl+Q, « Quit ChatonLM »), tous
// s'arrêtent proprement, sans processus laissé derrière elle (il garderait
// sa mémoire et sa VRAM). L'ancien réglage « Unload models when quitting »
// (Ollama de la machine, retiré de l'onglet Models) n'y change rien : coupé,
// les processus s'arrêtent aussi.
import assert from "node:assert/strict";
import { withApp } from "./lib/app.mjs";
import { fakeModel, startEngine } from "./lib/fake.mjs";

const binary = process.argv[2];
const MODELS = [fakeModel("simple"), fakeModel("vision")];
const engine = startEngine(MODELS);
const delay = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

const alive = (pid) => {
  try {
    process.kill(pid, 0);
    return true;
  } catch {
    return false;
  }
};

/** Attend que `check()` soit vrai (8 s au plus). */
async function waitUntil(check, what) {
  const deadline = Date.now() + 8000;
  while (!(await check())) {
    if (Date.now() > deadline) throw new Error(`Délai dépassé : ${what}`);
    await delay(100);
  }
}

/**
 * Lance l'app, charge les deux modèles (bouton Load de l'onglet Models),
 * règle `unloadOnQuit` s'il est donné, la fait quitter (`quit_app`, comme
 * Ctrl+Q) et rend les PID des faux llama-server qu'elle avait lancés.
 */
async function quitWith(name, unloadOnQuit) {
  let pids = [];
  await withApp(binary, { name, env: engine.env }, async (driver, app) => {
    await driver.waitFor("[data-testid=new-chat]");
    if (unloadOnQuit !== undefined) {
      await driver.invoke("update_settings", { patch: { unloadOnQuit } });
    }
    for (const model of MODELS) {
      await driver.invoke("load_model", { model });
    }
    await waitUntil(async () => {
      const running = await driver.invoke("running_models");
      return MODELS.every((id) =>
        running.some((model) => model.id === id && model.state === "ready"),
      );
    }, "les deux modèles prêts");
    pids = MODELS.map((id) => engine.server(id)?.pid);
    assert.ok(pids.every(Boolean), "instances absentes du registre");
    assert.ok(pids.every(alive));
    await driver
      .execute("window.__TAURI_INTERNALS__.invoke('quit_app'); return true")
      .catch(() => {});
    const code = await Promise.race([app.exited, delay(15000)]);
    assert.equal(code, 0, "l'app n'a pas quitté proprement");
  });
  return pids;
}

const defaultPids = await quitWith("unload-on-quit-default");
await waitUntil(
  () => defaultPids.every((pid) => !alive(pid)),
  "fin des llama-server",
);
// Arrêt propre (SIGTERM, pas SIGKILL) : chaque instance a retiré sa fiche.
// Fiches brutes, vivantes ou non (`servers()` écarte les mortes : il serait
// vide même après un SIGKILL).
assert.deepEqual(
  engine.registryFiles(),
  [],
  "fiche laissée : instance tuée au lieu d'être arrêtée",
);
console.log(`sortie : ${MODELS.length} processus du moteur arrêtés`);

const offPids = await quitWith("unload-on-quit-off", false);
await waitUntil(
  () => offPids.every((pid) => !alive(pid)),
  "fin des llama-server (réglage coupé)",
);
assert.deepEqual(
  engine.registryFiles(),
  [],
  "fiche laissée (réglage coupé) : instance tuée au lieu d'être arrêtée",
);
console.log("ancien réglage coupé : processus arrêtés aussi");
console.log("unload-on-quit : OK");
