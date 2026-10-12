// Raccourcis de la fenêtre dans l'app réelle (vraies touches) :
// - zoom : Ctrl+= quatre fois (borne 120 %), Ctrl+- cinq fois (borne 90 %),
//   Ctrl+0 (100 %), mesuré par la largeur CSS de la fenêtre ;
// - Ctrl+N ouvre une nouvelle conversation depuis les réglages ;
// - F5, Ctrl+R et Ctrl+Maj+R ne rechargent pas la page (marqueur gardé).
import assert from "node:assert/strict";
import { setTimeout as delay } from "node:timers/promises";
import { withApp } from "./lib/app.mjs";
import { startFake } from "./lib/fake.mjs";
import { pressKeys } from "./lib/keyboard.mjs";

const binary = process.argv[2];
const fake = await startFake("ollama");

async function waitUntil(check, what, timeoutMs = 10000) {
  const deadline = Date.now() + timeoutMs;
  for (;;) {
    const value = await check();
    if (value) return value;
    if (Date.now() > deadline) throw new Error(`Délai dépassé : ${what}`);
    await delay(100);
  }
}

const width = (driver) => driver.execute("return window.innerWidth");

/** Attend que la largeur CSS corresponde au zoom `level` (à 2 px près). */
async function waitZoom(driver, base, level) {
  let last;
  await waitUntil(
    async () => {
      last = await width(driver);
      return Math.abs(last - base / level) <= 2;
    },
    () => `zoom ${level} (largeur ${last}, attendue ${base / level})`,
  );
}

try {
  await withApp(
    binary,
    { name: "navigation", env: { OLLAMA_HOST: fake.url } },
    async (driver) => {
      await driver.waitFor("[data-testid=new-chat]");
      const base = await width(driver);

      // Zoom.
      for (let i = 0; i < 4; i++) await pressKeys("ctrl+equal");
      await waitZoom(driver, base, 1.2);
      for (let i = 0; i < 5; i++) await pressKeys("ctrl+minus");
      await waitZoom(driver, base, 0.9);
      await pressKeys("ctrl+0");
      await waitZoom(driver, base, 1);

      // Ctrl+N depuis les réglages.
      await pressKeys("ctrl+comma");
      await driver.waitFor("[data-testid=settings]");
      await pressKeys("ctrl+n");
      await driver.waitFor("[data-testid=new-chat]");

      // Pas de rechargement.
      await driver.execute("window.__notReloaded = true");
      for (const chord of ["f5", "ctrl+r", "ctrl+shift+r"]) {
        await pressKeys(chord, { settleMs: 500 });
        assert.equal(
          await driver.execute("return window.__notReloaded === true"),
          true,
          `${chord} a rechargé la page`,
        );
      }
    },
  );
} finally {
  await fake.stop();
}
