// Modèle installé dont Hugging Face a une version plus récente, dans l'app
// réelle (build e2e), avec le faux Hugging Face (dépôts synthétisés
// `chatonlm-dl/…`, `POST /fake/publish-hf` publie une nouvelle révision) et le
// faux moteur :
// - deux modèles installés par la file du ModelHub (commande hub_download) ;
// - modèle à jour : sa fiche est relue sur Hugging Face, aucun bandeau ;
// - nouvelle révision publiée : bandeau « A newer version of ... is
//   available » au-dessus de la zone de saisie, pastille « Update available »
//   et bouton Update dans ModelHub > Installed, pour ce modèle seulement ;
// - Update (bandeau) : le modèle est téléchargé de nouveau par la file du
//   ModelHub (fichiers redemandés), le bandeau montre la progression puis
//   disparaît ; la ligne des installés n'offre plus rien, et la fiche, relue,
//   ne signale plus le modèle ;
// - aucune violation de la CSP ni erreur ou avertissement dans la console.
import assert from "node:assert/strict";
import { withApp } from "./lib/app.mjs";
import { startEngine, startFakeHf } from "./lib/fake.mjs";
import { PENDING_CALLS, shownModel } from "./lib/page.mjs";

const binary = process.argv[2];
const hf = await startFakeHf("hf");
const engine = startEngine([]);
const env = {
  ...engine.env,
  CHATONLM_HF_URL: hf.url,
  CHATONLM_HF_STATUS_URL: hf.url,
};

/** Téléchargement lent (morceaux de 4 Kio toutes les 15 ms) : progression lisible. */
const REPO = "chatonlm-dl/Update-slow-big-GGUF";
const MODEL = `${REPO}:Q4_K_M`;
/** Autre modèle installé, jamais périmé. */
const STEADY = "chatonlm-dl/Steady-GGUF:Q4_K_M";
/** Nom court du modèle dans les textes (src/lib/modelNames.ts). */
const SHORT = "Update-slow-big:Q4_K_M";
const AVAILABLE = `A newer version of ${SHORT} is available`;

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

/** Texte du bandeau, ou null. */
const banner = (driver) =>
  driver.execute(
    "return document.querySelector('[data-testid=stale-model]')?.textContent ?? null",
  );

/** Nombre de fois où la fiche du dépôt a été lue sur Hugging Face. */
const asked = async () =>
  (await hf.requests()).filter((request) =>
    request.path.startsWith(`/api/models/${REPO}`),
  ).length;

/** Fichiers du modèle demandés à Hugging Face (`resolve/`). */
const fetched = async () =>
  (await hf.requests()).filter((request) =>
    request.path.startsWith(`/${REPO}/resolve/`),
  ).length;

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
  await withApp(binary, { name: "model-update", env }, async (driver) => {
    await driver.waitFor("[data-testid=new-chat]");
    await driver.execute(WATCH_PAGE);
    // Installés par la file du ModelHub, à la révision publiée.
    for (const model of [MODEL, STEADY]) {
      await driver.invoke("hub_download", {
        request: { model, repo: null, label: null },
      });
    }
    await waitUntil(
      async () => {
        const installed = (await driver.invoke("hub_installed")).map(
          (m) => m.model,
        );
        return installed.includes(MODEL) && installed.includes(STEADY);
      },
      "modèles installés",
      60000,
    );
    await driver.invoke("update_settings", {
      patch: { selectedModel: MODEL },
    });
    await reload(driver);
    await waitUntil(
      async () => (await shownModel(driver)) === MODEL,
      `modèle ${MODEL}`,
    );

    // À jour : la fiche est relue, rien n'est proposé.
    await waitUntil(async () => (await asked()) >= 1, "fiche relue");
    await delay(500);
    assert.equal(await banner(driver), null);
    await openInstalled(driver);
    await waitUntil(async () => (await asked()) >= 2, "installés vérifiés");
    await delay(500);
    for (const row of await installedRows(driver)) {
      assert.equal(row.chip, false, row.text);
      assert.equal(row.update, null, row.text);
    }

    // Nouvelle révision sur Hugging Face : bandeau de la conversation.
    const published = await fetch(new URL("/fake/publish-hf", hf.url), {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ repo: REPO }),
    });
    assert.ok(published.ok, "publication refusée par le faux Hugging Face");
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
    assert.ok(marked[0].text.includes("Update-slow-big"), marked[0].text);
    assert.match(marked[0].update ?? "", /^Update .*Update-slow-big/);
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
    const fetchedBefore = await fetched();
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
        return seen?.startsWith(`Updating ${SHORT}… `) && /\d+%$/.test(seen);
      },
      () => `progression dans le bandeau : ${seen}`,
    );
    await driver.screenshot(".tools/shots/model-update-progress.png");
    await waitUntil(
      async () => (await banner(driver)) === null,
      "bandeau retiré à la fin de la mise à jour",
      30000,
    );
    assert.ok((await fetched()) > fetchedBefore, "fichiers redemandés");
    assert.ok(
      (await driver.invoke("hub_downloads")).some(
        (download) => download.model === MODEL && download.state === "done",
      ),
      "mise à jour finie dans la file",
    );

    // Les installés n'offrent plus rien ; la fiche, relue, ne signale plus le
    // modèle.
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
  await hf.stop();
}

assert.deepEqual(violations, [], "violations de la CSP");
assert.deepEqual(consoleErrors, [], "erreurs dans la console");
assert.deepEqual(consoleWarnings, [], "avertissements dans la console");
