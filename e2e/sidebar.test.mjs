// Barre latérale (propre à ChatonLM), avec les conversations de démonstration
// (CHATONLM_SEED_DEMO) et un vrai pointeur (e2e/lib/pointer.mjs) :
// - l'historique reste affiché sur ModelHub et Settings, l'entrée de la page
//   surlignée, aucune conversation surlignée ; une conversation s'ouvre
//   depuis ces pages ;
// - au survol d'une ligne (vrai pointeur) : étoile et poubelle visibles,
//   cachées une fois le pointeur parti ; visibles aussi avec le focus
//   clavier dans la ligne (Tab) ;
// - étoile : la conversation passe dans « Favorites » en tête, sans changer
//   de date, étoile pleine gardée visible, toujours là après un
//   rechargement ; retirée par le menu contextuel (« Remove from
//   favorites ») ;
// - poubelle : même confirmation que le menu contextuel ; Cancel garde la
//   conversation, OK la supprime ;
// - aucune violation de la CSP ni erreur ou avertissement dans la console.
import assert from "node:assert/strict";
import { withApp } from "./lib/app.mjs";
import { startFake } from "./lib/fake.mjs";
import { PENDING_CALLS } from "./lib/page.mjs";
import { pressKeys } from "./lib/keyboard.mjs";
import { centerOf, pointerOffset } from "./lib/pointer.mjs";

const binary = process.argv[2];
const fake = await startFake("ollama");

const delay = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

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

/** Recharge la page une fois les appels au backend finis (voir thinking.test.mjs). */
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

async function showSidebar(driver) {
  if (await exists(driver, "button[aria-label='Show sidebar']")) {
    await driver.click(await driver.find("button[aria-label='Show sidebar']"));
  }
}

/** Groupes affichés : titre et libellés des conversations, dans l'ordre. */
const shownGroups = (driver) =>
  driver.execute(`
    return [...document.querySelectorAll("nav h3")].map((heading) => [
      heading.textContent,
      [...heading.parentElement.querySelectorAll("[data-chat-id]")].map(
        (row) => row.dataset.chatId,
      ),
    ]);`);

const row = (id) => `[data-chat-id='${id}']`;
const star = (id) => `${row(id)} [data-testid=chat-favorite]`;
const trash = (id) => `${row(id)} [data-testid=chat-delete]`;

/** Élément réellement visible (affiché, non caché, opacité non nulle). */
const visible = (driver, css) =>
  driver.execute(
    `const element = document.querySelector(arguments[0]);
     if (!element) return false;
     const box = element.getBoundingClientRect();
     if (box.width === 0 || box.height === 0) return false;
     for (let e = element; e; e = e.parentElement) {
       const style = getComputedStyle(e);
       if (style.display === "none" || style.visibility === "hidden" ||
           Number(style.opacity) === 0) return false;
     }
     return true;`,
    [css],
  );

/** Entrée de menu ou bouton désigné par son texte. */
const clickText = (driver, css, text) =>
  driver.execute(
    `const target = [...document.querySelectorAll(arguments[0])]
       .find((element) => element.textContent === arguments[1]);
     if (!target) throw new Error("introuvable : " + arguments[1]);
     target.click();`,
    [css, text],
  );

/** Clic droit au milieu de la ligne d'une conversation. */
const rightClick = (driver, id) =>
  driver.execute(
    `const row = document.querySelector(arguments[0]);
     const box = row.getBoundingClientRect();
     row.dispatchEvent(new MouseEvent("contextmenu", {
       bubbles: true, cancelable: true, button: 2,
       clientX: box.left + 24, clientY: box.top + box.height / 2,
     }));`,
    [row(id)],
  );

/** Classe de l'entrée de navigation (surlignage de la page courante). */
const navClass = (driver, href) =>
  driver.execute(
    "return document.querySelector(arguments[0])?.className ?? null",
    [`nav a[href='${href}']`],
  );

const highlightedRows = (driver) =>
  driver.execute(
    `return [...document.querySelectorAll("[data-chat-id]")]
       .filter((row) => row.className.split(/\\s+/).includes("bg-neutral-200"))
       .map((row) => row.dataset.chatId);`,
  );

try {
  await withApp(
    binary,
    {
      name: "sidebar",
      env: { OLLAMA_HOST: fake.url, CHATONLM_SEED_DEMO: "1" },
    },
    async (driver) => {
      await driver.waitFor("[data-testid=new-chat]");
      await driver.execute(WATCH_PAGE);
      await showSidebar(driver);
      await driver.waitFor("[data-chat-id]");
      const chats = await driver.invoke("list_chats");
      assert.equal(chats.length, 6);
      assert.ok(chats.every((chat) => chat.favorite === false));
      const [first, second, third] = chats;

      // --- Historique sur ModelHub et Settings.
      for (const href of ["/hub", "/settings"]) {
        await driver.click(await driver.waitFor(`nav a[href='${href}']`));
        await waitUntil(
          async () =>
            ((await navClass(driver, href)) ?? "")
              .split(/\s+/)
              .includes("bg-accent"),
          `entrée ${href} surlignée`,
        );
        const ids = (await shownGroups(driver)).flatMap(([, rows]) => rows);
        assert.equal(ids.length, 6, `historique sur ${href}`);
        assert.deepEqual(await highlightedRows(driver), []);
      }
      await driver.screenshot(".tools/shots/sidebar-settings.png");
      // Une conversation s'ouvre depuis les réglages.
      await driver.click(await driver.find(`${row(second.id)} a`));
      await driver.waitFor("[data-testid=chat]");
      assert.deepEqual(await highlightedRows(driver), [second.id]);

      // --- Survol au vrai pointeur : étoile et poubelle.
      await driver.click(await driver.find("nav a[href='/settings']"));
      await driver.waitFor("[data-testid=settings]");
      await delay(400); // transitions de la barre et de la page
      const [dx, dy] = await pointerOffset(driver);
      assert.equal(await visible(driver, star(third.id)), false);
      assert.equal(await visible(driver, trash(third.id)), false);
      const [x, y] = await centerOf(driver, row(third.id));
      // Par petits pas (un saut unique ne met pas toujours à jour :hover
      // sous WebKitGTK), depuis la droite de la ligne.
      let at = [x + 300, y];
      const moveTo = async (to) => {
        const path = [];
        for (let i = 1; i <= 10; i++) {
          const px = Math.round(at[0] + ((to[0] - at[0]) * i) / 10);
          const py = Math.round(at[1] + ((to[1] - at[1]) * i) / 10);
          path.push(`pointer:${px + dx},${py + dy}`, "wait:30");
        }
        await pressKeys(path, { settleMs: 300 });
        at = to;
      };
      await moveTo([x, y]);
      let hoverState = null;
      await waitUntil(
        async () =>
          (await visible(driver, star(third.id))) &&
          (await visible(driver, trash(third.id))),
        async () =>
          `étoile et poubelle au survol (${JSON.stringify(hoverState)})`,
      ).catch(async (error) => {
        hoverState = await driver.execute(
          `const row = document.querySelector(arguments[0]);
           const button = row.querySelector("[data-testid=chat-delete]");
           return { hoverMedia: matchMedia("(hover: hover)").matches,
             anyHover: matchMedia("(any-hover: hover)").matches,
             rowHovered: row.matches(":hover"),
             under: document.elementFromPoint(arguments[1], arguments[2])?.outerHTML.slice(0, 120),
             opacity: getComputedStyle(button.parentElement).opacity + "/" + getComputedStyle(button).opacity };`,
          [row(third.id), x, y],
        );
        await driver.screenshot(".tools/shots/sidebar-hover-fail.png");
        throw new Error(`${error.message} ${JSON.stringify(hoverState)}`);
      });
      assert.equal(await visible(driver, star(first.id)), false);
      await driver.screenshot(".tools/shots/sidebar-hover.png");
      // Pointeur parti : cachées.
      await moveTo(await centerOf(driver, "[data-testid=settings]"));
      await waitUntil(
        async () => !(await visible(driver, trash(third.id))),
        "poubelle cachée hors survol",
      );
      assert.equal(await visible(driver, star(third.id)), false);
      // Focus clavier dans la ligne (vraie touche Tab, du lien à l'étoile) :
      // visibles aussi. Un simple clic sur le lien ne les montre pas.
      await driver.execute("document.querySelector(arguments[0]).focus()", [
        `${row(first.id)} a`,
      ]);
      await pressKeys("tab");
      await waitUntil(
        async () =>
          (await visible(driver, star(first.id))) &&
          (await visible(driver, trash(first.id))),
        "étoile et poubelle avec le focus clavier dans la ligne",
      );
      await driver.execute("document.activeElement.blur()");

      // --- Étoile, au vrai clic de souris : rubrique Favorites en tête, date
      // inchangée ; nom fixe « Favorite » avec aria-pressed, infobulle
      // variable.
      const starState = (id) =>
        driver.execute(
          `const button = document.querySelector(arguments[0]);
           return [button.getAttribute("aria-label"), button.getAttribute("aria-pressed"), button.title];`,
          [star(id)],
        );
      assert.deepEqual(await starState(third.id), [
        "Favorite",
        "false",
        "Add to favorites",
      ]);
      await moveTo(await centerOf(driver, row(third.id)));
      await moveTo(await centerOf(driver, star(third.id)));
      await driver.execute(
        `window.__clicks = [];
         document.addEventListener("click", (e) => window.__clicks.push(
           [e.target.outerHTML.slice(0, 80), e.detail, e.clientX, e.clientY]), true);`,
      );
      const starAt = await centerOf(driver, star(third.id));
      // Bouton enfoncé dans le même envoi que le déplacement (sinon le clic
      // n'arrive pas à la fenêtre).
      await pressKeys(
        [
          `pointer:${starAt[0] + dx},${starAt[1] + dy}`,
          "wait:100",
          "button:down",
          "wait:100",
          "button:up",
        ],
        { settleMs: 300 },
      );
      await waitUntil(
        async () => (await shownGroups(driver))[0]?.[0] === "Favorites",
        "rubrique Favorites",
      ).catch(async (error) => {
        const clicks = await driver.execute("return window.__clicks");
        throw new Error(
          `${error.message} ${JSON.stringify({ starAt, at, clicks })}`,
        );
      });
      assert.deepEqual((await shownGroups(driver))[0], [
        "Favorites",
        [third.id],
      ]);
      assert.deepEqual(await starState(third.id), [
        "Favorite",
        "true",
        "Remove from favorites",
      ]);
      // Pointeur parti : pas de poubelle laissée visible par le clic (focus
      // de souris, sans :focus-visible).
      await moveTo(await centerOf(driver, "[data-testid=settings]"));
      await waitUntil(
        async () => !(await visible(driver, trash(third.id))),
        "poubelle cachée après le clic",
      );
      // Étoile pleine gardée visible sans survol.
      assert.equal(await visible(driver, star(third.id)), true);
      assert.equal(await visible(driver, trash(third.id)), false);
      const saved = (await driver.invoke("list_chats")).find(
        (chat) => chat.id === third.id,
      );
      assert.equal(saved.favorite, true);
      assert.equal(saved.updatedAt, third.updatedAt);
      await reload(driver);
      // Barre repliée au rechargement : attendre son bouton avant de l'ouvrir.
      await driver.waitFor("[data-testid=settings]");
      await driver.click(
        await driver.waitFor("button[aria-label='Show sidebar']"),
      );
      let groups = null;
      await waitUntil(
        async () =>
          (groups = await shownGroups(driver))[0]?.[0] === "Favorites",
        () =>
          `Favorites après rechargement (${JSON.stringify(groups)}, ${JSON.stringify(
            saved,
          )})`,
      ).catch(async (error) => {
        await driver.screenshot(".tools/shots/sidebar-reload-fail.png");
        const listed = await driver.invoke("list_chats");
        throw new Error(
          `${error.message} ${JSON.stringify(listed.map((c) => [c.id, c.favorite]))} ${await driver.execute("return location.href")}`,
        );
      });
      await delay(600); // animation d'ouverture de la barre (300 ms)
      await driver.screenshot(".tools/shots/sidebar-favorites.png");

      // Retirée par le menu contextuel.
      await rightClick(driver, third.id);
      await driver.waitFor("[role=menu]");
      await clickText(driver, "[role=menuitem]", "Remove from favorites");
      await waitUntil(
        async () =>
          !(await shownGroups(driver)).some(([name]) => name === "Favorites"),
        "rubrique Favorites retirée",
      );
      assert.equal(
        (await driver.invoke("list_chats")).find((c) => c.id === third.id)
          .favorite,
        false,
      );

      // --- Poubelle : confirmation, Cancel puis OK.
      await driver.click(await driver.find(trash(first.id)));
      const dialog = await driver.waitFor("[role=alertdialog]");
      assert.match(await driver.text(dialog), /remove this chat/);
      await clickText(driver, "[role=alertdialog] button", "Cancel");
      await waitUntil(
        async () => !(await exists(driver, "[role=alertdialog]")),
        "confirmation fermée",
      );
      assert.equal((await driver.invoke("list_chats")).length, 6);
      await driver.click(await driver.find(trash(first.id)));
      await driver.waitFor("[role=alertdialog]");
      await clickText(driver, "[role=alertdialog] button", "OK");
      await waitUntil(
        async () => !(await exists(driver, row(first.id))),
        "conversation supprimée",
      );
      assert.equal((await driver.invoke("list_chats")).length, 5);
      // Toujours sur les réglages.
      assert.equal(await exists(driver, "[data-testid=settings]"), true);

      await collectViolations(driver);
    },
  );
  assert.deepEqual(violations, [], "violations de la CSP");
  assert.deepEqual(consoleErrors, [], "erreurs dans la console");
  assert.deepEqual(consoleWarnings, [], "avertissements dans la console");
  console.log("OK : barre latérale");
} finally {
  await fake.stop();
}
