// Onglet Images du ModelHub dans l'app réelle (build e2e : frontend embarqué,
// CSP de production), avec le faux Ollama et le faux Hugging Face, familles
// d'essai à petites pièces (CHATONLM_IMAGE_FAMILIES ; contrat
// .tools/research/83-image-models-contract.md § 7) :
// - onglet Images après Voices ; dépôts populaires sans le dépôt non
//   censuré ; famille inconnue « Not supported », dépôt restreint
//   « Restricted » ;
// - recherche par l'adresse (q) : chatonlm/klein-test-GGUF ; sa fiche : fichier
//   conseillé coché, pièces annexes listées, total, Download → fichiers
//   servis par le faux Hugging Face à la révision de la fiche
//   (/resolve/<révision>/...) → « This file is installed. », fin prouvée
//   par la ligne data-state=done de l'onglet Downloads ;
// - « Your image models » : carte du modèle installée, Use → « In use » ; onglet
//   Downloads : section « Image models », ligne « Installed », Dismiss ;
//   suppression confirmée par la boîte, carte retirée ;
// - fiches refusées : famille inconnue et dépôt restreint (Download désactivé,
//   raison visible) ; dépôt non censuré atteint par son adresse : badge,
//   raison avec le lien vers les réglages ; réglage « Show uncensored
//   models » activé : le dépôt apparaît dans la recherche avec son badge ;
// - captures claires (.tools/shots/image-models/) ;
// - aucune violation de la CSP ni erreur ou avertissement dans la console.
//
// Suppose du faux Hugging Face (crates/fake-ollama, fixtures/hf/) : le
// fichier de familles d'essai `image-families.json`, qui reconnaît
// chatonlm/klein-test-GGUF (au moins un fichier conseillé et une pièce
// annexe) ; chatonlm/uncensored-image-GGUF (non censuré, famille reconnue),
// chatonlm/unknown-diffusion-GGUF (famille inconnue) et
// chatonlm/gated-image-GGUF (restreint) dans la liste text-to-image ; les
// fichiers servis à /<dépôt>/resolve/<révision>/<chemin>.
import assert from "node:assert/strict";
import { mkdirSync } from "node:fs";
import { setTimeout as delay } from "node:timers/promises";
import { ROOT, withApp } from "./lib/app.mjs";
import { startFake, startFakeHf } from "./lib/fake.mjs";

const binary = process.argv[2];
const SHOTS = ".tools/shots/image-models";
mkdirSync(SHOTS, { recursive: true });

const KLEIN = "chatonlm/klein-test-GGUF";
const UNCENSORED = "chatonlm/uncensored-image-GGUF";
const UNKNOWN = "chatonlm/unknown-diffusion-GGUF";
const GATED = "chatonlm/gated-image-GGUF";
const FAMILIES = `${ROOT}src-tauri/crates/fake-ollama/fixtures/hf/image-families.json`;

const fake = await startFake("ollama");
const hf = await startFakeHf("hf");

const env = {
  OLLAMA_HOST: fake.url,
  CHATONLM_HF_URL: hf.url,
  CHATONLM_HF_STATUS_URL: hf.url,
  CHATONLM_IMAGE_FAMILIES: FAMILIES,
};

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

/** Onglet du ModelHub. */
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

/** Dépôts listés par la recherche (« auteur/nom »), chargement fini. */
async function repoCards(driver) {
  await waitUntil(
    async () =>
      !(await exists(
        driver,
        "[data-testid=hub-scroll] [aria-busy=true], [aria-label='Searching Hugging Face']",
      )),
    "chargement des dépôts",
  );
  return driver.execute(
    `return [...document.querySelectorAll("[data-testid=image-repo-card]")].map((a) =>
       decodeURIComponent(a.getAttribute("href")).replace(/^.*\\/hub\\/images\\//, ""))`,
  );
}

/** Texte de la carte de dépôt `id`. */
const repoCardText = (driver, id) =>
  driver.execute(
    `return [...document.querySelectorAll("[data-testid=image-repo-card]")]
       .find((a) => decodeURIComponent(a.getAttribute("href")).endsWith("/hub/images/" + arguments[0]))
       ?.textContent ?? null`,
    [id],
  );

/** Titres des cartes de modèles d'une section (« Your image models »...). */
const modelTitles = (driver, list) =>
  driver.execute(
    `const list = document.querySelector("ul[aria-label='" + arguments[0] + "']");
     return list ? [...list.querySelectorAll("[data-testid=image-model] h4")]
       .map((h) => h.textContent.trim()) : [];`,
    [list],
  );

/** Raison d'un Download désactivé, et l'état du bouton. */
const downloadState = (driver) =>
  driver.execute(
    `const button = [...document.querySelectorAll("aside button")]
       .find((b) => b.textContent.trim() === "Download");
     return {
       disabled: button?.disabled ?? null,
       reason: document.querySelector("[data-testid=image-download-reason]")?.textContent ?? null,
     };`,
  );

async function shoot(driver, name) {
  await delay(600); // transitions (survol, opacité)
  await driver.screenshot(`${SHOTS}/${name}.png`);
}

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
       warnings: window.__warnings ?? [] };`,
  );
  violations.push(...found.csp);
  consoleErrors.push(...found.errors);
  consoleWarnings.push(...found.warnings);
}

/** Lance l'app, surveille la page, relève tout à la fin, capture si échec. */
async function launch(name, fn) {
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
  await launch("image-models", async (driver) => {
    // 1. Onglet, dépôts populaires (sans le non censuré).
    await sidebar(driver, "ModelHub");
    await waitUntil(
      async () =>
        (await textsOf(driver, "[data-testid=hub-tabs-browse] a")).length > 0,
      "onglets du ModelHub",
    );
    assert.deepEqual(await textsOf(driver, "[data-testid=hub-tabs-browse] a"), [
      "Discover",
      "Transcription",
      "Polish",
      "Voices",
      "Images",
    ]);
    await tab(driver, "Images");
    await waitText(driver, "[data-testid=hub-scroll] h2", "Image models");
    await waitText(
      driver,
      "[data-testid=hub-scroll] h3",
      "Popular on Hugging Face",
      20000,
    );
    const popular = await repoCards(driver);
    for (const id of [KLEIN, UNKNOWN, GATED]) {
      assert.ok(popular.includes(id), `${id} parmi ${JSON.stringify(popular)}`);
    }
    assert.ok(!popular.includes(UNCENSORED), "dépôt non censuré masqué");
    assert.match(await repoCardText(driver, UNKNOWN), /Not supported/);
    assert.match(await repoCardText(driver, GATED), /Restricted/);
    await shoot(driver, "images-tab");

    // 2. Recherche par l'adresse, puis la fiche.
    await navigate(driver, "/hub/images?q=klein");
    await waitText(
      driver,
      "[data-testid=hub-scroll] h3",
      "Results for “klein”",
      20000,
    );
    assert.ok((await repoCards(driver)).includes(KLEIN));
    await driver.execute(
      `[...document.querySelectorAll("[data-testid=image-repo-card]")]
         .find((a) => decodeURIComponent(a.getAttribute("href")).endsWith(arguments[0]))
         .click();`,
      [KLEIN],
    );
    await waitText(driver, "#image-repo-title", "klein-test-GGUF", 20000);
    const files = await driver.execute(
      `return [...document.querySelectorAll("[data-testid=image-file] input")]
         .map((input) => ({ file: input.value, checked: input.checked }))`,
    );
    const chosen = files.find((entry) => entry.checked);
    assert.ok(chosen, `fichier conseillé coché : ${JSON.stringify(files)}`);
    assert.ok(
      (await textsOf(driver, "[data-testid=image-component]")).length > 0,
      "pièces annexes",
    );
    assert.match(
      await textOf(driver, "[data-testid=image-total]"),
      /^To download: /,
    );
    assert.deepEqual(await downloadState(driver), {
      disabled: false,
      reason: null,
    });
    await shoot(driver, "klein-page");
    const before = (await hf.requests()).length;
    await clickText(driver, "aside button", "Download");
    await waitUntil(
      async () =>
        (await textsOf(driver, "aside p")).includes("This file is installed."),
      "téléchargement fini",
      60000,
    );
    const resolved = (await hf.requests())
      .slice(before)
      .map((request) => request.path)
      .filter((path) => path.includes("/resolve/"));
    assert.ok(
      resolved.some((path) => path.includes(`/${KLEIN}/resolve/`)),
      `fichiers du modèle demandés : ${JSON.stringify(resolved)}`,
    );
    // Une révision épinglée (commit), jamais « main ».
    assert.ok(
      resolved.every((path) => !/\/resolve\/main\//.test(path)),
      `révision épinglée : ${JSON.stringify(resolved)}`,
    );
    assert.ok(
      await exists(driver, "aside a[href='/hub/images']"),
      "lien « See your image models »",
    );
    await shoot(driver, "klein-installed");
    // Fin prouvée par la file : ligne « done » de l'onglet Downloads.
    await tab(driver, "Downloads");
    await waitUntil(
      () => exists(driver, "[data-testid=image-download][data-state=done]"),
      "téléchargement fini (data-state=done)",
      60000,
    );

    // 3. Vos modèles : carte installée (Use), Use → In use.
    const stem = chosen.file.replace(/\.[^.]+$/, "");
    await tab(driver, "Images");
    await waitUntil(
      async () =>
        (await modelTitles(driver, "Your image models")).includes(stem),
      `carte ${stem} dans « Your image models »`,
    );
    await driver.execute(
      `const card = [...document.querySelectorAll("[data-testid=image-model]")]
         .find((li) => li.querySelector("h4")?.textContent.trim() === arguments[0]);
       [...card.querySelectorAll("button")].find((b) => b.textContent.trim() === "Use").click();`,
      [stem],
    );
    await waitUntil(
      () =>
        driver.execute(
          `const card = [...document.querySelectorAll("[data-testid=image-model]")]
             .find((li) => li.querySelector("h4")?.textContent.trim() === arguments[0]);
           return card?.textContent.includes("In use") ?? false;`,
          [stem],
        ),
      "In use",
    );
    await shoot(driver, "your-models");

    // 4. Onglet Downloads : section « Image models », Dismiss.
    await tab(driver, "Downloads");
    await waitUntil(
      async () =>
        (await textsOf(driver, "[data-testid=image-download]")).some((text) =>
          text.includes("Installed"),
        ),
      "ligne installée",
    );
    assert.ok(
      (await textsOf(driver, "#image-downloads")).includes("Image models"),
    );
    await clickText(driver, "[data-testid=image-download] button", "Dismiss");
    await waitUntil(
      async () => !(await exists(driver, "[data-testid=image-download]")),
      "ligne retirée",
    );

    // 5. Suppression confirmée.
    await tab(driver, "Images");
    await waitUntil(
      async () =>
        (await modelTitles(driver, "Your image models")).includes(stem),
      "carte de retour",
    );
    await driver.execute(
      `document.querySelector("button[aria-label='Delete " + arguments[0] + "']").click();`,
      [stem],
    );
    await driver.waitFor("[role=alertdialog]");
    assert.match(
      await textOf(driver, "[role=alertdialog]"),
      /Image generation won't work until you download it again/,
    );
    await clickText(driver, "[role=alertdialog] button", "Delete");
    await waitUntil(
      async () =>
        !(await modelTitles(driver, "Your image models")).includes(stem),
      "carte retirée",
    );

    // 6. Fiches refusées : raison visible, Download désactivé.
    await navigate(driver, `/hub/images/${UNKNOWN}`);
    await waitText(
      driver,
      "#image-repo-title",
      "unknown-diffusion-GGUF",
      20000,
    );
    assert.deepEqual(await downloadState(driver), {
      disabled: true,
      reason:
        "ChatonLM doesn't recognize this model's family, so it doesn't know which other files it needs.",
    });
    await navigate(driver, `/hub/images/${GATED}`);
    await waitText(driver, "#image-repo-title", "gated-image-GGUF", 20000);
    await waitUntil(
      async () =>
        (await downloadState(driver)).reason ===
        "This model needs a Hugging Face account. ChatonLM can't download it.",
      "raison du dépôt restreint",
    );
    assert.equal((await downloadState(driver)).disabled, true);
    await navigate(driver, `/hub/images/${UNCENSORED}`);
    await waitText(driver, "#image-repo-title", "uncensored-image-GGUF", 20000);
    assert.ok(
      (await textsOf(driver, "article header span")).includes("Uncensored"),
      "badge non censuré",
    );
    assert.deepEqual(await downloadState(driver), {
      disabled: true,
      reason:
        "Turn on “Show uncensored models” in Settings › Models to download this model.",
    });
    await shoot(driver, "uncensored-page");

    // 7. Réglage activé : le dépôt non censuré apparaît, avec son badge.
    await sidebar(driver, "Settings");
    await driver.waitFor("[data-testid=settings-tab-models]");
    await driver.click(await driver.find("[data-testid=settings-tab-models]"));
    const toggle = "button[role=switch][aria-label='Show uncensored models']";
    await driver.waitFor(toggle);
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
    await tab(driver, "Images");
    await navigate(driver, "/hub/images?q=uncensored");
    await waitUntil(
      async () => (await repoCards(driver)).includes(UNCENSORED),
      "dépôt non censuré listé",
      20000,
    );
    assert.match(await repoCardText(driver, UNCENSORED), /Uncensored/);
  });

  assert.deepEqual(violations, [], "violations de la CSP");
  assert.deepEqual(consoleErrors, [], "erreurs dans la console");
  assert.deepEqual(consoleWarnings, [], "avertissements dans la console");
} finally {
  await Promise.all([hf.stop(), fake.stop()]);
}
