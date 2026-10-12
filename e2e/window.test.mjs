// Fenêtre : la taille est mémorisée à la fermeture et restaurée au lancement
// suivant ; une seconde instance s'arrête aussitôt (instance unique).
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { openSync } from "node:fs";
import { freePort, withApp } from "./lib/app.mjs";
import { startFake } from "./lib/fake.mjs";
import { forWindows, runKWinScript } from "./lib/kwin.mjs";

const binary = process.argv[2];
const fake = await startFake("ollama");
const env = { OLLAMA_HOST: fake.url };
const inner = (driver) =>
  driver.execute("return [window.innerWidth, window.innerHeight]");
const near = (a, b) => Math.abs(a - b) <= 2;

try {
  let resized;
  await withApp(binary, { name: "window-1", env }, async (driver) => {
    await driver.waitFor("[data-testid=new-chat]");
    const initial = await inner(driver);
    assert.ok(initial[0] >= 900 && initial[1] >= 600, `taille ${initial}`);

    await runKWinScript(
      forWindows(
        "ChatonLM",
        "w.frameGeometry = { x: 40, y: 30, width: 1040, height: 760 };",
      ),
    );
    resized = await inner(driver);
    assert.ok(!near(resized[0], initial[0]), "la fenêtre aurait dû changer");

    // Fermeture par le gestionnaire de fenêtres, comme un clic sur la croix.
    await runKWinScript(forWindows("ChatonLM", "w.closeWindow();"));
  });

  await withApp(binary, { name: "window-2", env }, async (driver) => {
    await driver.waitFor("[data-testid=new-chat]");
    const settings = await driver.invoke("get_settings");
    assert.ok(near(settings.windowWidth, resized[0]), JSON.stringify(settings));
    assert.ok(
      near(settings.windowHeight, resized[1]),
      JSON.stringify(settings),
    );
    const restored = await inner(driver);
    assert.ok(near(restored[0], resized[0]), `${restored} vs ${resized}`);
    assert.ok(near(restored[1], resized[1]), `${restored} vs ${resized}`);

    // Instance unique : la seconde sort tout de suite, la première reste.
    const log = openSync(".tools/logs/e2e-window-second.log", "w");
    const second = spawn(binary, [], {
      env: {
        ...process.env,
        ...env,
        TAURI_WEBDRIVER_PORT: String(await freePort()),
      },
      stdio: ["ignore", log, log],
    });
    const code = await new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        second.kill("SIGKILL");
        reject(new Error("la seconde instance ne s'est pas arrêtée"));
      }, 10000);
      second.once("exit", (exitCode) => {
        clearTimeout(timer);
        resolve(exitCode);
      });
    });
    assert.equal(code, 0);
    assert.equal(await driver.execute("return 1 + 1"), 2);
  });
} finally {
  await fake.stop();
}
