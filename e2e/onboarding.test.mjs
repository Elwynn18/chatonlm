// Premier démarrage dans l'app réelle (faux serveur, données neuves) :
// 1. bureau en français, données neuves : l'écran de la langue s'affiche à la
//    place de l'app, en français, « Français » choisi d'office ; « Español »
//    choisi : l'écran passe à l'espagnol sans rechargement et le réglage
//    Language est enregistré ; « Continuar » : l'app s'affiche avec la visite
//    guidée en espagnol. Chacune des quatre étapes ouvre sa page et met en
//    lumière son élément (zone mesurée contre l'élément, bulle entière dans
//    la fenêtre, hors de la zone) ; l'app dessous ne répond pas au clic ;
//    « Empezar » : visite fermée sur une nouvelle conversation, premier
//    démarrage enregistré, l'app répond ;
// 2. relance : l'app directement, toujours en espagnol, barre latérale fermée ;
// 3. CHATONLM_ONBOARDING=show : le premier démarrage revient, avec la langue
//    enregistrée ; au clavier (vraies touches) : Tab jusqu'au choix, flèche
//    bas (« Français » choisi et enregistré), Tab, Entrée (visite) ; Entrée
//    avance, Tab ne sort pas de la bulle, F5 ne recharge pas, Ctrl+N ne fait
//    rien, Échap ferme la visite ;
// 4. visite quittée en cours (app fermée à la deuxième étape) : le lancement
//    suivant reprend à la langue ;
// 5. CHATONLM_ONBOARDING=skip (défaut de withApp) : passé d'office.
// Captures de chaque page (.tools/shots/onboarding-*).
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { setTimeout as delay } from "node:timers/promises";
import { ROOT, withApp } from "./lib/app.mjs";
import { poll } from "./lib/desktop.mjs";
import { startFake } from "./lib/fake.mjs";
import { pressKeys } from "./lib/keyboard.mjs";

const binary = process.argv[2];

/** Texte `key` du catalogue du premier démarrage d'une langue. */
function text(lang, key) {
  const catalog = JSON.parse(
    readFileSync(`${ROOT}src/locales/${lang}/onboarding.json`, "utf8"),
  );
  const value = key.split(".").reduce((node, part) => node?.[part], catalog);
  assert.equal(typeof value, "string", `texte absent : ${lang} ${key}`);
  return value;
}

/**
 * Étapes de la visite : page ouverte et éléments mis en lumière. La nouvelle
 * conversation du lancement garde l'adresse « / » (route d'accueil masquée).
 */
const STEPS = [
  { id: "chat", path: "/c/new", targets: ['[data-tour="composer"]'] },
  { id: "hub", path: "/hub", targets: ['nav a[href="/hub"]'] },
  {
    id: "tools",
    path: "/proofread",
    targets: ['nav a[href="/proofread"]', 'nav a[href="/study"]'],
  },
  { id: "settings", path: "/settings", targets: ['nav a[href="/settings"]'] },
];
/** Bureau en français (withApp impose sinon l'anglais). */
const FR = { LANG: "fr_FR.UTF-8", LANGUAGE: "" };

const htmlLang = (driver) =>
  driver.execute("return document.documentElement.lang");
const textOf = (driver, css) =>
  driver.execute(
    "return document.querySelector(arguments[0])?.textContent ?? null",
    [css],
  );
const exists = (driver, css) =>
  driver.execute("return document.querySelector(arguments[0]) !== null", [css]);
const checked = (driver) =>
  driver.execute(
    "return document.querySelector('[data-testid=onboarding-language] input[type=radio]:checked')?.value ?? null",
  );
const settings = (driver) => driver.invoke("get_settings");
const path = (driver) => driver.execute("return location.pathname");
const focused = (driver) =>
  driver.execute("return document.activeElement?.dataset.testid ?? null");
const languageTitle = (driver) =>
  textOf(driver, "[data-testid=onboarding-language] h1");
const tourTitle = (driver) => textOf(driver, "[data-testid=tour] h2");
const waitFor = (read, expected, what) =>
  poll(read, (value) => value === expected, { what });
const press = async (driver, name) =>
  driver.click(await driver.waitFor(`[data-testid=${name}]`));
const shot = async (driver, name) => {
  await delay(400);
  await driver.screenshot(`.tools/shots/onboarding-${name}.png`);
};

/** Rectangles de la zone, de la bulle, des éléments montrés et de la fenêtre. */
const geometry = (driver, targets) =>
  driver.execute(
    `const rect = (element) => {
       if (!element) return null;
       const r = element.getBoundingClientRect();
       return { left: r.left, top: r.top, right: r.right, bottom: r.bottom };
     };
     return {
       spot: rect(document.querySelector("[data-testid=tour-spot]")),
       bubble: rect(document.querySelector("[data-testid=tour] [role=dialog]")),
       targets: arguments[0].map((css) => rect(document.querySelector(css))),
       window: { width: innerWidth, height: innerHeight },
     };`,
    [targets],
  );

/** L'étape affichée : page, textes, zone autour de ses éléments, bulle. */
async function checkStep(driver, lang, index) {
  const step = STEPS[index];
  await waitFor(
    () => tourTitle(driver),
    text(lang, `steps.${step.id}.title`),
    `titre de l'étape ${step.id}`,
  );
  await poll(
    () => path(driver),
    (value) => value === step.path || (index === 0 && value === "/"),
    { what: `page ${step.path}` },
  );
  const bubble = await textOf(driver, "[data-testid=tour] [role=dialog]");
  assert.ok(bubble.includes(text(lang, `steps.${step.id}.text`)), bubble);
  // La zone suit ses éléments (barre latérale qui s'ouvre, page qui charge) :
  // attendue stable, à 6 px autour d'eux.
  const near = (a, b) => Math.abs(a - b) <= 1.5;
  const settled = await poll(
    () => geometry(driver, step.targets),
    (g) => {
      if (!g.spot || !g.bubble || g.targets.some((t) => t === null)) {
        return false;
      }
      const left = Math.min(...g.targets.map((t) => t.left));
      const top = Math.min(...g.targets.map((t) => t.top));
      const right = Math.max(...g.targets.map((t) => t.right));
      const bottom = Math.max(...g.targets.map((t) => t.bottom));
      return (
        right > left &&
        near(g.spot.left, left - 6) &&
        near(g.spot.top, top - 6) &&
        near(g.spot.right, right + 6) &&
        near(g.spot.bottom, bottom + 6)
      );
    },
    { what: `zone de l'étape ${step.id}`, timeoutMs: 8000 },
  );
  const { spot, bubble: card, window: view } = settled;
  // Bulle entière dans la fenêtre, et hors de la zone mise en lumière.
  assert.ok(
    card.left >= 0 &&
      card.top >= 0 &&
      card.right <= view.width &&
      card.bottom <= view.height,
    JSON.stringify(settled),
  );
  assert.ok(
    card.left >= spot.right ||
      card.right <= spot.left ||
      card.top >= spot.bottom ||
      card.bottom <= spot.top,
    `bulle sur la zone : ${JSON.stringify(settled)}`,
  );
}

const fake = await startFake("ollama");
const env = { OLLAMA_HOST: fake.url };
const first = { ...env, ...FR, CHATONLM_ONBOARDING: "" };

try {
  // 1. Données neuves, bureau en français.
  await withApp(
    binary,
    { name: "onboarding-first", env: first },
    async (driver) => {
      await driver.waitFor("[data-testid=onboarding-language]");
      assert.equal(await exists(driver, "[data-testid=new-chat]"), false);
      assert.equal(await exists(driver, "[data-testid=tour]"), false);
      assert.equal(await htmlLang(driver), "fr");
      assert.equal(await languageTitle(driver), text("fr", "language.title"));
      assert.equal(await checked(driver), "fr");
      assert.deepEqual(
        await driver.execute(
          "return [...document.querySelectorAll('[data-testid=onboarding-language] label')].map((l) => l.textContent)",
        ),
        ["English", "Español", "Français"],
      );
      const before = await settings(driver);
      assert.equal(before.language, "system");
      assert.equal(before.onboardingDone, false);
      await shot(driver, "language-fr");

      // « Español » : même document, écran en espagnol, réglage enregistré.
      await driver.execute("window.__sameDocument = true");
      await driver.click(
        await driver.find("[data-testid=onboarding-language] input[value=es]"),
      );
      await waitFor(
        () => languageTitle(driver),
        text("es", "language.title"),
        "écran en espagnol",
      );
      assert.equal(await htmlLang(driver), "es");
      assert.equal(await checked(driver), "es");
      assert.equal((await settings(driver)).language, "es");
      await shot(driver, "language-es");

      // « Continuar » : l'app, avec la visite, barre latérale ouverte.
      await press(driver, "onboarding-continue");
      await driver.waitFor("[data-testid=tour]");
      await driver.waitFor("[data-testid=new-chat]");
      assert.equal(
        await exists(driver, "[data-testid=onboarding-language]"),
        false,
      );
      assert.equal((await settings(driver)).onboardingDone, false);
      for (const [index, step] of STEPS.entries()) {
        await checkStep(driver, "es", index);
        const bubble = await textOf(driver, "[data-testid=tour] [role=dialog]");
        assert.ok(bubble.includes(`Paso ${index + 1} de 4`), bubble);
        await shot(driver, `tour-${index + 1}-${step.id}-es`);
        if (index === 0) {
          // L'app dessous ne répond pas : ce qui est sous le pointeur, à
          // l'endroit de l'entrée Configuración comme de la zone de saisie,
          // c'est le voile de la visite ; la zone de saisie ne prend pas le
          // focus.
          assert.deepEqual(
            await driver.execute(
              `return ['nav a[href="/settings"]', "textarea"].map((css) => {
                 const r = document.querySelector(css).getBoundingClientRect();
                 const hit = document.elementFromPoint(
                   r.left + r.width / 2,
                   r.top + r.height / 2,
                 );
                 return hit?.closest("[data-testid=tour]") !== null;
               })`,
            ),
            [true, true],
          );
          await driver.execute("document.querySelector('textarea').focus()");
          assert.notEqual(
            await driver.execute("return document.activeElement?.tagName"),
            "TEXTAREA",
          );
          assert.equal(await exists(driver, "[data-testid=tour-back]"), false);
        }
        if (index < STEPS.length - 1) await press(driver, "tour-next");
      }
      // Dernière étape : ni « Siguiente » ni « Omitir » ; retour, puis encore.
      assert.equal(await exists(driver, "[data-testid=tour-next]"), false);
      assert.equal(await exists(driver, "[data-testid=tour-skip]"), false);
      await press(driver, "tour-back");
      await checkStep(driver, "es", 2);
      await press(driver, "tour-next");
      await checkStep(driver, "es", 3);

      await press(driver, "tour-start");
      await poll(
        () => exists(driver, "[data-testid=tour]"),
        (shown) => !shown,
        { what: "visite fermée" },
      );
      await driver.waitFor("[data-testid=new-chat]");
      assert.equal(await path(driver), "/c/new");
      assert.equal(await htmlLang(driver), "es");
      const after = await settings(driver);
      assert.equal(after.language, "es");
      assert.equal(after.effectiveLanguage, "es");
      assert.equal(after.onboardingDone, true);
      assert.equal(await driver.execute("return window.__sameDocument"), true);
      // L'app répond : la zone de saisie prend le focus.
      await driver.execute("document.querySelector('textarea').focus()");
      assert.equal(
        await driver.execute("return document.activeElement?.tagName"),
        "TEXTAREA",
      );
      await shot(driver, "after-es");
    },
  );

  // 2. Relance : plus de premier démarrage, langue gardée.
  await withApp(
    binary,
    { name: "onboarding-again", env: first },
    async (driver) => {
      await driver.waitFor("[data-testid=new-chat]");
      await delay(500);
      assert.equal(
        await exists(driver, "[data-testid=onboarding-language]"),
        false,
      );
      assert.equal(await exists(driver, "[data-testid=tour]"), false);
      assert.equal(await htmlLang(driver), "es");
      // Barre latérale fermée au lancement, comme toujours.
      assert.equal(await exists(driver, "nav a[href='/hub']"), false);
    },
  );

  // 3. Premier démarrage redemandé ; parcours au clavier.
  await withApp(
    binary,
    {
      name: "onboarding-show",
      env: { ...env, ...FR, CHATONLM_ONBOARDING: "show" },
    },
    async (driver) => {
      await driver.waitFor("[data-testid=onboarding-language]");
      assert.equal(await languageTitle(driver), text("es", "language.title"));
      assert.equal(await checked(driver), "es");
      assert.equal((await settings(driver)).onboardingDone, false);
      // Le titre a le focus à l'arrivée.
      assert.equal(
        await driver.execute("return document.activeElement?.tagName"),
        "H1",
      );

      // Tab : le choix coché ; flèche bas : « Français », enregistré.
      await pressKeys("tab");
      assert.equal(
        await driver.execute("return document.activeElement?.value ?? null"),
        "es",
      );
      await shot(driver, "language-focus");
      await pressKeys("down", { settleMs: 400 });
      await waitFor(
        () => languageTitle(driver),
        text("fr", "language.title"),
        "écran en français",
      );
      assert.equal(await checked(driver), "fr");
      assert.equal((await settings(driver)).language, "fr");
      // Tab : « Continuer » ; Entrée : la visite, son bouton principal en main.
      await pressKeys("tab");
      assert.equal(await focused(driver), "onboarding-continue");
      await pressKeys("enter", { settleMs: 400 });
      await driver.waitFor("[data-testid=tour]");
      await checkStep(driver, "fr", 0);
      assert.equal(await focused(driver), "tour-next");
      await shot(driver, "tour-1-chat-fr");

      // Entrée avance ; Tab et Maj+Tab ne sortent pas de la bulle.
      await pressKeys("enter", { settleMs: 400 });
      await checkStep(driver, "fr", 1);
      assert.equal(await focused(driver), "tour-next");
      await pressKeys("tab");
      assert.equal(await focused(driver), "tour-skip");
      await pressKeys("tab");
      assert.equal(await focused(driver), "tour-back");
      await pressKeys("shift+tab");
      await pressKeys("shift+tab");
      assert.equal(await focused(driver), "tour-next");
      await shot(driver, "tour-2-hub-fr");

      // F5 ne recharge pas la page ; Ctrl+N n'ouvre pas de conversation.
      await driver.execute("window.__sameDocument = true");
      await pressKeys("f5", { settleMs: 500 });
      assert.equal(await driver.execute("return window.__sameDocument"), true);
      await pressKeys("ctrl+n", { settleMs: 400 });
      assert.equal(await path(driver), "/hub");
      assert.equal(await exists(driver, "[data-testid=tour]"), true);

      // Échap ferme la visite, sur une nouvelle conversation.
      await pressKeys("escape", { settleMs: 400 });
      await poll(
        () => exists(driver, "[data-testid=tour]"),
        (shown) => !shown,
        { what: "visite fermée par Échap" },
      );
      await waitFor(() => path(driver), "/c/new", "nouvelle conversation");
      assert.equal(await htmlLang(driver), "fr");
      assert.equal((await settings(driver)).onboardingDone, true);
    },
  );

  // 4. Visite quittée en cours : le lancement suivant reprend à la langue.
  await withApp(
    binary,
    {
      name: "onboarding-left",
      env: { ...env, ...FR, CHATONLM_ONBOARDING: "show" },
    },
    async (driver) => {
      await press(driver, "onboarding-continue");
      await checkStep(driver, "fr", 0);
      await press(driver, "tour-next");
      await checkStep(driver, "fr", 1);
      assert.equal((await settings(driver)).onboardingDone, false);
    },
  );
  await withApp(
    binary,
    { name: "onboarding-resumed", env: first },
    async (driver) => {
      await driver.waitFor("[data-testid=onboarding-language]");
      assert.equal(await checked(driver), "fr");
      assert.equal(await exists(driver, "[data-testid=tour]"), false);
    },
  );

  // 5. Passé d'office (défaut de withApp), bien qu'il soit à refaire.
  await withApp(
    binary,
    { name: "onboarding-skip", env: { ...env, ...FR } },
    async (driver) => {
      await driver.waitFor("[data-testid=new-chat]");
      assert.equal(
        await exists(driver, "[data-testid=onboarding-language]"),
        false,
      );
      assert.equal(await exists(driver, "[data-testid=tour]"), false);
      assert.equal((await settings(driver)).onboardingDone, true);
    },
  );
  console.log("onboarding : OK");
} finally {
  await fake.stop();
}
