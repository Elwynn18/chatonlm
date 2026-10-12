// Langues de l'interface dans l'app réelle (faux serveur, faux bureau de la
// session virtuelle : notifications et zone de notification) :
// 1. bureau en français (LANG=fr_FR.UTF-8), réglage « System » : page en
//    français dès le premier affichage (<html lang>, composer, navigation),
//    menu et infobulle de la zone de notification en français ;
// 2. « Español » dans Settings › General : page en espagnol sans
//    rechargement (même document), réglage enregistré, menu de la zone de
//    notification refait en espagnol, notification de la première fermeture
//    en espagnol ;
// 3. relance, bureau toujours en français : l'espagnol est gardé ; « System »
//    rend le français ;
// 4. bureau en allemand (LANGUAGE=de, non proposée) : anglais.
// Captures de chaque écran en français et en espagnol (.tools/shots/i18n-*),
// pour relire la mise en page des textes plus longs.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { ROOT, withApp } from "./lib/app.mjs";
import { poll, startDesktopServices } from "./lib/desktop.mjs";
import { startFake } from "./lib/fake.mjs";
import { forWindows, runKWinScript } from "./lib/kwin.mjs";

const binary = process.argv[2];
const delay = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

/** Texte `key` (« espace.groupe.nom ») du catalogue d'une langue. */
function text(lang, key) {
  const [namespace, ...path] = key.split(".");
  const catalog = JSON.parse(
    readFileSync(`${ROOT}src/locales/${lang}/${namespace}.json`, "utf8"),
  );
  const value = path.reduce((node, part) => node?.[part], catalog);
  assert.equal(typeof value, "string", `texte absent : ${lang} ${key}`);
  return value;
}

/** Bureau en français (withApp impose sinon l'anglais). */
const FR = { LANG: "fr_FR.UTF-8", LANGUAGE: "" };

const htmlLang = (driver) =>
  driver.execute("return document.documentElement.lang");
const attribute = (driver, css, name) =>
  driver.execute(
    "return document.querySelector(arguments[0])?.getAttribute(arguments[1]) ?? null",
    [css, name],
  );
const textOf = (driver, css) =>
  driver.execute(
    "return document.querySelector(arguments[0])?.textContent ?? null",
    [css],
  );
const labels = (menu) =>
  menu.filter((item) => item.type !== "separator").map((item) => item.label);
const closeWindow = () =>
  runKWinScript(forWindows("ChatonLM", "w.closeWindow();"));

/** Ouvre la barre latérale si elle est repliée (libellé dans `lang`). */
async function openSidebar(driver, lang) {
  const show = `button[aria-label='${text(lang, "chat.layout.showSidebar")}']`;
  if (
    await driver.execute(
      "return document.querySelector(arguments[0]) !== null",
      [show],
    )
  ) {
    await driver.click(await driver.find(show));
  }
  await driver.waitFor("nav a[href='/settings']");
}

/** Le chat neuf affiché. */
async function toChat(driver, lang) {
  await openSidebar(driver, lang);
  await driver.click(await driver.waitFor("nav a[href$='/c/new']"));
  await driver.waitFor("[data-testid=new-chat]");
}

/** Captures des écrans principaux dans la langue affichée. */
async function screens(driver, lang) {
  await toChat(driver, lang);
  await delay(400);
  await driver.screenshot(`.tools/shots/i18n-${lang}-chat.png`);
  await driver.click(await driver.waitFor("nav a[href='/settings']"));
  await driver.waitFor("[data-testid=settings]");
  await delay(400);
  await driver.screenshot(`.tools/shots/i18n-${lang}-settings.png`);
  for (const tab of ["models", "account", "dictation"]) {
    await driver.click(
      await driver.waitFor(`[data-testid=settings-tab-${tab}]`),
    );
    await delay(600);
    await driver.screenshot(`.tools/shots/i18n-${lang}-settings-${tab}.png`);
  }
  await driver.click(await driver.waitFor("nav a[href='/hub']"));
  await delay(800);
  await driver.screenshot(`.tools/shots/i18n-${lang}-hub.png`);
  await driver.click(await driver.waitFor("a[href='/hub/transcription']"));
  await delay(600);
  await driver.screenshot(`.tools/shots/i18n-${lang}-hub-transcription.png`);
}

/** Ouvre Settings › General et choisit la langue `value`. */
async function chooseLanguage(driver, value, lang) {
  await openSidebar(driver, lang);
  await driver.click(await driver.waitFor("nav a[href='/settings']"));
  await driver.click(
    await driver.waitFor("[data-testid=settings-tab-general]"),
  );
  const radio = await driver.waitFor(
    `[data-testid=settings] input[type=radio][value='${value}']`,
  );
  await driver.click(radio);
  await poll(
    async () => (await driver.invoke("get_settings")).language,
    (language) => language === value,
    { what: `réglage ${value}` },
  );
}

const fake = await startFake("ollama");
const env = { OLLAMA_HOST: fake.url, CHATONLM_SEED_DEMO: "1" };

try {
  const desk = await startDesktopServices({
    notifications: true,
    watcher: true,
    portal: null,
  });
  try {
    // 1-2. Bureau en français, puis l'espagnol choisi.
    await withApp(
      binary,
      { name: "i18n-fr", env: { ...env, ...FR }, quit: "command" },
      async (driver) => {
        await driver.waitFor("[data-testid=new-chat]");
        // Dès le premier affichage (langue lue avant le rendu).
        assert.equal(await htmlLang(driver), "fr");
        assert.equal(
          await attribute(driver, "textarea", "aria-label"),
          text("fr", "chat.composer.label"),
        );
        await openSidebar(driver, "fr");
        assert.equal(
          await textOf(driver, "nav a[href='/settings']"),
          text("fr", "chat.nav.settings"),
        );
        const settings = await driver.invoke("get_settings");
        assert.equal(settings.language, "system");
        assert.equal(settings.effectiveLanguage, "fr");

        // Zone de notification en français.
        await desk.waitFor((e) => e.event === "item-registered");
        let menu = await desk.command("menu");
        assert.ok(
          labels(menu).includes(text("fr", "backend.tray.show")),
          JSON.stringify(menu),
        );
        assert.ok(labels(menu).includes(text("fr", "backend.tray.quit")));
        assert.equal(
          (await desk.command("item-props")).tooltip.description,
          text("fr", "backend.tray.ready"),
        );
        await screens(driver, "fr");

        // Espagnol : même document (pas de rechargement), tout change.
        await driver.execute("window.__sameDocument = true");
        await chooseLanguage(driver, "es", "fr");
        await poll(
          () => htmlLang(driver),
          (lang) => lang === "es",
          {
            what: "page en espagnol",
          },
        );
        assert.equal(
          await driver.execute("return window.__sameDocument === true"),
          true,
          "page rechargée",
        );
        assert.equal(await textOf(driver, "h1"), text("es", "settings.title"));
        assert.equal(
          (await driver.invoke("get_settings")).effectiveLanguage,
          "es",
        );
        menu = await poll(
          () => desk.command("menu"),
          (current) =>
            labels(current).includes(text("es", "backend.tray.show")),
          { what: "menu de la zone de notification en espagnol" },
        );
        assert.ok(labels(menu).includes(text("es", "backend.tray.quit")));
        assert.equal(
          (await desk.command("item-props")).tooltip.description,
          text("es", "backend.tray.ready"),
        );
        await screens(driver, "es");

        // Première fermeture : notification en espagnol.
        await closeWindow();
        const hint = await desk.waitFor(
          (e) =>
            e.event === "notify" &&
            e.summary === text("es", "backend.tray.hint.summary"),
        );
        assert.equal(hint.body, text("es", "backend.tray.hint.body"));
        await desk.command("menu-click", {
          label: text("es", "backend.tray.show"),
        });
        await poll(
          () => driver.invoke("plugin:window|is_visible", { label: "main" }),
          (visible) => visible === true,
          { what: "fenêtre montrée" },
        );
      },
    );

    // 3. Relance : l'espagnol est gardé ; System rend le français.
    await withApp(
      binary,
      { name: "i18n-relaunch", env: { ...env, ...FR }, quit: "command" },
      async (driver) => {
        await driver.waitFor("[data-testid=new-chat]");
        assert.equal(await htmlLang(driver), "es");
        assert.equal(
          await attribute(driver, "textarea", "aria-label"),
          text("es", "chat.composer.label"),
        );
        await desk.waitFor((e) => e.event === "item-registered");
        await chooseLanguage(driver, "system", "es");
        await poll(
          () => htmlLang(driver),
          (lang) => lang === "fr",
          {
            what: "retour au français du bureau",
          },
        );
        await poll(
          () => desk.command("menu"),
          (current) =>
            labels(current).includes(text("fr", "backend.tray.show")),
          { what: "menu de nouveau en français" },
        );
      },
    );
  } finally {
    await desk.stop();
  }

  // 4. Langue du bureau non proposée : anglais.
  await withApp(
    binary,
    {
      name: "i18n-de",
      env: { ...env, LANG: "de_DE.UTF-8", LANGUAGE: "de" },
    },
    async (driver) => {
      await driver.waitFor("[data-testid=new-chat]");
      assert.equal(await htmlLang(driver), "en");
      assert.equal(
        await attribute(driver, "textarea", "aria-label"),
        text("en", "chat.composer.label"),
      );
    },
  );
} finally {
  await fake.stop();
}

console.log("i18n : OK");
