// Dossier des modèles du moteur intégré (Settings › General, ligne « Models
// folder » : settings/ModelsFolder.tsx), dans l'app réelle avec le faux moteur
// et le sélecteur de dossier simulé (CHATONLM_E2E_PICK_FOLDER : fichier dont
// la première ligne est le dossier « choisi », vide = annulé) :
// - résumé : nombre de modèles et place, dossier par défaut ;
// - sélecteur annulé : rien ne change ;
// - nouveau dossier, « Move models » : le modèle chargé est d'abord déchargé
//   (processus arrêté), les fichiers (poids et projecteur) passent dans le
//   nouveau dossier et quittent l'ancien, la base garde ses modèles, le
//   moteur relance le modèle depuis le nouveau dossier pour la réponse
//   suivante ;
// - dossier vide, « Use it as it is » : plus aucun modèle listé (fichiers
//   absents), rien d'effacé nulle part ; retour au dossier des modèles « tel
//   quel » : les modèles reviennent ;
// - aucune violation de la CSP ni erreur dans la console.
// Remplace l'ancien essai du dossier d'Ollama (systemd, OLLAMA_MODELS).
import assert from "node:assert/strict";
import { existsSync, mkdirSync, readdirSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { withApp } from "./lib/app.mjs";
import { fakeModel, startEngine } from "./lib/fake.mjs";

const binary = process.argv[2];
const DATA = process.env.CHATONLM_DATA_DIR;
assert.ok(
  DATA,
  "CHATONLM_DATA_DIR absent : lancer dans scripts/e2e-session.sh",
);
const SIMPLE = fakeModel("simple");
const VISION = fakeModel("vision");
const engine = startEngine([SIMPLE, VISION]);
const OLD = engine.modelsDir;
const RUN = dirname(DATA);
const MOVED = join(RUN, "moved-models");
const EMPTY = join(RUN, "empty-models");
mkdirSync(MOVED, { recursive: true });
mkdirSync(EMPTY, { recursive: true });
const PICK = join(RUN, "picked-folder");
const pick = (path) => writeFileSync(PICK, `${path}\n`);

const delay = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

async function waitUntil(check, what, timeoutMs = 15000) {
  const deadline = Date.now() + timeoutMs;
  for (;;) {
    const value = await check();
    if (value) return value;
    if (Date.now() > deadline) throw new Error(`Délai dépassé : ${what}`);
    await delay(100);
  }
}

/** Fichiers (relatifs) sous `dir`, triés. */
function filesUnder(dir) {
  const found = [];
  const walk = (current, prefix) => {
    if (!existsSync(current)) return;
    for (const entry of readdirSync(current, { withFileTypes: true })) {
      const path = prefix ? `${prefix}/${entry.name}` : entry.name;
      if (entry.isDirectory()) walk(join(current, entry.name), path);
      else found.push(path);
    }
  };
  walk(dir, "");
  return found.sort();
}

const MODEL_FILES = engine.models
  .flatMap((model) => [...model.files, ...(model.mmproj ? [model.mmproj] : [])])
  .sort();

const summary = (driver) =>
  driver.execute(
    "return document.querySelector('[data-testid=models-folder-summary]')?.textContent ?? null",
  );

/** Bouton dont le texte est `label` (dans `scope`), cliqué ; vrai s'il existe. */
const clickText = (driver, scope, label) =>
  driver.execute(
    `const button = [...document.querySelectorAll(arguments[0] + " button")]
       .find((b) => b.textContent.trim() === arguments[1]);
     button?.click();
     return Boolean(button);`,
    [scope, label],
  );

async function openGeneralTab(driver) {
  if (
    await driver.execute(
      "return document.querySelector(\"button[aria-label='Show sidebar']\") !== null",
    )
  ) {
    await driver.click(await driver.find("button[aria-label='Show sidebar']"));
  }
  await driver.click(await driver.waitFor("nav a[href='/settings']"));
  await driver.click(
    await driver.waitFor("[data-testid=settings-tab-general]"),
  );
  await driver.waitFor("[data-testid=models-folder-summary]");
}

/** Change… puis, si `choice` est donné, le bouton de la confirmation. */
async function change(driver, choice) {
  assert.ok(
    await clickText(driver, "[data-testid=settings]", "Change…"),
    "bouton Change…",
  );
  if (choice) {
    await driver.waitFor("[role=alertdialog]");
    assert.ok(
      await clickText(driver, "[role=alertdialog]", choice),
      `bouton ${choice}`,
    );
  }
}

await withApp(
  binary,
  {
    name: "models-dir",
    env: { ...engine.env, CHATONLM_E2E_PICK_FOLDER: PICK },
  },
  async (driver) => {
    await driver.waitFor("[data-testid=new-chat]");
    await driver.execute(`
      window.__csp = []; window.__errors = [];
      document.addEventListener("securitypolicyviolation", (e) => window.__csp.push(e.violatedDirective));
      const error = console.error;
      console.error = (...args) => { window.__errors.push(args.map(String).join(" ")); error.apply(console, args); };`);
    assert.equal(await driver.invoke("engine_models_dir"), OLD);
    await driver.invoke("load_model", { model: SIMPLE });
    await waitUntil(() => engine.server(SIMPLE) !== null, "modèle chargé");
    const loadedPid = engine.server(SIMPLE).pid;

    await openGeneralTab(driver);
    assert.match(await summary(driver), /2 models \(.+\)/);

    // Sélecteur annulé : rien ne change.
    writeFileSync(PICK, "\n");
    await change(driver);
    await delay(1000);
    assert.equal(await driver.invoke("engine_models_dir"), OLD);
    assert.equal(
      await driver.execute(
        "return document.querySelector('[role=alertdialog]') !== null",
      ),
      false,
    );

    // Nouveau dossier, « Move models ».
    pick(MOVED);
    await change(driver, "Move models");
    await waitUntil(
      async () => (await driver.invoke("engine_models_dir")) === MOVED,
      "dossier changé",
      60000,
    );
    await waitUntil(
      () => JSON.stringify(filesUnder(MOVED)) === JSON.stringify(MODEL_FILES),
      () => `fichiers déplacés : ${JSON.stringify(filesUnder(MOVED))}`,
      60000,
    );
    assert.deepEqual(
      filesUnder(OLD),
      [],
      "fichiers restés dans l'ancien dossier",
    );
    assert.equal(
      engine.server(SIMPLE),
      null,
      "modèle pas déchargé avant le déplacement",
    );
    try {
      process.kill(loadedPid, 0);
      assert.fail("processus du modèle encore en vie");
    } catch (error) {
      if (error.code !== "ESRCH") throw error;
    }
    assert.deepEqual(
      (await driver.invoke("list_models")).map((model) => model.id).sort(),
      [SIMPLE, VISION].sort(),
    );
    await driver.screenshot(".tools/shots/models-folder-moved.png");

    // Le moteur relance le modèle depuis le nouveau dossier.
    await driver.invoke("load_model", { model: SIMPLE });
    await waitUntil(() => engine.server(SIMPLE) !== null, "modèle relancé");
    assert.ok(
      engine.server(SIMPLE).model.startsWith(MOVED),
      `modèle lancé depuis ${engine.server(SIMPLE).model}`,
    );

    // Dossier vide « tel quel » : plus aucun modèle, rien d'effacé.
    pick(EMPTY);
    await change(driver, "Use it as it is");
    await waitUntil(
      async () => (await driver.invoke("engine_models_dir")) === EMPTY,
      "dossier vide choisi",
    );
    await waitUntil(
      async () => (await driver.invoke("list_models")).length === 0,
      "liste vide",
    );
    assert.deepEqual(filesUnder(MOVED), MODEL_FILES, "fichiers effacés");
    assert.match(await summary(driver), /No models yet/);

    // Retour au dossier des modèles, tel quel.
    pick(MOVED);
    await change(driver, "Use it as it is");
    await waitUntil(
      async () => (await driver.invoke("list_models")).length === 2,
      "modèles revenus",
    );
    const problems = await driver.execute(
      "return { csp: window.__csp, errors: window.__errors }",
    );
    assert.deepEqual(problems.csp, [], "violations de la CSP");
    assert.deepEqual(problems.errors, [], "erreurs dans la console");
  },
);
console.log("models-dir : OK");
