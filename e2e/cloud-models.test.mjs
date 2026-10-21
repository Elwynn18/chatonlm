// Modèles cloud d'Ollama dans l'app réelle (build e2e), face au faux serveur
// qui joue aussi ollama.com (compte délié au départ, modèles « …cloud » :
// 401 sans compte, réponse une fois relié) :
// - réglage « Use cloud models » coupé par défaut : aucun modèle cloud dans
//   le sélecteur ;
// - activé dans Settings › Models : les modèles cloud recommandés par le
//   serveur arrivent en tête, avec un nuage et le forfait payant exigé
//   (« Pro ») ; capture cloud-models-picker.png ;
// - envoi sans compte : arrêt avant toute requête à /api/chat, erreur
//   « Cloud models require an Ollama account » et « Sign In », qui ouvre la
//   carte du compte ; une fois relié, de retour dans la conversation,
//   « Retry » relance le message, qui reçoit sa réponse ;
// - réglage coupé de nouveau : le modèle cloud choisi laisse la place au
//   modèle installé le plus récent, et le sélecteur ne les montre plus ;
// - aucune erreur ni avertissement dans la console.
import assert from "node:assert/strict";
import { withApp } from "./lib/app.mjs";
import { startFake } from "./lib/fake.mjs";

const binary = process.argv[2];
const fake = await startFake("ollama");

const MODEL = "kimi-k2.6:cloud";
const CLOUD = "Cloud model: runs on ollama.com";
const SIGNED_OUT = "Cloud models require an Ollama account";
const HELLO = "Say hello in one word.";
const SWITCH = "button[role=switch][aria-label='Use cloud models']";

const delay = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

async function waitUntil(check, what, timeoutMs = 10000) {
  const deadline = Date.now() + timeoutMs;
  for (;;) {
    const value = await check();
    if (value) return value;
    if (Date.now() > deadline) {
      throw new Error(
        `Délai dépassé : ${typeof what === "function" ? what() : what}`,
      );
    }
    await delay(100);
  }
}

async function post(path, body) {
  const response = await fetch(new URL(path, fake.url), {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(body),
  });
  assert.ok(response.ok, `${path} : ${response.status}`);
}

/** Corps des /api/chat reçus par le faux serveur, d'un modèle. */
async function requestsOf(model) {
  const response = await fetch(new URL("/fake/requests", fake.url));
  return (await response.json()).filter((request) => request.model === model);
}

const exists = (driver, css) =>
  driver.execute("return document.querySelector(arguments[0]) !== null", [css]);

const texts = (driver, css) =>
  driver.execute(
    "return [...document.querySelectorAll(arguments[0])].map((e) => e.textContent)",
    [css],
  );

const clickByText = (driver, css, text) =>
  driver.execute(
    `const found = [...document.querySelectorAll(arguments[0])]
       .find((e) => e.textContent.trim() === arguments[1]);
     found?.click();
     return found !== undefined;`,
    [css, text],
  );

const WATCH_PAGE = `
  if (!window.__errors) {
    window.__errors = [];
    const error = console.error;
    console.error = (...args) => {
      window.__errors.push(args.map(String).join(" "));
      error.apply(console, args);
    };
    const warn = console.warn;
    console.warn = (...args) => {
      window.__errors.push(args.map(String).join(" "));
      warn.apply(console, args);
    };
  }`;

const shownModel = (driver) =>
  driver.execute(
    "return document.querySelector('[title=\"Select model\"]')?.textContent ?? null",
  );

/** Lignes du sélecteur ouvert : texte et nuage. */
const pickerRows = (driver) =>
  driver.execute(
    `return [...document.querySelectorAll("[role=option]")].map((o) => ({
       text: o.textContent,
       cloud: o.querySelector("[aria-label='${CLOUD}']") !== null,
     }));`,
  );

/** Ouvre le sélecteur et attend ses lignes ; `first` : texte de la première. */
async function openPicker(driver, first) {
  await driver.click(await driver.waitFor("button[title='Select model']"));
  let rows = [];
  await waitUntil(
    async () => {
      rows = await pickerRows(driver);
      return rows.length > 0 && rows[0].text === first;
    },
    () => `première ligne ${first} (vu : ${JSON.stringify(rows)})`,
  );
  return rows;
}

async function closePicker(driver) {
  await driver.click(await driver.find("button[title='Select model']"));
  await waitUntil(
    async () => !(await exists(driver, "[role=listbox]")),
    "sélecteur fermé",
  );
}

async function showSidebar(driver) {
  if (await exists(driver, "button[aria-label='Show sidebar']")) {
    await driver.click(await driver.find("button[aria-label='Show sidebar']"));
  }
}

async function modelsSettings(driver) {
  await showSidebar(driver);
  await driver.click(await driver.waitFor("nav a[href='/settings']"));
  await driver.click(await driver.waitFor("[data-testid=settings-tab-models]"));
  await driver.waitFor(SWITCH);
}

const switchState = (driver) =>
  driver.execute(
    "return document.querySelector(arguments[0])?.getAttribute('aria-checked') ?? null",
    [SWITCH],
  );

async function newChat(driver) {
  await showSidebar(driver);
  await driver.click(await driver.waitFor("nav a[href$='/c/new']"));
  await driver.waitFor("[data-testid=new-chat]");
}

const submit = (driver) =>
  driver.execute(
    `document.querySelector("textarea[aria-label='Message']").dispatchEvent(
       new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true }));`,
  );

async function send(driver, text) {
  await driver.type(
    await driver.waitFor("textarea[aria-label='Message']"),
    text,
  );
  await submit(driver);
}

async function waitIdle(driver) {
  await waitUntil(
    async () => !(await exists(driver, "button[aria-label='Stop']")),
    "fin de la réponse",
    15000,
  );
}

const alertShown = (driver, text) =>
  driver.execute(
    `return [...document.querySelectorAll("[role=alert]")]
       .some((e) => e.textContent.includes(arguments[0]));`,
    [text],
  );

const currentChatId = (driver) =>
  driver.execute(
    "return (location.hash + location.pathname).match(/\\/c\\/([^/?#]+)/)?.[1] ?? null",
  );

async function openChat(driver, id) {
  await showSidebar(driver);
  await driver.click(await driver.waitFor(`[data-chat-id='${id}'] a`));
  await waitUntil(
    async () => (await currentChatId(driver)) === id,
    `conversation ${id} ouverte`,
  );
  await driver.waitFor("[data-testid=chat]");
}

try {
  await withApp(
    binary,
    {
      name: "cloud-models",
      env: { OLLAMA_HOST: fake.url, CHATONLM_OLLAMA_COM_URL: fake.url },
    },
    async (driver) => {
      await driver.waitFor("[data-testid=new-chat]");
      await driver.execute(WATCH_PAGE);
      await waitUntil(
        async () => (await shownModel(driver)) === "qwen3:0.6b",
        "modèle installé choisi par défaut",
      );
      assert.equal((await driver.invoke("get_settings")).cloudModels, false);

      // Coupé : aucun modèle cloud dans le sélecteur.
      let rows = await openPicker(driver, "gemma4");
      assert.deepEqual(
        rows.map((row) => row.text),
        ["gemma4", "qwen3.5", "qwen3:0.6b", "qwen3.5:0.8b"],
      );
      assert.ok(rows.every((row) => !row.cloud));
      await closePicker(driver);

      // Activé dans Settings › Models.
      await modelsSettings(driver);
      assert.equal(await switchState(driver), "false");
      await driver.click(await driver.find(SWITCH));
      await waitUntil(
        async () => (await switchState(driver)) === "true",
        "réglage activé",
      );
      assert.equal((await driver.invoke("get_settings")).cloudModels, true);
      await driver.screenshot(".tools/shots/cloud-models-setting.png");

      await newChat(driver);
      await driver.execute(WATCH_PAGE);
      rows = await openPicker(driver, `${MODEL}Pro`);
      assert.deepEqual(rows, [
        { text: `${MODEL}Pro`, cloud: true },
        { text: "glm-5.1:cloudPro", cloud: true },
        { text: "qwen3.5:cloud", cloud: true },
        { text: "minimax-m2.7:cloudPro", cloud: true },
        { text: "gemma4", cloud: false },
        { text: "qwen3.5", cloud: false },
        { text: "qwen3:0.6b", cloud: false },
        { text: "qwen3.5:0.8b", cloud: false },
      ]);
      await driver.screenshot(".tools/shots/cloud-models-picker.png");
      assert.ok(await clickByText(driver, "[role=option]", `${MODEL}Pro`));
      await waitUntil(
        async () => (await shownModel(driver)) === MODEL,
        "modèle cloud choisi",
      );

      // Sans compte : arrêt avant toute requête au modèle.
      await send(driver, HELLO);
      await waitUntil(
        () => alertShown(driver, SIGNED_OUT),
        "erreur sans compte",
      );
      await waitIdle(driver);
      assert.deepEqual(await requestsOf(MODEL), []);
      const chat = await currentChatId(driver);
      await driver.screenshot(".tools/shots/cloud-models-signed-out.png");
      assert.ok(await clickByText(driver, "button", "Sign In"));
      await driver.waitFor("[data-testid=ollama-account]");
      await post("/fake/account", { signedIn: true });

      // De retour dans la conversation, une fois relié : « Retry ».
      await newChat(driver);
      await openChat(driver, chat);
      assert.equal(await alertShown(driver, SIGNED_OUT), true);
      await waitUntil(
        () => clickByText(driver, "button", "Retry"),
        "bouton Retry une fois relié",
      );
      await waitUntil(
        async () =>
          (await texts(driver, "[data-role=assistant]")).some(
            (text) => text.trim().length > 0,
          ),
        "réponse du modèle cloud",
        15000,
      );
      await waitIdle(driver);
      assert.equal(await alertShown(driver, SIGNED_OUT), false);
      assert.equal((await requestsOf(MODEL)).length, 1);
      assert.deepEqual(
        await texts(driver, "[data-role=user] .whitespace-pre-wrap"),
        [HELLO],
      );

      // Coupé de nouveau : le modèle cloud laisse la place, et disparaît.
      await modelsSettings(driver);
      await driver.click(await driver.find(SWITCH));
      await waitUntil(
        async () => (await switchState(driver)) === "false",
        "réglage coupé",
      );
      await newChat(driver);
      await waitUntil(
        async () => (await shownModel(driver)) === "qwen3:0.6b",
        "modèle installé repris",
      );
      assert.equal(
        (await driver.invoke("get_settings")).selectedModel,
        "qwen3:0.6b",
      );
      rows = await openPicker(driver, "gemma4");
      assert.ok(rows.every((row) => !row.cloud));
      await closePicker(driver);

      const errors = await driver.execute("return window.__errors ?? []");
      assert.deepEqual(errors, [], "erreurs dans la console");
    },
  );
  console.log("cloud-models : OK");
} finally {
  await fake.stop();
}
