// ModelHub dans l'app réelle (build e2e : frontend embarqué, CSP de
// production), avec le faux Ollama et le faux Hugging Face (contrat 37) :
// - premier lancement, Hugging Face fermé (profil neuf) : Discover montre
//   « You're offline » sans aucune carte ; l'onglet Installed liste quand
//   même les modèles du serveur ;
// - accueil depuis le faux Hugging Face (deux listes : populaires et
//   tendances) : rubriques dans l'ordre, familles, pastille de la machine
//   « RTX 3090 · 24 GB VRAM · 32 GB RAM » ; tuile Qwen → vue de la famille
//   (4 modèles, « Search Hugging Face for “Qwen” ») → « All models » ;
//   « See all » de Popular ; puce Vision (aria-pressed, cartes de vision
//   seulement), Reset filters ; tout cela sans nouvelle requête à Hugging
//   Face (cache du moteur) ;
// - recherche : « / » (vraie touche) donne le focus au champ, frappe au vrai
//   clavier, recherche lancée seulement après le délai (q mis dans l'adresse
//   au moins 380 ms après la dernière frappe), badge Unverified (infobulle),
//   « Verified only » ; Échap (vraie touche) vide la recherche et rend
//   l'accueil ;
// - fiche de chatonlm/Tiny-Chat-GGUF : version conseillée (Q8_0) cochée,
//   README assaini (texte visible, <script> montré en texte et jamais
//   exécuté, lien relatif résolu vers Hugging Face, badge retiré), version
//   à étiquette ambiguë : nom Ollama avec le fichier complet ;
// - dépôt mixte (unsloth/Qwen3-Coder-30B-A3B-Instruct-GGUF) : version
//   découpée grisée, désactivée, avec sa raison ; dépôt restreint
//   (google/gemma-2-2b-it-GGUF) : texte Restricted, aucun bouton Download ;
// - fiche de chatonlm/Tiny-Vision-GGUF : Download → progression → Installed →
//   Use in chat : nouvelle conversation, modèle enregistré, sélecteur avec
//   titre et étiquette ; onglet Downloads (fini, Dismiss, état vide) ; onglet
//   Installed : suppression confirmée par la boîte, vue par le faux Ollama ;
// - téléchargement lent (faux Ollama servant aussi de registre Hugging
//   Face) : « Downloading 1 · n% » dans la barre latérale (n croissant,
//   nombre et pourcentage non tronqués dans sa largeur de 192 px), onglet
//   Downloads (nombre actif, progression, Cancel), fin ; téléchargement bloqué
//   arrêté par Cancel (déconnexion vue par le faux Ollama) ; manifestes
//   refusés (étiquette absente, version découpée, dépôt inconnu) : messages
//   lisibles, sans pull ;
// - pannes, sur des recherches pas encore en cache : 503 → « Hugging Face
//   isn't responding » et incident (page d'état interrogée) ; 429 →
//   « Hugging Face is limiting requests. Try again in 42 s. », décompte,
//   Retry désactivé, plus aucune requête avant l'échéance ;
// - captures claires et sombres de l'accueil, d'une fiche, de Downloads et
//   d'Installed (.tools/shots/hub/), et des autres états ;
// - aucune violation de la CSP ni erreur ou avertissement dans la console.
import assert from "node:assert/strict";
import { mkdirSync } from "node:fs";
import { setTimeout as delay } from "node:timers/promises";
import { withApp } from "./lib/app.mjs";
import { startFake, startFakeHf } from "./lib/fake.mjs";
import { chordsOf, pressKeys } from "./lib/keyboard.mjs";

const binary = process.argv[2];
const SHOTS = ".tools/shots/hub";
mkdirSync(SHOTS, { recursive: true });

const fake = await startFake("ollama");
const hf = await startFakeHf("hf");
const hfDown = await startFakeHf("hf-status:503");
const hfLimited = await startFakeHf("hf-ratelimit");

/** Environnement : faux Ollama, et ce Hugging Face (fermé par défaut). */
const envWith = (url) =>
  url
    ? {
        OLLAMA_HOST: fake.url,
        CHATONLM_HF_URL: url,
        CHATONLM_HF_STATUS_URL: url,
      }
    : { OLLAMA_HOST: fake.url };

const SECTIONS = [
  "Best for your machine",
  "Trending",
  "Families",
  "Popular",
  "For coding",
  "With vision",
  "Reasoning",
  "Small and fast",
];
const UNVERIFIED_HINT =
  "This publisher isn't on ChatonLM's list of trusted publishers. Check the model card before using it.";
const RESTRICTED =
  "This model needs a Hugging Face account. ChatonLM can't download it.";
const VISION_MODEL = "hf.co/chatonlm/Tiny-Vision-GGUF:Q4_K_M";

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

const exists = (driver, css) =>
  driver.execute("return document.querySelector(arguments[0]) !== null", [css]);

const textOf = (driver, css) =>
  driver.execute(
    "return document.querySelector(arguments[0])?.textContent.trim() ?? null",
    [css],
  );

const textsOf = (driver, css) =>
  driver.execute(
    "return [...document.querySelectorAll(arguments[0])].map((e) => e.textContent.trim())",
    [css],
  );

/** Clic (JS) sur l'élément `css` dont le texte est exactement `text`. */
async function clickText(driver, css, text) {
  const found = await driver.execute(
    `const found = [...document.querySelectorAll(arguments[0])]
       .find((e) => e.textContent.trim() === arguments[1]);
     found?.click();
     return found !== undefined;`,
    [css, text],
  );
  assert.ok(found, `${css} « ${text} » introuvable`);
}

/** Attend `css` dont le texte est exactement `text`. */
const waitText = (driver, css, text, timeoutMs) =>
  waitUntil(
    async () => (await textsOf(driver, css)).includes(text),
    `${css} « ${text} »`,
    timeoutMs,
  );

const where = (driver) =>
  driver.execute(
    "return decodeURIComponent(location.pathname + location.search)",
  );

/** Change de page comme un lien du routeur (pushState que TanStack suit). */
const navigate = (driver, path) =>
  driver.execute(
    `const index = (history.state?.__TSR_index ?? 0) + 1;
     const key = Math.random().toString(36).slice(2);
     history.pushState({ key, __TSR_key: key, __TSR_index: index }, "", arguments[0]);`,
    [path],
  );

/** Entrée de la barre latérale (Chat, ModelHub, Settings), ouverte au besoin. */
async function sidebar(driver, label) {
  if (await exists(driver, "button[aria-label='Show sidebar']")) {
    await driver.click(await driver.find("button[aria-label='Show sidebar']"));
  }
  await waitUntil(
    () =>
      driver.execute(
        `const link = [...document.querySelectorAll("a")]
           .find((a) => a.textContent.trim() === arguments[0] && a.querySelector("svg"));
         link?.click();
         return link !== undefined;`,
        [label],
      ),
    `entrée ${label} de la barre latérale`,
  );
}

/** Onglet du ModelHub (Discover, Installed, Downloads). */
async function tab(driver, label) {
  await driver.waitFor("nav[aria-label=ModelHub]");
  const found = await driver.execute(
    `const link = [...document.querySelectorAll("nav[aria-label=ModelHub] a")]
       .find((a) => a.textContent.trim().startsWith(arguments[0]));
     link?.click();
     return link !== undefined;`,
    [label],
  );
  assert.ok(found, `onglet ${label}`);
  await waitUntil(
    async () =>
      (
        await textOf(driver, "nav[aria-label=ModelHub] a[aria-current=page]")
      )?.startsWith(label),
    `onglet ${label} courant`,
  );
}

/** Cartes affichées (« auteur/dépôt »), une fois le chargement fini. */
async function cards(driver) {
  await waitUntil(
    async () =>
      !(await exists(
        driver,
        "[data-testid=hub-scroll] [aria-busy=true], [data-testid=hub-scroll] [aria-label='Loading models']",
      )),
    "chargement de la grille",
  );
  return driver.execute(
    `return [...document.querySelectorAll("[data-testid=model-card]")].map((a) =>
       decodeURIComponent(a.getAttribute("href")).replace(/^.*\\/hub\\/model\\//, ""))`,
  );
}

/** Titres de la page Découvrir, dans l'ordre. */
const headings = (driver) => textsOf(driver, "[data-testid=hub-scroll] h3");

/** Attend l'accueil complet (rubriques et familles). */
async function waitHome(driver) {
  let seen;
  await waitUntil(
    async () => {
      seen = await headings(driver);
      return JSON.stringify(seen) === JSON.stringify(SECTIONS);
    },
    () => `rubriques de l'accueil : ${JSON.stringify(seen)}`,
    20000,
  );
}

/** Attend la fiche du dépôt `id` (versions affichées). */
async function waitRepo(driver, id) {
  const name = id.split("/")[1].replace(/-GGUF$/i, "");
  await waitText(driver, "#model-title", name, 20000);
}

/** Versions de la fiche : libellé, cochée, désactivée, texte, classe. */
const versions = (driver) =>
  driver.execute(
    `return [...document.querySelectorAll("[data-testid=version]")].map((label) => {
       const input = label.querySelector("input[type=radio]");
       return {
         label: input.value,
         checked: input.checked,
         disabled: input.disabled,
         text: label.textContent,
         className: label.className,
       };
     })`,
  );

/** Bouton dont le texte commence par `start`, dans `scope`. */
const buttonStarting = (driver, scope, start) =>
  driver.execute(
    `return [...document.querySelectorAll(arguments[0] + " button")]
       .find((b) => b.textContent.trim().startsWith(arguments[1]))?.textContent.trim() ?? null`,
    [scope, start],
  );

/** Clic (JS) sur la version `label` de la fiche. */
const chooseVersion = (driver, label) =>
  driver.execute(
    `const input = [...document.querySelectorAll("[data-testid=version] input")]
       .find((i) => i.value === arguments[0]);
     input.click();`,
    [label],
  );

const isDark = (driver) =>
  driver.execute("return document.documentElement.classList.contains('dark')");

/** Thème « Light » ou « Dark » par la page Settings. */
async function setTheme(driver, label) {
  await sidebar(driver, "Settings");
  await driver.waitFor("[data-testid=settings]");
  await clickText(driver, "label", label);
  await waitUntil(
    async () => (await isDark(driver)) === (label === "Dark"),
    `thème ${label}`,
  );
}

async function shoot(driver, name) {
  await delay(600); // transitions (survol, opacité, thème)
  await driver.screenshot(`${SHOTS}/${name}.png`);
}

/**
 * Note dans la page chaque état des barres de progression (nom, valeur,
 * texte), pour relever une progression trop rapide pour un sondage.
 */
const WATCH_PROGRESS = `
  window.__progress?.disconnect();
  window.__seen = [];
  const note = () => {
    for (const bar of document.querySelectorAll("[role=progressbar]")) {
      const entry = [bar.getAttribute("aria-label"), bar.getAttribute("aria-valuenow"),
        bar.getAttribute("aria-valuetext")].join("|");
      if (!window.__seen.includes(entry)) window.__seen.push(entry);
    }
  };
  window.__progress = new MutationObserver(note);
  window.__progress.observe(document.body,
    { subtree: true, childList: true, attributes: true, characterData: true });
  note();`;

// Violations de la CSP, erreurs et avertissements de la console : notés par
// la page, relevés à la fin de chaque lancement.
const violations = [];
const consoleErrors = [];
const consoleWarnings = [];
const WATCH_PAGE = `
  if (!window.__csp) {
    window.__csp = [];
    window.__errors = [];
    window.__warnings = [];
    window.__alerts = [];
    window.alert = (...args) => window.__alerts.push(args.map(String).join(" "));
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
    `return { csp: window.__csp ?? [], errors: window.__errors ?? [],
       warnings: window.__warnings ?? [], alerts: window.__alerts ?? [] };`,
  );
  assert.deepEqual(found.alerts, [], "alert() appelé par la page");
  violations.push(...found.csp);
  consoleErrors.push(...found.errors);
  consoleWarnings.push(...found.warnings);
}

/** Lance l'app, surveille la page, relève tout à la fin, capture si échec. */
async function launch(name, env, fn) {
  await withApp(binary, { name, env }, async (driver) => {
    try {
      await driver.waitFor("[data-testid=new-chat]", { timeoutMs: 20000 });
      await driver.execute(WATCH_PAGE);
      await fn(driver);
      await collectViolations(driver);
    } catch (error) {
      await driver.screenshot(`${SHOTS}/failure-${name}.png`).catch(() => {});
      throw error;
    }
  });
}

try {
  // 1. Profil neuf, Hugging Face fermé : hors ligne, Installed marche.
  await launch("hub-offline", envWith(null), async (driver) => {
    await sidebar(driver, "ModelHub");
    await waitText(
      driver,
      "[data-testid=hub-empty] h3",
      "You're offline",
      20000,
    );
    assert.equal(
      await textOf(driver, "[data-testid=hub-empty] p"),
      "Connect to the internet to browse models on Hugging Face. Your installed models are still available.",
    );
    assert.ok(await buttonStarting(driver, "[data-testid=hub-empty]", "Retry"));
    assert.equal(await exists(driver, "[data-testid=model-card]"), false);
    await shoot(driver, "offline");

    await tab(driver, "Installed");
    await driver.waitFor("[data-testid=installed-model]");
    assert.deepEqual(
      await driver.execute(
        `return [...document.querySelectorAll("[data-testid=installed-model] [title]")]
           .filter((e) => e.matches("div")).map((e) => e.title)`,
      ),
      ["qwen3:0.6b", "qwen3.5:0.8b"],
    );
    assert.match(
      await textOf(driver, "[data-testid=hub-scroll] h2"),
      /^2 models · \d+(\.\d)? GB$/,
    );
  });

  // 2. Faux Hugging Face : accueil, familles, filtres, recherche, fiches,
  //    téléchargement, Downloads, Installed, captures.
  await launch("hub", envWith(hf.url), async (driver) => {
    // Thème clair d'abord (celui du système de la session est sombre).
    await setTheme(driver, "Light");
    await sidebar(driver, "ModelHub");
    await waitHome(driver);
    assert.equal(
      await textOf(driver, "[data-testid=machine]"),
      "RTX 3090 · 24 GB VRAM · 32 GB RAM",
    );
    assert.match(
      await driver.execute(
        "return document.querySelector('[data-testid=machine]').title",
      ),
      /^Estimates for a 32K context\.\nFits your GPU/,
    );
    assert.equal(
      await textOf(driver, "[data-testid=hub-scroll] h2"),
      "Discover models",
    );
    // Les rubriques de l'accueil : 6 cartes au plus chacune.
    const home = await cards(driver);
    assert.ok(home.includes("unsloth/Qwen3.5-9B-GGUF"), home.join(", "));
    assert.ok(home.includes("unsloth/Qwen3-Coder-30B-A3B-Instruct-GGUF"));
    // Familles : nom et nombre de modèles.
    const families = await driver.execute(
      `const section = [...document.querySelectorAll("section")]
         .find((s) => s.querySelector("h3")?.textContent === "Families");
       return [...section.querySelectorAll("li a")].map((a) =>
         [...a.querySelectorAll("span.relative")].map((s) => s.textContent).join(" | "))`,
    );
    assert.equal(families[0], "Qwen | 4 models");
    assert.ok(families.includes("Gemma | 1 model"), families.join(", "));
    // Deux listes lues (populaires, tendances), une fois chacune.
    const listRequests = (await hf.requests()).map((r) => r.path);
    assert.equal(listRequests.length, 2, listRequests.join("\n"));
    assert.ok(
      listRequests.some((p) => p.includes("sort=downloads&limit=500")) &&
        listRequests.some((p) => p.includes("sort=trendingScore&limit=200")),
      listRequests.join("\n"),
    );
    await shoot(driver, "discover-light");

    // Famille Qwen, puis retour.
    await driver.execute(
      `[...document.querySelectorAll("section li a")]
         .find((a) => a.querySelector("span.relative")?.textContent === "Qwen").click()`,
    );
    await driver.waitFor("section[aria-label=Qwen]");
    assert.equal(await where(driver), "/hub?family=qwen");
    const qwen = await cards(driver);
    assert.equal(qwen.length, 4, qwen.join(", "));
    assert.ok(
      qwen.every((id) => /qwen/i.test(id)),
      qwen.join(", "),
    );
    assert.ok(
      (await textsOf(driver, "section[aria-label=Qwen] > div span")).includes(
        "4 models",
      ),
    );
    assert.ok(
      (await textsOf(driver, "section[aria-label=Qwen] a")).includes(
        "Search Hugging Face for “Qwen”",
      ),
    );
    await clickText(driver, "section[aria-label=Qwen] a", "All models");
    await waitHome(driver);
    assert.equal(await where(driver), "/hub");

    // « See all » de Popular, puis filtre Vision.
    await driver.click(await driver.find("a[aria-label='See all Popular']"));
    await driver.waitFor("section[aria-label=Popular]");
    const popular = await cards(driver);
    assert.ok(popular.length > 6, popular.join(", "));
    assert.equal(popular[0], "unsloth/Qwen3-Coder-30B-A3B-Instruct-GGUF");
    await clickText(
      driver,
      "[role=toolbar][aria-label=Filters] button",
      "Vision",
    );
    await waitUntil(
      async () => (await where(driver)).includes("uses=vision"),
      "filtre Vision dans l'adresse",
    );
    assert.equal(
      await driver.execute(
        `return [...document.querySelectorAll("[role=toolbar] button")]
           .find((b) => b.textContent.trim() === "Vision").getAttribute("aria-pressed")`,
      ),
      "true",
    );
    let vision;
    await waitUntil(
      async () => {
        vision = await cards(driver);
        return vision.length > 0 && vision.length < popular.length;
      },
      () => `cartes filtrées : ${vision}`,
    );
    await shoot(driver, "filter");
    assert.ok(
      await driver.execute(
        `return [...document.querySelectorAll("[data-testid=model-card]")]
           .every((card) => card.querySelector("[role=img][aria-label=Vision]"))`,
      ),
      `une carte sans vision : ${vision}`,
    );
    assert.ok(vision.includes("unsloth/Qwen3.5-9B-GGUF"), vision.join(", "));
    await clickText(driver, "section[aria-label=Popular] a", "All models");
    await waitUntil(
      async () => (await where(driver)) === "/hub?uses=vision",
      "accueil filtré",
    );
    await clickText(driver, "button", "Reset filters");
    await waitHome(driver);
    assert.equal(await where(driver), "/hub");
    assert.equal(
      (await hf.requests()).length,
      2,
      "requête à Hugging Face pendant la navigation (cache du moteur)",
    );

    // Recherche : « / », frappe au vrai clavier, délai, Unverified.
    await driver.execute("document.activeElement?.blur()");
    await pressKeys("slash");
    assert.equal(
      await driver.execute(
        "return document.activeElement?.getAttribute('aria-label')",
      ),
      "Search Hugging Face models",
    );
    // Heures des frappes et des passages d'une recherche dans l'adresse
    // (replaceState du routeur).
    await driver.execute(
      `window.__inputs = [];
       window.__searches = [];
       document.addEventListener("input", () => {
         window.__inputs.push(performance.now());
       }, true);
       const replace = history.replaceState;
       history.replaceState = function (...args) {
         if (String(args[2]).includes("q=")) window.__searches.push(performance.now());
         return replace.apply(history, args);
       };`,
    );
    await pressKeys(chordsOf("tiny"));
    assert.equal(
      await driver.execute("return document.activeElement.value"),
      "tiny",
    );
    await waitText(driver, "[data-testid=hub-scroll] h3", "Results for “tiny”");
    assert.equal(await where(driver), "/hub?q=tiny");
    // Chaque recherche part au moins ~400 ms après la frappe qui la précède.
    const gaps = await driver.execute(
      `return window.__searches.map((at) =>
         at - Math.max(...window.__inputs.filter((input) => input <= at)))`,
    );
    assert.ok(
      gaps.length > 0 && gaps.every((gap) => gap >= 380),
      `recherche partie trop tôt après la frappe : ${gaps.join(", ")} ms`,
    );
    const found = await cards(driver);
    assert.deepEqual([...found].sort(), [
      "chatonlm/Tiny-Chat-GGUF",
      "chatonlm/Tiny-Vision-GGUF",
      "ibm-granite/granite-4.2-tiny-GGUF",
    ]);
    const trust = await driver.execute(
      `return Object.fromEntries([...document.querySelectorAll("[data-testid=model-card]")]
         .map((card) => {
           const badge = [...card.querySelectorAll("span")]
             .find((s) => s.textContent === "Unverified");
           return [card.getAttribute("href").split("/").at(-1), badge ? badge.title : "verified"];
         }))`,
    );
    assert.deepEqual(trust, {
      "Tiny-Chat-GGUF": UNVERIFIED_HINT,
      "Tiny-Vision-GGUF": UNVERIFIED_HINT,
      "granite-4.2-tiny-GGUF": "verified",
    });
    assert.equal(
      (await hf.requests()).filter((r) => r.path.includes("search=tiny"))
        .length,
      1,
    );
    await shoot(driver, "search");
    await clickText(driver, "[role=toolbar] button", "Verified only");
    await waitUntil(
      async () =>
        JSON.stringify(await cards(driver)) ===
        JSON.stringify(["ibm-granite/granite-4.2-tiny-GGUF"]),
      "Verified only : granite seul",
    );
    await clickText(driver, "[role=toolbar] button", "Verified only");
    await waitUntil(
      async () => (await cards(driver)).length === 3,
      "Verified only retiré",
    );

    // Fiche de Tiny-Chat : version conseillée, README assaini.
    await driver.execute(
      `[...document.querySelectorAll("[data-testid=model-card]")]
         .find((a) => a.getAttribute("href").endsWith("/Tiny-Chat-GGUF")).click()`,
    );
    await waitRepo(driver, "chatonlm/Tiny-Chat-GGUF");
    await driver.waitFor("[data-testid=readme]", { timeoutMs: 20000 });
    let list = await versions(driver);
    assert.deepEqual(
      list.map((v) => [v.label, v.checked, v.disabled]),
      [
        ["Q8_0", true, false],
        ["Q4_K_L", false, false],
        ["Q4_K_M", false, false],
      ],
    );
    assert.match(list[0].text, /^Q8_0Recommended/);
    const readme = await driver.execute(
      `const readme = document.querySelector("[data-testid=readme]");
       return {
         text: readme.textContent,
         scripts: readme.querySelectorAll("script, img, iframe").length,
         links: [...readme.querySelectorAll("a")].map((a) => [a.textContent, a.getAttribute("href")]),
       };`,
    );
    assert.match(readme.text, /A tiny model for ChatonLM tests\./);
    assert.match(readme.text, /<script>alert\(1\)<\/script>/);
    assert.doesNotMatch(readme.text, /Image:/, "badge shields.io gardé");
    assert.equal(readme.scripts, 0);
    assert.deepEqual(readme.links, [
      [
        "the guide",
        "https://huggingface.co/chatonlm/Tiny-Chat-GGUF/blob/main/docs/guide.md",
      ],
      ["a link", "https://example.com/"],
    ]);
    await shoot(driver, "readme");
    // Étiquette ambiguë (Q4_K_L) : le fichier complet.
    await chooseVersion(driver, "Q4_K_L");
    await waitUntil(
      async () =>
        (await textOf(driver, "[data-testid=ollama-name]")) ===
        "hf.co/chatonlm/Tiny-Chat-GGUF:Tiny-Chat-Q4_K_L.gguf",
      "nom Ollama de la version Q4_K_L",
    );
    assert.equal(
      await buttonStarting(driver, "aside", "Download"),
      "Download · 429 MB",
    );

    // Retour aux résultats, puis Échap : accueil.
    await clickText(driver, "button", "Back");
    await waitText(driver, "[data-testid=hub-scroll] h3", "Results for “tiny”");
    await driver.execute("document.activeElement?.blur()");
    await pressKeys("slash");
    await pressKeys("escape");
    await waitHome(driver);
    assert.equal(await where(driver), "/hub");
    assert.equal(
      await driver.execute(
        "return document.querySelector('[aria-label=\"Search Hugging Face models\"]').value",
      ),
      "",
    );

    // Dépôt mixte : version découpée grisée.
    await navigate(
      driver,
      "/hub/model/unsloth/Qwen3-Coder-30B-A3B-Instruct-GGUF",
    );
    await waitRepo(driver, "unsloth/Qwen3-Coder-30B-A3B-Instruct-GGUF");
    list = await versions(driver);
    assert.equal(list[0].label, "BF16");
    assert.equal(list[0].disabled, true);
    assert.equal(list[0].checked, false);
    assert.match(
      list[0].text,
      /Split into several files — not supported by Ollama yet/,
    );
    assert.match(list[0].className, /opacity-50/);
    const checked = list.filter((v) => v.checked);
    assert.deepEqual(
      checked.map((v) => v.label),
      ["Q4_K_M"],
    );
    assert.match(checked[0].text, /Recommended/);
    assert.ok(list.slice(1).every((v) => !v.disabled));
    await shoot(driver, "model-light");

    // Dépôt restreint : pas de téléchargement.
    await navigate(driver, "/hub/model/google/gemma-2-2b-it-GGUF");
    await waitRepo(driver, "google/gemma-2-2b-it-GGUF");
    await waitUntil(
      async () =>
        (
          await textOf(driver, "aside[aria-label='Choose a version']")
        )?.includes(RESTRICTED),
      "texte Restricted",
    );
    assert.equal(await buttonStarting(driver, "aside", "Download"), null);
    assert.equal(await exists(driver, "[data-testid=version]"), false);
    assert.ok(
      (await textsOf(driver, "aside a")).includes("View on Hugging Face"),
    );
    await shoot(driver, "restricted");

    // Tiny-Vision : téléchargement, puis Use in chat.
    await navigate(driver, "/hub/model/chatonlm/Tiny-Vision-GGUF");
    await waitRepo(driver, "chatonlm/Tiny-Vision-GGUF");
    list = await versions(driver);
    assert.deepEqual(
      list.map((v) => [v.label, v.checked]),
      [["Q4_K_M", true]],
    );
    assert.match(list[0].text, /Recommended/);
    assert.ok(
      (await textsOf(driver, "ul[aria-label=Capabilities] li")).includes(
        "Vision",
      ),
    );
    assert.equal(
      await textOf(driver, "[data-testid=ollama-name]"),
      VISION_MODEL,
    );
    await driver.execute(WATCH_PROGRESS);
    const pullsBefore = (await fake.pulls()).length;
    await clickText(driver, "aside button", "Download · 572 MB");
    await waitUntil(
      async () =>
        (await textsOf(driver, "aside p")).includes("Installed") &&
        (await buttonStarting(driver, "aside", "Use in chat")) !== null,
      "Installed et Use in chat",
      20000,
    );
    const seen = await driver.execute("return window.__seen");
    const bars = seen.filter((entry) =>
      entry.startsWith("Downloading Q4_K_M|"),
    );
    assert.ok(bars.length > 0, `aucune progression vue : ${seen.join(" / ")}`);
    const pulled = (await fake.pulls()).slice(pullsBefore);
    assert.deepEqual(
      pulled.map((p) => p.model),
      [VISION_MODEL],
    );
    // Installé : la version porte la coche (installés relus).
    await driver.waitFor("[data-testid=version] [aria-label=Installed]");
    await clickText(driver, "aside button", "Use in chat");
    await driver.waitFor("[data-testid=new-chat]");
    await waitUntil(
      async () =>
        (await driver.invoke("get_settings")).selectedModel === VISION_MODEL,
      "modèle choisi",
    );
    let picker;
    await waitUntil(
      async () => {
        picker = await driver.execute(
          `const button = document.querySelector("[title='Select model']");
           return [...button.querySelectorAll("span")]
             .filter((s) => s.children.length === 0).map((s) => s.textContent)`,
        );
        return picker.join() === "Tiny-Vision,Q4_K_M";
      },
      () => `sélecteur : ${JSON.stringify(picker)}`,
    );
    assert.match(await where(driver), /^\/c\/new/);

    // Downloads : fini ; Installed : le modèle en tête.
    await sidebar(driver, "ModelHub");
    await waitHome(driver);
    await tab(driver, "Downloads");
    await driver.waitFor("[data-testid=download]");
    assert.deepEqual(
      await driver.execute(
        `return [...document.querySelectorAll("[data-testid=download]")]
           .map((row) => row.dataset.state)`,
      ),
      ["done"],
    );
    assert.match(
      await textOf(driver, "[data-testid=download]"),
      /Tiny-VisionQ4_K_MInstalled · .*Use in chatDismiss$/,
    );
    await shoot(driver, "downloads-light");
    await tab(driver, "Installed");
    await waitUntil(
      async () =>
        (await driver.execute(
          `return document.querySelector("[data-testid=installed-model] div[title]")?.title`,
        )) === VISION_MODEL,
      "Tiny-Vision en tête des installés",
    );
    assert.match(
      await textOf(driver, "[data-testid=hub-scroll] h2"),
      /^3 models · /,
    );
    await shoot(driver, "installed-light");

    // Mêmes pages en sombre.
    await setTheme(driver, "Dark");
    await sidebar(driver, "ModelHub");
    await waitHome(driver);
    await shoot(driver, "discover-dark");
    await navigate(
      driver,
      "/hub/model/unsloth/Qwen3-Coder-30B-A3B-Instruct-GGUF",
    );
    await waitRepo(driver, "unsloth/Qwen3-Coder-30B-A3B-Instruct-GGUF");
    await shoot(driver, "model-dark");
    await tab(driver, "Downloads");
    await driver.waitFor("[data-testid=download]");
    await shoot(driver, "downloads-dark");
    await tab(driver, "Installed");
    await driver.waitFor("[data-testid=installed-model]");
    await shoot(driver, "installed-dark");

    // Downloads : Dismiss, puis l'état vide.
    await tab(driver, "Downloads");
    await clickText(driver, "[data-testid=download] button", "Dismiss");
    await waitText(driver, "[data-testid=hub-empty] h3", "No downloads yet.");
    assert.equal(
      await textOf(driver, "[data-testid=hub-empty] p"),
      "Models you download from ModelHub appear here.",
    );

    // Installed : suppression confirmée par la boîte.
    await tab(driver, "Installed");
    await driver.waitFor("[data-testid=installed-model]");
    await driver.click(
      await driver.find("button[aria-label='Delete Tiny-Vision:Q4_K_M']"),
    );
    await driver.waitFor("[role=alertdialog]");
    assert.equal(
      await textOf(driver, "[role=alertdialog] h2"),
      "Delete Tiny-Vision:Q4_K_M?",
    );
    assert.equal(
      await textOf(driver, "[role=alertdialog] p"),
      "This removes the model from Ollama. You can download it again later.",
    );
    assert.deepEqual(
      await fake.deletes(),
      [],
      "supprimé avant la confirmation",
    );
    await clickText(driver, "[role=alertdialog] button", "Delete");
    await waitUntil(
      async () =>
        (
          await driver.execute(
            `return [...document.querySelectorAll("[data-testid=installed-model] div[title]")]
             .map((e) => e.title)`,
          )
        ).join() === "qwen3:0.6b,qwen3.5:0.8b",
      "Tiny-Vision retiré des installés",
    );
    assert.deepEqual(await fake.deletes(), [VISION_MODEL]);
    assert.equal(await exists(driver, "[role=alertdialog]"), false);

    // Thème clair pour les lancements suivants.
    await setTheme(driver, "Light");
  });

  // 3. Faux Ollama aussi registre de Hugging Face (espace chatonlm) :
  //    téléchargement lent, bloqué puis arrêté, manifestes refusés.
  await launch("hub-downloads", envWith(fake.url), async (driver) => {
    const download = (model) =>
      driver.invoke("hub_download", {
        request: {
          model,
          repo: model.slice("hf.co/".length).split(":")[0],
          label: "Q4_K_M",
        },
      });
    const indicator = () =>
      textOf(driver, "[data-testid=download-indicator] span");
    await sidebar(driver, "Chat");
    await driver.waitFor("[data-testid=new-chat]");

    // Lent (~5 s) : barre latérale, puis onglet Downloads.
    const slow = "hf.co/chatonlm/Tiny-Slow-GGUF:Q4_K_M";
    await download(slow);
    const percents = [];
    await waitUntil(
      async () => {
        const text = await indicator();
        const match = text?.match(/^Downloading 1 · (\d+)%$/);
        if (match && Number(match[1]) !== percents.at(-1)) {
          percents.push(Number(match[1]));
        }
        return percents.length >= 3 || text === null;
      },
      () => `indicateur : ${percents.join(", ")}`,
      15000,
    );
    assert.ok(percents.length >= 3, `indicateur : ${percents.join(", ")}`);
    assert.deepEqual(
      percents,
      [...percents].sort((a, b) => a - b),
      "pourcentage qui recule",
    );
    // Barre latérale étroite : nombre et pourcentage lisibles (pas tronqués).
    const parts = await driver.execute(
      `return [...document.querySelectorAll("[data-testid=download-indicator] span")]
         .filter((s) => s.children.length === 0 && /\\d/.test(s.textContent))
         .map((s) => ({ text: s.textContent, scroll: s.scrollWidth, client: s.clientWidth }));`,
    );
    assert.ok(
      parts.some((part) => /\d+%$/.test(part.text)) &&
        parts.every((part) => part.scroll <= part.client),
      `indicateur tronqué : ${JSON.stringify(parts)}`,
    );
    await shoot(driver, "sidebar-indicator");
    await driver.click(await driver.find("[data-testid=download-indicator]"));
    await driver.waitFor("[data-testid=download][data-state=downloading]");
    assert.equal(
      await textOf(driver, "[data-testid=active-downloads]"),
      "1",
      "nombre actif sur l'onglet",
    );
    assert.ok(
      await buttonStarting(driver, "[data-testid=download]", "Cancel"),
      "Cancel pendant le téléchargement",
    );
    assert.match(
      await driver.execute(
        `return document.querySelector("[data-testid=download] [role=progressbar]")
           .getAttribute("aria-valuetext")`,
      ),
      /^\d+(\.\d)? (B|KB|MB|GB) of 1\.1 GB/,
    );
    await shoot(driver, "downloads-active");
    await driver.waitFor("[data-testid=download][data-state=done]", {
      timeoutMs: 20000,
    });
    await waitUntil(
      async () =>
        !(await exists(driver, "[data-testid=download-indicator]")) &&
        !(await exists(driver, "[data-testid=active-downloads]")),
      "indicateur retiré à la fin",
    );

    // Bloqué : Cancel ferme la requête.
    const disconnects = await fake.disconnects();
    await download("hf.co/chatonlm/Tiny-Stall-GGUF:Q4_K_M");
    await driver.waitFor("[data-testid=download][data-state=downloading]");
    await clickText(
      driver,
      "[data-testid=download][data-state=downloading] button",
      "Cancel",
    );
    await driver.waitFor("[data-testid=download][data-state=cancelled]");
    await waitUntil(
      async () => (await fake.disconnects()) === disconnects + 1,
      "téléchargement abandonné vu par le faux Ollama",
    );
    assert.deepEqual(
      await textsOf(
        driver,
        "[data-testid=download][data-state=cancelled] button",
      ),
      ["Try again", "Dismiss"],
    );

    // Pause : la requête est fermée, la ligne garde ce qui est reçu ;
    // Resume lance un nouveau pull (Ollama repart de ses morceaux).
    const stateOf = (repo) =>
      driver.execute(
        `const row = [...document.querySelectorAll("[data-testid=download]")]
           .find((r) => r.querySelector("[title]")?.title.includes(arguments[0]));
         return row?.getAttribute("data-state") ?? null;`,
        [repo],
      );
    const inRow = (repo, label) =>
      driver.execute(
        `const row = [...document.querySelectorAll("[data-testid=download]")]
           .find((r) => r.querySelector("[title]")?.title.includes(arguments[0]));
         const button = [...(row?.querySelectorAll("button") ?? [])]
           .find((b) => b.textContent.trim() === arguments[1]);
         button?.click();
         return button !== undefined;`,
        [repo, label],
      );
    const held = "hf.co/chatonlm/Held-Stall-GGUF:Q4_K_M";
    const closedBefore = await fake.disconnects();
    const pullsBefore = (await fake.pulls()).length;
    await download(held);
    await waitUntil(
      async () => (await stateOf("Held-Stall")) === "downloading",
      "téléchargement à mettre en pause",
    );
    assert.ok(await inRow("Held-Stall", "Pause"), "bouton Pause");
    await waitUntil(
      async () => (await stateOf("Held-Stall")) === "paused",
      "téléchargement en pause",
    );
    await waitUntil(
      async () => (await fake.disconnects()) === closedBefore + 1,
      "requête fermée par la pause",
    );
    assert.match(
      await driver.execute(
        `return document.querySelector("[data-testid=download][data-state=paused] [role=progressbar]")
           .getAttribute("aria-valuetext")`,
      ),
      /^Paused · \d+(\.\d)? (B|KB|MB|GB) of 1\.1 GB$/,
    );
    assert.deepEqual(
      await textsOf(driver, "[data-testid=download][data-state=paused] button"),
      ["Resume", "Cancel"],
    );
    assert.equal(
      await exists(driver, "[data-testid=active-downloads]"),
      false,
      "une pause n'est pas un téléchargement actif",
    );
    await shoot(driver, "downloads-paused");
    assert.ok(await inRow("Held-Stall", "Resume"), "bouton Resume");
    await waitUntil(
      async () => (await stateOf("Held-Stall")) === "downloading",
      "téléchargement repris",
    );
    assert.equal((await fake.pulls()).length, pullsBefore + 2, "nouveau pull");

    // Download first : le téléchargement en file démarre tout de suite, le
    // dernier en cours retourne en file (deux à la fois).
    await download("hf.co/chatonlm/Second-Stall-GGUF:Q4_K_M");
    await download("hf.co/chatonlm/Third-Stall-GGUF:Q4_K_M");
    await waitUntil(
      async () =>
        (await stateOf("Second-Stall")) === "downloading" &&
        (await stateOf("Third-Stall")) === "queued",
      "deux en cours, un en file",
    );
    assert.ok(await inRow("Third-Stall", "Download first"), "Download first");
    await waitUntil(
      async () =>
        (await stateOf("Third-Stall")) === "downloading" &&
        (await stateOf("Second-Stall")) === "queued" &&
        (await stateOf("Held-Stall")) === "downloading",
      "téléchargement passé en premier",
    );
    assert.equal(
      await driver.execute(
        `return document.querySelector("[data-testid=download]").querySelector("[title]").title`,
      ),
      "hf.co/chatonlm/Third-Stall-GGUF:Q4_K_M",
      "le téléchargement prioritaire est en tête de la liste",
    );
    for (const repo of ["Third-Stall", "Held-Stall", "Second-Stall"]) {
      assert.ok(await inRow(repo, "Cancel"), `Cancel de ${repo}`);
      await waitUntil(
        async () => (await stateOf(repo)) === "cancelled",
        `${repo} arrêté`,
      );
      assert.ok(await inRow(repo, "Dismiss"), `Dismiss de ${repo}`);
    }

    // Manifestes refusés par le registre : message lisible, aucun pull.
    const refused = {
      "hf.co/chatonlm/Tiny-Badtag-GGUF:Q4_K_M":
        "This version isn't available on Hugging Face. Choose another version.",
      "hf.co/chatonlm/Tiny-Sharded-GGUF:Q4_K_M":
        "This version is split into several files, which Ollama can't download from Hugging Face yet. Choose another version.",
      "hf.co/chatonlm/Tiny-Missing-GGUF:Q4_K_M":
        "This model doesn't exist on Hugging Face, is private, or needs an account to download.",
    };
    for (const model of Object.keys(refused)) await download(model);
    await waitUntil(
      async () =>
        (await driver.execute(
          `return document.querySelectorAll("[data-testid=download][data-state=failed]").length`,
        )) === 3,
      "trois échecs",
    );
    const failures = await driver.execute(
      `return Object.fromEntries([...document.querySelectorAll("[data-testid=download][data-state=failed]")]
         .map((row) => [row.querySelector("[title]").title, row.querySelector("[role=alert]").textContent]))`,
    );
    assert.deepEqual(failures, refused);
    const pulledNames = (await fake.pulls()).map((p) => p.model);
    for (const model of Object.keys(refused)) {
      assert.ok(!pulledNames.includes(model), `pull lancé pour ${model}`);
    }
    await shoot(driver, "downloads-states");
  });

  // 4. Hugging Face en panne (503, incident) sur une recherche nouvelle.
  await launch("hub-unavailable", envWith(hfDown.url), async (driver) => {
    await sidebar(driver, "ModelHub");
    await waitHome(driver); // accueil en cache, encore frais
    assert.equal(await exists(driver, "[data-testid=hub-banner]"), false);
    const search = await driver.find(
      "[aria-label='Search Hugging Face models']",
    );
    await driver.click(search);
    await driver.type(search, "mistral");
    await waitText(
      driver,
      "[data-testid=hub-empty] h3",
      "Hugging Face isn't responding",
      20000,
    );
    assert.equal(
      await textOf(driver, "[data-testid=hub-empty] p"),
      "Hugging Face reports an incident. Try again in a few minutes.",
    );
    assert.ok(await buttonStarting(driver, "[data-testid=hub-empty]", "Retry"));
    const paths = (await hfDown.requests()).map((r) => r.path);
    assert.ok(
      paths.some((p) => p.includes("search=mistral")),
      paths.join("\n"),
    );
    assert.ok(paths.includes("/index.json"), "page d'état non lue");
    await shoot(driver, "unavailable");
  });

  // 5. Limite de requêtes (429) : décompte, plus aucune requête.
  await launch("hub-ratelimit", envWith(hfLimited.url), async (driver) => {
    await sidebar(driver, "ModelHub");
    await waitHome(driver);
    const search = await driver.find(
      "[aria-label='Search Hugging Face models']",
    );
    await driver.click(search);
    await driver.type(search, "llama");
    await waitText(
      driver,
      "[data-testid=hub-empty] h3",
      "Too many requests",
      20000,
    );
    const wait = async () =>
      Number(
        (await textOf(driver, "[data-testid=hub-empty] p")).match(
          /^Hugging Face is limiting requests\. Try again in (\d+) s\.$/,
        )?.[1],
      );
    const first = await wait();
    assert.ok(first >= 38 && first <= 42, `décompte ${first}`);
    assert.equal(
      await driver.execute(
        `return [...document.querySelectorAll("[data-testid=hub-empty] button")]
           .find((b) => b.textContent.trim() === "Retry").disabled`,
      ),
      true,
    );
    await shoot(driver, "ratelimited");
    await waitUntil(async () => (await wait()) < first, "décompte", 3000);
    const count = (await hfLimited.requests()).length;
    await driver.click(await driver.find("button[aria-label='Clear search']"));
    await waitUntil(
      async () => (await where(driver)) === "/hub",
      "recherche vidée",
    );
    await driver.type(search, "gemma");
    await waitUntil(
      async () => (await where(driver)) === "/hub?q=gemma",
      "nouvelle recherche",
    );
    await waitText(driver, "[data-testid=hub-empty] h3", "Too many requests");
    assert.ok((await wait()) <= 42);
    assert.equal(
      (await hfLimited.requests()).length,
      count,
      "requête à Hugging Face avant l'échéance",
    );
  });

  // 6. Rubrique « Uncensored » : absente et jamais demandée par défaut ;
  //    montrée en dernier dès le réglage activé (Settings > Models), avec sa
  //    liste complète.
  await launch("hub-uncensored", envWith(hf.url), async (driver) => {
    const UNCENSORED = [
      "huihui-ai/Huihui-Qwen3.5-9B-abliterated-GGUF",
      "unsloth/Qwen3-4B-Uncensored-GGUF",
      "bartowski/huihui-ai_Huihui-Qwen3-14B-abliterated-v2-GGUF",
    ];
    const before = (await hf.requests()).length;
    const searched = async () =>
      (await hf.requests())
        .slice(before)
        .map((r) => r.path)
        .filter((p) => p.includes("search="));
    await sidebar(driver, "ModelHub");
    await waitHome(driver);
    assert.deepEqual(await searched(), []);

    await sidebar(driver, "Settings");
    await driver.waitFor("[data-testid=settings-tab-models]");
    await driver.click(await driver.find("[data-testid=settings-tab-models]"));
    const toggle = "button[role=switch][aria-label='Show uncensored models']";
    await driver.waitFor(toggle);
    assert.equal(
      await driver.execute(
        "return document.querySelector(arguments[0]).getAttribute('aria-checked')",
        [toggle],
      ),
      "false",
    );
    await driver.click(await driver.find(toggle));
    await waitUntil(
      () =>
        driver.execute(
          "return document.querySelector(arguments[0]).getAttribute('aria-checked') === 'true'",
          [toggle],
        ),
      "réglage activé",
    );

    await sidebar(driver, "ModelHub");
    let seen;
    await waitUntil(
      async () => {
        seen = await headings(driver);
        return (
          JSON.stringify(seen) === JSON.stringify([...SECTIONS, "Uncensored"])
        );
      },
      () => `rubriques : ${JSON.stringify(seen)}`,
      20000,
    );
    const home = await driver.execute(
      `const section = [...document.querySelectorAll("section")]
         .find((s) => s.querySelector("h3")?.textContent === "Uncensored");
       return [...section.querySelectorAll("[data-testid=model-card]")].map((a) =>
         decodeURIComponent(a.getAttribute("href")).replace(/^.*\\/hub\\/model\\//, ""))`,
    );
    assert.deepEqual(home, UNCENSORED);
    assert.deepEqual(
      (await searched()).map((p) => p.match(/search=(\w+)/)[1]).sort(),
      ["abliterated", "heretic", "uncensored"],
    );

    await driver.click(await driver.find("a[aria-label='See all Uncensored']"));
    await waitUntil(
      async () => (await where(driver)) === "/hub?section=uncensored",
      "liste complète",
    );
    assert.deepEqual(await cards(driver), UNCENSORED);
    await shoot(driver, "uncensored");
  });

  assert.deepEqual(violations, [], "violations de la CSP");
  assert.deepEqual(consoleErrors, [], "erreurs dans la console");
  assert.deepEqual(consoleWarnings, [], "avertissements dans la console");
} finally {
  await Promise.all([hf.stop(), hfDown.stop(), hfLimited.stop(), fake.stop()]);
}
