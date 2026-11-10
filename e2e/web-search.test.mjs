// Recherche web dans l'app réelle (build e2e), par le relais de ChatonLM :
// faux relais (fake-relay : vrai relais, faux moteurs Brave et Mojeek) et un
// modèle qui cherche (scénario `chatonlm-websearch` du faux llama-server,
// servi comme modèle distant « Other server » : aucun moteur local requis) :
// - envoi avec la recherche : inscription au relais (preuve de travail),
//   « Searching for … », bloc « Search results for … », mention « Powered by
//   Brave Search », réponse finale qui cite les extraits (le modèle les
//   voit) ;
// - l'historique (relu en base) garde les titres et les liens, jamais les
//   extraits ;
// - quota du jour épuisé (relais à une recherche par jour) : l'envoi
//   s'arrête avec le texte du relais et « Generate without web search » ;
// - service en pause : arrêt avec son texte ;
// - bulle du globe : recherches restantes ;
// - aucune violation de la CSP ni erreur dans la console.
// Écrit à l'étape 2 (lot L7), lancé à l'étape 3 : binaires fake-llama et
// fake-relay construits par scripts/e2e-build.sh.
import assert from "node:assert/strict";
import { withApp } from "./lib/app.mjs";
import { startFakeProvider, startFakeRelay } from "./lib/relay.mjs";

const binary = process.argv[2];
const MODEL = "chatonlm-websearch";
const ID = `@remote/custom/${MODEL}`;
const KEY = "sk-e2e-search-4b1d";
const delay = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

async function waitUntil(check, what, timeoutMs = 15000) {
  const deadline = Date.now() + timeoutMs;
  for (;;) {
    const value = await check();
    if (value) return value;
    if (Date.now() > deadline) throw new Error(`Délai dépassé : ${what}`);
    await delay(100);
  }
}

const bodyText = (driver) => driver.execute("return document.body.textContent");

/** Fournisseur, modèle choisi, recherche web activée. */
async function prepare(driver, provider) {
  await driver.waitFor("[data-testid=new-chat]");
  await driver.invoke("update_settings", { patch: { remoteModels: true } });
  await driver.invoke("save_remote_provider", {
    input: {
      id: null,
      kind: "custom",
      name: "Search box",
      baseUrl: provider.url,
      apiKey: KEY,
      enabled: true,
    },
  });
  await driver.invoke("update_settings", {
    patch: { selectedModel: ID, webSearchEnabled: true },
  });
  await driver.execute("location.reload()");
  await driver.waitFor("button[aria-label='Web search'][aria-pressed=true]");
}

async function send(driver, text) {
  const textarea = await driver.waitFor("textarea[aria-label='Message']");
  await driver.type(textarea, text);
  await driver.execute(
    `document.querySelector("textarea[aria-label='Message']").dispatchEvent(
       new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true }));`,
  );
}

const provider = await startFakeProvider({ alias: MODEL, key: KEY });
try {
  // 1. Recherche qui aboutit.
  const relay = await startFakeRelay();
  try {
    await withApp(
      binary,
      { name: "web-search", env: relay.env },
      async (driver) => {
        await prepare(driver, provider);
        await send(driver, "chaton roux");
        await waitUntil(
          async () => (await bodyText(driver)).includes("Found: "),
          "réponse qui cite la recherche",
          30000,
        );
        const text = await bodyText(driver);
        assert.match(text, /Searching for chaton roux…/);
        assert.match(text, /Search results for chaton roux/);
        assert.match(text, /Powered by Brave Search/);
        // Le modèle a vu les extraits (« Extrait n » du faux Brave).
        assert.match(text, /Found: .*Extrait 1/);
        await driver.screenshot(".tools/shots/web-search.png");
        // Historique : titres et liens, sans extrait.
        const chats = await driver.invoke("list_chats");
        const chat = await driver.invoke("get_chat", { id: chats[0].id });
        const calls = chat.messages.flatMap((message) => message.toolCalls);
        assert.equal(calls.length, 1);
        const stored = JSON.parse(calls[0].result);
        assert.equal(stored.provider, "brave");
        assert.ok(stored.results.length > 0);
        for (const result of stored.results) {
          assert.deepEqual(Object.keys(result).sort(), ["title", "url"]);
        }
        assert.ok(!calls[0].result.includes("Extrait"));
        // Bulle du globe : recherches restantes.
        const title = await driver.execute(
          "return document.querySelector(\"button[aria-label='Web search']\").title",
        );
        assert.match(title, /searches left today/);
      },
    );
  } finally {
    await relay.stop();
  }

  // 2. Quota du jour épuisé, puis service en pause.
  for (const [options, expected] of [
    [{ daily: 0 }, "Today's web searches have all been used"],
    [{ paused: true }, "Web search is paused for maintenance"],
  ]) {
    const refusing = await startFakeRelay(options);
    try {
      await withApp(
        binary,
        { name: "web-search-refused", env: refusing.env },
        async (driver) => {
          await prepare(driver, provider);
          await send(driver, "chaton roux");
          await waitUntil(
            async () => (await bodyText(driver)).includes(expected),
            expected,
          );
          assert.match(await bodyText(driver), /Generate without web search/);
        },
      );
    } finally {
      await refusing.stop();
    }
  }
  console.log("recherche web par le relais : résultats, historique, refus");
} finally {
  await provider.stop();
}
