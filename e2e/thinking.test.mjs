// Raisonnement dans l'app réelle (build e2e : frontend embarqué, CSP de
// production), avec le faux serveur et de vraies touches du clavier
// (e2e/lib/keyboard.mjs, client libei de KWin : Tab, Entrée, Espace, Échap et
// flèches ont leur effet natif, ce que les touches simulées de WebDriver n'ont
// pas) :
// - bouton Think selon /api/show : bascule de qwen3:0.6b ([false, true],
//   défaut true) et de chatonlm-nometa (capacité sans objet thinking), menu de
//   niveaux de chatonlm-levels (Low/Medium/High, défaut Medium) et de
//   chatonlm-levels-off (Off en plus, défaut Low, pilule grise sur Off), aucun
//   bouton pour chatonlm-always (raisonne toujours), chatonlm-never et
//   chatonlm-plain ; chatonlm-stubborn (comme LFM2.5) raisonne malgré Think
//   coupé : raisonnement affiché, puis plus de bouton ; infobulles ;
// - valeur envoyée, relue dans GET /fake/requests et citée par la réponse
//   (« Received think=… ») : true ou false explicites, niveau, ou champ absent
//   (bouton absent) ; chatonlm-plain : une seule requête, sans think ;
// - préférence mémorisée par modèle : relue en changeant de modèle et en
//   revenant (sélecteur, ou conversation rouverte), après un rechargement de
//   la page et après une relance de l'app ;
// - bloc de raisonnement : « Thinking... » et aperçu replié pendant le
//   raisonnement (chatonlm-slowthink), « Thought for N.N seconds » et repli à
//   l'arrivée du contenu, durée relue en base ; déplier et replier au clic, à
//   Entrée et à Espace ; arrêt pendant le raisonnement : titre jamais figé sur
//   « Thinking... », ni tout de suite, ni après relecture ; long raisonnement
//   (chatonlm-longthink) : l'aperçu de 12rem montre la fin, début coupé sous
//   un dégradé de la couleur du fond, en clair et en sombre (captures
//   thinking-light.png et thinking-dark.png) ;
// - clavier : Tab mène de la zone de saisie au bouton « + », au bouton
//   Think, au globe (modèle « tools »), au sélecteur puis à Send, et finit par sortir de la zone (pas de
//   piège) ; Maj+Tab en sort aussi ; menu des niveaux aux flèches, Home/End, Entrée ; Tab depuis le
//   menu le ferme ; Échap dans le menu le ferme sans arrêter la réponse en
//   cours (Échap suivante l'arrête) ; capture think-levels-menu.png ;
// - aucune violation de la CSP ni erreur ou avertissement dans la console.
import assert from "node:assert/strict";
import { withApp } from "./lib/app.mjs";
import { startFake } from "./lib/fake.mjs";
import { PENDING_CALLS } from "./lib/page.mjs";
import { pressKeys } from "./lib/keyboard.mjs";

const binary = process.argv[2];
const fake = await startFake("ollama");
const env = { OLLAMA_HOST: fake.url };

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

/** Corps des /api/chat reçus par le faux serveur, d'un modèle. */
async function requestsOf(model) {
  const response = await fetch(new URL("/fake/requests", fake.url));
  return (await response.json()).filter((request) => request.model === model);
}

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
 * Recharge la page et attend la nouvelle (écouteurs reposés). Seulement quand
 * aucun appel au backend n'est en cours (PENDING_CALLS, e2e/lib/page.mjs) :
 * la réponse d'un appel lancé par l'ancienne page (liste des modèles relue au
 * montage, puis détails du modèle) arriverait dans la nouvelle, où Tauri écrit
 * « Couldn't find callback id » dans la console. Vérification et
 * rechargement dans le même script : aucun appel ne part entre les deux.
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

/**
 * Choisit un modèle (absent de la liste), puis recharge la page. La page doit
 * déjà afficher un modèle : sinon son modèle de secours, écrit dès qu'elle a
 * lu la liste, pourrait remplacer celui-ci.
 */
async function useModel(driver, model) {
  assert.ok(await shownModel(driver), "aucun modèle affiché avant useModel");
  await driver.invoke("update_settings", { patch: { selectedModel: model } });
  await reload(driver);
  await waitUntil(
    async () => (await shownModel(driver)) === model,
    `modèle ${model}`,
  );
}

/** Choisit un modèle dans le sélecteur, une fois sa liste lue. */
async function pickModel(driver, model) {
  await driver.click(await driver.find("[title='Select model']"));
  await waitUntil(
    () =>
      driver.execute(
        `const option = [...document.querySelectorAll("[role=option]")]
           .find((o) => o.textContent === arguments[0]);
         option?.click();
         return option !== undefined;`,
        [model],
      ),
    `option ${model} du sélecteur`,
  );
  await waitUntil(
    async () => (await shownModel(driver)) === model,
    `modèle ${model} choisi`,
  );
}

/** Nouvelle conversation, par le bouton « New chat ». */
async function newChat(driver) {
  await driver.click(await driver.waitFor("a[title='New chat']"));
  await driver.waitFor("[data-testid=new-chat]");
}

/** Envoie le contenu de la zone de saisie avec Entrée. */
const submit = (driver) =>
  driver.execute(
    `document.querySelector("textarea[aria-label='Message']").dispatchEvent(
       new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true }));`,
  );

/** Tape un message et l'envoie avec Entrée. */
async function send(driver, text) {
  await driver.type(
    await driver.waitFor("textarea[aria-label='Message']"),
    text,
  );
  await submit(driver);
}

/** Identifiant de la conversation affichée. */
const currentChatId = (driver) =>
  driver.execute(
    "return (location.hash + location.pathname).match(/\\/c\\/([^/?#]+)/)?.[1] ?? null",
  );

/** Ouvre une conversation par la barre latérale. */
async function openChat(driver, id) {
  if (await exists(driver, "button[aria-label='Show sidebar']")) {
    await driver.click(await driver.find("button[aria-label='Show sidebar']"));
  }
  await driver.click(await driver.waitFor(`[data-chat-id='${id}'] a`));
  await waitUntil(
    async () => (await currentChatId(driver)) === id,
    `conversation ${id} ouverte`,
  );
  await driver.waitFor("[data-testid=chat]");
}

/** Contrôle du raisonnement affiché dans la zone de saisie. */
const thinkState = (driver) =>
  driver.execute(`
    const toggle = document.querySelector("button[aria-label='Think']");
    const levels = document.querySelector("button[aria-label^='Thinking level:']");
    const button = toggle ?? levels;
    if (!button) return { kind: "none" };
    const grey = button.classList.contains("text-neutral-500");
    if (toggle) {
      return {
        kind: toggle.getAttribute("aria-disabled") === "true" ? "fixed" : "toggle",
        pressed: toggle.getAttribute("aria-pressed"),
        title: toggle.title,
        grey,
      };
    }
    return {
      kind: "levels",
      label: levels.getAttribute("aria-label"),
      title: levels.title,
      text: levels.textContent,
      expanded: levels.getAttribute("aria-expanded"),
      grey,
    };`);

/** Attend un contrôle du raisonnement de cette sorte, et le renvoie. */
async function waitThink(driver, kind, more = () => true) {
  let state;
  await waitUntil(
    async () => {
      state = await thinkState(driver);
      return state.kind === kind && more(state);
    },
    () => `contrôle ${kind} : ${JSON.stringify(state)}`,
  );
  return state;
}

/** Menu des niveaux ouvert : entrées, entrée cochée, élément focalisé. */
const levelMenu = (driver) =>
  driver.execute(`
    const menu = document.querySelector("[role=menu][aria-label='Thinking level']");
    if (!menu) return null;
    const items = [...menu.querySelectorAll("[role=menuitemradio]")];
    return {
      items: items.map((item) => item.textContent),
      checked: items.filter((item) => item.getAttribute("aria-checked") === "true")
        .map((item) => item.textContent),
      focused: items.indexOf(document.activeElement),
    };`);

/** Ouvre le menu des niveaux par un clic sur son bouton. */
async function openLevelMenu(driver) {
  await driver.click(
    await driver.find("button[aria-label^='Thinking level:']"),
  );
  return waitUntil(() => levelMenu(driver), "menu des niveaux ouvert");
}

/** Texte de la dernière réponse affichée. */
const lastReply = (driver) =>
  driver.execute(
    "return [...document.querySelectorAll('[data-role=assistant]')].at(-1)?.textContent ?? null",
  );

async function waitReply(driver, text) {
  let shown;
  await waitUntil(
    async () => {
      shown = await lastReply(driver);
      return shown === text;
    },
    () =>
      `réponse ${JSON.stringify(text)} (affiché : ${JSON.stringify(shown)})`,
  );
  await driver.waitFor("button[aria-label='Send message']");
}

/**
 * Bloc de raisonnement du dernier message (null : aucun) : titre, état de
 * l'en-tête, style calculé du corps.
 */
const lastThinking = (driver) =>
  driver.execute(`
    const last = [...document.querySelectorAll("[data-message-index]")].at(-1);
    const header = last?.querySelector("h3 > button");
    if (!header) return null;
    const body = document.getElementById(header.getAttribute("aria-controls"));
    const style = getComputedStyle(body);
    return {
      title: header.textContent,
      expanded: header.getAttribute("aria-expanded"),
      maxHeight: style.maxHeight,
      opacity: style.opacity,
      visibility: style.visibility,
      height: body.getBoundingClientRect().height,
      text: body.textContent,
    };`);

/** États du corps : aperçu (pendant le raisonnement), replié, déplié. */
const PREVIEW = {
  expanded: "false",
  maxHeight: "192px",
  visibility: "visible",
};
const FOLDED = {
  expanded: "false",
  maxHeight: "0px",
  opacity: "0",
  visibility: "hidden",
};
const EXPANDED = {
  expanded: "true",
  maxHeight: "none",
  opacity: "1",
  visibility: "visible",
};
const matches = (state, expected) =>
  state !== null &&
  Object.entries(expected).every(([key, value]) => state[key] === value);

async function waitThinking(driver, expected, what) {
  let state;
  await waitUntil(
    async () => {
      state = await lastThinking(driver);
      return matches(state, expected);
    },
    () => `${what} : ${JSON.stringify(state && { ...state, text: undefined })}`,
  );
  return state;
}

const SECONDS = /^Thought for (\d+\.\d) seconds$/;

/** Message enregistré : durée du raisonnement en secondes (une décimale). */
const savedSeconds = (message) =>
  ((message.thinkingEndedAt - message.thinkingStartedAt) / 1000).toFixed(1);

/**
 * Élément focalisé : nom accessible (ou texte d'une entrée de menu), et s'il
 * est dans la zone de saisie. « out » : focus sorti de la page.
 */
const FOCUS_NAME = `
  const nameOf = (element) => {
    if (!element || element === document.body) return "out";
    if (element.getAttribute("role") === "menuitemradio") {
      return "item:" + element.textContent;
    }
    return element.getAttribute("aria-label") ?? element.getAttribute("title") ??
      element.tagName.toLowerCase();
  };
  const composer = document.querySelector("textarea[aria-label='Message']")
    .closest("div.rounded-3xl");`;

const focused = (driver) =>
  driver.execute(`${FOCUS_NAME}
    const active = document.activeElement;
    return { name: nameOf(active), inComposer: composer.contains(active),
      visible: active?.matches(":focus-visible") ?? false };`);

/** Note chaque élément qui prend le focus, et chaque sortie de la page. */
const recordFocus = (driver) =>
  driver.execute(`${FOCUS_NAME}
    window.__focusPath?.stop();
    const path = [];
    const onIn = (event) => path.push({ name: nameOf(event.target),
      inComposer: composer.contains(event.target) });
    const onOut = (event) => {
      if (event.relatedTarget === null) path.push({ name: "out", inComposer: false });
    };
    document.addEventListener("focusin", onIn);
    document.addEventListener("focusout", onOut);
    window.__focusPath = { path, stop: () => {
      document.removeEventListener("focusin", onIn);
      document.removeEventListener("focusout", onOut);
    } };`);

const focusPath = (driver) =>
  driver.execute(
    "const { path, stop } = window.__focusPath; stop(); return path;",
  );

const focusTextarea = (driver) =>
  driver.execute(
    "document.querySelector(\"textarea[aria-label='Message']\").focus()",
  );

/**
 * Ordre de Tab depuis la zone de saisie (texte tapé : Send actif) : « + »,
 * `think` (nom du bouton Think), globe de la recherche web (modèle « tools »,
 * `webSearch`), sélecteur, mode vocal, Send, puis sortie de la zone ; Tab finit par
 * revenir à la zone de saisie en passant par le reste de la page. Maj+Tab
 * depuis la zone de saisie en sort aussi.
 */
async function checkTabOrder(driver, think, { webSearch = false } = {}) {
  await focusTextarea(driver);
  await recordFocus(driver);
  // Assez de Tab pour faire le tour : barre latérale, en-têtes de
  // raisonnement et boutons copier des messages.
  await pressKeys(Array(24).fill("tab"));
  const path = await focusPath(driver);
  const names = path.map((step) => step.name);
  const expected = [
    "Upload multiple files",
    think,
    ...(webSearch ? ["Web search"] : []),
    "Select model",
    "Start voice mode",
    "Send message",
  ];
  assert.deepEqual(
    names.slice(0, expected.length),
    expected,
    `ordre de Tab : ${names.join(" → ")}`,
  );
  assert.ok(path.slice(0, expected.length).every((step) => step.inComposer));
  assert.equal(
    path[expected.length]?.inComposer,
    false,
    `Tab après Send reste dans la zone (piège) : ${names.join(" → ")}`,
  );
  const back = names.indexOf("Message", expected.length);
  assert.ok(
    back > expected.length,
    `Tab ne revient pas à la zone de saisie : ${names}`,
  );
  assert.ok(
    path.slice(expected.length, back).every((step) => !step.inComposer),
    `Tab reste dans la zone : ${names.join(" → ")}`,
  );
  await focusTextarea(driver);
  await pressKeys("shift+tab");
  const before = await focused(driver);
  assert.equal(before.inComposer, false, `Maj+Tab : ${before.name}`);
  await focusTextarea(driver);
}

/** Message tapé dans la zone de saisie (vrais caractères absents : JS). */
const typeMessage = async (driver, text) =>
  driver.type(await driver.waitFor("textarea[aria-label='Message']"), text);

/** Couleurs CSS → [r, g, b, a] (canvas), pour comparer des formats différents. */
const TO_RGB = `
  const toRgb = (color) => {
    const canvas = document.createElement("canvas");
    canvas.width = canvas.height = 1;
    const context = canvas.getContext("2d", { willReadFrequently: true });
    context.clearRect(0, 0, 1, 1);
    context.fillStyle = "rgb(1, 2, 3)";
    context.fillStyle = color;
    context.fillRect(0, 0, 1, 1);
    return [...context.getImageData(0, 0, 1, 1).data];
  };`;

/**
 * Aperçu du dernier raisonnement : hauteur, écart entre le bas du texte et le
 * bas de l'aperçu, paragraphes visibles, premier paragraphe coupé, dégradé
 * (couleur de départ et couleur du fond sous le bloc).
 */
const previewState = (driver) =>
  driver.execute(`${TO_RGB}
    const header = [...document.querySelectorAll("[data-message-index] h3 > button")].at(-1);
    if (!header) return null;
    const body = document.getElementById(header.getAttribute("aria-controls"));
    const box = body.getBoundingClientRect();
    const content = body.firstElementChild.getBoundingClientRect();
    const paragraphs = [...body.querySelectorAll("p")];
    const fade = body.querySelector("[data-testid=thinking-fade]");
    let background = null;
    for (let element = body; element; element = element.parentElement) {
      const color = getComputedStyle(element).backgroundColor;
      if (toRgb(color)[3] === 255) { background = toRgb(color); break; }
    }
    if (background === null) {
      // Rien d'opaque : fond du canevas de la page, selon son color-scheme.
      const probe = document.createElement("div");
      probe.style.cssText = "position: absolute; width: 0; height: 0; background-color: Canvas";
      document.body.append(probe);
      background = toRgb(getComputedStyle(probe).backgroundColor);
      probe.remove();
    }
    const start = fade &&
      /(?:rgba?|oklch|oklab|lab|lch|hsla?|color)\\([^)]*\\)|#[0-9a-f]{3,8}|white/i
        .exec(getComputedStyle(fade).backgroundImage)?.[0];
    return {
      title: header.textContent,
      height: box.height,
      bottomGap: box.bottom - content.bottom,
      overflow: content.top < box.top - 1,
      count: paragraphs.length,
      firstClipped: paragraphs.length > 0 &&
        paragraphs[0].getBoundingClientRect().bottom <= box.top + 1,
      lastVisible: paragraphs.length > 0 && (() => {
        const last = paragraphs.at(-1).getBoundingClientRect();
        return last.top >= box.top - 1 && last.bottom <= box.bottom + 1;
      })(),
      visible: paragraphs.filter((p) => {
        const r = p.getBoundingClientRect();
        return r.bottom > box.top + 1 && r.top < box.bottom - 1;
      }).map((p) => p.textContent.trim()),
      fade: fade !== null,
      fadeStart: start ? toRgb(start) : null,
      background,
      dark: document.documentElement.classList.contains("dark"),
    };`);

/** Fond du dégradé égal au fond de la page (à 3 près par canal, opaque). */
function assertFadeMatchesBackground(state, what) {
  assert.ok(state.fade, `${what} : pas de dégradé`);
  assert.ok(state.fadeStart, `${what} : couleur du dégradé illisible`);
  assert.notDeepEqual(
    state.fadeStart,
    [1, 2, 3, 255],
    `${what} : couleur inconnue du canvas`,
  );
  const gaps = state.fadeStart.map((channel, i) =>
    Math.abs(channel - state.background[i]),
  );
  assert.ok(
    gaps.every((gap) => gap <= 3),
    `${what} : dégradé ${state.fadeStart} sur un fond ${state.background}`,
  );
}

/**
 * Thème clair ou sombre, enregistré et appliqué aussitôt. La page ne relit le
 * réglage qu'au rechargement (cache des réglages), qui arrêterait la réponse
 * en cours : le test l'applique comme applyTheme (src/hooks/useTheme.ts),
 * classe « dark » et color-scheme de <html>.
 */
async function setTheme(driver, theme) {
  await driver.invoke("update_settings", { patch: { theme } });
  await driver.execute(
    `const root = document.documentElement;
     root.classList.toggle("dark", arguments[0] === "dark");
     root.style.colorScheme = arguments[0];`,
    [theme],
  );
  // Fin des transitions de couleur.
  await delay(400);
}

/** fn(driver), avec une capture de la fenêtre en cas d'échec. */
const shotOnFailure = (name, fn) => async (driver) => {
  try {
    await fn(driver);
  } catch (error) {
    await driver.screenshot(`.tools/shots/${name}-failure.png`).catch(() => {});
    throw error;
  }
};

/** Conversations créées par le premier lancement, relues au second. */
const chats = {};
/** Titres des raisonnements de la conversation lente, relus au second. */
let slowTitles;

try {
  await withApp(
    binary,
    { name: "thinking", env },
    shotOnFailure("thinking", async (driver) => {
      await driver.waitFor("[data-testid=new-chat]");
      // Modèle de secours écrit par la page (session neuve) avant tout choix.
      await waitUntil(
        async () => (await shownModel(driver)) === "qwen3:0.6b",
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

      // qwen3:0.6b : bascule réelle ([false, true], défaut true), envoyée
      // explicitement. Ordre de Tab : zone de saisie, Think, sélecteur, Send,
      // puis hors de la zone.
      let state = await waitThink(driver, "toggle");
      assert.deepEqual(state, {
        kind: "toggle",
        pressed: "true",
        title: "Disable think mode",
        grey: false,
      });
      await typeMessage(driver, "Say hello in one word.");
      // qwen3:0.6b a la capacité « tools » : globe après Think.
      await checkTabOrder(driver, "Think", { webSearch: true });
      await submit(driver);
      await driver.waitFor("[data-testid=chat]");
      await waitReply(driver, '"Hello"');
      chats.qwen = await currentChatId(driver);
      assert.equal((await requestsOf("qwen3:0.6b")).at(-1).think, true);
      // Raisonnement enregistré, rejoué sans délai : moins de 2 s.
      let thinking = await waitThinking(driver, FOLDED, "raisonnement replié");
      assert.equal(thinking.title, "Thought for a moment");

      // Bascule au clavier (Espace, vraie touche) : arrêt mémorisé, envoyé.
      await driver.execute(
        "document.querySelector(\"button[aria-label='Think']\").focus()",
      );
      await pressKeys("space");
      state = await waitThink(driver, "toggle", (s) => s.pressed === "false");
      assert.equal(state.title, "Enable think mode");
      assert.equal(state.grey, true);
      assert.equal(
        (await focused(driver)).name,
        "Think",
        "Espace a déplacé le focus",
      );
      assert.equal(
        await driver.invoke("get_think_pref", { model: "qwen3:0.6b" }),
        false,
      );
      await send(driver, "Without thinking.");
      await waitUntil(
        async () => (await requestsOf("qwen3:0.6b")).length === 2,
        "seconde requête de qwen3:0.6b",
      );
      await driver.waitFor("button[aria-label='Send message']");
      assert.equal((await requestsOf("qwen3:0.6b")).at(-1).think, false);
      assert.equal(await lastThinking(driver), null, "raisonnement sans think");

      // Changer de modèle et revenir (sélecteur) : préférence de chacun.
      await pickModel(driver, "qwen3.5:0.8b");
      await waitThink(driver, "toggle", (s) => s.pressed === "true");
      await pickModel(driver, "qwen3:0.6b");
      await waitThink(driver, "toggle", (s) => s.pressed === "false");

      // chatonlm-levels : niveaux seulement, défaut Medium.
      await newChat(driver);
      await useModel(driver, "chatonlm-levels");
      state = await waitThink(driver, "levels");
      assert.deepEqual(state, {
        kind: "levels",
        label: "Thinking level: Medium",
        title: "Thinking level: Medium",
        text: "Medium",
        expanded: "false",
        grey: false,
      });
      await send(driver, "Which level?");
      await driver.waitFor("[data-testid=chat]");
      await waitReply(driver, 'Received think="medium".');
      chats.levels = await currentChatId(driver);
      assert.equal(
        (await requestsOf("chatonlm-levels")).at(-1).think,
        "medium",
      );
      thinking = await waitThinking(
        driver,
        FOLDED,
        "raisonnement de chatonlm-levels",
      );
      assert.equal(thinking.title, "Thought for a moment");
      // Menu : niveaux dans l'ordre d'Ollama, niveau courant coché et
      // focalisé ; flèche bas puis Entrée (vraies touches) : High.
      let menu = await openLevelMenu(driver);
      assert.deepEqual(menu, {
        items: ["Low", "Medium", "High"],
        checked: ["Medium"],
        focused: 1,
      });
      assert.equal((await thinkState(driver)).expanded, "true");
      await delay(300);
      await driver.screenshot(".tools/shots/think-levels-menu.png");
      await pressKeys(["down", "enter"]);
      state = await waitThink(
        driver,
        "levels",
        (s) => s.label === "Thinking level: High",
      );
      assert.equal(state.expanded, "false");
      assert.equal(await levelMenu(driver), null, "menu resté ouvert");
      assert.equal(
        (await focused(driver)).name,
        "Thinking level: High",
        "focus non rendu au bouton",
      );
      assert.equal(
        await driver.invoke("get_think_pref", { model: "chatonlm-levels" }),
        "high",
      );
      await send(driver, "And now?");
      await waitReply(driver, 'Received think="high".');
      assert.equal((await requestsOf("chatonlm-levels")).at(-1).think, "high");
      // Ordre de Tab avec la pilule des niveaux.
      await typeMessage(driver, "Tab order.");
      await checkTabOrder(driver, "Thinking level: High");
      await driver.clear(await driver.find("textarea[aria-label='Message']"));

      // chatonlm-levels-off : Off en plus, défaut Low ; Off choisi au clic :
      // pilule grise, think false, pas de raisonnement.
      await newChat(driver);
      await useModel(driver, "chatonlm-levels-off");
      state = await waitThink(driver, "levels");
      assert.equal(state.label, "Thinking level: Low");
      assert.equal(state.grey, false);
      menu = await openLevelMenu(driver);
      assert.deepEqual(menu, {
        items: ["Off", "Low", "Medium", "High"],
        checked: ["Low"],
        focused: 1,
      });
      await driver.execute(
        `[...document.querySelectorAll("[role=menuitemradio]")]
           .find((item) => item.textContent === "Off").click()`,
      );
      state = await waitThink(
        driver,
        "levels",
        (s) => s.label === "Thinking level: Off",
      );
      assert.equal(state.grey, true, "pilule Off non grise");
      assert.equal(state.title, "Thinking level: Off");
      await send(driver, "Off?");
      await driver.waitFor("[data-testid=chat]");
      await waitReply(driver, "Received think=false.");
      chats.levelsOff = await currentChatId(driver);
      assert.equal(
        (await requestsOf("chatonlm-levels-off")).at(-1).think,
        false,
      );
      assert.equal(await lastThinking(driver), null);

      // chatonlm-always ([true]) : raisonne toujours, aucun bouton (choix de la
      // recette de l'étape 15) ; think absent ; raisonnement affiché.
      await newChat(driver);
      await useModel(driver, "chatonlm-always");
      await delay(1500);
      assert.deepEqual(await thinkState(driver), { kind: "none" });
      assert.equal(
        await driver.invoke("get_think_pref", { model: "chatonlm-always" }),
        null,
      );
      await typeMessage(driver, "Always?");
      await submit(driver);
      await driver.waitFor("[data-testid=chat]");
      await waitReply(driver, "Received think=absent.");
      assert.equal(
        "think" in (await requestsOf("chatonlm-always")).at(-1),
        false,
      );
      thinking = await waitThinking(
        driver,
        FOLDED,
        "raisonnement de chatonlm-always",
      );
      assert.equal(thinking.title, "Thought for a moment");

      // chatonlm-stubborn (LFM2.5 tiré de Hugging Face) : bascule tant qu'on
      // ne sait rien ; Think coupé, il raisonne quand même : raisonnement
      // affiché, puis plus de bouton pour ce modèle (relu après une relance).
      await newChat(driver);
      await useModel(driver, "chatonlm-stubborn");
      await waitThink(driver, "toggle", (s) => s.pressed === "true");
      await driver.click(await driver.find("button[aria-label='Think']"));
      await waitThink(driver, "toggle", (s) => s.pressed === "false");
      await send(driver, "Off?");
      await driver.waitFor("[data-testid=chat]");
      await waitReply(driver, "Received think=false.");
      assert.equal((await requestsOf("chatonlm-stubborn")).at(-1).think, false);
      thinking = await waitThinking(
        driver,
        FOLDED,
        "raisonnement de chatonlm-stubborn malgré think false",
      );
      assert.equal(thinking.title, "Thought for a moment");
      await waitThink(driver, "none");
      await send(driver, "Again?");
      await waitReply(driver, "Received think=absent.");
      assert.equal(
        "think" in (await requestsOf("chatonlm-stubborn")).at(-1),
        false,
      );
      await newChat(driver);
      await useModel(driver, "chatonlm-stubborn");
      await delay(1500);
      assert.deepEqual(await thinkState(driver), { kind: "none" });

      // chatonlm-never ([false]) et chatonlm-plain (sans capacité) : aucun
      // bouton, think absent ; chatonlm-plain : une seule requête.
      for (const model of ["chatonlm-never", "chatonlm-plain"]) {
        await newChat(driver);
        await useModel(driver, model);
        await delay(1500);
        assert.deepEqual(await thinkState(driver), { kind: "none" }, model);
        await send(driver, "Nothing?");
        await driver.waitFor("[data-testid=chat]");
        await waitReply(driver, "Received think=absent.");
        const sent = await requestsOf(model);
        assert.equal(sent.length, 1, `${model} : ${JSON.stringify(sent)}`);
        assert.equal("think" in sent[0], false, model);
        assert.equal(await lastThinking(driver), null, model);
      }

      // chatonlm-nometa : capacité sans objet thinking, bascule active.
      await newChat(driver);
      await useModel(driver, "chatonlm-nometa");
      await waitThink(driver, "toggle", (s) => s.pressed === "true");
      await send(driver, "On?");
      await driver.waitFor("[data-testid=chat]");
      await waitReply(driver, "Received think=true.");
      assert.equal((await requestsOf("chatonlm-nometa")).at(-1).think, true);
      assert.notEqual(await lastThinking(driver), null);
      await driver.click(await driver.find("button[aria-label='Think']"));
      await waitThink(driver, "toggle", (s) => s.pressed === "false");
      await send(driver, "Off?");
      await waitReply(driver, "Received think=false.");
      assert.equal((await requestsOf("chatonlm-nometa")).at(-1).think, false);

      // Revenir aux conversations : leur modèle redevient le modèle choisi,
      // avec sa préférence (Off, High) ; puis rechargement : relue en base.
      await openChat(driver, chats.levelsOff);
      await waitThink(
        driver,
        "levels",
        (s) => s.label === "Thinking level: Off",
      );
      await openChat(driver, chats.levels);
      await waitThink(
        driver,
        "levels",
        (s) => s.label === "Thinking level: High",
      );
      await openChat(driver, chats.levelsOff);
      await waitThink(
        driver,
        "levels",
        (s) => s.label === "Thinking level: Off",
      );
      await reload(driver);
      await waitThink(
        driver,
        "levels",
        (s) => s.label === "Thinking level: Off",
      );
      await openChat(driver, chats.qwen);
      await waitThink(driver, "toggle", (s) => s.pressed === "false");

      // Échap dans le menu des niveaux pendant une réponse : la réponse
      // (chatonlm-wait, qui ne répond jamais) continue. Elle n'a pas encore de
      // message de réponse : rouvrir sa conversation garde le modèle choisi,
      // donc chatonlm-levels (rouvert juste avant).
      await newChat(driver);
      await useModel(driver, "chatonlm-wait");
      await send(driver, "Wait.");
      await driver.waitFor("[data-testid=chat]");
      await driver.waitFor("button[aria-label='Stop']");
      chats.wait = await currentChatId(driver);
      await openChat(driver, chats.levels);
      await waitUntil(
        async () => (await shownModel(driver)) === "chatonlm-levels",
        "modèle de la conversation chatonlm-levels",
      );
      await openChat(driver, chats.wait);
      await driver.waitFor("button[aria-label='Stop']");
      assert.equal(await shownModel(driver), "chatonlm-levels");
      await waitThink(
        driver,
        "levels",
        (s) => s.label === "Thinking level: High",
      );
      const abandoned = await fake.disconnects();
      menu = await openLevelMenu(driver);
      assert.equal(menu.focused, 2);
      await recordFocus(driver);
      await pressKeys(["down", "up", "home", "end"]);
      assert.deepEqual(
        (await focusPath(driver)).map((step) => step.name),
        ["item:Low", "item:High", "item:Low", "item:High"],
      );
      await pressKeys("escape");
      await waitUntil(
        async () => (await levelMenu(driver)) === null,
        "menu fermé par Échap",
      );
      assert.equal((await focused(driver)).name, "Thinking level: High");
      await delay(500);
      assert.ok(
        await exists(driver, "button[aria-label='Stop']"),
        "Échap dans le menu a arrêté la réponse",
      );
      assert.equal(await fake.disconnects(), abandoned);
      // Tab depuis le menu : fermé, focus au contrôle suivant.
      await openLevelMenu(driver);
      await pressKeys("tab");
      await waitUntil(
        async () => (await levelMenu(driver)) === null,
        "menu fermé par Tab",
      );
      assert.equal((await focused(driver)).name, "Select model");
      // Échap suivante (aucun menu ouvert) : arrête la réponse.
      await pressKeys("escape");
      await driver.waitFor("button[aria-label='Send message']");
      await waitUntil(
        async () => (await fake.disconnects()) === abandoned + 1,
        "réponse arrêtée par Échap",
      );

      // Bloc de raisonnement en direct (chatonlm-slowthink, environ 9 s de
      // raisonnement ; /api/show sans capacité thinking : pas de bouton
      // Think).
      await newChat(driver);
      await useModel(driver, "chatonlm-slowthink");
      await send(driver, "Think slowly.");
      await driver.waitFor("[data-testid=chat]");
      chats.slow = await currentChatId(driver);
      thinking = await waitThinking(
        driver,
        { ...PREVIEW, title: "Thinking..." },
        "aperçu pendant le raisonnement",
      );
      assert.ok(thinking.height <= 192.5, `aperçu de ${thinking.height} px`);
      assert.ok(await exists(driver, "button[aria-label='Stop']"));
      assert.deepEqual(await thinkState(driver), { kind: "none" });
      thinking = await waitThinking(
        driver,
        { expanded: "false", visibility: "hidden" },
        "repli à l'arrivée du contenu",
      );
      assert.equal(thinking.maxHeight, "0px");
      assert.equal(thinking.opacity, "0");
      const seconds = Number(SECONDS.exec(thinking.title)?.[1]);
      assert.ok(seconds >= 5 && seconds <= 20, `titre ${thinking.title}`);
      await waitReply(driver, '"Hello"');
      let saved = (await driver.invoke("get_chat", { id: chats.slow }))
        .messages;
      assert.equal(
        thinking.title,
        `Thought for ${savedSeconds(saved[1])} seconds`,
      );
      // Clic : déplié (tout le raisonnement), puis replié.
      const header = "[data-message-index] h3 > button";
      await driver.click(await driver.find(header));
      thinking = await waitThinking(driver, EXPANDED, "déplié au clic");
      assert.equal(
        thinking.text.trim().slice(0, 30),
        saved[1].thinking.trim().slice(0, 30),
      );
      await driver.click(await driver.find(header));
      await waitThinking(driver, FOLDED, "replié au clic");
      // Clavier (vraies touches) : l'en-tête atteint par Tab (Maj+Tab puis
      // Tab), Entrée déplie, Espace replie ; anneau de focus visible.
      await driver.execute(`document.querySelector("${header}").focus()`);
      await pressKeys(["shift+tab", "tab"]);
      assert.equal(
        await driver.execute(
          `return document.activeElement === document.querySelector("${header}")`,
        ),
        true,
        "en-tête du raisonnement hors de l'ordre de Tab",
      );
      await pressKeys("enter");
      await waitThinking(driver, EXPANDED, "déplié par Entrée");
      await pressKeys("space");
      await waitThinking(driver, FOLDED, "replié par Espace");
      const headerFocus = await focused(driver);
      assert.equal(headerFocus.name, "button", "focus perdu par l'en-tête");
      assert.equal(
        headerFocus.visible,
        true,
        "anneau de focus absent au clavier",
      );

      // Arrêt pendant le raisonnement : jamais « Thinking... » figé.
      await send(driver, "Think again.");
      await waitThinking(
        driver,
        { title: "Thinking..." },
        "second raisonnement",
      );
      await delay(1000);
      await driver.click(await driver.find("button[aria-label='Stop']"));
      await driver.waitFor("button[aria-label='Send message']");
      thinking = await lastThinking(driver);
      assert.notEqual(thinking.title, "Thinking...", "titre figé à l'arrêt");
      await waitThinking(driver, FOLDED, "raisonnement arrêté replié");
      await delay(1500);
      thinking = await lastThinking(driver);
      assert.notEqual(
        thinking.title,
        "Thinking...",
        "titre figé après relecture",
      );
      saved = (await driver.invoke("get_chat", { id: chats.slow })).messages;
      assert.equal(saved.length, 4);
      assert.notEqual(
        saved[3].thinkingEndedAt,
        null,
        "fin du raisonnement non enregistrée",
      );
      assert.equal(saved[3].content, "");
      await reload(driver);
      await driver.waitFor(header);
      slowTitles = await driver.execute(
        `return [...document.querySelectorAll(arguments[0])].map((h) => h.textContent)`,
        [header],
      );
      assert.equal(slowTitles.length, 2);
      assert.equal(
        slowTitles[0],
        `Thought for ${savedSeconds(saved[1])} seconds`,
      );
      assert.notEqual(slowTitles[1], "Thinking...");
      await waitThinking(driver, FOLDED, "raisonnement arrêté relu");

      // Long raisonnement : l'aperçu de 12rem montre la fin, le début coupé
      // sous un dégradé de la couleur du fond (clair puis sombre).
      await newChat(driver);
      await useModel(driver, "chatonlm-longthink");
      await setTheme(driver, "light");
      await send(driver, "Think a lot.");
      await driver.waitFor("[data-testid=chat]");
      let preview;
      await waitUntil(
        async () => {
          preview = await previewState(driver);
          return (
            preview !== null &&
            preview.title === "Thinking..." &&
            preview.fade &&
            preview.count >= 12
          );
        },
        () => `aperçu qui déborde : ${JSON.stringify(preview)}`,
      );
      assert.ok(
        Math.abs(preview.height - 192) <= 1,
        `hauteur ${preview.height}`,
      );
      assert.ok(preview.overflow);
      assert.ok(
        Math.abs(preview.bottomGap) <= 1,
        `bas du texte à ${preview.bottomGap} px`,
      );
      assert.ok(preview.firstClipped, "début du raisonnement visible");
      assert.ok(preview.lastVisible, "fin du raisonnement cachée");
      assertFadeMatchesBackground(preview, "clair");
      await driver.screenshot(".tools/shots/thinking-light.png");
      await setTheme(driver, "dark");
      preview = await previewState(driver);
      assert.equal(
        preview.title,
        "Thinking...",
        "raisonnement fini avant la capture sombre",
      );
      assert.ok(preview.dark);
      assertFadeMatchesBackground(preview, "sombre");
      assert.ok(preview.lastVisible && preview.firstClipped);
      await driver.screenshot(".tools/shots/thinking-dark.png");
      await setTheme(driver, "light");
      await waitReply(driver, "Done thinking.");
      thinking = await waitThinking(driver, FOLDED, "long raisonnement replié");
      assert.match(thinking.title, SECONDS);

      // Modèle choisi au prochain lancement.
      await driver.invoke("update_settings", {
        patch: { selectedModel: "chatonlm-levels-off" },
      });
      await collectViolations(driver);
    }),
  );

  // Relance : préférences et durées relues en base.
  await withApp(
    binary,
    { name: "thinking-relaunch", env },
    shotOnFailure("thinking-relaunch", async (driver) => {
      await driver.waitFor("[data-testid=new-chat]");
      await driver.execute(WATCH_PAGE);
      await waitUntil(
        async () => (await shownModel(driver)) === "chatonlm-levels-off",
        "modèle choisi relu",
      );
      await waitThink(
        driver,
        "levels",
        (s) => s.label === "Thinking level: Off" && s.grey,
      );
      await pickModel(driver, "qwen3:0.6b");
      await waitThink(driver, "toggle", (s) => s.pressed === "false");
      assert.equal(
        await driver.invoke("get_think_pref", { model: "chatonlm-levels" }),
        "high",
      );
      assert.equal(
        await driver.invoke("get_think_pref", { model: "chatonlm-nometa" }),
        false,
      );
      await openChat(driver, chats.slow);
      await waitUntil(
        async () =>
          JSON.stringify(
            await driver.execute(
              `return [...document.querySelectorAll("[data-message-index] h3 > button")]
                 .map((h) => h.textContent)`,
            ),
          ) === JSON.stringify(slowTitles),
        () => `titres relus : ${JSON.stringify(slowTitles)}`,
      );
      await openChat(driver, chats.levels);
      await waitThink(
        driver,
        "levels",
        (s) => s.label === "Thinking level: High",
      );
      await collectViolations(driver);
    }),
  );
} finally {
  await fake.stop();
}

assert.deepEqual(violations, [], "violations de la CSP");
assert.deepEqual(consoleErrors, [], "erreurs dans la console");
assert.deepEqual(consoleWarnings, [], "avertissements dans la console");
