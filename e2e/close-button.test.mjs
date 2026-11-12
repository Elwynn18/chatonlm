// Croix de la fenêtre (Settings › General, réglage « Keep running in the
// system tray »), dans la session virtuelle avec une fausse zone de
// notification (e2e/lib/desktop-services.py) :
// - par défaut, la croix masque la fenêtre (l'app reste lancée) ;
// - réglage coupé, la croix quitte l'app comme Ctrl+Q : fin commune, modèle
//   chargé arrêté (processus du faux llama-server, fin propre) ;
// - le réglage est gardé : au lancement suivant, la croix quitte aussitôt.
import assert from "node:assert/strict";
import { withApp } from "./lib/app.mjs";
import { poll, startDesktopServices } from "./lib/desktop.mjs";
import { fakeModel, startEngine } from "./lib/fake.mjs";
import { forWindows, runKWinScript } from "./lib/kwin.mjs";

const binary = process.argv[2];
const MODEL = fakeModel("simple");
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

/**
 * Presse la croix jusqu'à ce que `done()` se vérifie (trois appuis au plus,
 * 5 s chacun). Sous charge disque, KWin peut décharger le script avant de
 * l'avoir exécuté : la croix n'est alors jamais pressée et l'app n'en sait
 * rien. Un appui de plus est noté dans le journal.
 */
async function pressClose(done, what) {
  for (let attempt = 1; ; attempt++) {
    await closeWindow();
    const deadline = Date.now() + 5000;
    while (Date.now() < deadline) {
      if (await done()) return;
      await delay(100);
    }
    if (attempt === 3) throw new Error(`Attente vaine ${what} après 3 appuis`);
    console.log(`croix pressée de nouveau (${what}, appui ${attempt + 1})`);
  }
}

const hideByClose = (driver) =>
  pressClose(async () => (await visible(driver)) === false, "fenêtre masquée");
const status = (driver) => driver.invoke("close_button_status");

const engine = startEngine([MODEL]);
const env = engine.env;

const alive = (pid) => {
  try {
    process.kill(pid, 0);
    return true;
  } catch {
    return false;
  }
};

/** Charge `model` dans l'app (comme Load de l'onglet Models) ; rend son PID. */
async function load(driver, model) {
  await driver.invoke("load_model", { model });
  await poll(
    () => driver.invoke("running_models"),
    (running) =>
      running.some((entry) => entry.id === model && entry.state === "ready"),
    { what: `${model} prêt` },
  );
  const pid = engine.server(model)?.pid;
  assert.ok(pid, "instance absente du registre");
  return pid;
}

/**
 * Croix pressée : l'app doit quitter seule, proprement. Elle a reçu la croix
 * dès qu'elle est sortie ou que sa page ne répond plus (processus web arrêté
 * au début de la fin commune).
 */
async function closeQuits(driver, app) {
  let exited = false;
  app.exited.then(() => {
    exited = true;
  });
  await pressClose(async () => {
    if (exited) return true;
    try {
      return (await visible(driver)) === false;
    } catch {
      return true;
    }
  }, "début de la fermeture");
  const code = await Promise.race([app.exited, delay(15000)]);
  assert.equal(code, 0, "la croix n'a pas fait quitter l'app");
}

const desk = await startDesktopServices({ notifications: true, watcher: true });
try {
  // 1. Par défaut : masquée ; réglage coupé : la croix quitte.
  let pid;
  await withApp(binary, { name: "close-button", env }, async (driver, app) => {
    await driver.waitFor("[data-testid=new-chat]");
    pid = await load(driver, MODEL);
    const first = await poll(
      () => status(driver),
      (s) => s.trayAvailable,
      { what: "zone de notification" },
    );
    assert.equal(first.closeToTray, true, "masquer par défaut");

    await hideByClose(driver);
    await desk.command("item-activate");
    await waitVisible(driver, true);
    assert.ok(alive(pid), "modèle arrêté alors que l'app reste");

    assert.deepEqual(
      await driver.invoke("set_close_to_tray", { enabled: false }),
      { trayAvailable: true, closeToTray: false },
    );
    await closeQuits(driver, app);
  });
  await poll(
    () => alive(pid),
    (running) => !running,
    { what: "fin du processus du modèle" },
  );
  assert.deepEqual(
    engine.registryFiles(),
    [],
    "fiche laissée : instance tuée au lieu d'être arrêtée",
  );
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
      await closeQuits(driver, app);
    },
  );
  console.log("croix : réglage gardé au lancement suivant");
  console.log("close-button : OK");
} finally {
  await desk.stop();
}
