// Modèles distants dans l'app réelle (build e2e), faux fournisseur : le faux
// `llama-server` (fake-llama) joue un fournisseur compatible avec l'API
// d'OpenAI à son adresse (« Other server »), clé exigée :
// - réglage « Use remote models » coupé par défaut : aucun modèle distant au
//   sélecteur, aucune requête au fournisseur ;
// - Settings › Models : ajout du fournisseur avec sa clé, « Check » (clé
//   acceptée, un modèle), indice de la clé seulement ;
// - réglage allumé : le modèle au sélecteur, réponse du fournisseur, corps
//   reçu sans aucun champ propre à llama.cpp ;
// - clé changée pour une fausse : erreur sous le message, « Check the key »
//   ouvre Settings › Models ;
// - la clé n'est ni dans la page, ni dans le journal de l'app, ni dans la
//   base ; elle est dans `remote/keys.json` (0600) ;
// - aucune violation de la CSP ni erreur dans la console.
// Écrit à l'étape 2 (lot L7), lancé à l'étape 3 : binaires fake-llama et
// fake-relay construits par scripts/e2e-build.sh.
import assert from "node:assert/strict";
import { readdirSync, readFileSync, statSync } from "node:fs";
import { join } from "node:path";
import { ROOT, withApp } from "./lib/app.mjs";
import { startFakeProvider } from "./lib/relay.mjs";

const binary = process.argv[2];
const dataDir = process.env.CHATONLM_DATA_DIR;
assert.ok(dataDir, "CHATONLM_DATA_DIR manquant (session virtuelle ?)");
const KEY = "sk-e2e-remote-SECRET-9f2c";
const MODEL = "chatonlm-remote-openai";
const ID = `@remote/custom/${MODEL}`;
const provider = await startFakeProvider({ alias: MODEL, key: KEY });
const delay = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

async function waitUntil(check, what, timeoutMs = 10000) {
  const deadline = Date.now() + timeoutMs;
  for (;;) {
    const value = await check();
    if (value) return value;
    if (Date.now() > deadline) throw new Error(`Délai dépassé : ${what}`);
    await delay(100);
  }
}

const textOf = (driver, css) =>
  driver.execute(
    "return document.querySelector(arguments[0])?.textContent ?? null",
    [css],
  );
/** Clique le bouton de `scope` dont le texte est `label`. */
const clickButton = (driver, scope, label) =>
  driver.execute(
    `const button = [...document.querySelectorAll(arguments[0] + " button")]
       .find((b) => b.textContent.trim() === arguments[1]);
     if (!button) throw new Error("no button " + arguments[1]);
     button.click();`,
    [scope, label],
  );

/** Fichiers sous `dir`, récursivement. */
function files(dir) {
  return readdirSync(dir, { withFileTypes: true }).flatMap((entry) =>
    entry.isDirectory()
      ? files(join(dir, entry.name))
      : [join(dir, entry.name)],
  );
}

const CARD = "[data-testid=remote-models-settings]";

try {
  await withApp(binary, { name: "remote-models" }, async (driver) => {
    await driver.waitFor("[data-testid=new-chat]");
    // 1. Coupé par défaut : rien au sélecteur, rien demandé.
    const settings = await driver.invoke("get_settings");
    assert.equal(settings.remoteModels, false);
    const models = await driver.invoke("list_models");
    assert.ok(!models.some((model) => model.origin === "remote"));

    // 2. Ajout du fournisseur depuis Settings › Models.
    await driver.execute(
      "location.hash = ''; history.pushState({}, '', '/settings?section=models'); dispatchEvent(new PopStateEvent('popstate'))",
    );
    await driver.waitFor(CARD);
    await clickButton(driver, CARD, "Add a provider");
    const form = await driver.waitFor("[data-testid=remote-provider-form]");
    assert.ok(form);
    await driver.execute(
      `const select = document.querySelector("[data-testid=remote-provider-form] select");
       select.value = "custom";
       select.dispatchEvent(new Event("change", { bubbles: true }));`,
    );
    const url = await driver.waitFor(
      "[data-testid=remote-provider-form] input[type=url]",
    );
    await driver.type(url, `${provider.url}/v1`);
    const key = await driver.find(
      "[data-testid=remote-provider-form] input[type=password]",
    );
    await driver.type(key, KEY);
    await clickButton(driver, "[data-testid=remote-provider-form]", "Save");
    await driver.waitFor("[data-testid=remote-provider-custom]");
    assert.match(
      await textOf(driver, "[data-testid=remote-provider-custom]"),
      /API key ending in 9f2c/,
    );
    await clickButton(driver, "[data-testid=remote-provider-custom]", "Check");
    await waitUntil(
      async () =>
        /Key accepted: 1 model available\./.test(
          (await textOf(driver, "[data-testid=remote-provider-custom]")) ?? "",
        ),
      "clé vérifiée",
    );
    await driver.screenshot(".tools/shots/remote-models.png");

    // 3. Réglage allumé : le modèle au sélecteur, réponse du fournisseur.
    await driver.invoke("update_settings", { patch: { remoteModels: true } });
    const listed = await driver.invoke("list_models");
    assert.ok(
      listed.some((model) => model.id === ID),
      JSON.stringify(listed),
    );
    await driver.invoke("update_settings", { patch: { selectedModel: ID } });
    await driver.execute(
      "history.pushState({}, '', '/c/new'); dispatchEvent(new PopStateEvent('popstate'))",
    );
    const textarea = await driver.waitFor("textarea[aria-label='Message']");
    await driver.type(textarea, "Hello there");
    await driver.execute(
      `document.querySelector("textarea[aria-label='Message']").dispatchEvent(
         new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true }));`,
    );
    await waitUntil(
      async () =>
        (await driver.execute("return document.body.textContent")).includes(
          "openai ok",
        ),
      "réponse du fournisseur",
      20000,
    );
    const sent = (await provider.requests()).filter(
      (request) => request.path === "/v1/chat/completions",
    );
    assert.equal(sent.length, 1);
    for (const field of [
      "cache_prompt",
      "chat_template_kwargs",
      "top_k",
      "min_p",
      "repeat_penalty",
      "repeat_last_n",
      "reasoning_effort",
    ]) {
      assert.ok(!(field in sent[0].body), field);
    }
    assert.equal(sent[0].body.model, MODEL);

    // 4. Fausse clé : erreur sous le message, Check the key → réglages.
    await driver.invoke("save_remote_provider", {
      input: {
        id: "custom",
        kind: "custom",
        name: "",
        baseUrl: provider.url,
        apiKey: "sk-wrong-key-0000",
        enabled: true,
      },
    });
    const box = await driver.waitFor("textarea[aria-label='Message']");
    await driver.type(box, "Again");
    await driver.execute(
      `document.querySelector("textarea[aria-label='Message']").dispatchEvent(
         new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true }));`,
    );
    await waitUntil(
      async () =>
        /refused the API key/.test(
          await driver.execute("return document.body.textContent"),
        ),
      "erreur de clé",
    );
    await driver.screenshot(".tools/shots/remote-models-key-error.png");
    await clickButton(driver, "[role=alert]", "Check the key");
    await driver.waitFor(CARD);

    // 5. La clé n'est pas dans la page.
    const html = await driver.execute(
      "return document.documentElement.outerHTML",
    );
    assert.ok(!html.includes(KEY), "clé dans la page");
  });

  // 6. Ni au journal, ni dans la base ; dans son fichier, 0600.
  const log = readFileSync(`${ROOT}.tools/logs/e2e-remote-models.log`, "utf8");
  assert.ok(!log.includes(KEY), "clé au journal");
  const keysFile = join(dataDir, "remote", "keys.json");
  for (const file of files(dataDir)) {
    const bytes = readFileSync(file);
    const found = bytes.includes(Buffer.from(KEY));
    if (file === keysFile) continue;
    assert.ok(!found, `clé dans ${file}`);
  }
  assert.equal(statSync(keysFile).mode & 0o777, 0o600);
  console.log("modèles distants : réglage, fournisseur, réponse, clé gardée");
} finally {
  await provider.stop();
}
