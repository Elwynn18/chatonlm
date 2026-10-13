// Easy Link dans l'app réelle (build e2e), avec le faux Ollama et les faux
// Telegram et Discord (binaire fake-messengers) :
// - Settings > Easy Link : un jeton refusé est dit, le bon relie le bot
//   (nom, « Connected ») et ne reste nulle part dans la page ;
// - « Link an account » montre un code ; envoyé au bot, il lie le compte, qui
//   apparaît dans la liste sans rechargement ;
// - un message du compte lié reçoit la réponse du modèle dans la messagerie,
//   et la conversation apparaît dans la barre latérale avec sa marque,
//   pendant que la page est ailleurs ;
// - un inconnu reçoit l'avis « bot privé » et rien d'autre ;
// - Discord : même parcours par message privé, commande `!status` ;
// - après un redémarrage de l'app, le bot écoute de nouveau sans rien régler ;
// - aucune violation de la CSP ni erreur ou avertissement dans la console.
import assert from "node:assert/strict";
import { withApp } from "./lib/app.mjs";
import { startFake } from "./lib/fake.mjs";
import { FAKE_TOKENS, startFakeMessenger } from "./lib/messenger.mjs";

const binary = process.argv[2];
const fake = await startFake("ollama");
const telegram = await startFakeMessenger("telegram");
const discord = await startFakeMessenger("discord");
const env = {
  OLLAMA_HOST: fake.url,
  CHATONLM_TELEGRAM_URL: telegram.url,
  CHATONLM_DISCORD_URL: discord.url,
};
const MODEL = "qwen3:0.6b";

const delay = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

/** `what` : texte, ou fonction appelée à l'échec (dernier état observé). */
async function waitUntil(check, what, timeoutMs = 20000) {
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

/** Change de page comme un lien du routeur (pushState que TanStack suit). */
const navigate = (driver, path) =>
  driver.execute(
    `const index = (history.state?.__TSR_index ?? 0) + 1;
     const key = Math.random().toString(36).slice(2);
     history.pushState({ key, __TSR_key: key, __TSR_index: index }, "", arguments[0]);`,
    [path],
  );

const card = (platform) => `[data-testid=easy-link-${platform}]`;

/** Texte d'un élément de la carte d'une messagerie, ou null. */
const inCard = (driver, platform, selector) =>
  driver.execute(
    `return document.querySelector(arguments[0] + " " + arguments[1])?.textContent ?? null`,
    [card(platform), selector],
  );

/** Clique le bouton de la carte dont le texte est `label`. */
const press = (driver, platform, label) =>
  driver.execute(
    `const button = [...document.querySelectorAll(arguments[0] + " button")]
       .find((b) => b.textContent.trim() === arguments[1]);
     if (!button || button.disabled) return false;
     button.click();
     return true;`,
    [card(platform), label],
  );

/** Colle un jeton dans la carte et demande la connexion. */
async function connect(driver, platform, token) {
  const input = await driver.waitFor(`${card(platform)} input[type=password]`);
  await driver.clear(input);
  await driver.type(input, token);
  await waitUntil(
    () => press(driver, platform, "Connect"),
    `bouton Connect de ${platform}`,
  );
}

/** Demande un code d'appairage et le rend (six chiffres). */
async function pairingCode(driver, platform) {
  await waitUntil(
    () => press(driver, platform, "Link an account"),
    `bouton Link an account de ${platform}`,
  );
  const shown = await waitUntil(
    () => inCard(driver, platform, "[data-testid=easy-link-code]"),
    "code d'appairage",
  );
  const code = shown.replace(/\D/g, "");
  assert.match(code, /^\d{6}$/, shown);
  return code;
}

const linkedNames = (driver, platform) =>
  driver.execute(
    `return [...document.querySelectorAll(arguments[0] + " [data-testid=easy-link-account]")]
       .map((row) => row.textContent)`,
    [card(platform)],
  );

/** Conversations de la barre latérale qui portent une marque de messagerie. */
const markedChats = (driver) =>
  driver.execute(
    `return [...document.querySelectorAll("[data-testid=chat-source]")].map((mark) => ({
       label: mark.getAttribute("aria-label") ?? mark.getAttribute("title"),
       row: mark.closest("a")?.textContent ?? "",
     }))`,
  );

try {
  await withApp(binary, { name: "easy-link", env }, async (driver) => {
    await driver.waitFor("[data-testid=new-chat]");
    await driver.execute(WATCH_PAGE);
    await driver.invoke("update_settings", {
      patch: { selectedModel: MODEL },
    });
    await navigate(driver, "/settings?section=easyLink");
    await driver.waitFor(card("telegram"));
    await driver.waitFor(card("discord"));
    assert.equal(
      await driver.execute(
        "return document.querySelector('[data-testid=settings-tab-easyLink]')?.getAttribute('aria-current')",
      ),
      "page",
    );

    // Jeton refusé par Telegram : dit dans la carte, rien n'est enregistré.
    await connect(driver, "telegram", "1:not-the-token-not-the-token");
    const refusal = await waitUntil(
      () => inCard(driver, "telegram", "[role=alert]"),
      "refus du jeton",
    );
    assert.match(refusal, /Telegram refused this token/);
    assert.equal(
      (await driver.invoke("easy_link_state")).bots[0].configured,
      false,
    );

    // Bon jeton : bot relié, nommé, connecté.
    await connect(driver, "telegram", FAKE_TOKENS.telegram);
    await waitUntil(
      async () =>
        (
          await inCard(driver, "telegram", "[data-testid=easy-link-bot-name]")
        )?.includes("chatonlm_test_bot"),
      "nom du bot Telegram",
    );
    await waitUntil(
      async () =>
        (
          await inCard(driver, "telegram", "[data-testid=easy-link-status]")
        )?.includes("Connected"),
      "Telegram connecté",
    );
    // Le jeton n'est plus nulle part : ni dans la page, ni dans l'état que
    // le moteur lui donne.
    assert.equal(
      await driver.execute(
        `const token = arguments[0];
           return document.documentElement.outerHTML.includes(token)
             || [...document.querySelectorAll("input")].some((i) => i.value.includes(token));`,
        [FAKE_TOKENS.telegram],
      ),
      false,
      "jeton encore dans la page",
    );
    assert.ok(
      !JSON.stringify(await driver.invoke("easy_link_state")).includes(
        FAKE_TOKENS.telegram,
      ),
    );
    await driver.screenshot(".tools/shots/easy-link-connected.png");

    // Un inconnu : un avis, rien d'autre.
    await telegram.message({ user: "666", name: "Eve", text: "hello bot" });
    await waitUntil(
      async () =>
        (await telegram.shown()).some((t) => t.includes("This bot is private")),
      "avis au compte inconnu",
    );

    // Appairage : le code affiché, envoyé au bot, lie le compte.
    const code = await pairingCode(driver, "telegram");
    await driver.screenshot(".tools/shots/easy-link-pairing.png");
    await telegram.message({
      user: "1001",
      name: "Ana",
      text: `/start ${code}`,
    });
    await waitUntil(
      async () => (await linkedNames(driver, "telegram")).length === 1,
      "compte lié dans la liste",
    );
    assert.match((await linkedNames(driver, "telegram"))[0], /Ana/);
    assert.equal(
      await inCard(driver, "telegram", "[data-testid=easy-link-code]"),
      null,
      "le code reste affiché après la liaison",
    );
    await driver.screenshot(".tools/shots/easy-link-linked.png");

    // La page est ailleurs : le message arrive par Telegram, la réponse y
    // repart, et la conversation apparaît dans la barre latérale, marquée.
    await navigate(driver, "/c/new");
    await driver.waitFor("[data-testid=new-chat]");
    await driver.click(
      await driver.waitFor("button[aria-label='Show sidebar']"),
    );
    const before = (await telegram.shown()).length;
    await telegram.message({
      user: "1001",
      name: "Ana",
      text: "Hello from my phone",
    });
    await waitUntil(
      async () => (await telegram.shown()).length > before,
      "réponse du modèle dans Telegram",
      30000,
    );
    let marked;
    await waitUntil(
      async () => {
        marked = await markedChats(driver);
        return marked.length === 1;
      },
      () => `conversation marquée : ${JSON.stringify(marked)}`,
    );
    assert.equal(marked[0].label, "Telegram chat");
    assert.match(marked[0].row, /Hello from my phone/);
    await driver.screenshot(".tools/shots/easy-link-sidebar.png");
    // La réponse montrée dans Telegram est celle de l'historique.
    const chats = await driver.invoke("list_chats");
    assert.equal(chats[0].source, "telegram");
    const chat = await waitUntil(async () => {
      const read = await driver.invoke("get_chat", { id: chats[0].id });
      return read.messages.length === 2 && read.messages[1].content
        ? read
        : null;
    }, "réponse enregistrée");
    const firstWord = chat.messages[1].content.trim().split(/\s+/)[0];
    await waitUntil(
      async () => (await telegram.shown()).at(-1).includes(firstWord),
      "réponse complète dans Telegram",
    );

    // Discord : même parcours par message privé.
    await navigate(driver, "/settings?section=easyLink");
    await driver.waitFor(card("discord"));
    await connect(driver, "discord", FAKE_TOKENS.discord);
    await waitUntil(
      async () =>
        (
          await inCard(driver, "discord", "[data-testid=easy-link-status]")
        )?.includes("Connected"),
      "Discord connecté",
    );
    const discordCode = await pairingCode(driver, "discord");
    await discord.message({ user: "3003", name: "Luis", text: discordCode });
    await waitUntil(
      async () => (await linkedNames(driver, "discord")).length === 1,
      "compte Discord lié",
    );
    await discord.message({ user: "3003", name: "Luis", text: "!status" });
    await waitUntil(
      async () =>
        (await discord.shown()).some((t) => t.includes("Web search: off")),
      "réponse à !status",
    );
    await collectViolations(driver);
  });

  // Redémarrage : les bots écoutent de nouveau sans rien régler.
  await withApp(binary, { name: "easy-link-restart", env }, async (driver) => {
    await driver.waitFor("[data-testid=new-chat]");
    await driver.execute(WATCH_PAGE);
    await waitUntil(async () => {
      const state = await driver.invoke("easy_link_state");
      return state.bots.every((bot) => bot.link.state === "connected");
    }, "bots reconnectés au lancement");
    const before = (await telegram.shown()).length;
    await telegram.message({ user: "1001", name: "Ana", text: "/status" });
    await waitUntil(
      async () => (await telegram.shown()).length > before,
      "réponse après le redémarrage",
    );
    // Les conversations des bots sont toujours marquées.
    await driver.execute(
      "document.querySelector(\"button[aria-label='Show sidebar']\")?.click()",
    );
    await waitUntil(
      async () => (await markedChats(driver)).length === 1,
      "marque gardée après le redémarrage",
    );
    await collectViolations(driver);
  });
} finally {
  await Promise.all([fake.stop(), telegram.stop(), discord.stop()]);
}

assert.deepEqual(violations, [], "violations de la CSP");
assert.deepEqual(consoleErrors, [], "erreurs dans la console");
assert.deepEqual(consoleWarnings, [], "avertissements dans la console");
