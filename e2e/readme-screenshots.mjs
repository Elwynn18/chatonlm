// Captures du README (docs/screenshots/) : l'app réelle dans la session
// virtuelle, interface en anglais, avec l'instance Ollama de test
// (scripts/test-ollama.sh, 127.0.0.1:11435, qwen3:0.6b), le vrai Hugging Face
// (réseau) et l'historique de démonstration. Ce n'est pas un essai :
// check.sh ne le lance pas, et seule la forme des écrans est attendue.
//   scripts/test-ollama.sh start
//   E2E_TIMEOUT=900 scripts/e2e-session.sh e2e/run.sh \
//     e2e/readme-screenshots.mjs "$(scripts/e2e-build.sh)"
// Captures brutes (PNG 1360 × 960) dans .tools/shots/readme/, relues à l'œil
// (réponses d'un vrai modèle) puis converties en WebP (qualité 85) :
// chat-light → chat.webp, hub-dark → modelhub.webp, proofread-light →
// proofread.webp, study-form-light → study.webp, settings-models-dark →
// settings.webp.
// Saisie par insertText et clics, sans e2e/lib/keyboard.mjs : le script peut
// ainsi tourner depuis une copie de travail (la garde du clavier exige le
// HOME de la session du dépôt qui contient le script).
import { mkdirSync } from "node:fs";
import { setTimeout as delay } from "node:timers/promises";
import { ROOT, withApp } from "./lib/app.mjs";

const binary = process.argv[2];
const OLLAMA = "http://127.0.0.1:11435";
const MODEL = "qwen3:0.6b";
const OUT = `${ROOT}.tools/shots/readme`;
mkdirSync(OUT, { recursive: true });

const tags = await fetch(`${OLLAMA}/api/tags`)
  .then((response) => response.json())
  .catch(() => null);
if (!tags?.models.some((m) => m.name === MODEL)) {
  throw new Error(
    `${MODEL} absent ou instance arrêtée : scripts/test-ollama.sh start`,
  );
}

async function waitUntil(check, what, timeoutMs = 10000) {
  const deadline = Date.now() + timeoutMs;
  for (;;) {
    const value = await check();
    if (value) return value;
    if (Date.now() > deadline) throw new Error(`Délai dépassé : ${what}`);
    await delay(100);
  }
}

const exists = (driver, css) =>
  driver.execute("return document.querySelector(arguments[0]) !== null", [css]);

/**
 * Lien, bouton ou option (label d'un bouton radio) de `scope` dont le texte
 * est `text`, cliqué.
 */
const clickText = (driver, scope, text) =>
  waitUntil(
    () =>
      driver.execute(
        `const target = [...document.querySelectorAll(
             ["a", "button", "label"].map((e) => arguments[0] + " " + e).join(", "))]
           .find((e) => e.textContent.trim() === arguments[1]);
         target?.click();
         return target !== undefined;`,
        [scope, text],
      ),
    `« ${text} » dans ${scope}`,
  );

/** Texte saisi dans un champ (événements d'entrée, comme une frappe). */
const insertText = (driver, css, text) =>
  driver.execute(
    `const area = document.querySelector(arguments[0]);
     area.focus();
     area.select();
     document.execCommand("insertText", false, arguments[1]);`,
    [css, text],
  );

async function showSidebar(driver) {
  if (await exists(driver, "button[aria-label='Show sidebar']")) {
    await driver.click(await driver.find("button[aria-label='Show sidebar']"));
  }
}

/** Entrée de la barre latérale (Chat, Proofread, Study, ModelHub, Settings). */
async function sidebar(driver, label) {
  await showSidebar(driver);
  await clickText(driver, "nav", label);
}

/** Capture après un temps de pose (animations, images). */
async function shot(driver, name, settleMs = 800) {
  await delay(settleMs);
  await driver.execute("document.activeElement?.blur()");
  const file = await driver.screenshot(`${OUT}/${name}.png`);
  console.log(`  ${file}`);
}

/** Attend la fin d'une réponse : plus de bouton Stop pendant 1,5 s. */
async function replyDone(driver) {
  let since = null;
  await waitUntil(
    async () => {
      if (await exists(driver, "button[aria-label='Stop']")) since = null;
      else since ??= Date.now();
      return since !== null && Date.now() - since > 1500;
    },
    "fin de la réponse",
    240_000,
  );
}

await withApp(
  binary,
  {
    name: "readme-screenshots",
    env: {
      OLLAMA_HOST: OLLAMA,
      CHATONLM_SEED_DEMO: "1",
      CHATONLM_HF_URL: "https://huggingface.co",
      CHATONLM_HF_STATUS_URL: "https://status.huggingface.co",
    },
  },
  async (driver) => {
    await driver.waitFor("[data-testid=new-chat]");

    // Modèle de l'instance de test.
    await driver.click(await driver.find("[title='Select model']"));
    await waitUntil(
      () =>
        driver.execute(
          `const option = [...document.querySelectorAll("[role=option]")]
             .find((o) => o.title === arguments[0]);
           option?.click();
           return option !== undefined;`,
          [MODEL],
        ),
      `${MODEL} dans le sélecteur`,
    );
    await waitUntil(
      async () => (await driver.invoke("get_settings")).selectedModel === MODEL,
      `${MODEL} enregistré`,
    );

    // Thème clair imposé : « System » suit l'apparence de la session
    // virtuelle, sombre.
    await sidebar(driver, "Settings");
    await clickText(driver, "[data-testid=settings]", "Light");
    await waitUntil(
      async () => (await driver.invoke("get_settings")).theme === "light",
      "thème clair",
    );
    await sidebar(driver, "Chat");

    // Chat (clair) : une vraie réponse, raisonnement replié, bloc de code.
    await driver.waitFor("textarea[aria-label='Message']");
    await insertText(
      driver,
      "textarea[aria-label='Message']",
      "Write a Python function that checks whether a word is a palindrome, then explain in two sentences how it works.",
    );
    await driver.click(
      await driver.waitFor("button[aria-label='Send message']:not([disabled])"),
    );
    await driver.waitFor("[data-role=assistant]", { timeoutMs: 30000 });
    await replyDone(driver);
    await shot(driver, "chat-light", 1500);

    // Proofread (clair) : corrections d'un vrai modèle, deux langues.
    await sidebar(driver, "Proofread");
    await driver.waitFor("[data-testid=proofread]");
    await insertText(
      driver,
      "textarea[aria-label='Text to correct']",
      "We are going to the libary tomorow to return the books we borowed last weak.\n\nJe suis allé au cinema hier soir avec mes amis, le film etait magnifique.",
    );
    await shot(driver, "proofread-input");
    await clickText(driver, "[data-testid=proofread]", "Correct");
    await driver.waitFor("[data-testid=proofread-review]", {
      timeoutMs: 180000,
    });
    await shot(driver, "proofread-light", 1500);

    // Study (clair) : le formulaire (un modèle de 0,6 milliard voit ses
    // questions écartées par la vérification : pas de cours à montrer).
    await sidebar(driver, "Study");
    await driver.waitFor("[data-testid=study-form]");
    await insertText(
      driver,
      "[data-testid=study-form] textarea",
      "How vaccines train the immune system",
    );
    await shot(driver, "study-form-light");

    // Settings › Models (sombre) : le modèle chargé par le chat, une fois
    // partie la bulle « Saved » du changement de thème. Pas de capture de
    // l'onglet General : il montre les chemins de la machine qui l'exécute.
    await sidebar(driver, "Settings");
    await driver.waitFor("[data-testid=settings]");
    await clickText(driver, "[data-testid=settings]", "Dark");
    await waitUntil(
      async () => (await driver.invoke("get_settings")).theme === "dark",
      "thème sombre",
    );
    await clickText(driver, "body", "Models");
    await waitUntil(
      () =>
        driver.execute(
          `const page = document.querySelector("[data-testid=settings]");
           const saved = [...document.querySelectorAll("body *")]
             .some((e) => e.children.length === 0 && e.textContent === "Saved");
           return !saved && page.textContent.includes(arguments[0]);`,
          [MODEL],
        ),
      "modèle chargé listé, bulle « Saved » partie",
      20000,
    );
    await shot(driver, "settings-models-dark");

    // ModelHub (sombre) : le vrai catalogue de Hugging Face.
    await sidebar(driver, "ModelHub");
    await driver.waitFor("nav[aria-label=ModelHub]");
    await waitUntil(
      () =>
        driver.execute(
          "return document.querySelectorAll('[data-testid=model-card]').length >= 6",
        ),
      "catalogue de Hugging Face",
      60000,
    );
    await shot(driver, "hub-dark", 2500);
  },
);
