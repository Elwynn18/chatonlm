// Compte Ollama et recherche web dans l'app réelle (build e2e : frontend
// embarqué, CSP de production). Le faux serveur joue aussi ollama.com
// (CHATONLM_OLLAMA_COM_URL : page de connexion, image du compte) ; un faux
// xdg-open note les liens ouverts :
// - globe pour un modèle « tools » (chatonlm-search), absent sans cette
//   capacité (chatonlm-plain) ou quand le serveur a coupé le cloud ;
// - envoi avec recherche sans compte : arrêt avant toute requête à
//   /api/chat, erreur « Web search requires an Ollama account » avec deux
//   choix : « Generate without web search » coupe le globe et relance le
//   message à l'identique (pas de doublon, requête sans outils) ; « Sign In »
//   ouvre les réglages, focus sur le Sign In de la carte du compte (Entrée
//   ouvre la page /connect, avec launch=true), compte relu jusqu'à relié ;
//   de retour dans la conversation, « Retry » relance avec la recherche ;
// - envoi avec recherche : deux outils dans la requête, appel exécuté par le
//   serveur (/api/experimental/web_search), « Searching for … », bloc
//   « Search results for … » replié puis déplié (JSON), réponse finale qui
//   cite le résultat ; même affichage relu en base après rechargement ;
// - web_fetch (chatonlm-fetch) : adresse écrite par l'utilisateur, puis lien de
//   la page lue ; « Fetching for … » et « Fetch results for … » ;
// - Stop pendant une recherche lente : requête interrompue, appel gardé sans
//   résultat, la conversation repart ;
// - réglages, onglet Account : carte « Ollama account » reliée (nom,
//   e-mail, image en data:, Upgrade, Manage), Sign out, Sign In depuis la
//   carte puis relecture toutes les 5 s jusqu'au compte relié ; captures
//   web-search.png et settings-account.png ;
// - recherche coupée : requête sans outils ;
// - aucune violation de la CSP ni erreur ou avertissement dans la console.
import assert from "node:assert/strict";
import {
  existsSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { join } from "node:path";
import { ROOT, withApp } from "./lib/app.mjs";
import { startFake } from "./lib/fake.mjs";
import { pressKeys } from "./lib/keyboard.mjs";
import { PENDING_CALLS } from "./lib/page.mjs";

const binary = process.argv[2];
const fake = await startFake("ollama");

// Faux xdg-open : note le lien et réussit.
const fakeBin = mkdtempSync(join(ROOT, ".tools/tmp/xdg-open-"));
process.on("exit", () => rmSync(fakeBin, { recursive: true, force: true }));
writeFileSync(
  join(fakeBin, "xdg-open"),
  `#!/bin/sh\necho "$1" >> "${fakeBin}/opened"\nexit 0\n`,
  { mode: 0o755 },
);
const opened = () =>
  existsSync(join(fakeBin, "opened"))
    ? readFileSync(join(fakeBin, "opened"), "utf8").trim().split("\n")
    : [];

const CONNECT = `${fake.url}/connect?name=fake-host&key=ZmFrZQ&launch=true`;

const delay = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

/** `what` : texte, ou fonction appelée à l'échec (dernier état observé). */
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

/** Requêtes web reçues par le faux serveur, dans l'ordre. */
async function webRequests() {
  const response = await fetch(new URL("/fake/web", fake.url));
  const body = await response.json();
  return Array.isArray(body) ? body : body.web;
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

const violations = [];
const consoleErrors = [];
const consoleWarnings = [];
const WATCH_PAGE = `
  if (!window.__csp) {
    window.__csp = [];
    window.__errors = [];
    window.__warnings = [];
    document.addEventListener("securitypolicyviolation", (event) =>
      window.__csp.push(event.violatedDirective + " " + event.blockedURI));
    const error = console.error;
    console.error = (...args) => {
      window.__errors.push(args.map(String).join(" "));
      error.apply(console, args);
    };
    const warn = console.warn;
    console.warn = (...args) => {
      window.__warnings.push(args.map(String).join(" "));
      warn.apply(console, args);
    };
  }`;

async function collectViolations(driver) {
  const found = await driver.execute(
    `const found = { csp: window.__csp ?? [], errors: window.__errors ?? [],
       warnings: window.__warnings ?? [] };
     window.__csp = [];
     window.__errors = [];
     window.__warnings = [];
     return found;`,
  );
  violations.push(...found.csp);
  consoleErrors.push(...found.errors);
  consoleWarnings.push(...found.warnings);
}

/** Recharge la page une fois les appels au backend finis (voir thinking.test.mjs). */
async function reload(driver) {
  await collectViolations(driver);
  await waitUntil(
    () =>
      driver.execute(
        `${PENDING_CALLS}
         if (pending.length > 0) return false;
         window.__oldPage = true;
         location.reload();
         return true;`,
      ),
    "appels au backend toujours en cours avant le rechargement",
  );
  await waitUntil(
    async () => !(await driver.execute("return window.__oldPage === true")),
    "rechargement",
  );
  await driver.execute(WATCH_PAGE);
}

const shownModel = (driver) =>
  driver.execute(
    "return document.querySelector('[title=\"Select model\"]')?.textContent ?? null",
  );

/** Choisit un modèle (absent de la liste), puis recharge la page. */
async function useModel(driver, model) {
  await waitUntil(() => shownModel(driver), "un modèle affiché");
  await driver.invoke("update_settings", { patch: { selectedModel: model } });
  await reload(driver);
  let shown = null;
  await waitUntil(
    async () => (shown = await shownModel(driver)) === model,
    () => `modèle ${model} (affiché : ${shown})`,
  );
}

async function newChat(driver) {
  if (await exists(driver, "button[aria-label='Show sidebar']")) {
    await driver.click(await driver.find("button[aria-label='Show sidebar']"));
  }
  await driver.click(await driver.waitFor("a[title='New chat']"));
  await driver.waitFor("[data-testid=new-chat]");
}

const textarea = (driver) => driver.waitFor("textarea[aria-label='Message']");

/** Entrée dans la zone de saisie. */
const submit = (driver) =>
  driver.execute(
    `document.querySelector("textarea[aria-label='Message']").dispatchEvent(
       new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true }));`,
  );

async function send(driver, text) {
  await driver.type(await textarea(driver), text);
  await submit(driver);
}

/** Attend la fin de la réponse en cours (plus de bouton Stop). */
async function waitIdle(driver) {
  await waitUntil(
    async () => !(await exists(driver, "button[aria-label='Stop']")),
    "fin de la réponse",
    15000,
  );
}

/** État du globe : null s'il est absent. */
const globe = (driver) =>
  driver.execute(`
    const button = document.querySelector("button[aria-label='Web search']");
    return button && { pressed: button.getAttribute("aria-pressed"), title: button.title };`);

const SIGNED_OUT = "Web search requires an Ollama account";
/** Erreur « sans compte » affichée dans la conversation. */
const signedOutShown = (driver) =>
  driver.execute(
    `return [...document.querySelectorAll("[role=alert]")]
       .some((e) => e.textContent.includes(arguments[0]));`,
    [SIGNED_OUT],
  );

/** Textes des bulles de l'utilisateur, dans l'ordre. */
const userMessages = (driver) =>
  texts(driver, "[data-role=user] .whitespace-pre-wrap");

/** Le focus est sur le bouton Sign In de la carte du compte. */
const signInFocused = (driver) =>
  driver.execute(
    `const active = document.activeElement;
     return active !== null &&
       active.closest("[data-testid=ollama-account]") !== null &&
       active.getAttribute("aria-label") === "Sign In";`,
  );

/** Ligne d'appel et titres des résultats affichés, dans l'ordre. */
const toolCalls = (driver) => texts(driver, "[data-role=tool-calls] > *");
const toolResults = (driver) => texts(driver, "[data-role=tool] button");

/** Ouvre une conversation par la barre latérale. */
async function openChat(driver, id) {
  if (await exists(driver, "button[aria-label='Show sidebar']")) {
    await driver.click(await driver.find("button[aria-label='Show sidebar']"));
  }
  await driver.click(await driver.waitFor(`[data-chat-id='${id}'] a`));
  await waitUntil(
    async () =>
      (await driver.execute(
        "return (location.hash + location.pathname).match(/\\/c\\/([^/?#]+)/)?.[1] ?? null",
      )) === id,
    `conversation ${id} ouverte`,
  );
  await driver.waitFor("[data-testid=chat]");
}

const currentChatId = (driver) =>
  driver.execute(
    "return (location.hash + location.pathname).match(/\\/c\\/([^/?#]+)/)?.[1] ?? null",
  );

const account = (driver) =>
  driver.execute(
    `const card = document.querySelector("[data-testid=ollama-account]");
     if (!card) return null;
     return {
       text: card.textContent,
       buttons: [...card.querySelectorAll("button")].map((b) => b.textContent.trim() || b.getAttribute("aria-label")),
       avatar: card.querySelector("img")?.getAttribute("src") ?? null,
     };`,
  );

try {
  await withApp(
    binary,
    {
      name: "web-search",
      env: {
        OLLAMA_HOST: fake.url,
        CHATONLM_OLLAMA_COM_URL: fake.url,
        PATH: `${fakeBin}:${process.env.PATH}`,
      },
    },
    async (driver) => {
      await driver.waitFor("[data-testid=new-chat]");
      // Modèle de secours écrit par la page (session neuve) avant tout choix.
      await waitUntil(
        async () =>
          (await driver.invoke("get_settings")).selectedModel === "qwen3:0.6b",
        "modèle par défaut enregistré",
      );
      await driver.execute(WATCH_PAGE);
      await useModel(driver, "chatonlm-plain");
      // Sans capacité « tools » : pas de globe.
      assert.equal(await globe(driver), null);

      await useModel(driver, "chatonlm-search");
      assert.deepEqual(await waitUntil(() => globe(driver), "globe"), {
        pressed: "false",
        title: "Enable web search",
      });
      assert.equal(await signedOutShown(driver), false);

      // Activé sans compte : l'envoi s'arrête avant la génération, erreur
      // avec deux choix.
      await driver.click(await driver.find("button[aria-label='Web search']"));
      await waitUntil(
        async () => (await globe(driver))?.pressed === "true",
        "globe actif",
      );
      assert.equal((await globe(driver)).title, "Disable web search");
      assert.equal(
        (await driver.invoke("get_settings")).webSearchEnabled,
        true,
      );
      const question = "Who won the match?";
      await send(driver, question);
      await waitUntil(() => signedOutShown(driver), "erreur sans compte");
      await waitIdle(driver);
      assert.equal((await requestsOf("chatonlm-search")).length, 0);
      assert.deepEqual(await userMessages(driver), [question]);
      await driver.screenshot(".tools/shots/web-search-signed-out.png");

      // Générer sans recherche web : globe coupé, message relancé à
      // l'identique, sans outils ni doublon.
      assert.ok(
        await clickByText(driver, "button", "Generate without web search"),
      );
      await waitUntil(
        async () =>
          (await texts(driver, "[data-role=assistant]")).some((t) =>
            t.includes("Answered without web search."),
          ),
        "réponse sans recherche",
      );
      await waitIdle(driver);
      assert.equal(await signedOutShown(driver), false);
      assert.equal((await globe(driver)).pressed, "false");
      assert.equal(
        (await driver.invoke("get_settings")).webSearchEnabled,
        false,
      );
      const retried = await requestsOf("chatonlm-search");
      assert.equal(retried.length, 1);
      assert.equal(retried[0].tools, undefined);
      assert.deepEqual(await userMessages(driver), [question]);
      await reload(driver);
      await waitUntil(
        async () => (await userMessages(driver)).length === 1,
        "conversation relue",
      );
      assert.deepEqual(await userMessages(driver), [question]);

      // Sans compte, « Sign In » : réglages, focus sur le Sign In de la
      // carte ; Entrée ouvre la page de connexion ; compte relu jusqu'à relié.
      await newChat(driver);
      await driver.click(
        await driver.waitFor("button[aria-label='Web search']"),
      );
      await waitUntil(
        async () => (await globe(driver))?.pressed === "true",
        "globe de nouveau actif",
      );
      await send(driver, question);
      await waitUntil(() => signedOutShown(driver), "erreur sans compte");
      await waitIdle(driver);
      const signInChat = await currentChatId(driver);
      assert.ok(await clickByText(driver, "button", "Sign In"));
      await driver.waitFor("[data-testid=settings]");
      await waitUntil(() => signInFocused(driver), "focus sur Sign In");
      await pressKeys("enter");
      await waitUntil(
        () => opened().includes(CONNECT),
        () => `page de connexion ouverte : ${opened().join(", ")}`,
      );
      await post("/fake/account", { signedIn: true });
      await waitUntil(
        async () => (await account(driver))?.text.includes("ada@example.com"),
        "compte relu après la connexion",
        8000,
      );

      // Retour dans la conversation : compte relié, l'erreur propose
      // « Retry », qui relance le message avec la recherche.
      if (await exists(driver, "button[aria-label='Show sidebar']")) {
        await driver.click(
          await driver.find("button[aria-label='Show sidebar']"),
        );
      }
      await driver.click(await driver.waitFor("nav a[href$='/c/new']"));
      await driver.waitFor("[data-testid=new-chat]");
      await openChat(driver, signInChat);
      assert.equal(await signedOutShown(driver), true);
      await waitUntil(
        () => clickByText(driver, "button", "Retry"),
        "bouton Retry une fois relié",
      );
      const sentBefore = (await requestsOf("chatonlm-search")).length;
      await waitUntil(
        async () =>
          (await texts(driver, "[data-role=assistant]")).some((t) =>
            t.includes(`Result 1 for ${question}`),
          ),
        "réponse qui cite le résultat",
        15000,
      );
      await waitIdle(driver);
      assert.deepEqual(await toolCalls(driver), [`Searching for ${question}…`]);
      assert.deepEqual(await toolResults(driver), [
        `Search results for ${question}`,
      ]);
      assert.deepEqual(await userMessages(driver), [question]);
      const requests = (await requestsOf("chatonlm-search")).slice(sentBefore);
      assert.equal(requests.length, 2);
      assert.deepEqual(
        requests[0].tools.map((tool) => tool.function.name),
        ["web_search", "web_fetch"],
      );
      const toolMessage = requests[1].messages.at(-1);
      assert.equal(toolMessage.role, "tool");
      assert.equal(toolMessage.tool_name, "web_search");
      assert.ok(toolMessage.content.includes(`Result 1 for ${question}`));
      const searches = await webRequests();
      assert.equal(searches.length, 1);
      assert.equal(searches[0].path, "/api/experimental/web_search");
      assert.equal(searches[0].body.query, question);

      // Résultat replié, puis déplié : JSON des résultats.
      const header = await driver.find("[data-role=tool] button");
      assert.equal(
        await driver.execute(
          "return document.querySelector('[data-role=tool] button').getAttribute('aria-expanded')",
        ),
        "false",
      );
      await driver.click(header);
      await waitUntil(
        () =>
          driver.execute(
            "return document.querySelector('[data-role=tool] button').getAttribute('aria-expanded') === 'true'",
          ),
        "résultat déplié",
      );
      const json = (await texts(driver, "[data-role=tool] pre"))[0];
      assert.ok(json.includes(`"title": "Result 1 for ${question}"`), json);
      await delay(400);
      await driver.screenshot(".tools/shots/web-search.png");

      // Relu en base.
      const searchChat = await currentChatId(driver);
      await reload(driver);
      await waitUntil(
        async () => (await toolResults(driver)).length === 1,
        "résultat relu",
      );
      assert.deepEqual(await toolCalls(driver), [`Searching for ${question}…`]);
      assert.ok(
        (await texts(driver, "[data-role=assistant]")).some((t) =>
          t.includes("Found:"),
        ),
      );

      // web_fetch : adresse de l'utilisateur, puis lien de la page lue.
      await newChat(driver);
      await useModel(driver, "chatonlm-fetch");
      await send(driver, "Read https://example.com/page please");
      await waitUntil(
        async () =>
          (await texts(driver, "[data-role=assistant]")).some((t) =>
            t.includes("Found:"),
          ),
        "réponse après lecture des pages",
        15000,
      );
      await waitIdle(driver);
      assert.deepEqual(await toolCalls(driver), [
        "Fetching for https://example.com/page…",
        "Fetching for https://example.com/next…",
      ]);
      assert.deepEqual(await toolResults(driver), [
        "Fetch results for https://example.com/page",
        "Fetch results for https://example.com/next",
      ]);
      assert.deepEqual(
        (await webRequests()).slice(1).map((r) => [r.path, r.body.url]),
        [
          ["/api/experimental/web_fetch", "https://example.com/page"],
          ["/api/experimental/web_fetch", "https://example.com/next"],
        ],
      );

      // Stop pendant une recherche lente.
      await newChat(driver);
      await useModel(driver, "chatonlm-search");
      const disconnects = await fake.disconnects();
      await send(driver, "slow news");
      await waitUntil(
        async () =>
          (await toolCalls(driver)).includes("Searching for slow news…"),
        "appel lent en cours",
      );
      // Recherche partie vers le serveur, qui la fait attendre 30 s.
      await waitUntil(
        async () =>
          (await webRequests()).some((r) => r.body.query === "slow news"),
        "recherche lente reçue par le serveur",
      );
      await driver.click(await driver.waitFor("button[aria-label='Stop']"));
      await waitIdle(driver);
      await waitUntil(
        async () => (await fake.disconnects()) > disconnects,
        "recherche interrompue côté serveur",
      );
      await reload(driver);
      await waitUntil(
        async () => (await toolCalls(driver)).length === 1,
        "appel arrêté relu",
      );
      assert.deepEqual(await toolResults(driver), []);
      assert.equal(await exists(driver, "[data-role=tool]"), false);

      // Réglages : compte relié, dans l'onglet Account (Ctrl+, ouvre
      // General).
      await pressKeys("ctrl+comma");
      await driver.waitFor("[data-testid=settings]");
      assert.equal(await exists(driver, "[data-testid=ollama-account]"), false);
      await driver.click(
        await driver.waitFor("[data-testid=settings-tab-account]"),
      );
      let card = await waitUntil(async () => {
        const found = await account(driver);
        return found?.text.includes("ada@example.com") && found;
      }, "carte du compte relié");
      assert.ok(card.text.includes("ada"));
      assert.deepEqual(card.buttons, ["Upgrade", "Manage", "Sign out"]);
      assert.ok(card.avatar?.startsWith("data:image/png;base64,"), card.avatar);
      await driver.screenshot(".tools/shots/settings-account.png");
      assert.ok(
        await clickByText(
          driver,
          "[data-testid=ollama-account] button",
          "Manage",
        ),
      );
      assert.ok(
        await clickByText(
          driver,
          "[data-testid=ollama-account] button",
          "Upgrade",
        ),
      );
      await waitUntil(
        () =>
          opened().includes("https://ollama.com/settings") &&
          opened().includes("https://ollama.com/upgrade"),
        () => `Manage et Upgrade ouverts : ${opened().join(", ")}`,
      );

      // Sign out, puis Sign In depuis la carte : relecture toutes les 5 s.
      assert.ok(
        await clickByText(
          driver,
          "[data-testid=ollama-account] button",
          "Sign out",
        ),
      );
      card = await waitUntil(async () => {
        const found = await account(driver);
        return found?.text.includes("Not connected") && found;
      }, "carte déliée");
      assert.ok(card.text.includes("Ollama account"));
      const before = opened().filter((link) => link === CONNECT).length;
      await driver.click(
        await driver.find(
          "[data-testid=ollama-account] button[aria-label='Sign In']",
        ),
      );
      await waitUntil(
        () => opened().filter((link) => link === CONNECT).length > before,
        "page de connexion ouverte depuis la carte",
      );
      await post("/fake/account", { signedIn: true });
      await waitUntil(
        async () => (await account(driver))?.text.includes("ada@example.com"),
        "compte relu après la connexion",
        8000,
      );

      // Recherche coupée : requête sans outils.
      if (await exists(driver, "button[aria-label='Show sidebar']")) {
        await driver.click(
          await driver.find("button[aria-label='Show sidebar']"),
        );
      }
      await driver.click(await driver.waitFor("nav a[href$='/c/new']"));
      await driver.waitFor("[data-testid=new-chat]");
      await openChat(driver, searchChat);
      await driver.click(
        await driver.waitFor("button[aria-label='Web search']"),
      );
      await waitUntil(
        async () => (await globe(driver))?.pressed === "false",
        "globe coupé",
      );
      const sent = (await requestsOf("chatonlm-search")).length;
      await send(driver, "No search this time");
      await waitUntil(
        async () =>
          (await texts(driver, "[data-role=assistant]")).some((t) =>
            t.includes("Answered without web search."),
          ),
        "réponse sans recherche",
      );
      await waitIdle(driver);
      const plain = (await requestsOf("chatonlm-search")).slice(sent);
      assert.equal(plain.length, 1);
      assert.equal(plain[0].tools, undefined);

      // Cloud coupé par le serveur : plus de globe.
      await post("/fake/cloud", { disabled: true });
      await reload(driver);
      await waitUntil(() => shownModel(driver), "modèle affiché");
      await delay(1000);
      assert.equal(await globe(driver), null);
      await post("/fake/cloud", { disabled: false });

      await collectViolations(driver);
    },
  );
} finally {
  await fake.stop();
}

assert.deepEqual(violations, [], "violations de la CSP");
assert.deepEqual(consoleErrors, [], "erreurs dans la console");
assert.deepEqual(consoleWarnings, [], "avertissements dans la console");
console.log("web-search : OK");
