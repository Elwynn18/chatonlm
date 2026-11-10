// Chargement des modèles dans l'app réelle (propre à ChatonLM), avec le faux
// moteur (fake-llama lancé par l'app : un processus par modèle) et de vraies
// touches du clavier (e2e/lib/keyboard.mjs) :
// - chat : « Loading model... » (role=status) à la place des trois points
//   pendant le chargement de chatonlm-slowload (1,5 s), puis la réponse ;
//   second envoi : même processus, aucune ligne ni nouveau lancement ;
//   Stop pendant le chargement (chatonlm-longload, 6 s) : arrêt propre, pas
//   de réponse, envoi suivant normal ; annonce par une région role=status
//   toujours présente ;
// - onglet Models des réglages : cran « 15 min » au clavier → échéance de
//   15 min pour le modèle chargé ensuite ; liste des modèles chargés
//   (carte/processeur, contexte, échéance ; chatonlm-split en partie sur le
//   processeur : 22 couches sur 29), Unload (processus arrêté), Load depuis la
//   liste, « Always » → « Stays loaded », Unload all → « No models are
//   loaded. » et plus aucun processus ; jauge de la VRAM (machine injectée de
//   24 Gio dont 22 libres) : une part par modèle sur la carte, part de
//   chatonlm-split en mémoire vive, pastilles des lignes ; escargot de
//   chatonlm-split sur sa ligne, puis à côté du sélecteur de la zone de
//   saisie quand il est choisi ;
// - pré-prompt : tapé dans l'onglet Models, enregistré en quittant le champ
//   (vraie touche Tab), envoyé en premier message système du chat suivant,
//   jamais affiché ni enregistré dans la conversation ;
// - préchargement : interrupteur → modèle choisi lancé aussitôt, autre
//   modèle choisi → lancé au démarrage de la page ; coupé → plus rien ;
// - aucune violation de la CSP ni erreur ou avertissement dans la console.
// Les tailles et pourcentages affichés viennent des lignes de chargement du
// faux (copie de celles du vrai, voir fake-llama cli.rs) : leur forme est
// vérifiée, pas leur valeur.
import assert from "node:assert/strict";
import { withApp } from "./lib/app.mjs";
import { fakeModel, startEngine } from "./lib/fake.mjs";
import { PENDING_CALLS } from "./lib/page.mjs";
import { pressKeys } from "./lib/keyboard.mjs";

const binary = process.argv[2];
/** Le plus récent d'abord : modèle choisi par défaut. */
const SIMPLE = fakeModel("simple");
const SLOWLOAD = fakeModel("slowload");
const LONGLOAD = fakeModel("longload");
const SPLIT = fakeModel("split");
const OTHER = fakeModel("tools");
const THIRD = fakeModel("simple", "Q8_0");
const engine = startEngine([SIMPLE, SLOWLOAD, LONGLOAD, SPLIT, OTHER, THIRD]);

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

/** PID du processus du modèle `id` (null : pas lancé). */
const pidOf = (id) => engine.server(id)?.pid ?? null;

/** Modèle du moteur (`running_models`), ou undefined. */
const running = async (driver, id) =>
  (await driver.invoke("running_models")).find((model) => model.id === id);

const exists = (driver, css) =>
  driver.execute("return document.querySelector(arguments[0]) !== null", [css]);

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

/** Modèle choisi enregistré. */
const chosenModel = async (driver) =>
  (await driver.invoke("get_settings")).selectedModel;

/** Choisit un modèle, puis recharge la page (depuis le chat). */
async function useModel(driver, model) {
  await driver.waitFor("[title='Select model']");
  await driver.invoke("update_settings", { patch: { selectedModel: model } });
  await reload(driver);
  await driver.waitFor("[title='Select model']");
  await waitUntil(
    async () => (await chosenModel(driver)) === model,
    `modèle ${model}`,
  );
}

async function showSidebar(driver) {
  if (await exists(driver, "button[aria-label='Show sidebar']")) {
    await driver.click(await driver.find("button[aria-label='Show sidebar']"));
  }
}

async function newChat(driver) {
  await showSidebar(driver);
  await driver.click(await driver.waitFor("a[title='New chat']"));
  await driver.waitFor("[data-testid=new-chat]");
}

const textarea = (driver) => driver.waitFor("textarea[aria-label='Message']");

async function send(driver, text) {
  await driver.type(await textarea(driver), text);
  await driver.execute(
    `document.querySelector("textarea[aria-label='Message']").dispatchEvent(
       new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true }));`,
  );
}

const busy = (driver) => exists(driver, "button[aria-label='Stop']");

/** Attend la fin de la réponse en cours (plus de bouton Stop). */
async function waitIdle(driver) {
  await waitUntil(
    async () => !(await busy(driver)),
    "fin de la réponse",
    15000,
  );
}

/**
 * Ligne de chargement visible (null si absente) et région d'annonce
 * (role=status, toujours présente pendant la réponse : seul son texte change).
 */
const loadingLine = (driver) =>
  driver.execute(
    `const line = document.querySelector("[data-testid=loading-model]");
     const region = document.querySelector("[data-testid=loading-status]");
     return line && {
       text: line.textContent.trim(),
       role: region?.getAttribute("role") ?? null,
       announced: region?.textContent.trim() ?? null,
     };`,
  );

const assistantCount = (driver) =>
  driver.execute(
    "return document.querySelectorAll('[data-role=assistant]').length",
  );

/** Ouvre l'onglet Models des réglages. */
async function openModelsTab(driver) {
  await showSidebar(driver);
  await driver.click(await driver.waitFor("nav a[href='/settings']"));
  await driver.click(await driver.waitFor("[data-testid=settings-tab-models]"));
  await driver.waitFor("[data-testid=models-settings]");
  await driver.waitFor("[data-testid=loaded-models]");
}

/** Revient au chat (nouvelle conversation) depuis les réglages. */
async function toChat(driver) {
  await showSidebar(driver);
  await driver.click(await driver.waitFor("nav a[href$='/c/new']"));
  await driver.waitFor("[data-testid=new-chat]");
}

/** Lignes de la carte « Loaded models » : nom → texte. */
const loadedRows = (driver) =>
  driver.execute(
    `return Object.fromEntries([...document.querySelectorAll("[data-testid=loaded-model]")]
       .map((row) => [row.dataset.model, row.textContent]));`,
  );

const cardText = (driver) =>
  driver.execute(
    "return document.querySelector('[data-testid=loaded-models]')?.textContent ?? ''",
  );

const slider = "[data-testid=models-settings] [role=slider]";
const sliderText = (driver) =>
  driver.execute(
    "return document.querySelector(arguments[0]).getAttribute('aria-valuetext')",
    [slider],
  );

/** Déplace le curseur au clavier jusqu'au cran `text`. */
async function setKeepAlive(driver, text, keepAlive) {
  await driver.execute("document.querySelector(arguments[0]).focus()", [
    slider,
  ]);
  for (let i = 0; i < 8 && (await sliderText(driver)) !== text; i++) {
    await pressKeys("right");
  }
  assert.equal(await sliderText(driver), text);
  await waitUntil(
    async () => (await driver.invoke("get_settings")).keepAlive === keepAlive,
    `keepAlive ${keepAlive} enregistré`,
  );
}

await withApp(
  binary,
  { name: "model-loading", env: engine.env },
  async (driver) => {
    await driver.waitFor("[data-testid=new-chat]");
    await waitUntil(
      async () => (await chosenModel(driver)) === SIMPLE,
      "modèle par défaut enregistré",
    );
    await driver.execute(WATCH_PAGE);

    // --- Chat : chargement annoncé, puis réponse.
    await useModel(driver, SLOWLOAD);
    await send(driver, "Hello");
    const line = await waitUntil(
      () => loadingLine(driver),
      "ligne « Loading model... »",
    );
    assert.deepEqual(line, {
      text: "Loading model...",
      role: "status",
      announced: "Loading model...",
    });
    await driver.screenshot(".tools/shots/loading-model.png");
    assert.equal(await exists(driver, "[data-testid=waiting]"), false);
    await waitIdle(driver);
    assert.equal(await loadingLine(driver), null);
    assert.equal(await assistantCount(driver), 1);
    const firstPid = pidOf(SLOWLOAD);
    assert.ok(firstPid, "processus lancé par le chat");

    // Second envoi : même processus, aucune ligne de chargement.
    await send(driver, "Again");
    let seen = false;
    await waitUntil(async () => {
      seen ||= (await loadingLine(driver)) !== null;
      return (await assistantCount(driver)) === 2 && !(await busy(driver));
    }, "seconde réponse");
    assert.equal(seen, false, "aucun chargement pour un modèle prêt");
    assert.equal(pidOf(SLOWLOAD), firstPid, "aucun nouveau lancement");
    assert.equal((await engine.chatRequests(SLOWLOAD)).length, 2);

    // Stop pendant le chargement (chatonlm-longload : 6 s) : arrêt propre,
    // pas de réponse. Nouvelle conversation d'abord : dans une conversation,
    // le sélecteur montre le modèle de la conversation.
    await newChat(driver);
    await useModel(driver, LONGLOAD);
    await send(driver, "Stop me");
    await waitUntil(() => loadingLine(driver), "ligne pendant le chargement");
    await driver.click(await driver.find("button[aria-label='Stop']"));
    await waitIdle(driver);
    assert.equal(await loadingLine(driver), null);
    assert.equal(await assistantCount(driver), 0);
    await send(driver, "Now answer");
    await waitUntil(
      async () => (await assistantCount(driver)) === 1 && !(await busy(driver)),
      "réponse après l'arrêt",
      15000,
    );
    assert.equal(await loadingLine(driver), null);
    await driver.invoke("unload_model", { model: LONGLOAD });
    await waitUntil(() => pidOf(LONGLOAD) === null, "chatonlm-longload arrêté");

    // --- Onglet Models : durée avant déchargement.
    await openModelsTab(driver);
    assert.equal(await sliderText(driver), "Default");
    await setKeepAlive(driver, "15 min", 900);

    // Modèles chargés : chatonlm-split lancé par Load.
    await driver.invoke("load_model", { model: SPLIT });
    let rows = {};
    await waitUntil(
      async () => (rows = await loadedRows(driver))[SPLIT],
      () => `chatonlm-split listé (${JSON.stringify(rows)})`,
    );
    assert.match(rows[SLOWLOAD], /100% GPU/);
    assert.match(rows[SLOWLOAD], /\d+k context/);
    assert.match(rows[SLOWLOAD], /Unloads in \d+ (minutes?|seconds?)/);
    assert.match(rows[SPLIT], /\d+%\/\d+% CPU\/GPU/);
    assert.match(rows[SPLIT], /Unloads in 1[45] minutes/);
    assert.match(await cardText(driver), /^.*2 models loaded · .+ in memory/);

    // Jauge de la VRAM : une part par modèle sur la carte, autres usages,
    // part de chatonlm-split en mémoire vive.
    const gauge = await waitUntil(
      () =>
        driver.execute(
          `const gauge = document.querySelector("[data-testid=vram-gauge]");
           return gauge && {
             usage: gauge.querySelector("[data-testid=vram-usage]").textContent,
             meter: gauge.querySelector("[role=meter]").getAttribute("aria-valuetext"),
             parts: [...gauge.querySelectorAll("[data-testid=vram-model]")]
               .map((part) => [part.dataset.model, part.style.width]),
             free: gauge.querySelector("[data-testid=vram-free]").textContent,
             spilled: gauge.querySelector("[data-testid=vram-spilled]")?.textContent ?? null,
             swatches: document.querySelectorAll("[data-testid=loaded-model] [data-testid=vram-swatch]").length,
           };`,
        ),
      "jauge de la VRAM",
    );
    assert.match(gauge.usage, /^.+ of 24 GB used$/);
    assert.equal(gauge.meter, gauge.usage);
    assert.deepEqual(
      gauge.parts.map(([model]) => model).sort(),
      [SLOWLOAD, SPLIT].sort(),
    );
    assert.match(gauge.free, /^Free: .+$/);
    assert.match(
      gauge.spilled ?? "",
      /^.+ doesn't fit in VRAM: that part runs from system memory, more slowly\.$/,
    );
    assert.equal(gauge.swatches, 2);
    await driver.screenshot(".tools/shots/models-vram.png");

    // Escargot : seul chatonlm-split est en partie sur le processeur.
    const snails = await driver.execute(
      `return [...document.querySelectorAll("[data-testid=loaded-model] [data-testid=slow-model]")]
         .map((snail) => [snail.closest("[data-testid=loaded-model]").dataset.model,
                          snail.getAttribute("aria-label")]);`,
    );
    assert.deepEqual(
      snails.map(([model]) => model),
      [SPLIT],
    );
    const slow = snails[0][1];
    assert.match(
      slow,
      /^Slower replies: \d+% of .+ runs on the processor \(not enough graphics memory\)$/,
    );
    // Choisi dans le chat : l'escargot suit le sélecteur de modèle.
    await toChat(driver);
    const chosen = await chosenModel(driver);
    const composerSnail = () =>
      driver.execute(
        `const snail = document.querySelector("[data-testid=slow-model]");
         const picker = document.querySelector('[title="Select model"]');
         return snail && {
           label: snail.getAttribute("aria-label"),
           after: Boolean(picker.compareDocumentPosition(snail) & Node.DOCUMENT_POSITION_FOLLOWING),
         };`,
      );
    assert.equal(await composerSnail(), null);
    await useModel(driver, SPLIT);
    const snail = await waitUntil(
      composerSnail,
      "escargot de la zone de saisie",
    );
    assert.deepEqual(snail, { label: slow, after: true });
    await driver.screenshot(".tools/shots/slow-model.png");
    await useModel(driver, chosen);
    assert.equal(await composerSnail(), null);
    await openModelsTab(driver);
    await waitUntil(
      async () => (await loadedRows(driver))[SPLIT],
      "chatonlm-split listé de nouveau",
    );

    // Unload d'une ligne : processus arrêté.
    const splitPid = pidOf(SPLIT);
    await driver.click(
      await driver.find(
        `[data-testid=loaded-model][data-model='${SPLIT}'] [data-testid=unload-model]`,
      ),
    );
    await waitUntil(
      async () => !(await loadedRows(driver))[SPLIT],
      "chatonlm-split déchargé",
    );
    await waitUntil(
      () => pidOf(SPLIT) === null,
      "processus de chatonlm-split arrêté",
    );
    assert.ok(splitPid);

    // Load depuis la liste des installés : durée réglée (15 min).
    const loadFromList = async (model) => {
      await driver.execute(
        `const select = document.querySelector("[data-testid=load-model-select]");
         select.value = arguments[0];
         select.dispatchEvent(new Event("change", { bubbles: true }));`,
        [model],
      );
      await driver.click(await driver.find("[data-testid=load-model-button]"));
    };
    await loadFromList(OTHER);
    await waitUntil(
      async () => (await loadedRows(driver))[OTHER],
      "chargé par Load",
    );
    assert.match((await loadedRows(driver))[OTHER], /Unloads in 1[45] minutes/);
    assert.equal((await running(driver, OTHER)).kept, false);

    // « Always » : gardé chargé.
    await setKeepAlive(driver, "Always", -1);
    await loadFromList(THIRD);
    await waitUntil(
      async () => /Stays loaded/.test((await loadedRows(driver))[THIRD] ?? ""),
      "gardé chargé",
    );
    assert.equal((await running(driver, THIRD)).kept, true);
    await driver.screenshot(".tools/shots/models-tab.png");

    // Pré-prompt : tapé, enregistré en quittant le champ (vraie touche Tab).
    const PROMPT = "Always answer like a cat.";
    await driver.type(
      await driver.waitFor("[data-testid=system-prompt]"),
      PROMPT,
    );
    await pressKeys("tab");
    await waitUntil(
      async () => (await driver.invoke("get_settings")).systemPrompt === PROMPT,
      "pré-prompt enregistré",
    );

    // Le chat suivant porte le pré-prompt et la durée réglée.
    await toChat(driver);
    await useModel(driver, SIMPLE);
    await send(driver, "Keep me");
    await waitIdle(driver);
    const sent = (await engine.chatRequests(SIMPLE)).at(-1);
    assert.deepEqual(sent.messages[0], { role: "system", content: PROMPT });
    assert.equal(
      sent.messages.filter((message) => message.role === "system").length,
      1,
    );
    assert.equal((await running(driver, SIMPLE)).kept, true);
    // Ni affiché ni enregistré dans la conversation.
    assert.equal(
      await driver.execute(
        "return document.body.textContent.includes(arguments[0])",
        [PROMPT],
      ),
      false,
    );

    // Unload all : plus aucun processus.
    await openModelsTab(driver);
    await driver.click(await driver.waitFor("[data-testid=unload-all]"));
    await waitUntil(
      async () => (await cardText(driver)).includes("No models are loaded."),
      "plus aucun modèle chargé",
    );
    await waitUntil(
      () => engine.servers().length === 0,
      "tous les processus arrêtés",
    );

    // --- Préchargement du modèle choisi.
    await toChat(driver);
    await useModel(driver, SIMPLE);
    await openModelsTab(driver);
    const switchCss = "[data-testid=models-settings] [role=switch]";
    assert.equal(
      await driver.execute(
        "return document.querySelector(arguments[0]).getAttribute('aria-label')",
        [switchCss],
      ),
      "Preload the chosen model",
    );
    await driver.click(await driver.find(switchCss));
    await waitUntil(
      async () => (await driver.invoke("get_settings")).preloadModel,
      "préchargement activé",
    );
    await waitUntil(() => pidOf(SIMPLE) !== null, "modèle choisi préchargé");
    // Autre modèle choisi : lancé au démarrage de la page.
    await toChat(driver);
    await useModel(driver, OTHER);
    await waitUntil(() => pidOf(OTHER) !== null, "nouveau modèle préchargé");
    // Coupé : plus de préchargement.
    await driver.invoke("update_settings", { patch: { preloadModel: false } });
    await driver.invoke("unload_all_models");
    await waitUntil(
      () => engine.servers().length === 0,
      "tout arrêté avant la coupure",
    );
    await useModel(driver, THIRD);
    await delay(2000);
    assert.deepEqual(
      engine.servers(),
      [],
      "rien de lancé, préchargement coupé",
    );

    await collectViolations(driver);
  },
);
assert.deepEqual(violations, [], "violations de la CSP");
assert.deepEqual(consoleErrors, [], "erreurs dans la console");
assert.deepEqual(consoleWarnings, [], "avertissements dans la console");
console.log("OK : chargement des modèles");
