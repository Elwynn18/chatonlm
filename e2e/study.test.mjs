// Onglet Study dans l'app réelle (faux serveur, modèle des cours
// `chatonlm-study` choisi par le réglage `studyModel`), avec de vraies touches
// et un vrai pointeur (e2e/lib/keyboard.mjs) :
// - entrée « Study » de la barre latérale, entre Proofread et ModelHub,
//   surlignée ; accueil vide (capture) ;
// - nouveau cours : sujet tapé, 10 min, un document par le crochet
//   CHATONLM_E2E_PICK_FILE (notes.md, pastille), création par une vraie
//   touche Entrée ; plan (titre, deux paliers, document dans les sources ;
//   captures claire et sombre) ; Start ;
// - palier 1 : accroche du tuteur en bulle, sections, exemple résolu (la
//   solution révélée), « À retenir » révélés un à un, par de vraies touches
//   Entrée (titre de carte focalisé) ; entraînement répondu juste au vrai
//   pointeur ; premier essai manqué (erreurs expliquées, capture), « Explain
//   differently » (Entrée), explication du tuteur (capture), second essai
//   manqué, « Continue anyway » : palier 1 « à revoir » dans le parcours ;
// - palier 2 : « I don't understand » (explication en flux, bulle du
//   tuteur), « Ask a question » (question tapée, réponse en flux, bulle de
//   l'apprenant et du tuteur ; Échap ferme le panneau ; capture) ; essai
//   réussi sans faute : célébration (trois étoiles, confettis ; capture),
//   série de bonnes réponses dans l'en-tête ;
// - bilan final répondu juste, fin du cours (« Course completed! », 3 / 3,
//   capture) ; Copy : la fiche dans le presse-papiers ; « Save as
//   Markdown » : fichier écrit (crochet CHATONLM_E2E_SAVE_FILE) ;
// - accueil : le cours dans « Completed » (capture), supprimé après
//   confirmation ;
// - aucune violation de la CSP ni erreur dans la console.
// Questions répondues selon leur type (choix, vrai/faux, trou, ordre,
// association), lu dans la page : le moteur choisit les types et mélange
// les options.
import assert from "node:assert/strict";
import {
  existsSync,
  mkdirSync,
  readFileSync,
  realpathSync,
  writeFileSync,
} from "node:fs";
import path from "node:path";
import { setTimeout as delay } from "node:timers/promises";
import { ROOT, withApp } from "./lib/app.mjs";
import { readClipboard } from "./lib/clipboard.mjs";
import { startFake } from "./lib/fake.mjs";
import { chordsOf, pressKeys } from "./lib/keyboard.mjs";
import { PENDING_CALLS } from "./lib/page.mjs";
import { centerOf, pointerOffset } from "./lib/pointer.mjs";

const binary = process.argv[2];

// Fichiers de l'essai : à côté du HOME de la session (.tools/e2e-runs/run.*,
// supprimé à la fin de la session), jamais ailleurs.
const home = realpathSync(process.env.HOME ?? "/");
const runs = path.join(realpathSync(ROOT), ".tools", "e2e-runs") + path.sep;
assert.ok(home.startsWith(runs), `HOME hors de .tools/e2e-runs : ${home}`);
const FILES = path.join(path.dirname(home), "Study Files");
mkdirSync(FILES, { recursive: true });
const file = (name) => path.join(FILES, name);
const PICKS = file("picks.txt");
const SAVED = file("notes-saved.md");
writeFileSync(
  file("notes.md"),
  "# Photosynthesis\n\nThe leaf uses light to make sugar.\n",
);
/** Fichiers « choisis » au prochain clic sur « Add a document ». */
const pick = (...names) =>
  writeFileSync(PICKS, `${names.map(file).join("\n")}\n`);

const MODEL = "chatonlm-study";
const fake = await startFake("ollama");

async function waitUntil(check, what, timeoutMs = 15000) {
  const deadline = Date.now() + timeoutMs;
  let last;
  for (;;) {
    last = await check();
    if (last) return last;
    if (Date.now() > deadline) {
      throw new Error(
        `Délai dépassé : ${typeof what === "function" ? await what() : what}`,
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

/** Recharge la page une fois aucun appel au backend en cours. */
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

/**
 * Carte montrée par le lecteur (data-testid de son article), ou null : le
 * lecteur montre une seule carte à la fois (il n'est pas dans le
 * `main[data-testid=study-course]` des autres états de la page).
 */
const cardKind = (driver) =>
  driver.execute(
    `const card = document.querySelector("article[data-testid^='study-']");
     return card ? card.dataset.testid : null;`,
  );

const cardTitle = (driver) =>
  driver.execute(
    "return document.querySelector('[data-card-heading]')?.textContent ?? null",
  );

/** Attend une carte d'une sorte (« study-section »...). */
async function waitCard(driver, kind, timeoutMs = 20000) {
  await waitUntil(
    async () => (await cardKind(driver)) === kind,
    async () => {
      // Diagnostic : capture et texte de la page au moment de l'échec.
      await driver.screenshot(".tools/shots/study-failure.png");
      const text = await driver.execute(
        "return document.querySelector('main')?.innerText ?? ''",
      );
      const testIds = await driver.execute(
        `return [...document.querySelectorAll("main[data-testid], article[data-testid]")]
           .map((e) => e.tagName + ":" + e.dataset.testid).join(", ")`,
      );
      return `carte ${kind} (montrée : ${await cardKind(driver)}) ; éléments : ${testIds} ; page : ${String(text).slice(0, 300)}`;
    },
    timeoutMs,
  );
}

/** Clic JavaScript sur un bouton de la page par son texte exact. */
const clickButton = (driver, name) =>
  driver.execute(
    `const button = [...document.querySelectorAll("main button")]
       .find((b) => b.textContent.trim() === arguments[0] && !b.disabled);
     button?.click();
     return button !== undefined;`,
    [name],
  );

/** Vraie touche Entrée, une fois la carte stable (titre focalisé). */
async function enter() {
  await pressKeys("enter", { settleMs: 250 });
}

let offset = null;
/** Capture d'une réponse choisie, pas encore corrigée : une seule fois. */
let chosenShot = false;

/** Clic du vrai pointeur au centre de `css`. */
async function pointerClick(driver, css) {
  offset ??= await pointerOffset(driver);
  const [x, y] = await centerOf(driver, css);
  await pressKeys(
    [
      `pointer:${x + offset[0]},${y + offset[1]}`,
      "wait:100",
      "button:down",
      "wait:50",
      "button:up",
    ],
    { settleMs: 250 },
  );
}

/** Type de la question montrée, lu dans la page. */
const questionType = (driver) =>
  driver.execute(`
    const card = document.querySelector("[data-testid=study-question]");
    if (!card) return null;
    const labels = [...card.querySelectorAll("[data-testid=study-option]")];
    if (labels.length > 0) {
      return labels.some((label) => label.textContent.includes("True"))
        ? "trueFalse"
        : "choice";
    }
    if (card.querySelector("input[type=text]")) return "blank";
    if (card.querySelector("[data-testid=study-order-item]")) return "order";
    if (card.querySelector("[data-testid=study-match-row]")) return "match";
    return "unknown";`);

/** Rang d'un élément d'ordre du faux serveur (« First step » = 0…). */
const ORDINAL = `(text) => ["First", "Second", "Third", "Fourth", "Fifth"]
  .findIndex((word) => text.includes(word))`;

/**
 * Répond à la question montrée (juste ou faux), puis la corrige : options au
 * vrai pointeur et Entrée, trou tapé au clavier et Entrée, ordre et
 * association en JavaScript et « Check ». Attend le retour.
 */
async function answer(driver, correct) {
  const type = await questionType(driver);
  if (type === "choice" || type === "trueFalse") {
    const index = await driver.execute(
      `const labels = [...document.querySelectorAll("[data-testid=study-option]")];
       const wanted = arguments[0]
         ? (t) => t.includes("Right answer") || t.includes("True")
         : (t) => t.includes("Wrong") || t.includes("False");
       return labels.findIndex((label) => wanted(label.textContent));`,
      [correct],
    );
    assert.ok(index >= 0, `option ${correct ? "juste" : "fausse"} introuvable`);
    await pointerClick(
      driver,
      `[data-testid=study-question] [role=radiogroup] > label:nth-of-type(${index + 1})`,
    );
    if (!chosenShot) {
      chosenShot = true;
      await driver.screenshot(".tools/shots/study-chosen.png");
    }
    await enter();
  } else if (type === "blank") {
    await driver.execute(
      "document.querySelector('[data-testid=study-question] input[type=text]').focus()",
    );
    await pressKeys([...chordsOf(correct ? "right" : "wrong"), "enter"], {
      settleMs: 250,
    });
  } else if (type === "order") {
    if (correct) {
      // Tri par « Move up » : un clic à la fois, la page rendue entre deux.
      for (let i = 0; i < 10; i += 1) {
        const moved = await driver.execute(
          `const rank = ${ORDINAL};
           const items = [...document.querySelectorAll("[data-testid=study-order-item]")];
           const at = items.findIndex((item, i) =>
             i > 0 && rank(item.textContent) < rank(items[i - 1].textContent));
           if (at === -1) return false;
           items[at].querySelectorAll("button")[0].click();
           return true;`,
        );
        if (!moved) break;
      }
    }
    assert.ok(await clickButton(driver, "Check"));
  } else if (type === "match") {
    await driver.execute(
      `const setValue = Object.getOwnPropertyDescriptor(
         HTMLSelectElement.prototype, "value").set;
       const words = { One: "1", Two: "2", Three: "3" };
       for (const row of document.querySelectorAll("[data-testid=study-match-row]")) {
         const select = row.querySelector("select");
         const left = row.querySelector("label").textContent.trim();
         const options = [...select.options].filter((o) => !o.disabled);
         const option = arguments[0]
           ? options.find((o) => o.textContent.trim() === words[left])
           : options[0];
         setValue.call(select, option.value);
         select.dispatchEvent(new Event("change", { bubbles: true }));
       }`,
      [correct],
    );
    assert.ok(await clickButton(driver, "Check"));
  } else {
    throw new Error(`question de type inconnu : ${type}`);
  }
  await driver.waitFor("[data-testid=study-feedback]");
  const right = await driver.execute(
    "return document.querySelector('[data-testid=study-feedback]').dataset.correct",
  );
  assert.equal(right, String(correct), `réponse ${type} corrigée`);
}

/** Répond aux questions d'un essai (toutes justes ou toutes fausses). */
async function answerAttempt(driver, correct) {
  for (;;) {
    await waitUntil(
      async () =>
        ["study-question", "study-result"].includes(await cardKind(driver)),
      "question ou résultat",
    );
    if ((await cardKind(driver)) === "study-result") return;
    await answer(driver, correct);
    // Continue (le focus y est, ou dans le trou) : vraie touche Entrée.
    await enter();
  }
}

/**
 * Répond juste au bilan, question après question, jusqu'à la fin ; après
 * la dernière réponse, « Continue » attend la relecture du cours (fini).
 */
async function answerFinal(driver) {
  for (;;) {
    await waitUntil(
      async () =>
        ["study-question", "study-end"].includes(await cardKind(driver)),
      "question du bilan ou fin",
      30000,
    );
    if ((await cardKind(driver)) === "study-end") return;
    if (!(await exists(driver, "[data-testid=study-feedback]"))) {
      await answer(driver, true);
    }
    await waitUntil(
      () =>
        driver.execute(
          `return [...document.querySelectorAll("main button")]
             .some((b) => b.textContent.trim() === "Continue")`,
        ),
      "« Continue » du bilan",
    );
    await enter();
  }
}

/** Lit une leçon jusqu'à l'entraînement, par de vraies touches Entrée. */
async function readLesson(driver) {
  for (const kind of ["study-section", "study-section", "study-section"]) {
    await waitCard(driver, kind);
    assert.equal(
      await driver.execute(
        "return document.activeElement?.hasAttribute('data-card-heading') ?? false",
      ),
      true,
      "titre de la carte focalisé",
    );
    await enter();
  }
  await waitCard(driver, "study-example");
  await enter(); // révèle la solution (une étape)
  await driver.waitFor("[data-testid=study-solution-step]");
  await enter();
  await waitCard(driver, "study-recap");
  assert.equal(
    await driver.execute(
      "return document.querySelectorAll('[data-testid=study-key-point]').length",
    ),
    1,
  );
  await enter(); // second point
  await driver.waitFor("[data-testid=study-pitfall]");
  await enter();
}

const nodeStatuses = (driver) =>
  driver.execute(
    `return [...document.querySelectorAll("[data-testid=study-path-node]")]
       .map((node) => node.dataset.status);`,
  );

/**
 * Attend que le parcours commence par ces états : il est relu après chaque
 * essai enregistré, et la carte suivante peut s'afficher avant (machine
 * chargée).
 */
const waitNodeStatuses = (driver, expected) =>
  waitUntil(
    async () =>
      JSON.stringify((await nodeStatuses(driver)).slice(0, expected.length)) ===
      JSON.stringify(expected),
    async () =>
      `parcours ${JSON.stringify(expected)} : ${JSON.stringify(await nodeStatuses(driver))}`,
  );

try {
  await withApp(
    binary,
    {
      name: "study",
      env: {
        OLLAMA_HOST: fake.url,
        CHATONLM_E2E_PICK_FILE: PICKS,
        CHATONLM_E2E_SAVE_FILE: SAVED,
      },
    },
    async (driver) => {
      await driver.waitFor("[data-testid=new-chat]");
      await driver.execute(WATCH_PAGE);
      await driver.invoke("update_settings", { patch: { studyModel: MODEL } });
      await reload(driver);
      await driver.waitFor("[data-testid=new-chat]");

      // Barre latérale : Study entre Proofread et ModelHub.
      if (await exists(driver, "button[aria-label='Show sidebar']")) {
        await driver.click(
          await driver.find("button[aria-label='Show sidebar']"),
        );
      }
      const link = await driver.waitFor("nav a[href$='/study']");
      assert.deepEqual(
        await driver.execute(
          `return [...document.querySelector("nav a[href$='/study']")
             .parentElement.querySelectorAll(":scope > a")]
             .map((a) => a.textContent.trim()).slice(0, 5);`,
        ),
        ["Chat", "Proofread", "Study", "ModelHub", "Settings"],
      );
      await driver.click(link);
      await driver.waitFor("[data-testid=study-home]");
      await driver.waitFor("[data-testid=study-empty]");
      assert.equal(
        await driver.execute("return document.querySelector('h1').textContent"),
        "Study",
      );
      assert.ok(
        await driver.execute(
          `return document.querySelector("nav a[href$='/study']")
             .classList.contains("bg-accent")`,
        ),
        "entrée Study surlignée",
      );
      await delay(400); // transition de la barre latérale
      await driver.screenshot(".tools/shots/study-home-empty.png");

      // Nouveau cours : document, 10 min, sujet, Entrée.
      pick("notes.md");
      assert.ok(await clickButton(driver, "Add a document"));
      await waitUntil(
        () =>
          driver.execute(
            `return [...document.querySelectorAll("[data-testid=study-document]")]
               .map((chip) => chip.textContent).join(",") === "notes.md"`,
          ),
        "pastille notes.md",
      );
      await driver.execute(
        `[...document.querySelectorAll("[data-testid=study-form] label")]
           .find((label) => label.textContent.trim() === "10 min").click();`,
      );
      await driver.execute(
        "document.querySelector('[data-testid=study-form] textarea').focus()",
      );
      await pressKeys(chordsOf("Photosynthesis"), { settleMs: 200 });
      await driver.screenshot(".tools/shots/study-form.png");
      await pressKeys("enter");

      // Plan : titre, deux paliers, document dans les sources.
      await driver.waitFor("[data-testid=study-plan]", { timeoutMs: 20000 });
      const plan = await driver.execute(
        "return document.querySelector('[data-testid=study-plan]').textContent",
      );
      assert.match(plan, /Photosynthesis/);
      assert.match(plan, /2 levels/);
      assert.match(plan, /notes\.md/);
      await delay(400);
      await driver.screenshot(".tools/shots/study-plan.png");
      await driver.invoke("update_settings", { patch: { theme: "dark" } });
      await reload(driver);
      await driver.waitFor("[data-testid=study-plan]");
      await delay(400);
      await driver.screenshot(".tools/shots/study-plan-dark.png");
      await driver.invoke("update_settings", { patch: { theme: "light" } });
      await reload(driver);
      await driver.waitFor("[data-testid=study-plan]");

      // Start : premier palier, accroche du tuteur.
      assert.ok(await clickButton(driver, "Start"));
      await waitCard(driver, "study-section");
      assert.ok(await exists(driver, "[data-testid=study-hook]"));
      assert.equal(
        await driver.execute(
          "return document.querySelector('[data-testid=study-hook]').dataset.speaker",
        ),
        "tutor",
      );
      await delay(400);
      await driver.screenshot(".tools/shots/study-lesson.png");
      await readLesson(driver);

      // Entraînement juste (vrai pointeur pour une option), puis premier
      // essai manqué.
      await waitCard(driver, "study-question");
      await answer(driver, true);
      await enter();
      await answerAttempt(driver, false);
      assert.equal(await cardTitle(driver), "Not passed yet");
      assert.ok(await exists(driver, "[data-testid=study-missed]"));
      await driver.screenshot(".tools/shots/study-failed.png");

      // Explain differently (Entrée), explication du tuteur, second essai.
      await enter();
      await waitUntil(
        () =>
          driver.execute(
            `return [...document.querySelectorAll("main button")]
               .some((b) => b.textContent.trim() === "See the new explanation")`,
          ),
        "nouvel essai prêt",
        30000,
      );
      assert.ok(await clickButton(driver, "See the new explanation"));
      await waitCard(driver, "study-remedy");
      assert.ok(await exists(driver, "[data-testid=study-remedy-text]"));
      await driver.screenshot(".tools/shots/study-remedy.png");
      await enter();
      await answerAttempt(driver, false);
      assert.ok(await clickButton(driver, "Continue anyway"));
      await waitCard(driver, "study-section", 30000);
      await waitNodeStatuses(driver, ["review"]);

      // Palier 2 : « I don't understand », puis « Ask a question ».
      assert.ok(await clickButton(driver, "I don't understand"));
      await waitUntil(
        () =>
          driver.execute(
            `return document.querySelector("[data-testid=study-explain]")
               ?.textContent.includes("Simpler:") ?? false`,
          ),
        "explication gardée",
      );
      assert.ok(await clickButton(driver, "Ask a question"));
      await driver.waitFor("[data-testid=study-ask] textarea");
      assert.ok(
        await driver.execute(
          "return document.activeElement === document.querySelector('[data-testid=study-ask] textarea')",
        ),
        "champ de la question focalisé",
      );
      await pressKeys([...chordsOf("why"), "enter"]);
      await waitUntil(
        () =>
          driver.execute(
            `return document.querySelector("[data-testid=study-ask-answer]")
               ?.textContent.includes("Answer:") ?? false`,
          ),
        "réponse du tuteur gardée",
      );
      assert.equal(
        await driver.execute(
          `return [...document.querySelectorAll("[data-testid=study-ask] [data-speaker]")]
             .map((bubble) => bubble.dataset.speaker).join(",")`,
        ),
        "learner,tutor",
      );
      await driver.screenshot(".tools/shots/study-tutor.png");
      await pressKeys("escape", { settleMs: 300 });
      assert.equal(await exists(driver, "[data-testid=study-ask]"), false);
      // Le focus revient au bouton du panneau : Entrée le rouvrirait ; le
      // titre de la carte le reprend.
      await driver.execute(
        "document.querySelector('[data-card-heading]').focus()",
      );
      await readLesson(driver);

      // Essai réussi sans faute : célébration.
      await waitCard(driver, "study-question");
      await answer(driver, true);
      await enter();
      await answerAttempt(driver, true);
      assert.equal(await cardTitle(driver), "Level complete!");
      assert.equal(
        await driver.execute(
          "return document.querySelector('[data-testid=study-result] [data-stars]').dataset.stars",
        ),
        "3",
      );
      assert.ok(await exists(driver, "[data-testid=study-confetti]"));
      assert.ok(await exists(driver, "[data-testid=study-streak]"));
      await delay(700); // étoiles apparues
      await driver.screenshot(".tools/shots/study-passed.png");
      await waitNodeStatuses(driver, ["review", "passed"]);
      assert.equal((await nodeStatuses(driver)).length, 2);

      // Bilan final, puis la fin.
      await enter();
      await waitCard(driver, "study-question", 30000);
      await answerFinal(driver);
      assert.equal(await cardTitle(driver), "Course completed!");
      assert.equal(
        await driver.execute(
          "return document.querySelector('[data-testid=study-final-score]').textContent",
        ),
        "3 / 3",
      );
      await delay(800);
      await driver.screenshot(".tools/shots/study-end.png");

      // Copy : la fiche dans le presse-papiers ; Save as Markdown.
      await waitUntil(
        () =>
          driver.execute(
            `return [...document.querySelectorAll("main button")]
               .some((b) => b.textContent.trim() === "Copy" && !b.disabled)`,
          ),
        "fiche lue",
      );
      assert.ok(await clickButton(driver, "Copy"));
      await waitUntil(
        () =>
          driver.execute(
            `return [...document.querySelectorAll("main button")]
               .some((b) => b.textContent.trim() === "Copied")`,
          ),
        "« Copied »",
      );
      const clipboard = await readClipboard();
      assert.match(clipboard.text ?? "", /Key point of/);
      assert.ok(await clickButton(driver, "Save as Markdown"));
      await waitUntil(() => existsSync(SAVED), "fiche enregistrée");
      assert.match(readFileSync(SAVED, "utf8"), /Key point of/);
      await driver.waitFor("[data-testid=study-end]");

      // Accueil : le cours terminé, puis supprimé. La barre latérale a pu
      // revenir à son état par défaut (repliée) avec les rechargements.
      if (await exists(driver, "button[aria-label='Show sidebar']")) {
        await driver.click(
          await driver.find("button[aria-label='Show sidebar']"),
        );
      }
      await driver.click(await driver.waitFor("nav a[href$='/study']"));
      await driver.waitFor("[data-testid=study-completed]");
      await delay(300);
      await driver.screenshot(".tools/shots/study-home.png");
      await driver.execute(
        `document.querySelector("[data-testid=study-completed] [data-testid=study-course] button").click()`,
      );
      await driver.waitFor("[role=alertdialog]");
      await driver.execute(
        `[...document.querySelectorAll("[role=alertdialog] button")]
           .find((b) => b.textContent.trim() === "Delete").click()`,
      );
      await driver.waitFor("[data-testid=study-empty]");

      await collect(driver);
      assert.deepEqual(violations, [], "violations de la CSP");
      assert.deepEqual(consoleErrors, [], "erreurs de la console");
    },
  );
} finally {
  await fake.stop();
}
console.log("✓ study");
