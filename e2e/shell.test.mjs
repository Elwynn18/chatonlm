// Coquille de l'app : barre latérale fermée au lancement, ouverture et
// fermeture, navigation Chat / Settings, bouton « New chat » seulement sur les
// pages sans titre et barre fermée, état de la barre conservé entre les pages.
import assert from "node:assert/strict";
import { withApp } from "./lib/app.mjs";
import { startFake } from "./lib/fake.mjs";

const binary = process.argv[2];
const fake = await startFake("ollama");

const exists = (driver, css) =>
  driver.execute("return document.querySelector(arguments[0]) !== null", [css]);
const opacity = (driver, css) =>
  driver.execute(
    "return getComputedStyle(document.querySelector(arguments[0])).opacity",
    [css],
  );

try {
  await withApp(
    binary,
    { name: "shell", env: { OLLAMA_HOST: fake.url } },
    async (driver) => {
      await driver.waitFor("[data-testid=new-chat]");
      // Fermée au lancement : pas de navigation, « New chat » visible.
      assert.equal(await exists(driver, "nav"), false);
      await driver.waitFor("a[title='New chat']");
      await driver.screenshot(".tools/shots/shell-closed.png");

      await driver.click(
        await driver.find("button[aria-label='Show sidebar']"),
      );
      await driver.waitFor("nav");
      const links = await driver.execute(
        "return [...document.querySelectorAll('nav a')].map((a) => a.textContent.trim())",
      );
      assert.deepEqual(links, [
        "Chat",
        "Proofread",
        "Study",
        "ModelHub",
        "Settings",
      ]);
      // Barre ouverte : « New chat » s'efface (fondu de 375 ms).
      await new Promise((resolve) => setTimeout(resolve, 600));
      assert.equal(await opacity(driver, "a[title='New chat']"), "0");
      await driver.screenshot(".tools/shots/shell-open.png");

      // Settings : titre de page, pas de « New chat », barre toujours ouverte.
      // Par l'adresse : la barre a aussi le lien ModelHub (étape 15).
      const settingsLink = await driver.find("nav a[href='/settings']");
      await driver.click(settingsLink);
      await driver.waitFor("[data-testid=settings]");
      assert.equal(await driver.text(await driver.find("main h1")), "Settings");
      assert.equal(await exists(driver, "a[title='New chat']"), false);
      assert.equal(await exists(driver, "nav"), true);
      await driver.screenshot(".tools/shots/shell-settings.png");

      // Retour au chat, puis fermeture de la barre.
      await driver.click((await driver.findAll("nav a"))[0]);
      await driver.waitFor("[data-testid=new-chat]");
      await driver.click(
        await driver.find("button[aria-label='Hide sidebar']"),
      );
      await driver.waitFor("button[aria-label='Show sidebar']");
      assert.equal(await exists(driver, "nav"), false);
    },
  );
} finally {
  await fake.stop();
}
