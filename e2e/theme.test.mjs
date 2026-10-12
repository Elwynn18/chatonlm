// Thème : clair, sombre ou système, appliqué dès le lancement (avant que la
// fenêtre ne s'affiche) d'après le réglage enregistré ; puis les écrans de
// l'app (pages, onglets des réglages et du ModelHub) dans les deux thèmes :
// aucun texte à lire sous 4,5:1 de contraste
// (mesuré dans la page, e2e/lib/contrast.mjs), captures
// .tools/shots/theme-<écran>-<light|dark>.png.
import assert from "node:assert/strict";
import { setTimeout as delay } from "node:timers/promises";
import { withApp } from "./lib/app.mjs";
import {
  forceTheme,
  measureContrast,
  restoreTheme,
  unreadable,
} from "./lib/contrast.mjs";
import { startFake } from "./lib/fake.mjs";

const binary = process.argv[2];
const fake = await startFake("ollama");
const env = { OLLAMA_HOST: fake.url };

const state = (driver) =>
  driver.execute(`return {
    dark: document.documentElement.classList.contains("dark"),
    scheme: document.documentElement.style.colorScheme,
    systemDark: matchMedia("(prefers-color-scheme: dark)").matches,
  }`);

async function launchWith(theme, check) {
  await withApp(binary, { name: `theme-${theme}`, env }, async (driver) => {
    await driver.waitFor("[data-testid=new-chat]");
    await driver.invoke("update_settings", { patch: { theme } });
  });
  await withApp(binary, { name: `theme-${theme}-2`, env }, async (driver) => {
    await driver.waitFor("[data-testid=new-chat]");
    await check(await state(driver));
    await driver.screenshot(`.tools/shots/theme-${theme}.png`);
  });
}

/** Onglets du ModelHub et des réglages, ouverts par leur lien. */
const hubTab = (name) => [
  `hub-${name}`,
  `nav[aria-label=ModelHub] a[href$='/hub/${name}']`,
  `nav[aria-label=ModelHub] a[href$='/hub/${name}'][aria-current=page]`,
];
const settingsTab = (name) => [
  `settings-${name}`,
  `[data-testid=settings-tab-${name}]`,
  `[data-testid=settings-tab-${name}][aria-current=page]`,
];

/**
 * Écrans de l'app : nom, lien à cliquer (barre latérale ou onglet), élément
 * attendu.
 */
const SCREENS = [
  ["chat", "[data-chat-id] a", "[data-role=assistant]"],
  ["proofread", "nav a[href$='/proofread']", "[data-testid=proofread]"],
  ["study", "nav a[href$='/study']", "[data-testid=study-form]"],
  ["hub", "nav a[href$='/hub']", "nav[aria-label=ModelHub]"],
  ...["transcription", "polish", "voices", "installed", "downloads"].map(
    hubTab,
  ),
  ["settings", "nav a[href$='/settings']", "[data-testid=settings]"],
  ...[
    "models",
    "account",
    "dictation",
    "readAloud",
    "voiceMode",
    "shortcuts",
  ].map(settingsTab),
];

/**
 * Chaque écran de l'app, barre latérale ouverte, en clair puis en sombre
 * (classe « dark » imposée, comme le fait useTheme) : tout texte à lire y a
 * un contraste d'au moins 4,5:1 avec le fond peint dessous.
 */
async function checkContrast() {
  await withApp(
    binary,
    { name: "theme-contrast", env: { ...env, CHATONLM_SEED_DEMO: "1" } },
    async (driver) => {
      await driver.waitFor("[data-testid=new-chat]");
      await driver.click(
        await driver.waitFor("button[aria-label='Show sidebar']"),
      );
      for (const [name, link, shown] of SCREENS) {
        await driver.click(await driver.waitFor(link));
        await driver.waitFor(shown);
        // Contenu chargé après la navigation (réglages, catalogue fermé).
        await delay(800);
        for (const theme of ["light", "dark"]) {
          const before = await forceTheme(driver, theme === "dark");
          try {
            const entries = await measureContrast(driver);
            assert.ok(
              entries.length >= 10,
              `${name} (${theme}) : ${entries.length} textes mesurés seulement`,
            );
            assert.deepEqual(
              unreadable(entries).map(
                (entry) =>
                  `${entry.ratio} : ${entry.color} sur ${entry.background}, « ${entry.text} » (${entry.classes})`,
              ),
              [],
              `textes sous 4,5:1, écran ${name}, thème ${theme}`,
            );
            await driver.screenshot(`.tools/shots/theme-${name}-${theme}.png`);
          } finally {
            await restoreTheme(driver, before);
          }
        }
      }
    },
  );
}

try {
  await launchWith("dark", (s) => {
    assert.equal(s.dark, true);
    assert.equal(s.scheme, "dark");
  });
  await launchWith("light", (s) => {
    assert.equal(s.dark, false);
    assert.equal(s.scheme, "light");
  });
  await launchWith("system", (s) => {
    assert.equal(s.dark, s.systemDark);
    assert.equal(s.scheme, "light dark");
  });
  await checkContrast();
} finally {
  await fake.stop();
}
