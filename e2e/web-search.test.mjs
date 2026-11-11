// Recherche web dans l'app réelle (build e2e), faite depuis la machine :
// faux moteurs (fake-search : DuckDuckGo, Brave, Mojeek, SearXNG) et un
// modèle qui cherche (scénario `chatonlm-websearch` du faux llama-server,
// servi comme modèle distant « Other server » : aucun moteur local requis) :
// - DuckDuckGo par défaut, sans clé : « Searching for … », bloc « Search
//   results for … », lien « About web search » sans « Powered by Brave
//   Search », réponse finale qui cite les extraits (le modèle les voit) ;
// - l'historique (relu en base) garde les titres et les liens, jamais les
//   extraits ;
// - DuckDuckGo bloque : l'envoi s'arrête avec le texte du blocage, « Web
//   search settings » et « Generate without web search » ; le premier ouvre
//   l'onglet Web search ;
// - dans l'onglet : Brave choisi, clé enregistrée (jamais relue par la page),
//   « Test » dit que le moteur répond ; la recherche suivante passe par Brave
//   d'abord (« Powered by Brave Search ») ;
// - aucune violation de la CSP ni erreur dans la console.
// Binaires fake-llama et fake-search construits par scripts/e2e-build.sh.
import assert from "node:assert/strict";
import { withApp } from "./lib/app.mjs";
import { startFakeProvider } from "./lib/provider.mjs";
import { startFakeSearch } from "./lib/search.mjs";

const binary = process.argv[2];
const MODEL = "chatonlm-websearch";
const ID = `@remote/custom/${MODEL}`;
const KEY = "sk-e2e-search-4b1d";
const BRAVE_KEY = "BSA-e2e-very-secret-77";
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

/** Violations de la CSP et erreurs de la console, notées par la page. */
const WATCH_PAGE = `
  if (!window.__csp) {
    window.__csp = [];
    window.__errors = [];
    document.addEventListener("securitypolicyviolation", (event) =>
      window.__csp.push(event.violatedDirective + " " + event.blockedURI));
    const error = console.error;
    console.error = (...args) => {
      window.__errors.push(args.map(String).join(" "));
      error.apply(console, args);
    };
  }`;

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
  await driver.execute(WATCH_PAGE);
}

async function send(driver, text) {
  const textarea = await driver.waitFor("textarea[aria-label='Message']");
  await driver.type(textarea, text);
  await driver.execute(
    `document.querySelector("textarea[aria-label='Message']").dispatchEvent(
       new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true }));`,
  );
}

/** Clique le bouton (ou le lien) dont le texte est `text`. */
async function clickText(driver, text, selector = "button") {
  await waitUntil(
    () =>
      driver.execute(
        `return [...document.querySelectorAll(arguments[0])].some(
           (b) => b.textContent.trim() === arguments[1]);`,
        [selector, text],
      ),
    `bouton « ${text} »`,
  );
  await driver.execute(
    `[...document.querySelectorAll(arguments[0])]
       .find((b) => b.textContent.trim() === arguments[1]).click();`,
    [selector, text],
  );
}

/**
 * Nouvelle conversation : entrée « Chat » de la barre latérale (montrée si
 * besoin), qui mène à /c/new.
 */
async function newChat(driver) {
  const shown = await driver.execute(
    "return Boolean(document.querySelector(\"button[aria-label='Show sidebar']\"))",
  );
  if (shown) {
    await driver.click(await driver.find("button[aria-label='Show sidebar']"));
  }
  await driver.click(await driver.waitFor("a[href$='/c/new']"));
  await driver.waitFor("[data-testid=new-chat]");
  await driver.waitFor("textarea[aria-label='Message']");
}

const provider = await startFakeProvider({ alias: MODEL, key: KEY });
const engines = await startFakeSearch();
try {
  await withApp(
    binary,
    { name: "web-search", env: engines.env },
    async (driver) => {
      await prepare(driver, provider);

      // 1. DuckDuckGo par défaut, sans clé.
      await send(driver, "chaton roux");
      await waitUntil(
        async () => (await bodyText(driver)).includes("Found: "),
        "réponse qui cite la recherche",
        30000,
      );
      let text = await bodyText(driver);
      assert.match(text, /Searching for chaton roux…/);
      assert.match(text, /Search results for chaton roux/);
      assert.match(text, /About web search/);
      assert.doesNotMatch(text, /Powered by Brave Search/);
      // Le modèle a vu les extraits (« Extrait n » du faux DuckDuckGo).
      assert.match(text, /Found: .*Extrait 1 \(duckduckgo\)/);
      await driver.screenshot(".tools/shots/web-search.png");
      let requests = await engines.requests();
      assert.deepEqual(
        requests.map((request) => [request.engine, request.query]),
        [["duckduckgo", "chaton roux"]],
      );
      // Historique : titres et liens, sans extrait ; moteur gardé.
      const chats = await driver.invoke("list_chats");
      const chat = await driver.invoke("get_chat", { id: chats[0].id });
      const calls = chat.messages.flatMap((message) => message.toolCalls);
      assert.equal(calls.length, 1);
      const stored = JSON.parse(calls[0].result);
      assert.equal(stored.provider, "duckduckgo");
      assert.ok(stored.results.length > 0);
      for (const result of stored.results) {
        assert.deepEqual(Object.keys(result).sort(), ["title", "url"]);
      }
      assert.ok(!calls[0].result.includes("Extrait"));

      // 2. DuckDuckGo bloque : arrêt, texte du blocage, accès aux réglages.
      await engines.setBehavior("duckduckgo", "blocked");
      await newChat(driver);
      await send(driver, "chaton noir");
      await waitUntil(
        async () =>
          (await bodyText(driver)).includes(
            "DuckDuckGo is limiting searches from this computer",
          ),
        "texte du blocage",
      );
      text = await bodyText(driver);
      assert.match(text, /Generate without web search/);
      assert.match(text, /Settings › Web search/);
      await clickText(driver, "Web search settings");
      await driver.waitFor("[data-testid=web-search-settings]");

      // 3. Onglet Web search : Brave, clé, essai.
      await driver.execute(
        `document.querySelector("input[type=radio][value=brave]").click();`,
      );
      const field = await driver.waitFor("input[type=password]");
      await driver.type(field, BRAVE_KEY);
      await clickText(driver, "Save");
      await waitUntil(
        async () => (await bodyText(driver)).includes("A key is saved."),
        "clé enregistrée",
      );
      // Jamais relue : ni dans le champ, ni dans la page.
      const html = await driver.execute("return document.body.innerHTML");
      assert.ok(!html.includes(BRAVE_KEY), "la clé est revenue dans la page");
      await clickText(driver, "Test");
      await waitUntil(
        async () =>
          (await bodyText(driver)).includes(
            "Brave Search answered. Web search works.",
          ),
        "essai de Brave",
      );
      await driver.screenshot(".tools/shots/web-search-settings.png");

      // 4. Recherche suivante : Brave d'abord (DuckDuckGo bloque toujours).
      await newChat(driver);
      await send(driver, "chaton blanc");
      await waitUntil(
        async () => (await bodyText(driver)).includes("Found: "),
        "réponse par Brave",
        30000,
      );
      text = await bodyText(driver);
      assert.match(text, /Powered by Brave Search/);
      assert.match(text, /Found: .*Extrait 1 \(brave\)/);
      requests = await engines.requests();
      const last = requests.at(-1);
      assert.deepEqual([last.engine, last.query], ["brave", "chaton blanc"]);

      const found = await driver.execute(
        "return { csp: window.__csp ?? [], errors: window.__errors ?? [] };",
      );
      assert.deepEqual(found.csp, [], "violations de la CSP");
      assert.deepEqual(found.errors, [], "erreurs de la console");
    },
  );
  console.log(
    "recherche web depuis la machine : DuckDuckGo, blocage, réglages, Brave",
  );
} finally {
  await engines.stop();
  await provider.stop();
}
