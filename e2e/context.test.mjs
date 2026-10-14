// Jauge du contexte et compactage (propres à ChatonLM) dans l'app réelle
// (build e2e : frontend embarqué, CSP de production), avec le faux serveur
// (modèle chatonlm-context : réponse de 600 jetons, jetons comptés à 4
// caractères, `num_predict` respecté, résumé quand on le lui demande) et de
// vraies touches du clavier (e2e/lib/keyboard.mjs : Échap). Réglage du
// contexte : 4096 (le plus petit cran). Vérifié :
// - page d'une nouvelle conversation : pas de jauge ;
// - premier message long (11 200 caractères) : réponse limitée dès le premier
//   envoi (taille connue, aucun compte : num_predict = 4096 − estimation du
//   message × 0,75), sans couper les 600 jetons ; après la réponse, jauge
//   « Context: 83% used » (3 400 jetons comptés par le faux serveur sur
//   4 096 : prompt_eval_count + eval_count, réponse sans raisonnement ;
//   pourcentage arrondi vers le bas), aria-haspopup="dialog", aria-expanded ;
//   get_chat : context { used: 3400, size: 4096 } ; clic : panneau (dialog
//   « Context ») focalisé, « 3,400 / 4,096 tokens (83%) », règle des 80 %,
//   « Compact now » actif ; vraie touche Échap : panneau fermé, focus rendu
//   à la jauge ;
// - second message (passé 80 %) : compactage automatique avant la réponse
//   (« Compacting conversation… » noté s'il paraît : le faux serveur résume
//   sans délai, sous les 750 ms d'attente des points) ; requête du résumé
//   (GET /fake/requests) : les deux premiers messages (tout ce qui tient avec
//   la place du résumé) puis « Summarize our conversation… », num_predict
//   512 (4096 / 8) ; requête de la réponse : un message system qui finit par
//   le résumé, puis le seul nouveau message, limitée au-dessus de 600 ;
//   séparateur « Context compacted automatically » après la première
//   réponse, qui se déplie sur le résumé ; les quatre messages restent
//   affichés ; jauge redescendue (valeur recalculée) ; get_chat : compactage
//   automatique jusqu'à la première réponse ;
// - contexte plein (autre conversation, 78 % après le premier échange) :
//   réponse limitée à ce qui reste (entre 256 et 600 jetons, sous
//   taille − contexte compté), coupée (done_reason « length ») : erreur
//   « The conversation filled the model's context… » avec « Compact now »
//   dessous, réponse partielle gardée, jauge à 95 % au moins, aucun
//   compactage automatique ; « Compact now » : résumé du seul premier échange
//   (le reste ne tiendrait pas avec la place du résumé, num_predict 512),
//   séparateur « Context compacted » après la première réponse, erreur
//   effacée, jauge à la valeur enregistrée (relevée dans le rapport) ;
// - compactage manuel (troisième conversation, 14 %) : panneau → « Compact
//   now » → séparateur « Context compacted » (dépliable), panneau resté
//   ouvert, jauge à l'estimation du résumé, « Compact now » désactivé (rien
//   de nouveau) ; après un rechargement de la page : séparateur, jauge et
//   compactage (get_chat) toujours là ; envoi suivant : résumé en message
//   system, puis le seul nouveau message ;
// - captures : context-gauge.png, context-panel.png, context-compacted.png,
//   context-full.png, context-manual.png ;
// - aucune violation de la CSP ni erreur ou avertissement dans la console.
import assert from "node:assert/strict";
import { withApp } from "./lib/app.mjs";
import { startFake } from "./lib/fake.mjs";
import { PENDING_CALLS } from "./lib/page.mjs";
import { pressKeys } from "./lib/keyboard.mjs";

const binary = process.argv[2];
const fake = await startFake("ollama");
const env = { OLLAMA_HOST: fake.url };

const MODEL = "chatonlm-context";
const SIZE = 4096;
/** Réponse de chatonlm-context (fake-ollama, CONTEXT_ANSWER × 40). */
const ANSWER =
  "This answer fills the context of the model, fifteen tokens. ".repeat(40);
const ANSWER_TOKENS = ANSWER.length / 4;
/** Résumé écrit par chatonlm-context (fake-ollama, CONTEXT_SUMMARY). */
const SUMMARY = "The user said hello; the model answered at length.";
const ERROR_FULL =
  "The conversation filled the model's context, so the reply was cut. Compact the conversation to continue.";

const delay = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

/** `what` : texte, ou fonction appelée à l'échec (dernier état observé). */
async function waitUntil(check, what, timeoutMs = 15000) {
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

/** Texte de `length` caractères (pas d'espace au bout, rien à rogner). */
function note(length) {
  const base = "Please read this long note. ".repeat(
    Math.ceil(length / 28) + 1,
  );
  const text = base.slice(0, length - 1).replace(/ $/, "x");
  return `${text}.`;
}

/** Jetons comptés par le faux serveur pour un texte (4 octets par jeton). */
const fakeTokens = (text) => Math.floor(Buffer.byteLength(text) / 4);

/** Jetons du prompt d'une requête, comptés comme le faux serveur. */
const promptTokens = (request) =>
  request.messages.reduce(
    (sum, message) => sum + fakeTokens(message.content ?? ""),
    0,
  );

/** Corps des /api/chat reçus par le faux serveur pour chatonlm-context. */
async function requests() {
  const response = await fetch(new URL("/fake/requests", fake.url));
  return (await response.json()).filter((request) => request.model === MODEL);
}

const isSummaryRequest = (request) =>
  request.messages.at(-1)?.content?.startsWith("Summarize our conversation");

/** Pourcentage affiché : arrondi vers le bas, borné à 100. */
const percentOf = (used) => Math.min(Math.floor((used * 100) / SIZE), 100);

/**
 * Limite d'un premier envoi (taille connue, aucun compte d'Ollama) : taille
 * moins l'estimation du message (caractères / 3 + 16), corrigée par 0,75
 * (UNCALIBRATED, src-tauri/src/chat/compact.rs), arrondie par excès.
 */
const firstLimit = (text) =>
  SIZE - Math.ceil((Math.floor([...text].length / 3) + 16) * 0.75);
const formatInt = (value) => value.toLocaleString("en-US");

const exists = (driver, css) =>
  driver.execute("return document.querySelector(arguments[0]) !== null", [css]);

// Violations de la CSP, erreurs et avertissements de la console : notés par
// la page (écouteurs reposés après chaque rechargement), relevés avant chaque
// rechargement et à la fin.
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

/**
 * Recharge la page et attend la nouvelle (écouteurs reposés), seulement quand
 * aucun appel au backend n'est en cours (PENDING_CALLS, e2e/lib/page.mjs).
 */
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

/** Nouvelle conversation, par le bouton « New chat ». */
async function newChat(driver) {
  await driver.click(await driver.waitFor("a[title='New chat']"));
  await driver.waitFor("[data-testid=new-chat]");
}

/**
 * Remplit la zone de saisie (texte long : valeur posée d'un coup, comme un
 * collage, puis événement input lu par React), puis l'envoie avec Entrée.
 */
async function send(driver, text) {
  await driver.waitFor("textarea[aria-label='Message']");
  await driver.execute(
    `const area = document.querySelector("textarea[aria-label='Message']");
     const setter = Object.getOwnPropertyDescriptor(
       HTMLTextAreaElement.prototype, "value").set;
     setter.call(area, arguments[0]);
     area.dispatchEvent(new Event("input", { bubbles: true }));`,
    [text],
  );
  await waitUntil(
    () =>
      driver.execute(
        `const send = document.querySelector("button[aria-label='Send message']");
         return send !== null && !send.disabled &&
           document.querySelector("textarea[aria-label='Message']").value.length ===
             arguments[0];`,
        [text.length],
      ),
    "zone de saisie remplie",
  );
  await driver.execute(
    `document.querySelector("textarea[aria-label='Message']").dispatchEvent(
       new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true }));`,
  );
}

/** Attend la fin de l'envoi en cours (plus de flux, bouton d'envoi revenu). */
async function waitIdle(driver, messages) {
  await waitUntil(
    async () =>
      (await driver.execute(
        `return document.querySelectorAll("[data-message-index]").length`,
      )) === messages &&
      !(await exists(driver, "[data-testid=loading-status]")) &&
      (await exists(driver, "button[aria-label='Send message']")),
    `fin de l'envoi (${messages} messages affichés)`,
    30000,
  );
}

/** Identifiant de la conversation affichée. */
const currentChatId = (driver) =>
  driver.execute(
    "return (location.hash + location.pathname).match(/\\/c\\/([^/?#]+)/)?.[1] ?? null",
  );

/** Jauge du contexte (null : absente). */
const gauge = (driver) =>
  driver.execute(`
    const button = document.querySelector("button[aria-label^='Context: ']");
    if (!button) return null;
    return {
      label: button.getAttribute("aria-label"),
      title: button.title,
      haspopup: button.getAttribute("aria-haspopup"),
      expanded: button.getAttribute("aria-expanded"),
      sector: button.querySelector("[data-testid=context-sector]") !== null,
      focused: document.activeElement === button,
    };`);

async function waitGauge(driver, percent) {
  let state;
  await waitUntil(
    async () => {
      state = await gauge(driver);
      return state?.label === `Context: ${percent}% used`;
    },
    () => `jauge à ${percent} % (vue : ${JSON.stringify(state)})`,
  );
  return state;
}

/** Panneau de la jauge (null : fermé). */
const panel = (driver) =>
  driver.execute(`
    const button = document.querySelector("button[aria-label^='Context: ']");
    const id = button?.getAttribute("aria-controls");
    const dialog = id ? document.getElementById(id) : null;
    if (!dialog) return null;
    const title = document.getElementById(dialog.getAttribute("aria-labelledby"));
    const action = dialog.querySelector("button");
    return {
      role: dialog.getAttribute("role"),
      title: title?.textContent ?? null,
      lines: [...dialog.querySelectorAll("p")].map((p) => p.textContent),
      action: action && { text: action.textContent, disabled: action.disabled },
      focusInside: dialog.contains(document.activeElement),
    };`);

async function openPanel(driver) {
  await driver.click(await driver.find("button[aria-label^='Context: ']"));
  return waitUntil(() => panel(driver), "panneau du contexte ouvert");
}

/**
 * Messages affichés, dans l'ordre : rôle, et séparateurs de compactage qui
 * les suivent (texte du bouton, déplié ou non, résumé affiché).
 */
const listState = (driver) =>
  driver.execute(`
    return [...document.querySelectorAll("[data-message-index]")].map((item) => ({
      role: item.querySelector("[data-role=user]") ? "user"
        : item.querySelector("[data-role=assistant]") ? "assistant" : "?",
      separators: [...item.querySelectorAll("[data-testid=compaction]")].map((sep) => {
        const button = sep.querySelector("button");
        return {
          text: button.textContent,
          expanded: button.getAttribute("aria-expanded"),
          summary: sep.querySelector("[data-testid=compaction-summary]")?.textContent ?? null,
        };
      }),
    }));`);

/** Séparateurs affichés, à plat, avec l'indice du message qu'ils suivent. */
async function separators(driver) {
  return (await listState(driver)).flatMap((item, index) =>
    item.separators.map((separator) => ({ after: index, ...separator })),
  );
}

/** Erreur affichée sous les messages (texte), et son bouton éventuel. */
const shownError = (driver) =>
  driver.execute(`
    const alert = [...document.querySelectorAll("[data-role=message-list] [role=alert], [role=alert]")]
      .find((element) => element.querySelector("h3")?.textContent === "Error");
    if (!alert) return null;
    const action = alert.querySelector("button");
    return {
      text: alert.querySelector(".text-sm")?.textContent ?? null,
      action: action && { text: action.textContent, disabled: action.disabled },
    };`);

/** fn(driver), avec une capture de la fenêtre en cas d'échec. */
const shotOnFailure = (name, fn) => async (driver) => {
  try {
    await fn(driver);
  } catch (error) {
    await driver.screenshot(`.tools/shots/${name}-failure.png`).catch(() => {});
    throw error;
  }
};

/** Observations rapportées (pas des échecs). */
const notes = [];

try {
  await withApp(
    binary,
    { name: "context", env },
    shotOnFailure("context", async (driver) => {
      await driver.waitFor("[data-testid=new-chat]");
      // Modèle de secours écrit par la page (session neuve) avant tout choix.
      await waitUntil(
        async () => (await shownModel(driver)) !== null,
        "modèle par défaut",
      );

      // Écouteur de la CSP, vérifié sur un script en ligne refusé.
      await driver.execute(WATCH_PAGE);
      await driver.execute(`
        const script = document.createElement("script");
        script.textContent = "window.__inline = true";
        document.head.append(script);`);
      await waitUntil(
        async () => (await driver.execute("return window.__csp.length")) > 0,
        "violation témoin de la CSP",
      );
      assert.equal(
        await driver.execute("return typeof window.__inline"),
        "undefined",
      );
      await driver.execute("window.__csp = []");

      // Contexte de 4096 jetons, modèle chatonlm-context, page relue.
      await driver.invoke("update_settings", {
        patch: { contextLength: SIZE, selectedModel: MODEL },
      });
      await reload(driver);
      // Premier rendu après le rechargement : lent sous charge (contrôle
      // complet, disque ntfs3), d'où un délai plus long.
      await waitUntil(
        async () => (await shownModel(driver)) === MODEL,
        `modèle ${MODEL}`,
        60_000,
      );
      await driver.waitFor("[data-testid=new-chat]");
      assert.equal(
        await gauge(driver),
        null,
        "jauge sur la page d'une nouvelle conversation",
      );

      // ---- 1. Premier message : 83 % après la réponse.
      const long = note(11200);
      await send(driver, long);
      await driver.waitFor("[data-testid=chat]");
      await waitIdle(driver, 2);
      const auto = await currentChatId(driver);
      assert.ok(auto, "conversation créée");
      let sent = await requests();
      assert.equal(sent.length, 1, "une seule requête pour le premier envoi");
      assert.deepEqual(
        sent[0].messages.map((m) => m.role),
        ["user"],
      );
      assert.equal(sent[0].messages[0].content, long);
      assert.equal(sent[0].options?.num_ctx, SIZE);
      // Taille connue : la réponse est limitée dès le premier envoi, sans
      // couper la réponse de 600 jetons.
      assert.equal(sent[0].options?.num_predict, firstLimit(long));
      assert.ok(sent[0].options.num_predict > ANSWER_TOKENS);
      const used1 = promptTokens(sent[0]) + ANSWER_TOKENS;
      assert.equal(used1, 3400);
      let state = await waitGauge(driver, percentOf(used1));
      assert.equal(state.label, "Context: 83% used");
      assert.equal(state.title, "Context: 83% used");
      assert.equal(state.haspopup, "dialog");
      assert.equal(state.expanded, "false");
      assert.equal(state.sector, true, "secteur roux absent");
      let chat = await driver.invoke("get_chat", { id: auto });
      assert.deepEqual(chat.context, { used: used1, size: SIZE });
      assert.deepEqual(chat.compactions, []);
      await driver.screenshot(".tools/shots/context-gauge.png");

      // Panneau : jetons, pourcentage, règle, « Compact now » ; Échap.
      let opened = await openPanel(driver);
      assert.deepEqual(opened, {
        role: "dialog",
        title: "Context",
        lines: [
          "3,400 / 4,096 tokens (83%)",
          "When the conversation passes 80%, older messages are summarized automatically.",
        ],
        action: { text: "Compact now", disabled: false },
        focusInside: true,
      });
      assert.equal((await gauge(driver)).expanded, "true");
      await driver.screenshot(".tools/shots/context-panel.png");
      await pressKeys("escape");
      await waitUntil(
        async () => (await panel(driver)) === null,
        "panneau fermé par Échap",
      );
      state = await gauge(driver);
      assert.equal(state.expanded, "false");
      assert.equal(
        state.focused,
        true,
        "focus non rendu à la jauge après Échap",
      );

      // ---- 2. Second message : compactage automatique avant la réponse.
      await driver.execute(`
        window.__compactingSeen = false;
        window.__compactingObserver?.disconnect();
        window.__compactingObserver = new MutationObserver(() => {
          const line = document.querySelector("[data-testid=compacting]");
          if (line) window.__compactingSeen = line.textContent;
        });
        window.__compactingObserver.observe(document.body, { childList: true, subtree: true });`);
      await send(driver, "Second question");
      await waitIdle(driver, 4);
      const compactingSeen = await driver.execute(
        "window.__compactingObserver.disconnect(); return window.__compactingSeen",
      );
      if (compactingSeen) {
        assert.equal(compactingSeen, "Compacting conversation…");
        notes.push(
          `« ${compactingSeen} » vu pendant le compactage automatique`,
        );
      } else {
        notes.push(
          "« Compacting conversation… » pas vu (résumé du faux serveur plus rapide que les 750 ms des points)",
        );
      }
      sent = await requests();
      assert.equal(
        sent.length,
        3,
        "requêtes du second envoi : résumé puis réponse",
      );
      const summarize = sent[1];
      assert.ok(
        isSummaryRequest(summarize),
        "deuxième requête : pas le résumé",
      );
      assert.deepEqual(
        summarize.messages.map((m) => m.role),
        ["user", "assistant", "user"],
      );
      assert.equal(summarize.messages[0].content, long);
      assert.equal(summarize.messages[1].content, ANSWER);
      // Place du résumé : un huitième du contexte.
      assert.equal(summarize.options?.num_predict, SIZE / 8);
      const reply2 = sent[2];
      assert.deepEqual(
        reply2.messages.map((m) => m.role),
        ["system", "user"],
        "requête après le compactage",
      );
      assert.ok(
        reply2.messages[0].content.endsWith(SUMMARY),
        `message system sans le résumé : ${reply2.messages[0].content}`,
      );
      assert.equal(reply2.messages[1].content, "Second question");
      assert.ok(
        reply2.options?.num_predict > ANSWER_TOKENS,
        `réponse limitée trop bas : ${reply2.options?.num_predict}`,
      );
      const summarySystem = reply2.messages[0].content;
      // Tous les messages affichés, séparateur après la première réponse.
      let list = await listState(driver);
      assert.deepEqual(
        list.map((item) => item.role),
        ["user", "assistant", "user", "assistant"],
      );
      assert.deepEqual(await separators(driver), [
        {
          after: 1,
          text: "Context compacted automatically",
          expanded: "false",
          summary: null,
        },
      ]);
      await driver.click(await driver.find("[data-testid=compaction] button"));
      await waitUntil(
        async () => (await separators(driver))[0]?.expanded === "true",
        "séparateur déplié",
      );
      assert.deepEqual(await separators(driver), [
        {
          after: 1,
          text: "Context compacted automatically",
          expanded: "true",
          summary: SUMMARY,
        },
      ]);
      const used2 = promptTokens(reply2) + ANSWER_TOKENS;
      assert.ok(used2 < used1, "contexte pas redescendu");
      await waitGauge(driver, percentOf(used2));
      chat = await driver.invoke("get_chat", { id: auto });
      assert.deepEqual(chat.context, { used: used2, size: SIZE });
      assert.equal(chat.messages.length, 4);
      assert.equal(chat.compactions.length, 1);
      assert.equal(chat.compactions[0].automatic, true);
      assert.equal(chat.compactions[0].summary, SUMMARY);
      assert.equal(chat.compactions[0].throughMessageId, chat.messages[1].id);
      await driver.screenshot(".tools/shots/context-compacted.png");

      // ---- 3. Contexte plein : réponse coupée, « Compact now » sous l'erreur.
      await newChat(driver);
      const long2 = note(10400);
      await send(driver, long2);
      await driver.waitFor("[data-testid=chat]");
      await waitIdle(driver, 2);
      const full = await currentChatId(driver);
      assert.notEqual(full, auto);
      sent = await requests();
      assert.equal(sent.at(-1).options?.num_predict, firstLimit(long2));
      const used3 = promptTokens(sent.at(-1)) + ANSWER_TOKENS;
      assert.equal(used3, 3200);
      await waitGauge(driver, percentOf(used3)); // 78 %, sous le seuil
      const before = sent.length;
      const mid = note(1200);
      await send(driver, mid);
      await waitIdle(driver, 4);
      sent = await requests();
      assert.equal(sent.length, before + 1, "compactage automatique sous 80 %");
      const cut = sent.at(-1);
      assert.ok(!isSummaryRequest(cut));
      // Limite : taille − contexte compté − estimation du nouveau message
      // (1200 caractères / 3 + 16, corrigée par le rapport compté/estimé) :
      // sous les 600 jetons de la réponse, au moins 256.
      const limit = cut.options?.num_predict;
      assert.ok(
        Number.isInteger(limit) && limit >= 256 && limit < ANSWER_TOKENS,
        `limite de la réponse : ${limit}`,
      );
      assert.ok(limit < SIZE - used3, "limite sans le nouveau message");
      let error;
      await waitUntil(
        async () => (error = await shownError(driver))?.text === ERROR_FULL,
        () => `erreur du contexte plein (vue : ${JSON.stringify(error)})`,
      );
      assert.deepEqual(error.action, { text: "Compact now", disabled: false });
      const partial = await driver.execute(
        "return [...document.querySelectorAll('[data-role=assistant]')].at(-1).textContent",
      );
      assert.equal(
        partial.trim(),
        ANSWER.slice(0, limit * 4).trim(),
        "réponse coupée non gardée",
      );
      const used4 = promptTokens(cut) + limit;
      await waitGauge(driver, percentOf(used4));
      assert.ok(percentOf(used4) >= 95, `jauge à ${percentOf(used4)} %`);
      chat = await driver.invoke("get_chat", { id: full });
      assert.deepEqual(chat.context, { used: used4, size: SIZE });
      // L'envoi garde le message envoyé en haut : erreur amenée à l'écran.
      await driver.execute(
        `[...document.querySelectorAll("[role=alert]")].at(-1)
           .scrollIntoView({ block: "end" });`,
      );
      await driver.screenshot(".tools/shots/context-full.png");
      await driver.click(await driver.find("[role=alert] button"));
      await waitUntil(
        async () => (await separators(driver)).length === 1,
        "séparateur après « Compact now » sous l'erreur",
      );
      // Ne tient avec la place du résumé (4096 / 8) que le premier échange :
      // séparateur après la première réponse.
      assert.deepEqual(await separators(driver), [
        {
          after: 1,
          text: "Context compacted",
          expanded: "false",
          summary: null,
        },
      ]);
      await waitUntil(
        async () => (await shownError(driver)) === null,
        "erreur effacée",
      );
      sent = await requests();
      assert.ok(isSummaryRequest(sent.at(-1)), "pas de requête de résumé");
      assert.deepEqual(
        sent.at(-1).messages.map((m) => m.role),
        ["user", "assistant", "user"],
      );
      assert.equal(sent.at(-1).messages[0].content, long2);
      assert.equal(sent.at(-1).options?.num_predict, SIZE / 8);
      const estimate = Math.floor(summarySystem.length / 3) + 16;
      chat = await driver.invoke("get_chat", { id: full });
      assert.equal(chat.compactions.length, 1);
      assert.equal(chat.compactions[0].automatic, false);
      assert.equal(chat.compactions[0].throughMessageId, chat.messages[1].id);
      assert.equal(chat.context.size, SIZE);
      assert.ok(chat.context.used >= estimate);
      const shownFull = await waitGauge(driver, percentOf(chat.context.used));
      // Messages gardés après le résumé (le message de 1200 caractères et la
      // réponse coupée) : estimation de leur place, pour le rapport.
      const rest =
        Math.floor(mid.length / 3) + 16 + Math.floor(partial.length / 3) + 16;
      notes.push(
        `compactage manuel partiel : contexte enregistré ${chat.context.used} (résumé seul : ${estimate}, ` +
          `messages gardés estimés à ${rest} de plus), jauge « ${shownFull.label} »`,
      );

      // ---- 4. Compactage manuel par le panneau.
      await newChat(driver);
      await send(driver, "hello");
      await driver.waitFor("[data-testid=chat]");
      await waitIdle(driver, 2);
      const manual = await currentChatId(driver);
      sent = await requests();
      const used5 = promptTokens(sent.at(-1)) + ANSWER_TOKENS;
      assert.equal(used5, 601);
      await waitGauge(driver, 14);
      opened = await openPanel(driver);
      assert.equal(opened.lines[0], "601 / 4,096 tokens (14%)");
      assert.deepEqual(opened.action, { text: "Compact now", disabled: false });
      await driver.click(await driver.find("[role=dialog] button"));
      await waitUntil(
        async () => (await separators(driver)).length === 1,
        "séparateur après « Compact now » du panneau",
      );
      assert.deepEqual(await separators(driver), [
        {
          after: 1,
          text: "Context compacted",
          expanded: "false",
          summary: null,
        },
      ]);
      // Contexte enregistré : le résumé estimé, corrigé par le compte
      // d'Ollama de la conversation (jamais plus que l'estimation brute).
      const compacted = (await driver.invoke("get_chat", { id: manual }))
        .context.used;
      assert.ok(compacted > 0 && compacted <= estimate, `${compacted}`);
      await waitGauge(driver, percentOf(compacted));
      await waitUntil(
        async () => (await panel(driver))?.action?.disabled === true,
        () => "« Compact now » toujours actif sans rien de nouveau",
      );
      opened = await panel(driver);
      assert.equal(
        opened.lines[0],
        `${formatInt(compacted)} / 4,096 tokens (${percentOf(compacted)}%)`,
      );
      assert.equal(opened.action.text, "Compact now");
      sent = await requests();
      assert.ok(isSummaryRequest(sent.at(-1)));
      assert.deepEqual(
        sent.at(-1).messages.map((m) => m.content),
        ["hello", ANSWER, sent.at(-1).messages[2].content],
      );
      await driver.click(await driver.find("[data-testid=compaction] button"));
      await waitUntil(
        async () => (await separators(driver))[0]?.summary === SUMMARY,
        "résumé du compactage manuel déplié",
      );
      await driver.screenshot(".tools/shots/context-manual.png");
      await pressKeys("escape");
      await waitUntil(
        async () => (await panel(driver)) === null,
        "panneau fermé",
      );

      // Rechargement : séparateur et jauge relus.
      await reload(driver);
      await driver.waitFor("[data-testid=chat]");
      assert.equal(
        await currentChatId(driver),
        manual,
        "conversation perdue au rechargement",
      );
      await waitUntil(
        async () => (await separators(driver)).length === 1,
        "séparateur après le rechargement",
      );
      assert.deepEqual(await separators(driver), [
        {
          after: 1,
          text: "Context compacted",
          expanded: "false",
          summary: null,
        },
      ]);
      await waitGauge(driver, percentOf(compacted));
      opened = await openPanel(driver);
      assert.equal(
        opened.action.disabled,
        true,
        "« Compact now » actif après le rechargement",
      );
      await pressKeys("escape");
      await waitUntil(
        async () => (await panel(driver)) === null,
        "panneau fermé",
      );
      chat = await driver.invoke("get_chat", { id: manual });
      assert.equal(chat.compactions.length, 1);
      assert.equal(chat.compactions[0].automatic, false);
      assert.equal(chat.compactions[0].throughMessageId, chat.messages[1].id);

      // Envoi suivant : seul le résumé et le nouveau message partent.
      await send(driver, "Next question");
      await waitIdle(driver, 4);
      sent = await requests();
      assert.deepEqual(
        sent.at(-1).messages.map((m) => [m.role, m.content]),
        [
          ["system", summarySystem],
          ["user", "Next question"],
        ],
      );
      list = await listState(driver);
      assert.deepEqual(
        list.map((item) => item.role),
        ["user", "assistant", "user", "assistant"],
      );
      assert.equal((await separators(driver))[0].after, 1);
      await waitGauge(
        driver,
        percentOf(promptTokens(sent.at(-1)) + ANSWER_TOKENS),
      );

      await collectViolations(driver);
    }),
  );
} finally {
  await fake.stop();
}

for (const line of notes) console.log(`Note : ${line}`);
assert.deepEqual(violations, [], "violations de la CSP");
assert.deepEqual(consoleErrors, [], "erreurs dans la console");
assert.deepEqual(consoleWarnings, [], "avertissements dans la console");
console.log("context : OK");
