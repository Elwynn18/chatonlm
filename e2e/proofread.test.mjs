// Page Proofread et onglet « Keyboard shortcuts » dans l'app réelle (faux
// serveur ; modèle de la correction choisi par le réglage `proofreadModel`) :
// - entrée « Proofread » de la barre latérale, entre Chat et Study,
//   surlignée sur sa page ; volet vide, boutons désactivés (capture) ;
// - Correct par Ctrl+Entrée (vraies touches) avec `chatonlm-proofread` : deux
//   passages, revue avec mots supprimés et ajoutés, « 3 corrections »
//   (capture claire et sombre) ;
// - Copy : le presse-papiers reçoit le texte final ;
// - frappe dans la revue (vraie touche) : édition du texte final, la touche
//   à la fin ; Ctrl+Z natif retire la touche, puis revient au texte d'avant
//   (remplacement par insertText) ;
// - Undo par Ctrl+Z dans la revue (le focus y est après Ctrl+Entrée), Redo
//   par Ctrl+Maj+Z depuis la zone de texte puis Ctrl+Y (vraies touches,
//   avant l'annuler/rétablir natif de la route racine) ; Retour arrière dans
//   la revue (évènement de la page, après la garde de navigation) : édition
//   du texte final, dernier caractère effacé ; Rephrase par Ctrl+Maj+Entrée :
//   texte nouveau sans marques ;
// - Stop par Échap avec `chatonlm-proofread-slow` : fin aussitôt, passages
//   terminés gardés ou texte rendu ;
// - garde-fou avec `chatonlm-proofread-off` : passage gardé, remarque ;
// - modèle absent : erreur du moteur, « Choose a model » met le focus sur le
//   sélecteur ;
// - réglages, onglet « Keyboard shortcuts » : raccourcis de l'app listés
//   (plusieurs combinaisons : « Ctrl+Shift+Z or Ctrl+Y »), interrupteur
//   « Correct selected text » coupé par défaut (capture) ;
// - en français (réglage `language`) : revue et onglet Keyboard shortcuts
//   (captures proofread-fr.png, shortcuts-fr.png), puis « system » remis ;
// - aucune violation de la CSP ni erreur dans la console.
import assert from "node:assert/strict";
import { setTimeout as delay } from "node:timers/promises";
import { withApp } from "./lib/app.mjs";
import { readClipboard } from "./lib/clipboard.mjs";
import { startFake } from "./lib/fake.mjs";
import { pressKeys } from "./lib/keyboard.mjs";
import { PENDING_CALLS } from "./lib/page.mjs";

const binary = process.argv[2];
const fake = await startFake("ollama");

const FIRST = "i saw teh film.";
const SECOND = "Je suis allez au cinema hier.";
const TEXT = `${FIRST}\n\n${SECOND}`;
const CORRECTED = "i saw the film.\n\nJe suis allé au cinéma hier.";

async function waitUntil(check, what, timeoutMs = 10000) {
  const deadline = Date.now() + timeoutMs;
  let last;
  for (;;) {
    last = await check();
    if (last) return last;
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

const WATCH_PAGE = `
  if (!window.__csp) {
    window.__csp = [];
    window.__errors = [];
    document.addEventListener("securitypolicyviolation", (event) =>
      window.__csp.push(event.violatedDirective + " " + event.blockedURI));
    const error = console.error;
    console.error = (...args) => {
      window.__errors.push(args.map(String).join(" "));
      error.apply(console, args);
    };
  }`;

/**
 * Recharge la page une fois aucun appel au backend en cours (la réponse
 * arriverait dans la nouvelle page), et attend la nouvelle. Les violations
 * et erreurs de l'ancienne page sont gardées.
 */
async function reload(driver) {
  await collect(driver);
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

const violations = [];
const consoleErrors = [];

/** Relève les violations de la CSP et les erreurs de la page courante. */
async function collect(driver) {
  violations.push(
    ...((await driver.execute("return window.__csp ?? []")) ?? []),
  );
  consoleErrors.push(
    ...((await driver.execute("return window.__errors ?? []")) ?? []),
  );
  await driver.execute("window.__csp = []; window.__errors = [];");
}

/** Choisit le modèle de la correction (le moteur le lit à chaque travail). */
const useModel = (driver, model) =>
  driver.invoke("update_settings", { patch: { proofreadModel: model } });

/** Texte final montré par la revue (sans mots supprimés ni textes cachés). */
const reviewText = (driver) =>
  driver.execute(
    `const review = document.querySelector("[data-testid=proofread-review]");
     if (!review) return null;
     const copy = review.cloneNode(true);
     copy.querySelectorAll("[data-skip]").forEach((e) => e.remove());
     return copy.textContent;`,
  );

/** Mots marqués de la revue (sans le texte des lecteurs d'écran). */
const marks = (driver, tag) =>
  driver.execute(
    `return [...document.querySelectorAll(
       "[data-testid=proofread-review] " + arguments[0])].map((mark) => {
       const copy = mark.cloneNode(true);
       copy.querySelectorAll(".sr-only").forEach((e) => e.remove());
       return copy.textContent;
     });`,
    [tag],
  );

const statusText = (driver) =>
  driver.execute(
    "return document.querySelector('[data-testid=proofread-status]')?.textContent",
  );

/** Zone de texte de la page (sélecteur sans texte : toutes les langues). */
const EDITOR = "[data-testid=proofread] textarea";

const editorValue = (driver) =>
  driver.execute(
    `const area = document.querySelector(arguments[0]);
     return area && !area.hidden ? area.value : null;`,
    [EDITOR],
  );

/** Remplace le texte de la zone (valeur posée comme une frappe pour React). */
async function setText(driver, text) {
  await driver.execute(
    `const area = document.querySelector(arguments[1]);
     area.focus();
     area.select();
     document.execCommand("insertText", false, arguments[0]);`,
    [text, EDITOR],
  );
  await waitUntil(
    async () => (await editorValue(driver)) === text,
    "texte posé dans la zone",
  );
}

const clickButton = (driver, name) =>
  driver.execute(
    `const button = [...document.querySelectorAll("[data-testid=proofread] button")]
       .find((b) => b.textContent.trim() === arguments[0]);
     button?.click();
     return button !== undefined;`,
    [name],
  );

/** Attend la revue (fin du travail). */
const waitReview = (driver, timeoutMs = 10000) =>
  driver.waitFor("[data-testid=proofread-review]", { timeoutMs });

try {
  await withApp(
    binary,
    { name: "proofread", env: { OLLAMA_HOST: fake.url } },
    async (driver) => {
      await driver.waitFor("[data-testid=new-chat]");
      await driver.execute(WATCH_PAGE);
      await useModel(driver, "chatonlm-proofread");
      await reload(driver);
      await driver.waitFor("[data-testid=new-chat]");

      // Barre latérale : Proofread entre Chat et Study.
      if (await exists(driver, "button[aria-label='Show sidebar']")) {
        await driver.click(
          await driver.find("button[aria-label='Show sidebar']"),
        );
      }
      const link = await driver.waitFor("nav a[href$='/proofread']");
      assert.deepEqual(
        await driver.execute(
          `return [...document.querySelector("nav a[href$='/proofread']")
             .parentElement.querySelectorAll(":scope > a")]
             .map((a) => a.textContent.trim()).slice(0, 5);`,
        ),
        ["Chat", "Proofread", "Study", "ModelHub", "Settings"],
      );
      await driver.click(link);
      await driver.waitFor("[data-testid=proofread]");
      assert.equal(
        await driver.execute("return document.querySelector('h1').textContent"),
        "Proofread",
      );
      assert.ok(
        await driver.execute(
          `return document.querySelector("nav a[href$='/proofread']")
             .classList.contains("bg-accent")`,
        ),
        "entrée Proofread surlignée",
      );
      assert.deepEqual(
        await driver.execute(
          `return ["Correct", "Rephrase"].map((name) =>
             [...document.querySelectorAll("[data-testid=proofread] button")]
               .find((b) => b.textContent.trim() === name).disabled);`,
        ),
        [true, true],
      );
      await delay(400); // transition de la barre latérale
      await driver.screenshot(".tools/shots/proofread-empty.png");

      // Correct par Ctrl+Entrée (vraies touches), depuis la zone de texte.
      await setText(driver, TEXT);
      await pressKeys("ctrl+enter");
      await waitReview(driver);
      assert.equal(await reviewText(driver), CORRECTED);
      assert.deepEqual(await marks(driver, "del"), ["teh", "allez", "cinema"]);
      assert.deepEqual(await marks(driver, "ins"), ["the", "allé", "cinéma"]);
      assert.equal(await statusText(driver), "3 correctionschatonlm-proofread");
      await driver.screenshot(".tools/shots/proofread-review.png");
      await driver.invoke("update_settings", { patch: { theme: "dark" } });
      await reload(driver);
      // L'état de la page ne survit pas au rechargement : nouvelle correction.
      await driver.waitFor("[data-testid=proofread]");
      await setText(driver, TEXT);
      await pressKeys("ctrl+enter");
      await waitReview(driver);
      await delay(400);
      await driver.screenshot(".tools/shots/proofread-review-dark.png");
      await driver.invoke("update_settings", { patch: { theme: "light" } });

      // Copy : texte final dans le presse-papiers.
      assert.ok(await clickButton(driver, "Copy"));
      await waitUntil(
        () =>
          driver.execute(
            `return [...document.querySelectorAll("[data-testid=proofread] button")]
             .some((b) => b.textContent.trim() === "Copied")`,
          ),
        "« Copied »",
      );
      const clipboard = await readClipboard();
      assert.equal(clipboard.text, CORRECTED);

      // Frappe dans la revue : édition du texte final, la touche à la fin ;
      // Ctrl+Z natif retire la touche, puis revient au texte d'avant.
      await driver.execute(
        "document.querySelector('[data-testid=proofread-review]').focus()",
      );
      await pressKeys("x");
      await waitUntil(
        async () => (await editorValue(driver)) === `${CORRECTED}x`,
        "touche tapée à la fin du texte corrigé",
      );
      await pressKeys("ctrl+z");
      await waitUntil(
        async () => (await editorValue(driver)) === CORRECTED,
        "Ctrl+Z : touche retirée",
      );
      await pressKeys("ctrl+z");
      await waitUntil(
        async () => (await editorValue(driver)) === TEXT,
        () => "Ctrl+Z natif : texte d'avant la correction",
      );

      // Undo par Ctrl+Z dans la revue : le focus y est arrivé (Stop, puis
      // la revue) depuis la zone de texte cachée.
      await pressKeys("ctrl+enter");
      await waitReview(driver);
      assert.equal(
        await driver.execute(
          "return document.activeElement?.dataset.testid ?? null",
        ),
        "proofread-review",
      );
      await pressKeys("ctrl+z");
      await waitUntil(
        async () => (await editorValue(driver)) === TEXT,
        "Undo par Ctrl+Z",
      );
      // Redo depuis la zone de texte (Ctrl+Maj+Z), puis Ctrl+Z et Ctrl+Y.
      await pressKeys("ctrl+shift+z");
      await waitUntil(
        async () => (await reviewText(driver)) === CORRECTED,
        "Redo par Ctrl+Maj+Z",
      );
      await pressKeys("ctrl+z");
      await waitUntil(
        async () => (await editorValue(driver)) === TEXT,
        "Undo après Redo",
      );
      await pressKeys("ctrl+y");
      await waitUntil(
        async () => (await reviewText(driver)) === CORRECTED,
        "Redo par Ctrl+Y",
      );
      // Retour arrière dans la revue (touche synthétique : même chemin dans
      // la page, après la garde de navigation) : édition, dernier caractère
      // effacé par l'édition native.
      assert.equal(
        await driver.execute(
          `const review = document.querySelector("[data-testid=proofread-review]");
           return review.dispatchEvent(new KeyboardEvent("keydown",
             { key: "Backspace", bubbles: true, cancelable: true }));`,
        ),
        false,
      );
      await waitUntil(
        async () => (await editorValue(driver)) === CORRECTED.slice(0, -1),
        "Retour arrière : dernier caractère effacé",
      );
      await setText(driver, TEXT);

      // Rephrase par Ctrl+Maj+Entrée : texte nouveau, sans marques.
      await pressKeys("ctrl+shift+enter");
      await waitUntil(
        () =>
          driver.execute(
            `return document.querySelector(
               "[data-testid=proofread-review][aria-label='Rephrased text']") !== null`,
          ),
        "revue de la reformulation",
      );
      assert.equal(
        await driver.execute(
          "return document.querySelectorAll('[data-testid=proofread-review] del, [data-testid=proofread-review] ins').length",
        ),
        0,
      );
      assert.notEqual(await reviewText(driver), "");
      assert.match(await statusText(driver), /^Text rephrased/);
      assert.ok(await clickButton(driver, "Undo"));
      await waitUntil(
        async () => (await editorValue(driver)) === TEXT,
        "Undo après Rephrase",
      );

      // Stop par Échap pendant un flux lent.
      await useModel(driver, "chatonlm-proofread-slow");
      assert.ok(await clickButton(driver, "Correct"));
      await driver.waitFor(
        "[data-testid=proofread-running] [data-state=streaming]",
        {
          timeoutMs: 15000,
        },
      );
      const stoppedAt = Date.now();
      await pressKeys("escape");
      await waitUntil(
        async () => !(await exists(driver, "[data-testid=proofread-running]")),
        "fin après Échap",
        5000,
      );
      assert.ok(Date.now() - stoppedAt < 3000, "arrêt aussitôt");
      if (await exists(driver, "[data-testid=proofread-review]")) {
        assert.ok(await exists(driver, "[data-testid=proofread-note-partial]"));
        assert.ok(await clickButton(driver, "Undo"));
      }
      await waitUntil(
        async () => (await editorValue(driver)) === TEXT,
        "texte d'avant après l'arrêt",
      );

      // Garde-fou : réponse écartée, passage gardé tel quel.
      await useModel(driver, "chatonlm-proofread-off");
      await setText(driver, FIRST);
      await pressKeys("ctrl+enter");
      await waitReview(driver);
      assert.equal(await reviewText(driver), FIRST);
      assert.ok(await exists(driver, "[data-testid=proofread-note-kept]"));
      await driver.screenshot(".tools/shots/proofread-kept.png");
      assert.ok(await clickButton(driver, "Undo"));

      // Modèle absent : erreur du moteur, action vers le sélecteur.
      await useModel(driver, "chatonlm-missing");
      await pressKeys("ctrl+enter");
      await driver.waitFor("[data-testid=proofread] [role=alert]");
      assert.match(
        await driver.execute(
          "return document.querySelector('[data-testid=proofread] [role=alert]').textContent",
        ),
        /chatonlm-missing/,
      );
      assert.equal(await editorValue(driver), FIRST);
      assert.ok(await clickButton(driver, "Choose a model"));
      assert.equal(
        await driver.execute(
          "return document.activeElement === document.querySelector('[data-testid=proofread] select')",
        ),
        true,
      );
      await driver.screenshot(".tools/shots/proofread-error.png");

      // Réglages, onglet Keyboard shortcuts.
      await pressKeys("ctrl+comma");
      await driver.click(
        await driver.waitFor("[data-testid=settings-tab-shortcuts]"),
      );
      await driver.waitFor("[data-testid=shortcuts]");
      assert.deepEqual(
        await driver.execute(
          `return [...document.querySelectorAll("[data-testid=shortcut-group-proofread] dt")]
             .map((dt) => [dt.textContent, dt.nextElementSibling.textContent]);`,
        ),
        [
          ["Correct", "Ctrl+Enter"],
          ["Rephrase", "Ctrl+Shift+Enter"],
          ["Stop", "Esc"],
          ["Undo the correction", "Ctrl+Z"],
          ["Redo the correction", "Ctrl+Shift+Z or Ctrl+Y"],
          ["Paste at the end of the corrected text", "Ctrl+V"],
        ],
      );
      assert.equal(
        await driver.execute(
          `return document.querySelector("[role=switch][aria-label='Correct selected text']")
             .getAttribute("aria-checked")`,
        ),
        "false",
      );
      await driver.screenshot(".tools/shots/settings-shortcuts.png");

      // En français : retour à la page Proofread, langue réglée, page
      // rechargée (le réglage passe par le moteur, pas par la page).
      if (await exists(driver, "button[aria-label='Show sidebar']")) {
        await driver.click(
          await driver.find("button[aria-label='Show sidebar']"),
        );
      }
      await driver.click(await driver.waitFor("nav a[href$='/proofread']"));
      await driver.waitFor("[data-testid=proofread]");
      await useModel(driver, "chatonlm-proofread");
      await driver.invoke("update_settings", { patch: { language: "fr" } });
      await reload(driver);
      await waitUntil(
        async () =>
          (await driver.execute("return document.documentElement.lang")) ===
          "fr",
        "page en français",
      );
      await driver.waitFor("[data-testid=proofread]");
      assert.equal(
        await driver.execute("return document.querySelector('h1').textContent"),
        "Correction",
      );
      await setText(driver, TEXT);
      await pressKeys("ctrl+enter");
      await waitReview(driver);
      assert.equal(await reviewText(driver), CORRECTED);
      assert.equal(await statusText(driver), "3 correctionschatonlm-proofread");
      await delay(400);
      await driver.screenshot(".tools/shots/proofread-fr.png");

      await pressKeys("ctrl+comma");
      await driver.click(
        await driver.waitFor("[data-testid=settings-tab-shortcuts]"),
      );
      await driver.waitFor("[data-testid=shortcuts]");
      assert.deepEqual(
        await driver.execute(
          `return [...document.querySelectorAll("[data-testid=shortcut-group-proofread] dt")]
             .map((dt) => [dt.textContent, dt.nextElementSibling.textContent]);`,
        ),
        [
          ["Corriger", "Ctrl+Entrée"],
          ["Reformuler", "Ctrl+Maj+Entrée"],
          ["Arrêter", "Échap"],
          ["Annuler la correction", "Ctrl+Z"],
          ["Rétablir la correction", "Ctrl+Maj+Z ou Ctrl+Y"],
          ["Coller à la fin du texte corrigé", "Ctrl+V"],
        ],
      );
      await delay(400);
      await driver.screenshot(".tools/shots/shortcuts-fr.png");

      await driver.invoke("update_settings", { patch: { language: "system" } });
      await reload(driver);
      await waitUntil(
        async () =>
          (await driver.execute("return document.documentElement.lang")) ===
          "en",
        "page de nouveau en anglais",
      );

      await collect(driver);
      assert.deepEqual(violations, [], "violations de la CSP");
      assert.deepEqual(consoleErrors, [], "erreurs de la console");
    },
  );
} finally {
  await fake.stop();
}
console.log("✓ proofread");
