// Croix de la fenêtre (Settings › General, réglage « Keep running in the
// system tray »), dans la session virtuelle avec une fausse zone de
// notification (e2e/lib/desktop-services.py) :
// - par défaut, la croix masque la fenêtre (l'app reste lancée) ;
// - réglage coupé, la croix quitte l'app comme Ctrl+Q : fin commune, modèles
//   en mémoire déchargés (réglage « Unload models when quitting ») ;
// - le réglage est gardé : au lancement suivant, la croix quitte aussitôt.
import assert from "node:assert/strict";
import { withApp } from "./lib/app.mjs";
import { poll, startDesktopServices } from "./lib/desktop.mjs";
import { startFake } from "./lib/fake.mjs";
import { forWindows, runKWinScript } from "./lib/kwin.mjs";

const binary = process.argv[2];
const MODEL = "qwen3:0.6b";
const delay = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

const closeWindow = () =>
  runKWinScript(forWindows("ChatonLM", "w.closeWindow();"));
const visible = (driver) =>
  driver.invoke("plugin:window|is_visible", { label: "main" });
const waitVisible = (driver, expected) =>
  poll(
    () => visible(driver),
    (value) => value === expected,
    { what: `fenêtre ${expected ? "visible" : "masquée"}` },
  );
const status = (driver) => driver.invoke("close_button_status");

const fake = await startFake("ollama");
const env = { OLLAMA_HOST: fake.url };

/** Charge `model` sur le faux serveur, sans échéance. */
async function load(model) {
  const response = await fetch(new URL("/api/generate", fake.url), {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ model, keep_alive: -1 }),
  });
  assert.equal(response.status, 200, `chargement de ${model}`);
}

/** Déchargements (`keep_alive: 0`) reçus par le faux serveur. */
async function unloads() {
  const response = await fetch(new URL("/fake/loads", fake.url));
  const body = await response.json();
  return (Array.isArray(body) ? body : body.loads)
    .filter((entry) => entry.kind === "unload")
    .map((entry) => entry.model);
}

/** Croix pressée : l'app doit quitter seule, proprement. */
async function closeQuits(app) {
  await closeWindow();
  const code = await Promise.race([app.exited, delay(15000)]);
  assert.equal(code, 0, "la croix n'a pas fait quitter l'app");
}

const desk = await startDesktopServices({ notifications: true, watcher: true });
try {
  // 1. Par défaut : masquée ; réglage coupé : la croix quitte.
  await load(MODEL);
  await withApp(binary, { name: "close-button", env }, async (driver, app) => {
    await driver.waitFor("[data-testid=new-chat]");
    const first = await poll(
      () => status(driver),
      (s) => s.trayAvailable,
      { what: "zone de notification" },
    );
    assert.equal(first.closeToTray, true, "masquer par défaut");

    await closeWindow();
    await waitVisible(driver, false);
    await desk.command("item-activate");
    await waitVisible(driver, true);
    assert.deepEqual(await unloads(), [], "déchargé alors que l'app reste");

    assert.deepEqual(
      await driver.invoke("set_close_to_tray", { enabled: false }),
      { trayAvailable: true, closeToTray: false },
    );
    await closeQuits(app);
  });
  assert.deepEqual(await unloads(), [MODEL], "modèle gardé en mémoire");
  console.log("croix : masque par défaut, quitte une fois le réglage coupé");

  // 2. Réglage gardé : la croix quitte dès le lancement suivant.
  await withApp(
    binary,
    { name: "close-button-kept", env },
    async (driver, app) => {
      await driver.waitFor("[data-testid=new-chat]");
      const kept = await poll(
        () => status(driver),
        (s) => s.trayAvailable,
        { what: "zone de notification" },
      );
      assert.equal(kept.closeToTray, false, "réglage perdu");
      await closeQuits(app);
    },
  );
  console.log("croix : réglage gardé au lancement suivant");
  console.log("close-button : OK");
} finally {
  await desk.stop();
  await fake.stop();
}
