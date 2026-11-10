// Réglages dans l'app réelle (faux moteur : fake-llama, chatonlm-simple) :
// - Ctrl+, (vraies touches), depuis la zone de saisie, ouvre la page ;
// - thème Light puis Dark : classe « dark » de <html>, enregistré, « Saved »
//   (captures claire et sombre de la page) ;
// - longueur de contexte : clic sur « 8k » et glisser jusqu'à « 32k » (vrai
//   pointeur de KWin), puis ← (vraie touche) : 16k ; le modèle est lancé
//   avec `-c 16384` et la demande de chat suivante porte la limite
//   max_tokens de la réponse (registre et /fake/requests du faux) ;
// - ligne Engine (moteur intégré) à la place de l'adresse du serveur ;
// - Reset : contexte et thème par défaut, modèle choisi gardé, la
//   conversation répond toujours.
import assert from "node:assert/strict";
import { setTimeout as delay } from "node:timers/promises";
import { withApp } from "./lib/app.mjs";
import { fakeModel, startEngine } from "./lib/fake.mjs";
import { pressKeys } from "./lib/keyboard.mjs";
import { pointerOffset } from "./lib/pointer.mjs";

const binary = process.argv[2];
const MODEL = fakeModel("simple");
const engine = startEngine([MODEL]);

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

/** Tape un message dans la zone de saisie et l'envoie (Entrée). */
async function send(driver, text) {
  await driver.type(
    await driver.waitFor("textarea[aria-label='Message']"),
    text,
  );
  await driver.execute(
    `document.querySelector("textarea[aria-label='Message']").dispatchEvent(
       new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true }));`,
  );
}

/** Attend `count` réponses finies, la dernière contenant `text`. */
async function waitReply(driver, text, count) {
  await waitUntil(async () => {
    const replies = await driver.execute(
      "return [...document.querySelectorAll('[data-role=assistant]')].map((e) => e.textContent)",
    );
    return (
      replies.length === count &&
      replies.at(-1).includes(text) &&
      !(await exists(driver, "[aria-label='Stop']"))
    );
  }, `réponse « ${text} »`);
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

await withApp(binary, { name: "settings", env: engine.env }, async (driver) => {
  await driver.waitFor("[data-testid=new-chat]");
  // Modèle de secours enregistré dès que la liste est lue.
  const model = await waitUntil(
    async () => (await driver.invoke("get_settings")).selectedModel,
    "modèle choisi au départ",
  );

  // Ctrl+, depuis la zone de saisie.
  await driver.click(await driver.waitFor("textarea[aria-label='Message']"));
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

  // Le modèle est lancé avec ce contexte, et la demande suivante porte
  // la limite de la réponse.
  assert.equal(model, MODEL, "modèle choisi au départ");
  await toChat(driver);
  await send(driver, "Hello");
  const request = await waitUntil(
    async () => (await engine.chatRequests(MODEL)).at(-1),
    "demande de chat",
  );
  const args = (await engine.args(MODEL)) ?? [];
  assert.equal(args[args.indexOf("-c") + 1], "16384", JSON.stringify(args));
  // Limite de la réponse à ce qui reste du contexte (propre à ChatonLM,
  // compact.rs).
  assert.ok(
    request.max_tokens > 16000 && request.max_tokens < 16384,
    `max_tokens ${request.max_tokens}`,
  );
  await waitReply(driver, "Hello! You said: Hello", 1);

  // Moteur intégré : la ligne Engine (version, accélération) à la place
  // de l'adresse du serveur d'Ollama.
  await openSettingsWithKeys(driver);
  await driver.waitFor(
    "[data-testid=settings] [data-testid=engine-acceleration]",
  );

  // Reset.
  assert.ok(
    await clickByText(
      driver,
      "[data-testid=settings] button",
      "Reset to defaults",
    ),
  );
  const reset = await waitSettings(driver, {
    contextLength: 0,
    theme: "system",
  });
  assert.equal(reset.selectedModel, model);
  await waitSaved(driver);
  assert.equal(
    await isDark(driver),
    await driver.execute(
      "return matchMedia('(prefers-color-scheme: dark)').matches",
    ),
  );
  await toChat(driver);
  await send(driver, "Hello again");
  await waitReply(driver, "Hello! You said: Hello again", 1);
});
