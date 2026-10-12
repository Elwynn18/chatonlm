// Menus contextuels dans l'app réelle (vrai clic droit du pointeur de KWin ;
// entrées du menu WebKit relues par la commande e2e_last_context_menu, qui
// donne les noms d'action gardés après filtrage) :
// - sur une réponse sans sélection : ni Back, ni Forward, ni Reload, ni Stop ;
// - sur un texte sélectionné : Copy ;
// - dans la zone de saisie : Paste et SelectAll ;
// - hors des zones (titre de la page des réglages) : aucun menu.
import assert from "node:assert/strict";
import { setTimeout as delay } from "node:timers/promises";
import { withApp } from "./lib/app.mjs";
import { startFake } from "./lib/fake.mjs";
import { chordsOf, pressKeys } from "./lib/keyboard.mjs";
import { centerOf, pointerOffset } from "./lib/pointer.mjs";

const binary = process.argv[2];
const fake = await startFake("ollama");

// Navigation, liens et entrées ajoutées par WebKit (Copy Link with Highlight).
const NAVIGATION = [
  "GoBack",
  "GoForward",
  "Reload",
  "Stop",
  "OpenLink",
  "Custom",
];

async function waitUntil(check, what, timeoutMs = 10000) {
  const deadline = Date.now() + timeoutMs;
  for (;;) {
    const value = await check();
    if (value) return value;
    if (Date.now() > deadline) {
      const detail = typeof what === "function" ? await what() : what;
      throw new Error(`Délai dépassé : ${detail}`);
    }
    await delay(100);
  }
}

/** Clic droit au centre de `css`, puis entrées du menu montré (Échap le ferme). */
async function rightClick(driver, offset, css) {
  await driver.invoke("e2e_last_context_menu"); // vide l'état
  const [x, y] = await centerOf(driver, css);
  await pressKeys(
    [
      `pointer:${x + offset[0]},${y + offset[1]}`,
      "wait:100",
      "rbutton:down",
      "wait:50",
      "rbutton:up",
      "wait:400",
    ],
    { settleMs: 200 },
  );
  const entries = await driver.invoke("e2e_last_context_menu");
  await pressKeys("escape", { settleMs: 300 });
  return entries;
}

try {
  await withApp(
    binary,
    { name: "context-menu", env: { OLLAMA_HOST: fake.url } },
    async (driver) => {
      await driver.waitFor("[data-testid=new-chat]");
      // Une réponse à montrer.
      await waitUntil(
        () =>
          driver.execute(
            "return document.querySelector('[title=\"Select model\"]')?.textContent",
          ),
        "modèle affiché",
      );
      // Vraies touches : une Entrée simulée en JavaScript peut se perdre.
      await driver.click(
        await driver.waitFor("textarea[aria-label='Message']"),
      );
      await pressKeys([...chordsOf("Hello"), "enter"]);
      await waitUntil(
        () =>
          driver.execute(
            "return document.querySelector('[data-role=user]') !== null",
          ),
        async () =>
          `message non envoyé : ${await driver.execute("return document.body.innerText.slice(0, 400)")}`,
      );
      await driver.waitFor("[data-role=assistant]", { timeoutMs: 20000 });
      await waitUntil(
        async () =>
          !(await driver.execute(
            "return document.querySelector(\"[aria-label='Stop']\") !== null",
          )),
        "fin de la réponse",
      );
      const offset = await pointerOffset(driver);

      // Réponse, sans sélection : menu sans navigation.
      const plain = await rightClick(driver, offset, "[data-role=assistant]");
      for (const action of NAVIGATION) {
        assert.ok(!plain.includes(action), `${action} dans ${plain}`);
      }

      // Texte sélectionné : Copy.
      await driver.execute(
        `const node = document.querySelector("[data-role=assistant]");
         const range = document.createRange();
         range.selectNodeContents(node);
         const selection = getSelection();
         selection.removeAllRanges();
         selection.addRange(range);`,
      );
      const selected = await rightClick(
        driver,
        offset,
        "[data-role=assistant]",
      );
      assert.ok(selected.includes("Copy"), `Copy absent : ${selected}`);
      for (const action of NAVIGATION) {
        assert.ok(!selected.includes(action), `${action} dans ${selected}`);
      }

      // Zone de saisie : coller.
      const composer = await rightClick(
        driver,
        offset,
        "textarea[aria-label='Message']",
      );
      assert.ok(composer.includes("Paste"), `Paste absent : ${composer}`);
      assert.ok(
        composer.includes("SelectAll"),
        `SelectAll absent : ${composer}`,
      );

      // Hors des zones : aucun menu (pas même l'inspecteur du build de
      // débogage).
      await pressKeys("ctrl+comma");
      await driver.waitFor("[data-testid=settings]");
      const outside = await rightClick(driver, offset, "main h1");
      console.log(
        "MENUS",
        JSON.stringify({ plain, selected, composer, outside }),
      );
      assert.deepEqual(outside, []);
    },
  );
} finally {
  await fake.stop();
}
