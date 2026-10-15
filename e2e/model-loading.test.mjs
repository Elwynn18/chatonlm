// Chargement des modèles dans l'app réelle (propre à ChatonLM), avec le faux
// serveur et de vraies touches du clavier (e2e/lib/keyboard.mjs) :
// - chat : « Loading model... » (role=status) à la place des trois points
//   pendant le chargement de chatonlm-slowload (1,5 s), puis la réponse ;
//   second envoi : modèle en mémoire, aucune ligne ni nouveau chargement ;
//   Stop pendant le chargement (chatonlm-longload, 6 s) : arrêt propre, pas
//   de réponse, envoi suivant normal ; annonce par une région role=status
//   toujours présente ;
// - onglet Models des réglages : cran « 15 min » au clavier → keep_alive 900
//   dans le chargement et le chat suivants ; liste des modèles chargés
//   (processeur, contexte, échéance ; chatonlm-split en partie sur CPU),
//   Unload, Load depuis la liste, « Always » → « Stays loaded », Unload all →
//   « No models are loaded. » ; jauge de la VRAM (machine injectée de 24 Gio
//   dont 22 libres) : une part par modèle en VRAM, autres usages, part de
//   chatonlm-split en mémoire vive, pastilles des lignes ;
// - pré-prompt : tapé dans l'onglet Models, enregistré en quittant le champ
//   (vraie touche Tab), envoyé en premier message système du chat suivant,
//   jamais affiché ni enregistré dans la conversation ;
// - préchargement : interrupteur → modèle choisi chargé aussitôt, autre
//   modèle choisi → chargé au démarrage de la page ; coupé → plus rien ;
// - aucune violation de la CSP ni erreur ou avertissement dans la console.
import assert from "node:assert/strict";
import { withApp } from "./lib/app.mjs";
import { startFake } from "./lib/fake.mjs";
import { PENDING_CALLS } from "./lib/page.mjs";
import { pressKeys } from "./lib/keyboard.mjs";

const binary = process.argv[2];
const fake = await startFake("ollama");

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

/** Demandes de chargement et de déchargement reçues, dans l'ordre. */
async function loads() {
  const response = await fetch(new URL("/fake/loads", fake.url));
  return response.json();
}

/** Corps des /api/chat reçus par le faux serveur, d'un modèle. */
async function requestsOf(model) {
  const response = await fetch(new URL("/fake/requests", fake.url));
  return (await response.json()).filter((request) => request.model === model);
}

/** Charge ou décharge un modèle sur le faux serveur, hors de l'app. */
async function generate(body) {
  const response = await fetch(new URL("/api/generate", fake.url), {
    method: "POST",
    body: JSON.stringify(body),
  });
  assert.equal(response.status, 200, await response.text());
}

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

const shownModel = (driver) =>
  driver.execute(
    "return document.querySelector('[title=\"Select model\"]')?.textContent ?? null",
  );

/** Choisit un modèle, puis recharge la page (depuis le chat). */
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

/** Dernière demande de chargement d'un modèle (null : aucune). */
async function lastLoad(model) {
  return (
    (await loads())
      .filter((entry) => entry.model === model && entry.kind === "load")
      .at(-1) ?? null
  );
}

try {
  await withApp(
    binary,
    { name: "model-loading", env: { OLLAMA_HOST: fake.url } },
    async (driver) => {
      await driver.waitFor("[data-testid=new-chat]");
      await waitUntil(
        async () =>
          (await driver.invoke("get_settings")).selectedModel === "qwen3:0.6b",
        "modèle par défaut enregistré",
      );
      await driver.execute(WATCH_PAGE);

      // --- Chat : chargement annoncé, puis réponse.
      await useModel(driver, "chatonlm-slowload");
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
      const first = await lastLoad("chatonlm-slowload");
      assert.ok(first, "chargement demandé par le chat");
      // Durée par défaut : rien d'envoyé.
      assert.equal(first.keep_alive, undefined);
      assert.equal(
        (await requestsOf("chatonlm-slowload"))[0].keep_alive,
        undefined,
      );

      // Second envoi : modèle en mémoire, aucune ligne ni chargement.
      const before = (await loads()).length;
      await send(driver, "Again");
      let seen = false;
      await waitUntil(async () => {
        seen ||= (await loadingLine(driver)) !== null;
        return (await assistantCount(driver)) === 2 && !(await busy(driver));
      }, "seconde réponse");
      assert.equal(seen, false, "aucun chargement pour un modèle en mémoire");
      assert.equal((await loads()).length, before);

      // Stop pendant le chargement (chatonlm-longload : 6 s) : arrêt propre,
      // pas de réponse.
      // Nouvelle conversation d'abord : dans une conversation, le sélecteur
      // montre le modèle de la conversation.
      await newChat(driver);
      await useModel(driver, "chatonlm-longload");
      await send(driver, "Stop me");
      await waitUntil(() => loadingLine(driver), "ligne pendant le chargement");
      await driver.click(await driver.find("button[aria-label='Stop']"));
      await waitIdle(driver);
      assert.equal(await loadingLine(driver), null);
      assert.equal(await assistantCount(driver), 0);
      await send(driver, "Now answer");
      await waitUntil(
        async () =>
          (await assistantCount(driver)) === 1 && !(await busy(driver)),
        "réponse après l'arrêt",
        15000,
      );
      assert.equal(await loadingLine(driver), null);
      await generate({ model: "chatonlm-longload", keep_alive: 0 });

      // --- Onglet Models : durée avant déchargement.
      await openModelsTab(driver);
      assert.equal(await sliderText(driver), "Default");
      await setKeepAlive(driver, "15 min", 900);

      // Modèles chargés, nommés comme par /api/ps (`:latest` ajouté) ;
      // contexte du faux serveur (4096 sans num_ctx).
      await generate({ model: "chatonlm-split" });
      let rows = {};
      await waitUntil(
        async () => (rows = await loadedRows(driver))["chatonlm-split:latest"],
        () => `chatonlm-split listé (${JSON.stringify(rows)})`,
      );
      assert.match(rows["chatonlm-slowload:latest"], /100% GPU/);
      assert.match(rows["chatonlm-slowload:latest"], /4k context/);
      assert.match(
        rows["chatonlm-slowload:latest"],
        /Unloads in \d+ (minutes?|seconds?)/,
      );
      assert.match(rows["chatonlm-split:latest"], /25%\/75% CPU\/GPU/);
      assert.match(
        await cardText(driver),
        /2 models loaded · 2\.0 GB in memory/,
      );

      // Jauge de la VRAM : 2 Gio occupés d'après la machine injectée, dont
      // 1,75 pour les modèles (un quart de chatonlm-split en mémoire vive).
      const gauge = await waitUntil(
        () =>
          driver.execute(
            `const gauge = document.querySelector("[data-testid=vram-gauge]");
             return gauge && {
               usage: gauge.querySelector("[data-testid=vram-usage]").textContent,
               meter: gauge.querySelector("[role=meter]").getAttribute("aria-valuetext"),
               parts: [...gauge.querySelectorAll("[data-testid=vram-model]")]
                 .map((part) => [part.dataset.model, part.style.width]),
               other: gauge.querySelector("[data-testid=vram-other]")?.style.width ?? null,
               free: gauge.querySelector("[data-testid=vram-free]").textContent,
               spilled: gauge.querySelector("[data-testid=vram-spilled]")?.textContent ?? null,
               swatches: document.querySelectorAll("[data-testid=loaded-model] [data-testid=vram-swatch]").length,
             };`,
          ),
        "jauge de la VRAM",
      );
      assert.equal(gauge.usage, "2.0 GB of 24 GB used");
      assert.equal(gauge.meter, "2.0 GB of 24 GB used");
      assert.deepEqual(gauge.parts.map(([model]) => model).sort(), [
        "chatonlm-slowload:latest",
        "chatonlm-split:latest",
      ]);
      assert.ok(gauge.other, "autres usages montrés");
      assert.equal(gauge.free, "Free: 22 GB");
      assert.equal(
        gauge.spilled,
        "256 MB doesn't fit in VRAM: that part runs from system memory, more slowly.",
      );
      assert.equal(gauge.swatches, 2);
      await driver.screenshot(".tools/shots/models-vram.png");

      // Unload d'une ligne.
      await driver.click(
        await driver.find(
          "[data-testid=loaded-model][data-model='chatonlm-split:latest'] [data-testid=unload-model]",
        ),
      );
      await waitUntil(
        async () => !(await loadedRows(driver))["chatonlm-split:latest"],
        "chatonlm-split déchargé",
      );
      assert.deepEqual((await loads()).at(-1), {
        model: "chatonlm-split:latest",
        kind: "unload",
        keep_alive: 0,
      });

      // Load depuis la liste des installés : durée réglée envoyée.
      await driver.execute(
        `const select = document.querySelector("[data-testid=load-model-select]");
         select.value = arguments[0];
         select.dispatchEvent(new Event("change", { bubbles: true }));`,
        ["qwen3:0.6b"],
      );
      await driver.click(await driver.find("[data-testid=load-model-button]"));
      await waitUntil(
        async () => (await loadedRows(driver))["qwen3:0.6b"],
        "qwen3:0.6b chargé par Load",
      );
      assert.equal((await lastLoad("qwen3:0.6b")).keep_alive, 900);
      assert.match(
        (await loadedRows(driver))["qwen3:0.6b"],
        /Unloads in 1[45] minutes/,
      );

      // « Always » : gardé chargé.
      await setKeepAlive(driver, "Always", -1);
      await driver.execute(
        `const select = document.querySelector("[data-testid=load-model-select]");
         select.value = arguments[0];
         select.dispatchEvent(new Event("change", { bubbles: true }));`,
        ["qwen3.5:0.8b"],
      );
      await driver.click(await driver.find("[data-testid=load-model-button]"));
      await waitUntil(
        async () =>
          /Stays loaded/.test((await loadedRows(driver))["qwen3.5:0.8b"] ?? ""),
        "qwen3.5:0.8b gardé chargé",
      );
      assert.equal((await lastLoad("qwen3.5:0.8b")).keep_alive, -1);
      await driver.screenshot(".tools/shots/models-tab.png");

      // Pré-prompt : tapé, enregistré en quittant le champ (vraie touche Tab).
      const PROMPT = "Always answer like a cat.";
      await driver.type(
        await driver.waitFor("[data-testid=system-prompt]"),
        PROMPT,
      );
      await pressKeys("tab");
      await waitUntil(
        async () =>
          (await driver.invoke("get_settings")).systemPrompt === PROMPT,
        "pré-prompt enregistré",
      );

      // Le chat suivant porte la durée réglée et le pré-prompt.
      await toChat(driver);
      await useModel(driver, "chatonlm-fast");
      await send(driver, "Keep me");
      await waitIdle(driver);
      const sent = (await requestsOf("chatonlm-fast")).at(-1);
      assert.equal(sent.keep_alive, -1);
      assert.deepEqual(sent.messages[0], { role: "system", content: PROMPT });
      assert.equal(
        sent.messages.filter((message) => message.role === "system").length,
        1,
      );
      assert.equal((await lastLoad("chatonlm-fast")).keep_alive, -1);
      // Ni affiché ni enregistré dans la conversation.
      assert.equal(
        await driver.execute(
          "return document.body.textContent.includes(arguments[0])",
          [PROMPT],
        ),
        false,
      );

      // Unload all.
      await openModelsTab(driver);
      await driver.click(await driver.waitFor("[data-testid=unload-all]"));
      await waitUntil(
        async () => (await cardText(driver)).includes("No models are loaded."),
        "plus aucun modèle chargé",
      );
      const ps = await (await fetch(new URL("/api/ps", fake.url))).json();
      assert.deepEqual(ps.models, []);

      // --- Préchargement du modèle choisi.
      await toChat(driver);
      await useModel(driver, "qwen3:0.6b");
      await openModelsTab(driver);
      const switchCss = "[data-testid=models-settings] [role=switch]";
      assert.equal(
        await driver.execute(
          "return document.querySelector(arguments[0]).getAttribute('aria-label')",
          [switchCss],
        ),
        "Preload the chosen model",
      );
      const count = (await loads()).length;
      await driver.click(await driver.find(switchCss));
      await waitUntil(
        async () => (await driver.invoke("get_settings")).preloadModel,
        "préchargement activé",
      );
      await waitUntil(
        async () =>
          (await loads())
            .slice(count)
            .some(
              (entry) => entry.model === "qwen3:0.6b" && entry.kind === "load",
            ),
        "modèle choisi préchargé",
      );
      // Autre modèle choisi : chargé au démarrage de la page.
      await toChat(driver);
      const count2 = (await loads()).length;
      await useModel(driver, "qwen3.5:0.8b");
      await waitUntil(
        async () =>
          (await loads())
            .slice(count2)
            .some(
              (entry) =>
                entry.model === "qwen3.5:0.8b" && entry.kind === "load",
            ),
        "nouveau modèle préchargé",
      );
      // Coupé : plus de préchargement.
      await driver.invoke("update_settings", {
        patch: { preloadModel: false },
      });
      const count3 = (await loads()).length;
      await useModel(driver, "qwen3:0.6b");
      await delay(2000);
      assert.deepEqual((await loads()).slice(count3), []);

      await collectViolations(driver);
    },
  );
  assert.deepEqual(violations, [], "violations de la CSP");
  assert.deepEqual(consoleErrors, [], "erreurs dans la console");
  assert.deepEqual(consoleWarnings, [], "avertissements dans la console");
  console.log("OK : chargement des modèles");
} finally {
  await fake.stop();
}
