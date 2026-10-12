// Réglages dans l'app réelle (faux serveur) :
// - Ctrl+, (vraies touches), depuis la zone de saisie, ouvre la page ;
// - thème Light puis Dark : classe « dark » de <html>, enregistré, « Saved »
//   (captures claire et sombre de la page) ;
// - longueur de contexte : clic sur « 8k » et glisser jusqu'à « 32k » (vrai
//   pointeur de KWin), puis ← (vraie touche) : 16k ; la requête /api/chat
//   suivante porte options.num_ctx 16384 (GET /fake/requests) ;
// - adresse : invalide refusée avec le message du moteur ; port fermé
//   enregistré, la conversation signale alors qu'Ollama ne répond plus ;
// - Reset : adresse, contexte et thème par défaut, modèle choisi gardé, la
//   conversation joint de nouveau le serveur.
import assert from "node:assert/strict";
import { setTimeout as delay } from "node:timers/promises";
import { freePort, withApp } from "./lib/app.mjs";
import { startFake } from "./lib/fake.mjs";
import { pressKeys } from "./lib/keyboard.mjs";
import { pointerOffset } from "./lib/pointer.mjs";

const binary = process.argv[2];
const fake = await startFake("ollama");
const closed = await freePort();

async function waitUntil(check, what, timeoutMs = 10000) {
  const deadline = Date.now() + timeoutMs;
  for (;;) {
    const value = await check();
    if (value) return value;
    if (Date.now() > deadline) throw new Error(`Délai dépassé : ${what}`);
    await delay(100);
  }
}

const exists = (driver, css) =>
  driver.execute("return document.querySelector(arguments[0]) !== null", [css]);

const isDark = (driver) =>
  driver.execute("return document.documentElement.classList.contains('dark')");

/** Attend que le backend ait enregistré `expected` (champs donnés). */
async function waitSettings(driver, expected) {
  let last;
  await waitUntil(
    async () => {
      last = await driver.invoke("get_settings");
      return Object.entries(expected).every(
        ([key, value]) => last[key] === value,
      );
    },
    () =>
      `réglages ${JSON.stringify(expected)} (lus : ${JSON.stringify(last)})`,
  );
  return last;
}

/** Badge « Saved » affiché, puis retiré pour le contrôle suivant. */
async function waitSaved(driver) {
  await waitUntil(
    () =>
      driver.execute(
        "return document.querySelector('[role=status]')?.textContent === 'Saved'",
      ),
    "badge Saved",
  );
}

async function openSettingsWithKeys(driver) {
  await pressKeys("ctrl+comma");
  await driver.waitFor("[data-testid=settings]");
}

async function toChat(driver) {
  if (await exists(driver, "button[aria-label='Show sidebar']")) {
    await driver.click(await driver.find("button[aria-label='Show sidebar']"));
  }
  await driver.click(await driver.waitFor("nav a[href$='/c/new']"));
  await driver.waitFor("[data-testid=new-chat]");
}

const clickByText = (driver, css, text) =>
  driver.execute(
    `const found = [...document.querySelectorAll(arguments[0])]
       .find((e) => e.textContent.trim() === arguments[1]);
     found?.click();
     return found !== undefined;`,
    [css, text],
  );

/** Centre (coordonnées de la page) de la zone du cran `label` du curseur. */
const notch = (driver, label) =>
  driver.execute(
    `const text = [...document.querySelectorAll("[data-testid=settings] div")]
       .find((e) => e.children.length === 0 && e.textContent === arguments[0]);
     const r = text.previousElementSibling.getBoundingClientRect();
     return [Math.round(r.left + r.width / 2), Math.round(r.top + r.height / 2)];`,
    [label],
  );

try {
  await withApp(
    binary,
    { name: "settings", env: { OLLAMA_HOST: fake.url } },
    async (driver) => {
      await driver.waitFor("[data-testid=new-chat]");
      // Modèle de secours enregistré dès que la liste est lue.
      const model = await waitUntil(
        async () => (await driver.invoke("get_settings")).selectedModel,
        "modèle choisi au départ",
      );

      // Ctrl+, depuis la zone de saisie.
      await driver.click(
        await driver.waitFor("textarea[aria-label='Message']"),
      );
      await openSettingsWithKeys(driver);
      assert.equal(
        await driver.execute("return document.querySelector('h1').textContent"),
        "Settings",
      );

      // Thème.
      assert.ok(await clickByText(driver, "label", "Light"));
      await waitSettings(driver, { theme: "light" });
      await waitSaved(driver);
      assert.equal(await isDark(driver), false);
      await delay(500); // transition-all (300 ms) de la zone principale
      await driver.screenshot(".tools/shots/settings-light.png");
      assert.ok(await clickByText(driver, "label", "Dark"));
      await waitSettings(driver, { theme: "dark" });
      assert.equal(await isDark(driver), true);
      await delay(500); // transition-all (300 ms) de la zone principale
      await driver.screenshot(".tools/shots/settings-dark.png");

      // Longueur de contexte : clic, glisser, clavier.
      const [dx, dy] = await pointerOffset(driver);
      const [x8, y8] = await notch(driver, "8k");
      await pressKeys(
        [
          `pointer:${x8 + dx},${y8 + dy}`,
          "wait:100",
          "button:down",
          "wait:100",
          "button:up",
        ],
        { settleMs: 300 },
      );
      await waitSettings(driver, { contextLength: 8192 });
      const [x32] = await notch(driver, "32k");
      const steps = [];
      for (let i = 1; i <= 10; i++) {
        const x = Math.round(x8 + ((x32 - x8) * i) / 10);
        steps.push(`pointer:${x + dx},${y8 + dy}`, "wait:30");
      }
      await pressKeys(
        [
          `pointer:${x8 + dx},${y8 + dy}`,
          "wait:100",
          "button:down",
          "wait:100",
          ...steps,
          "wait:200",
          "button:up",
        ],
        { settleMs: 300 },
      );
      await waitSettings(driver, { contextLength: 32768 });
      await driver.execute("document.querySelector('[role=slider]').focus()");
      await pressKeys("left");
      await waitSettings(driver, { contextLength: 16384 });
      assert.equal(
        await driver.execute(
          "return document.querySelector('[role=slider]').getAttribute('aria-valuetext')",
        ),
        "16k",
      );

      // La requête suivante porte num_ctx.
      await toChat(driver);
      await driver.type(
        await driver.waitFor("textarea[aria-label='Message']"),
        "Hello",
      );
      await driver.execute(
        `document.querySelector("textarea[aria-label='Message']").dispatchEvent(
           new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true }));`,
      );
      const request = await waitUntil(async () => {
        const response = await fetch(new URL("/fake/requests", fake.url));
        return (await response.json()).at(-1);
      }, "requête /api/chat");
      assert.deepEqual(request.options, { num_ctx: 16384 });
      await waitUntil(
        async () => !(await exists(driver, "[aria-label='Stop']")),
        "fin de la réponse",
      );

      // Adresse : refusée, puis port fermé.
      await openSettingsWithKeys(driver);
      const input = await driver.waitFor(
        "[data-testid=settings] input[type=text], [data-testid=settings] input:not([type])",
      );
      const save = async () =>
        assert.ok(
          await clickByText(driver, "[data-testid=settings] button", "Save"),
        );
      await driver.clear(input);
      await driver.type(input, "ftp://example.com");
      await save();
      const alert = await driver.waitFor("[data-testid=settings] [role=alert]");
      assert.match(await driver.text(alert), /unsupported scheme ftp/);
      const unreachable = `http://127.0.0.1:${closed}`;
      await driver.clear(input);
      await driver.type(input, unreachable);
      await save();
      await waitSettings(driver, { serverUrl: unreachable });
      await waitSaved(driver);
      await toChat(driver);
      await waitUntil(
        () =>
          driver.execute(
            "return [...document.querySelectorAll('[role=alert]')].some((e) => e.textContent.includes(\"Ollama isn't responding.\"))",
          ),
        "bannière « Ollama isn't responding. »",
      );

      // Reset.
      await openSettingsWithKeys(driver);
      assert.ok(
        await clickByText(
          driver,
          "[data-testid=settings] button",
          "Reset to defaults",
        ),
      );
      const reset = await waitSettings(driver, {
        serverUrl: "",
        contextLength: 0,
        theme: "system",
      });
      assert.equal(reset.effectiveServerUrl, fake.url);
      assert.equal(reset.selectedModel, model);
      await waitSaved(driver);
      assert.equal(
        await isDark(driver),
        await driver.execute(
          "return matchMedia('(prefers-color-scheme: dark)').matches",
        ),
      );
      assert.equal(
        await driver.execute(
          "return document.querySelector('[data-testid=settings] input:not([type=radio])').value",
        ),
        "",
      );
      await toChat(driver);
      await waitUntil(
        async () =>
          !(await driver.execute(
            "return [...document.querySelectorAll('[role=alert]')].some((e) => e.textContent.includes(\"Ollama isn't responding.\"))",
          )),
        "bannière toujours là après le Reset",
      );
    },
  );
} finally {
  await fake.stop();
}
