// Modèle installé dont le registre a une version plus récente, dans l'app
// réelle (build e2e), avec le faux serveur, qui sert aussi de registre
// (CHATONLM_REGISTRY_URL) et où POST /fake/publish publie une nouvelle
// version :
// - modèle à jour : le registre est interrogé (HEAD de son manifeste), aucun
//   bandeau ;
// - nouvelle version publiée : bandeau « A newer version of ... is
//   available » au-dessus de la zone de saisie, pastille « Update available »
//   et bouton Update dans ModelHub > Installed, pour ce modèle seulement ;
// - Update (bandeau) : le modèle est téléchargé de nouveau par la file du
//   ModelHub, le bandeau montre la progression puis disparaît ; la ligne des
//   installés n'offre plus rien, et le registre, interrogé de nouveau, ne
//   signale plus le modèle ;
// - aucune violation de la CSP ni erreur ou avertissement dans la console.
import assert from "node:assert/strict";
import { withApp } from "./lib/app.mjs";
import { startFake } from "./lib/fake.mjs";
import { PENDING_CALLS } from "./lib/page.mjs";

const binary = process.argv[2];
const fake = await startFake("ollama");
const env = { OLLAMA_HOST: fake.url, CHATONLM_REGISTRY_URL: fake.url };

/** Téléchargement de 5 s environ : le temps de lire la progression. */
const MODEL = "chatonlm-pull-slow";
const HEAD = `HEAD /v2/library/${MODEL}/manifests/latest`;
const AVAILABLE = `A newer version of ${MODEL} is available`;

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
 * Recharge la page (nouvelle vérification au registre : la précédente est
 * gardée 5 minutes) et attend la nouvelle, seulement quand aucun appel au
 * backend n'est en cours.
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

/** Change de page comme un lien du routeur (pushState que TanStack suit). */
const navigate = (driver, path) =>
  driver.execute(
    `const index = (history.state?.__TSR_index ?? 0) + 1;
     const key = Math.random().toString(36).slice(2);
     history.pushState({ key, __TSR_key: key, __TSR_index: index }, "", arguments[0]);`,
    [path],
  );

const shownModel = (driver) =>
  driver.execute(
    "return document.querySelector('[title=\"Select model\"]')?.textContent ?? null",
  );

/** Texte du bandeau, ou null. */
const banner = (driver) =>
  driver.execute(
    "return document.querySelector('[data-testid=stale-model]')?.textContent ?? null",
  );

/** Nombre de fois où le registre a été interrogé sur le modèle. */
const asked = async () =>
  (await fake.manifests()).filter((line) => line === HEAD).length;

/**
 * Lignes de ModelHub > Installed : texte, pastille « Update available »,
 * bouton Update.
 */
const installedRows = (driver) =>
  driver.execute(
    `return [...document.querySelectorAll("[data-testid=installed-model]")].map((row) => ({
       text: row.textContent,
       chip: [...row.querySelectorAll("span")].some((s) => s.textContent === "Update available"),
       update: [...row.querySelectorAll("button")]
         .find((b) => b.textContent === "Update")?.getAttribute("aria-label") ?? null,
     }))`,
  );

/** Ouvre ModelHub > Installed et attend ses lignes. */
async function openInstalled(driver) {
  await navigate(driver, "/hub/installed");
  await driver.waitFor("[data-testid=installed-model]");
}

try {
  // Installé avant le lancement de l'app, à la version du registre.
  await fake.install(MODEL);
  await withApp(binary, { name: "model-update", env }, async (driver) => {
    await driver.waitFor("[data-testid=new-chat]");
    await driver.execute(WATCH_PAGE);
    await waitUntil(() => shownModel(driver), "modèle affiché");
    await driver.invoke("update_settings", {
      patch: { selectedModel: MODEL },
    });
    await reload(driver);
    await waitUntil(
      async () => (await shownModel(driver)) === MODEL,
      `modèle ${MODEL}`,
    );

    // À jour : le registre est interrogé, rien n'est proposé.
    await waitUntil(async () => (await asked()) >= 1, "registre interrogé");
    await delay(500);
    assert.equal(await banner(driver), null);
    await openInstalled(driver);
    await waitUntil(async () => (await asked()) >= 2, "installés vérifiés");
    await delay(500);
    for (const row of await installedRows(driver)) {
      assert.equal(row.chip, false, row.text);
      assert.equal(row.update, null, row.text);
    }

    // Nouvelle version au registre : bandeau de la conversation.
    await fake.publish(MODEL);
    await navigate(driver, "/c/new");
    await driver.waitFor("[data-testid=new-chat]");
    await reload(driver);
    await waitUntil(
      async () => (await banner(driver))?.includes(AVAILABLE),
      "bandeau du modèle périmé",
    );
    await driver.screenshot(".tools/shots/model-update-banner.png");

    // ModelHub > Installed : ce modèle seulement.
    await openInstalled(driver);
    let rows;
    await waitUntil(
      async () => {
        rows = await installedRows(driver);
        return rows.some((row) => row.chip);
      },
      () => `pastille Update available : ${JSON.stringify(rows)}`,
    );
    const marked = rows.filter((row) => row.chip);
    assert.equal(marked.length, 1, JSON.stringify(rows));
    assert.ok(marked[0].text.includes(MODEL), marked[0].text);
    assert.equal(marked[0].update, `Update ${MODEL}:latest`);
    assert.deepEqual(
      rows.filter((row) => row.update !== null),
      marked,
    );
    await driver.screenshot(".tools/shots/model-update-installed.png");

    // Update depuis le bandeau : progression, puis plus de bandeau.
    await navigate(driver, "/c/new");
    await driver.waitFor("[data-testid=new-chat]");
    await waitUntil(
      async () => (await banner(driver))?.includes(AVAILABLE),
      "bandeau de retour dans la conversation",
    );
    const pullsBefore = (await fake.pulls()).length;
    assert.ok(
      await driver.execute(
        `const button = [...document.querySelectorAll("[data-testid=stale-model] button")]
           .find((b) => b.textContent === "Update");
         button?.click();
         return button !== undefined && !button.disabled;`,
      ),
      "bouton Update du bandeau",
    );
    let seen;
    await waitUntil(
      async () => {
        seen = await banner(driver);
        return /^Updating chatonlm-pull-slow… \d+%$/.test(seen ?? "");
      },
      () => `progression dans le bandeau : ${seen}`,
    );
    await driver.screenshot(".tools/shots/model-update-progress.png");
    await waitUntil(
      async () => (await banner(driver)) === null,
      "bandeau retiré à la fin de la mise à jour",
      30000,
    );
    const pulls = (await fake.pulls()).slice(pullsBefore);
    assert.deepEqual(
      pulls.map((pull) => pull.model),
      [`${MODEL}:latest`],
    );

    // Les installés n'offrent plus rien ; le registre, interrogé de nouveau,
    // ne signale plus le modèle.
    await openInstalled(driver);
    const before = await asked();
    await reload(driver);
    await driver.waitFor("[data-testid=installed-model]");
    await waitUntil(async () => (await asked()) > before, "nouvelle demande");
    await delay(500);
    for (const row of await installedRows(driver)) {
      assert.equal(row.chip, false, row.text);
      assert.equal(row.update, null, row.text);
    }
    await navigate(driver, "/c/new");
    await driver.waitFor("[data-testid=new-chat]");
    await waitUntil(
      async () => (await shownModel(driver)) === MODEL,
      "modèle toujours choisi",
    );
    await delay(500);
    assert.equal(await banner(driver), null);
    await collectViolations(driver);
  });
} finally {
  await fake.stop();
}

assert.deepEqual(violations, [], "violations de la CSP");
assert.deepEqual(consoleErrors, [], "erreurs dans la console");
assert.deepEqual(consoleWarnings, [], "avertissements dans la console");
