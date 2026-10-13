// Premier démarrage dans l'app réelle (faux serveur, données neuves) :
// 1. bureau en français, données neuves : l'écran de la langue s'affiche à la
//    place de l'app, en français, « Français » choisi d'office ; « Español »
//    choisi : l'écran passe à l'espagnol sans rechargement et le réglage
//    Language est enregistré ; les quatre étapes de la présentation, puis
//    « Empezar » : l'app s'affiche en espagnol, premier démarrage enregistré ;
// 2. relance : l'app directement, toujours en espagnol ;
// 3. CHATONLM_ONBOARDING=show : le premier démarrage revient, avec la langue
//    enregistrée ; au clavier (vraies touches) : Tab jusqu'au choix, flèche
//    bas (« Français » choisi et enregistré), Tab, Entrée (étape 1) ; « Passer »
//    ferme la présentation ;
// 4. CHATONLM_ONBOARDING=skip (défaut de withApp) sur des données où il est à
//    refaire : passé d'office.
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

const STEPS = ["chat", "hub", "tools", "voice"];
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
    "return document.querySelector('[data-testid=onboarding] input[type=radio]:checked')?.value ?? null",
  );
const settings = (driver) => driver.invoke("get_settings");
const title = (driver) => textOf(driver, "[data-testid=onboarding] h1");
const waitTitle = (driver, expected) =>
  poll(
    () => title(driver),
    (value) => value === expected,
    { what: `titre « ${expected} »` },
  );
const press = async (driver, name) =>
  driver.click(await driver.waitFor(`[data-testid=onboarding-${name}]`));
const shot = async (driver, name) => {
  await delay(300);
  await driver.screenshot(`.tools/shots/onboarding-${name}.png`);
};

const fake = await startFake("ollama");
const env = { OLLAMA_HOST: fake.url };

try {
  // 1. Données neuves, bureau en français.
  await withApp(
    binary,
    {
      name: "onboarding-first",
      env: { ...env, ...FR, CHATONLM_ONBOARDING: "" },
    },
    async (driver) => {
      await driver.waitFor("[data-testid=onboarding]");
      assert.equal(await exists(driver, "[data-testid=new-chat]"), false);
      assert.equal(await exists(driver, "textarea"), false);
      assert.equal(await htmlLang(driver), "fr");
      assert.equal(await title(driver), text("fr", "language.title"));
      assert.equal(await checked(driver), "fr");
      assert.deepEqual(
        await driver.execute(
          "return [...document.querySelectorAll('[data-testid=onboarding] label')].map((l) => l.textContent)",
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
        await driver.find("[data-testid=onboarding] input[value=es]"),
      );
      await waitTitle(driver, text("es", "language.title"));
      assert.equal(await htmlLang(driver), "es");
      assert.equal(await checked(driver), "es");
      assert.equal((await settings(driver)).language, "es");
      assert.equal(await driver.execute("return window.__sameDocument"), true);
      await shot(driver, "language-es");

      await press(driver, "continue");
      for (const [index, step] of STEPS.entries()) {
        await waitTitle(driver, text("es", `steps.${step}.title`));
        const page = await textOf(driver, "[data-testid=onboarding]");
        assert.ok(page.includes(text("es", `steps.${step}.text`)), page);
        assert.ok(page.includes(`Paso ${index + 1} de 4`), page);
        await shot(driver, `step-${index + 1}-es`);
        if (index < STEPS.length - 1) await press(driver, "next");
      }
      // Dernière étape : ni « Siguiente » ni « Omitir ».
      assert.equal(
        await exists(driver, "[data-testid=onboarding-next]"),
        false,
      );
      assert.equal(
        await exists(driver, "[data-testid=onboarding-skip]"),
        false,
      );
      // Retour, puis de nouveau la dernière.
      await press(driver, "back");
      await waitTitle(driver, text("es", "steps.tools.title"));
      assert.equal((await settings(driver)).onboardingDone, false);
      await press(driver, "next");
      await waitTitle(driver, text("es", "steps.voice.title"));

      await press(driver, "start");
      await driver.waitFor("[data-testid=new-chat]");
      assert.equal(await exists(driver, "[data-testid=onboarding]"), false);
      assert.equal(await htmlLang(driver), "es");
      const after = await settings(driver);
      assert.equal(after.language, "es");
      assert.equal(after.effectiveLanguage, "es");
      assert.equal(after.onboardingDone, true);
      assert.equal(await driver.execute("return window.__sameDocument"), true);
    },
  );

  // 2. Relance : plus de premier démarrage, langue gardée.
  await withApp(
    binary,
    {
      name: "onboarding-again",
      env: { ...env, ...FR, CHATONLM_ONBOARDING: "" },
    },
    async (driver) => {
      await driver.waitFor("[data-testid=new-chat]");
      assert.equal(await exists(driver, "[data-testid=onboarding]"), false);
      assert.equal(await htmlLang(driver), "es");
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
      await driver.waitFor("[data-testid=onboarding]");
      assert.equal(await title(driver), text("es", "language.title"));
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
      await waitTitle(driver, text("fr", "language.title"));
      assert.equal(await checked(driver), "fr");
      assert.equal((await settings(driver)).language, "fr");
      // Tab : « Continuer » ; Entrée : première étape.
      await pressKeys("tab");
      assert.equal(
        await driver.execute(
          "return document.activeElement?.dataset.testid ?? null",
        ),
        "onboarding-continue",
      );
      await pressKeys("enter", { settleMs: 400 });
      await waitTitle(driver, text("fr", "steps.chat.title"));
      await shot(driver, "step-1-fr");

      // F5 ne recharge pas la page (garde de navigation tenue ici aussi).
      await driver.execute("window.__sameDocument = true");
      await pressKeys("f5", { settleMs: 500 });
      assert.equal(await driver.execute("return window.__sameDocument"), true);
      assert.equal(await title(driver), text("fr", "steps.chat.title"));

      await press(driver, "skip");
      await driver.waitFor("[data-testid=new-chat]");
      assert.equal(await htmlLang(driver), "fr");
      assert.equal((await settings(driver)).onboardingDone, true);
      // Réglage à refaire pour le dernier lancement.
      await driver.invoke("update_settings", {
        patch: { onboardingDone: false },
      });
    },
  );

  // 4. Passé d'office (défaut de withApp), bien qu'il soit à refaire.
  await withApp(
    binary,
    { name: "onboarding-skip", env: { ...env, ...FR } },
    async (driver) => {
      await driver.waitFor("[data-testid=new-chat]");
      assert.equal(await exists(driver, "[data-testid=onboarding]"), false);
      assert.equal((await settings(driver)).onboardingDone, true);
    },
  );
  console.log("onboarding : OK");
} finally {
  await fake.stop();
}
