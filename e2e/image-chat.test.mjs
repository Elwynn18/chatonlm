// Génération d'images dans la conversation, dans l'app réelle (build e2e :
// frontend embarqué, CSP de production ; contrat
// .tools/research/84-image-chat-contract.md § 4), avec le faux Ollama, le
// faux Hugging Face (familles d'essai à petites pièces,
// CHATONLM_IMAGE_FAMILIES) et le faux moteur d'images (fake-sd-server,
// CHATONLM_SD_SERVER_BIN) :
// - modèle d'images installé et choisi par les commandes (image_repo,
//   image_download, image_use_model) ;
// - outil appelé par le modèle (chatonlm-image, capacité « tools ») : outil
//   generate_image dans la requête, « Creating an image… », barre d'étapes,
//   aperçu (data:), image affichée et servie par le protocole des pièces
//   jointes (chargée, 512 × 512), « Created an image », réponse suivante ;
// - bouton Image : invite propre, aucune requête /api/chat au faux Ollama,
//   image affichée, bouton toujours actif dans la conversation créée ;
// - Stop pendant une image lente ([slow]) : « Image stopped », pas d'image ;
//   l'image suivante se fait ;
// - aucun modèle d'images choisi : envoi possible une fois les modèles
//   d'images lus, avis au-dessus de la zone de saisie, texte gardé,
//   « Choose an image model » mène à l'onglet Images du ModelHub ;
// - sans carte graphique (second lancement, CHATONLM_E2E_HARDWARE sans
//   GPU) : ligne ambre sous la zone de saisie, avertissement processeur dans
//   l'avancement ;
// - captures nommées (.tools/shots/image-chat/), prises aussi dans les deux
//   thèmes avec E2E_THEME_AUDIT ;
// - aucune violation de la CSP ni erreur ou avertissement dans la console.
//
// Suppose : le faux Ollama du lot 4 (modèle chatonlm-image : appel
// generate_image {prompt: <dernier message>, width: 512, height: 512}, puis
// « Here is your image. ») ; le faux sd-server (`[slow]` : 400 ms par étape,
// aperçus) ; chatonlm/klein-test-GGUF reconnu par image-families.json.
import assert from "node:assert/strict";
import { existsSync, mkdirSync } from "node:fs";
import { setTimeout as delay } from "node:timers/promises";
import { E2E_HARDWARE, ROOT, withApp } from "./lib/app.mjs";
import { startFake, startFakeHf } from "./lib/fake.mjs";
import { PENDING_CALLS } from "./lib/page.mjs";

const binary = process.argv[2];
const SHOTS = ".tools/shots/image-chat";
mkdirSync(SHOTS, { recursive: true });

const KLEIN = "chatonlm/klein-test-GGUF";
const FAMILIES = `${ROOT}src-tauri/crates/fake-ollama/fixtures/hf/image-families.json`;
const SD_SERVER = `${ROOT}.tools/e2e-target/debug/fake-sd-server`;
assert.ok(existsSync(SD_SERVER), `faux moteur d'images absent : ${SD_SERVER}`);

const TOOL_MODEL = "chatonlm-image";
const PLAIN_MODEL = "qwen3:0.6b";

const fake = await startFake("ollama");
const hf = await startFakeHf("hf");

const env = {
  OLLAMA_HOST: fake.url,
  CHATONLM_HF_URL: hf.url,
  CHATONLM_HF_STATUS_URL: hf.url,
  CHATONLM_IMAGE_FAMILIES: FAMILIES,
  CHATONLM_SD_SERVER_BIN: SD_SERVER,
};

/** Machine sans carte graphique (images par le processeur). */
const NO_GPU = JSON.stringify({ ...JSON.parse(E2E_HARDWARE), gpus: [] });

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

/** Corps des /api/chat reçus par le faux serveur. */
async function chatRequests() {
  const response = await fetch(new URL("/fake/requests", fake.url));
  return response.json();
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

async function shoot(driver, name) {
  await delay(600); // transitions (survol, opacité)
  await driver.screenshot(`${SHOTS}/${name}.png`);
}

// Violations de la CSP, erreurs et avertissements de la console : notés par
// la page, relevés avant chaque rechargement et à la fin de chaque lancement.
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

/** Recharge la page une fois les appels au backend finis. */
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

/** Bouton du sélecteur des modèles d'images (mode Image) : son texte, ou null. */
const shownImageModel = (driver) =>
  driver.execute(
    "return document.querySelector('[title=\"Select image model\"]')?.textContent ?? null",
  );

/** Attend que le sélecteur (d'Ollama ou d'images) affiche `expected`. */
async function waitShown(driver, read, expected, what) {
  let shown = null;
  await waitUntil(
    async () => (shown = await read(driver)) === expected,
    () => `${what} : ${expected} (affiché : ${shown})`,
  );
}

/** Choisit un modèle de langage (absent de la liste), puis recharge. */
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

/** Installe le modèle d'essai de la famille klein et le choisit. */
async function installImageModel(driver) {
  const detail = await driver.invoke("image_repo", { repo: KLEIN });
  const file = detail.files.find((f) => f.recommended) ?? detail.files[0];
  assert.ok(file, `fichiers de ${KLEIN} : ${JSON.stringify(detail.files)}`);
  const id = `${KLEIN}/${file.file}`;
  await driver.invoke("image_download", {
    repo: KLEIN,
    file: file.file,
    revision: detail.revision,
  });
  let last = null;
  await waitUntil(
    async () => {
      const downloads = await driver.invoke("image_downloads");
      last = downloads.find((d) => d.id === id)?.state ?? null;
      assert.notEqual(last, "failed", `téléchargement de ${id} en échec`);
      return last === "done";
    },
    () => `modèle d'images installé (état : ${last})`,
    60000,
  );
  await driver.invoke("image_use_model", { id });
  return id;
}

/**
 * Nouvelle conversation : bouton « New chat » de la page de conversation, ou
 * entrée « Chat » de la barre latérale ailleurs (le ModelHub n'a pas le
 * bouton) ; les deux mènent à /c/new.
 */
async function newChat(driver) {
  if (await exists(driver, "button[aria-label='Show sidebar']")) {
    await driver.click(await driver.find("button[aria-label='Show sidebar']"));
  }
  await driver.click(await driver.waitFor("a[href$='/c/new']"));
  await driver.waitFor("[data-testid=new-chat]");
}

const textarea = (driver) => driver.waitFor("textarea[aria-label='Message']");

/** Entrée dans la zone de saisie. */
const submit = (driver) =>
  driver.execute(
    `document.querySelector("textarea[aria-label='Message']").dispatchEvent(
       new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true }));`,
  );

async function send(driver, text) {
  await driver.type(await textarea(driver), text);
  await submit(driver);
}

/** Attend la fin de la réponse en cours (plus de bouton Stop). */
async function waitIdle(driver, timeoutMs = 30000) {
  await waitUntil(
    async () => !(await exists(driver, "button[aria-label='Stop']")),
    "fin de la réponse",
    timeoutMs,
  );
}

/** État du bouton Image : null s'il est absent. */
const imageButton = (driver) =>
  driver.execute(`
    const button = document.querySelector("button[aria-label='Image']");
    return button && { pressed: button.getAttribute("aria-pressed"),
      disabled: button.getAttribute("aria-disabled"), title: button.title };`);

/** Active le mode Image (bouton montré une fois la machine lue). */
async function enableImageMode(driver) {
  const state = await waitUntil(() => imageButton(driver), "bouton Image");
  if (state.pressed !== "true") {
    await driver.click(await driver.find("button[aria-label='Image']"));
  }
  await waitUntil(
    async () =>
      (await driver.execute(
        "return document.querySelector(\"textarea[aria-label='Message']\")?.placeholder",
      )) === "Describe the image to create",
    "invite du mode Image",
  );
}

/** Images créées affichées : adresse, taille demandée, chargement. */
const generatedImages = (driver) =>
  driver.execute(
    `return [...document.querySelectorAll("[data-role=generated-image] img")].map((img) => ({
       src: img.getAttribute("src"),
       alt: img.getAttribute("alt"),
       width: img.getAttribute("width"),
       height: img.getAttribute("height"),
       loaded: img.complete && img.naturalWidth > 0,
       naturalWidth: img.naturalWidth,
     }))`,
  );

/** Attend `count` images créées, chargées par le protocole. */
async function waitImages(driver, count) {
  let images = [];
  await waitUntil(
    async () => {
      images = await generatedImages(driver);
      return images.length === count && images.every((image) => image.loaded);
    },
    () => `${count} image(s) chargée(s) : ${JSON.stringify(images)}`,
    30000,
  );
  return images;
}

/** Avancement de l'image en cours (null : aucun). */
const progress = (driver) =>
  driver.execute(`
    const block = document.querySelector("[data-role=image-progress]");
    if (!block) return null;
    const bar = block.querySelector("[role=progressbar]");
    return {
      text: block.textContent,
      step: bar?.getAttribute("aria-valuenow") ?? null,
      total: bar?.getAttribute("aria-valuemax") ?? null,
      preview: block.querySelector("[data-testid=image-frame] img")?.getAttribute("src") ?? null,
    };`);

/** Lance l'app, surveille la page, relève tout à la fin, capture si échec. */
async function launch(name, extraEnv, fn) {
  await withApp(
    binary,
    { name, env: { ...env, ...extraEnv } },
    async (driver) => {
      try {
        await driver.waitFor("[data-testid=new-chat]", { timeoutMs: 20000 });
        await driver.execute(WATCH_PAGE);
        await fn(driver);
        await collectViolations(driver);
      } catch (error) {
        await driver.screenshot(`${SHOTS}/failure-${name}.png`).catch(() => {});
        throw error;
      }
    },
  );
}

try {
  await launch("image-chat", {}, async (driver) => {
    // Modèle de secours écrit par la page (session neuve) avant tout choix.
    await waitUntil(
      async () =>
        (await driver.invoke("get_settings")).selectedModel === PLAIN_MODEL,
      "modèle par défaut enregistré",
    );

    // 1. Aucun modèle d'images : avis, lien vers l'onglet Images.
    assert.deepEqual(await waitUntil(() => imageButton(driver), "bouton"), {
      pressed: "false",
      disabled: null,
      title: "Enable image mode",
    });
    await enableImageMode(driver);
    // Le sélecteur montre les modèles d'images (aucun choisi) à la place de
    // ceux d'Ollama.
    await waitShown(
      driver,
      shownImageModel,
      "Choose an image model",
      "sélecteur d'images sans modèle",
    );
    assert.equal(await shownModel(driver), null, "sélecteur d'Ollama masqué");
    // L'envoi attend que les modèles d'images soient lus (bouton d'envoi
    // grisé jusque-là) : refus ici, avec le lien. Un refus du moteur passe
    // par le même avis (texte rendu, lien une fois les modèles relus).
    await driver.type(await textarea(driver), "A red kitten");
    await waitUntil(
      () =>
        driver.execute(
          `const button = document.querySelector("button[aria-label='Send message']");
           return button !== null && !button.disabled;`,
        ),
      "bouton d'envoi utilisable",
    );
    await submit(driver);
    await waitUntil(
      () => exists(driver, "[data-testid=image-notice]"),
      "avis « aucun modèle d'images »",
    );
    assert.match(
      await textOf(driver, "[data-testid=image-notice]"),
      /No image model chosen: pick one in the ModelHub, Images tab\./,
    );
    assert.equal(
      await driver.execute(
        "return document.querySelector(\"textarea[aria-label='Message']\").value",
      ),
      "A red kitten",
      "zone de saisie gardée",
    );
    await waitUntil(
      () =>
        driver.execute(
          `return [...document.querySelectorAll("[data-testid=image-notice] a")]
             .some((a) => a.textContent.trim() === "Choose an image model");`,
        ),
      "lien « Choose an image model »",
    );
    await shoot(driver, "no-model");
    await driver.execute(
      `[...document.querySelectorAll("[data-testid=image-notice] a")]
         .find((a) => a.textContent.trim() === "Choose an image model").click();`,
    );
    await waitUntil(
      async () =>
        /^\/hub\/images\/?$/.test(
          await driver.execute("return location.pathname"),
        ),
      "onglet Images du ModelHub",
    );

    // 2. Modèle d'images installé et choisi.
    await installImageModel(driver);
    await newChat(driver);

    // 3. Outil appelé par le modèle : avancement, aperçu, image, réponse.
    await useModel(driver, TOOL_MODEL);
    const before = (await chatRequests()).length;
    const toolPrompt = "Draw a red kitten on a cream sofa [slow]";
    await send(driver, toolPrompt);
    const firstRequest = await waitUntil(
      async () => (await chatRequests())[before],
      "requête /api/chat du modèle",
    );
    assert.ok(
      (firstRequest.tools ?? []).some(
        (tool) => tool.function?.name === "generate_image",
      ),
      `outil generate_image offert : ${JSON.stringify(firstRequest.tools)}`,
    );
    let seen = null;
    await waitUntil(
      async () => {
        seen = await progress(driver);
        return seen?.step !== null && seen?.preview !== null && seen;
      },
      () => `étapes et aperçu : ${JSON.stringify(seen)}`,
      20000,
    );
    assert.match(seen.text, /Creating an image…/);
    assert.match(seen.text, /Draw a red kitten on a cream sofa \[slow\]/);
    assert.match(seen.preview, /^data:image\/png;base64,/);
    assert.match(seen.text, /Step \d+ of \d+/);
    await shoot(driver, "tool-progress");
    await waitIdle(driver);
    const [toolImage] = await waitImages(driver, 1);
    assert.match(toolImage.src, /attachment\/\d+$/);
    assert.equal(toolImage.alt, toolPrompt);
    assert.deepEqual([toolImage.width, toolImage.height], ["512", "512"]);
    assert.equal(toolImage.naturalWidth, 512);
    assert.ok(
      (await textsOf(driver, "[data-role=tool-calls] button")).includes(
        "Created an image",
      ),
    );
    await waitUntil(
      async () =>
        (await textsOf(driver, "[data-role=assistant]")).includes(
          "Here is your image.",
        ),
      "réponse suivante",
    );
    await shoot(driver, "tool-done");

    // 4. Bouton Image : aucune requête au modèle de langage. Le sélecteur
    // montre le modèle d'images installé, puis de nouveau celui d'Ollama une
    // fois le mode coupé.
    await newChat(driver);
    const chosen = (await driver.invoke("image_models")).find(
      (model) => model.selected && model.installed,
    );
    assert.ok(chosen, "modèle d'images choisi et installé");
    // Nom clair, comme le ModelHub : nom de la recommandation, sinon celui de
    // la famille, suivi de la variante.
    const title = chosen.recommended ? chosen.name : chosen.family.name;
    const imageModelName = chosen.quant ? `${title} ${chosen.quant}` : title;
    await waitShown(driver, shownModel, TOOL_MODEL, "sélecteur d'Ollama");
    await enableImageMode(driver);
    await waitShown(
      driver,
      shownImageModel,
      imageModelName,
      "sélecteur d'images",
    );
    assert.equal(await shownModel(driver), null, "sélecteur d'Ollama masqué");
    await driver.click(await driver.find("button[aria-label='Image']"));
    await waitShown(driver, shownModel, TOOL_MODEL, "sélecteur d'Ollama rendu");
    assert.equal(
      await shownImageModel(driver),
      null,
      "sélecteur d'images masqué",
    );
    await enableImageMode(driver);
    await waitShown(
      driver,
      shownImageModel,
      imageModelName,
      "sélecteur d'images de nouveau",
    );
    const requestsBefore = (await chatRequests()).length;
    const buttonPrompt = "A cream sofa in a sunny room";
    await send(driver, buttonPrompt);
    await waitIdle(driver);
    const [buttonImage] = await waitImages(driver, 1);
    assert.equal(buttonImage.alt, buttonPrompt);
    assert.equal(
      (await chatRequests()).length,
      requestsBefore,
      "aucune requête /api/chat pour le bouton Image",
    );
    // Pas de bloc de texte vide pour cette réponse.
    assert.deepEqual(await textsOf(driver, "[data-role=assistant]"), []);
    assert.equal((await imageButton(driver)).pressed, "true");
    await shoot(driver, "button-done");

    // 5. Stop pendant une image lente : arrêtée, puis la suivante se fait.
    await send(driver, "A very slow kitten [slow]");
    await waitUntil(
      async () => (await progress(driver))?.step != null,
      "image lente commencée",
      20000,
    );
    await driver.click(await driver.find("button[aria-label='Stop']"));
    await waitIdle(driver);
    await waitUntil(
      () => exists(driver, "[data-role=image-stopped]"),
      "« Image stopped »",
    );
    assert.equal(
      await textOf(driver, "[data-role=image-stopped]"),
      "Image stopped",
    );
    assert.equal((await generatedImages(driver)).length, 1);
    await shoot(driver, "stopped");
    await send(driver, "A quick kitten");
    await waitIdle(driver);
    await waitImages(driver, 2);
  });

  // 6. Sans carte graphique : ligne ambre, avertissement processeur.
  await launch(
    "image-chat-cpu",
    { CHATONLM_E2E_HARDWARE: NO_GPU },
    async (driver) => {
      await enableImageMode(driver);
      await waitUntil(
        () => exists(driver, "[data-testid=image-no-gpu]"),
        "ligne « No graphics card »",
      );
      assert.equal(
        await textOf(driver, "[data-testid=image-no-gpu]"),
        "No graphics card: each image takes several minutes on the processor",
      );
      await send(driver, "A slow processor kitten [slow]");
      let seen = null;
      await waitUntil(
        async () => {
          seen = await progress(driver);
          return seen?.text.includes(
            "Made by the processor (no graphics card): this takes several minutes.",
          );
        },
        () => `avertissement processeur : ${JSON.stringify(seen)}`,
        20000,
      );
      await shoot(driver, "cpu-progress");
      await waitIdle(driver);
      await waitImages(driver, 1);
    },
  );

  assert.deepEqual(violations, [], "violations de la CSP");
  assert.deepEqual(consoleErrors, [], "erreurs dans la console");
  assert.deepEqual(consoleWarnings, [], "avertissements dans la console");
} finally {
  await Promise.all([hf.stop(), fake.stop()]);
}
