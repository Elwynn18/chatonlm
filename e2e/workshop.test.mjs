// Atelier (moteur de tâches longues, propre à ChatonLM) dans l'app réelle,
// sans modèle : le pipeline est remplacé par un déroulé écrit (crochet
// CHATONLM_E2E_WORKSHOP de la fonctionnalité e2e, `workshop/scripted.rs`),
// mené contre le vrai socle (base, file, questions, écriture du livrable).
// Vraies touches (e2e/lib/keyboard.mjs) pour la tâche et Maj+Tab.
// - carte « Workshop » des Outils, accueil vide (capture) ;
// - nouveau projet : modèle retenu et son avertissement (`small`), pièce
//   jointe notes.md (crochet CHATONLM_E2E_PICK_FILE), tâche tapée, autonomie
//   changée par Maj+Tab dans la zone de la tâche (le focus y reste, la ligne
//   d'aide change ; capture) ; création ;
// - vue du projet : phases, activité, puis « J'ai besoin de toi » : plan à
//   approuver (captures claire et sombre), titre d'une partie modifié,
//   approuvé ; le plan de la vue suit la modification (relu de l'état sauvé) ;
// - parties : panneau d'une partie (texte, vérifications) ; pause pendant
//   une correction (état « en pause », capture), reprise ;
// - livraison : rapport (captures claire et sombre), livrable et rapport.md
//   écrits dans le dossier du projet, « Ouvrir le dossier » et « Ouvrir le
//   livrable » par un faux xdg-open (aucune fenêtre) ;
// - notifications du bureau (faux service, aucun service du système activé) :
//   « ChatonLM needs you » à la question, « Project finished » à la fin ;
// - accueil : le projet fini dans la liste ;
// - aucune violation de la CSP ni erreur dans la console.
import assert from "node:assert/strict";
import {
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  realpathSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import path from "node:path";
import { setTimeout as delay } from "node:timers/promises";
import { ROOT, withApp } from "./lib/app.mjs";
import { startDesktopServices } from "./lib/desktop.mjs";
import { chordsOf, pressKeys } from "./lib/keyboard.mjs";
import { PENDING_CALLS } from "./lib/page.mjs";

const binary = process.argv[2];

// Fichiers de l'essai : à côté du HOME de la session (.tools/e2e-runs/run.*,
// supprimé à la fin de la session), jamais ailleurs.
const home = realpathSync(process.env.HOME ?? "/");
const runs = path.join(realpathSync(ROOT), ".tools", "e2e-runs") + path.sep;
assert.ok(home.startsWith(runs), `HOME hors de .tools/e2e-runs : ${home}`);
const FILES = path.join(path.dirname(home), "Workshop Files");
mkdirSync(FILES, { recursive: true });
const file = (name) => path.join(FILES, name);
const PICKS = file("picks.txt");
const SCRIPT = file("script.json");
writeFileSync(
  file("notes.md"),
  "# My balcony\n\nSouth facing, 4 square metres, windy.\n",
);
writeFileSync(PICKS, `${file("notes.md")}\n`);

// Faux xdg-open : note la cible et réussit (aucune fenêtre).
mkdirSync(path.join(ROOT, ".tools/tmp"), { recursive: true });
const fakeBin = mkdtempSync(path.join(ROOT, ".tools/tmp/xdg-open-"));
process.on("exit", () => rmSync(fakeBin, { recursive: true, force: true }));
writeFileSync(
  path.join(fakeBin, "xdg-open"),
  `#!/bin/sh\necho "$1" >> "${fakeBin}/opened"\n`,
  { mode: 0o755 },
);
const opened = () =>
  existsSync(path.join(fakeBin, "opened"))
    ? readFileSync(path.join(fakeBin, "opened"), "utf8").trim().split("\n")
    : [];

// ---------------------------------------------------------------------------
// Déroulé écrit du projet (formes du pipeline : `WorkshopEventKind`, états
// sauvés sous leur nom `StateKey::name`, `Report`).

const TASK = "Write a guide to growing tomatoes on a balcony";
const DELIVERABLE = "tomatoes-on-a-balcony.md";
const brickPlan = (id, title, kind, words, dependsOn, criteria) => ({
  id,
  title,
  goal: `Cover ${title.toLowerCase()}`,
  kind,
  words,
  dependsOn,
  criteria,
  forbidden: [],
});
const PLAN = {
  title: "Tomatoes on a balcony",
  bricks: [
    brickPlan(1, "Introduction", "intro", 150, [], []),
    brickPlan(2, "Choosing the varieties", "text", 500, [], ["C1"]),
    brickPlan(3, "Planting and watering", "text", 500, [2], ["C2"]),
    brickPlan(4, "Conclusion", "conclusion", 150, [], []),
  ],
};
const EDITED_TITLE = "Choosing dwarf varieties";
const SPEC = {
  goal: "A beginner's guide to balcony tomatoes",
  deliverables: ["Guide"],
  criteria: [
    { id: "C1", text: "Name at least three dwarf varieties", scope: "section" },
    { id: "C2", text: "Give a weekly watering schedule", scope: "section" },
  ],
  audience: "Beginners",
  language: "en",
  register: "informal",
  units: "metric",
  lengthWords: 1300,
  glossary: [],
  assumptions: [],
  constraints: [],
};
const check = (criterion, method, passed, evidence, where = null) => ({
  criterion,
  method,
  passed,
  evidence,
  where,
  blocking: method !== "factJudge" || !passed,
});
const PASSING_2 = [
  check("length", "program", true, "496/500"),
  check("C1", "formJudge", true, "« Tiny Tim, Balconi Red and Minibel »"),
  check("Tomatoes need six hours of sun.", "factJudge", true, "right"),
];
const FAILING_3 = [
  check("length", "program", true, "512/500"),
  check("C2", "formJudge", false, "no schedule", "Water regularly."),
];
const FIXED_3 = [
  check("length", "program", true, "505/500"),
  check("C2", "formJudge", false, "no weekly schedule", "Water often."),
];
const TEXTS = {
  1: "Tomatoes grow well on a sunny balcony.",
  2: "## Choosing dwarf varieties\n\nPick Tiny Tim, Balconi Red and Minibel.",
  3: "## Planting and watering\n\nUse a 20 litre pot. Water often.",
  4: "## Conclusion\n\nEnjoy your harvest.",
};
const record = (id, state, checks, fixed = false) => ({
  id,
  title: PLAN.bricks[id - 1].title,
  kind: PLAN.bricks[id - 1].kind,
  state,
  text: TEXTS[id],
  fixed,
  checks,
});
const REPORT = {
  strategy: "longForm",
  route: "parts",
  file: DELIVERABLE,
  words: 1290,
  targetWords: 1300,
  checks: [check("C1", "formJudge", true, "« three varieties »")],
  judgedOnOutline: false,
  bricks: [1, 2, 3, 4].map((id) => ({
    id,
    title: PLAN.bricks[id - 1].title,
    state: id === 3 ? "toReview" : "done",
    fixed: id === 3,
    words: 300,
    checks: id === 3 ? FIXED_3 : id === 2 ? PASSING_2 : [],
  })),
  notChecked: ["sources"],
  warnings: [],
  doubtful: 1,
  toReview: 1,
  proofreadBlocks: 4,
  proofreadChanged: 1,
  proofreadRejected: 0,
};

const emit = (kind) => ({ emit: kind });
const phase = (name, started) =>
  emit({ type: started ? "phaseStarted" : "phaseDone", phase: name });
const activity = (code, fields = {}) =>
  emit({ type: "activity", activity: { code, ...fields } });
const brick = (id, state, attempt = 0) =>
  emit({ type: "brick", id, state, attempt });
const checks = (id, attempt, list) =>
  emit({ type: "brickChecks", id, attempt, checks: list });
const save = (key, value) => ({ save: { key, value } });
const wait = (ms) => ({ wait: ms });

/** Déroulé ; `holdMs` : attente pendant la correction de la partie 3. */
function script(holdMs) {
  const simple = (id) => [
    brick(id, "running"),
    activity("writingBrick", { id }),
    wait(200),
    save(`text:${id}:0`, TEXTS[id]),
    brick(id, "checking"),
    activity("checkingBrick", { id }),
    checks(id, 0, id === 2 ? PASSING_2 : []),
    save(`brick:${id}`, record(id, "done", id === 2 ? PASSING_2 : [])),
    brick(id, "done"),
  ];
  return {
    model: "tomato-writer:27b",
    warnings: ["small"],
    steps: [
      phase("collect", true),
      wait(400),
      phase("collect", false),
      phase("strategy", true),
      activity("choosingStrategy", { draw: 1 }),
      wait(400),
      emit({ type: "strategyChosen", strategy: "longForm", unanimous: true }),
      phase("strategy", false),
      phase("framing", true),
      phase("framing", false),
      phase("spec", true),
      activity("writingSpec", { attempt: 1 }),
      wait(400),
      save("spec", SPEC),
      phase("spec", false),
      phase("plan", true),
      activity("writingPlan", { attempt: 1 }),
      wait(300),
      activity("judgingPlan", { attempt: 1 }),
      wait(300),
      // Plan approuvé : sauvé (modifié) et `planReady` par le déroulé.
      { ask: { type: "approvePlan", value: PLAN } },
      phase("plan", false),
      phase("build", true),
      ...simple(2),
      brick(3, "running"),
      activity("writingBrick", { id: 3 }),
      save("text:3:0", TEXTS[3].replace("often", "regularly")),
      brick(3, "checking"),
      activity("checkingBrick", { id: 3 }),
      checks(3, 0, FAILING_3),
      brick(3, "fixing", 1),
      activity("fixingBrick", { id: 3 }),
      wait(holdMs),
      save("text:3:1", TEXTS[3]),
      brick(3, "checking", 1),
      checks(3, 1, FIXED_3),
      save("brick:3", record(3, "toReview", FIXED_3, true)),
      brick(3, "toReview", 1),
      ...simple(1),
      ...simple(4),
      phase("build", false),
      phase("assemble", true),
      activity("assembling"),
      wait(300),
      phase("assemble", false),
      phase("verify", true),
      activity("checkingWhole"),
      wait(300),
      checks(0, 0, REPORT.checks),
      phase("verify", false),
      phase("deliver", true),
      activity("proofreading", { block: 0 }),
      wait(300),
      activity("delivering"),
      {
        write: {
          path: DELIVERABLE,
          text: `# Tomatoes on a balcony\n\n${Object.values(TEXTS).join("\n\n")}\n`,
        },
      },
      { write: { path: ".atelier/rapport.md", text: "# Report\n" } },
      save("report", REPORT),
      save("delivered", DELIVERABLE),
      phase("deliver", false),
      emit({ type: "finished", state: "done" }),
    ],
  };
}
// Correction de la partie 3 tenue jusqu'à la pause ; raccourcie avant la
// reprise (le script est relu à chaque lancement, et la reprise refait
// l'étape interrompue).
writeFileSync(SCRIPT, JSON.stringify(script(120_000)));

// ---------------------------------------------------------------------------
// Outils de la page.

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

const text = (driver, css) =>
  driver.execute(
    "return document.querySelector(arguments[0])?.textContent ?? null",
    [css],
  );
const attribute = (driver, css, name) =>
  driver.execute(
    "return document.querySelector(arguments[0])?.getAttribute(arguments[1]) ?? null",
    [css, name],
  );
const exists = (driver, css) =>
  driver.execute("return document.querySelector(arguments[0]) !== null", [css]);
/** Attend que l'attribut `name` de `css` vaille `value`. */
const waitAttribute = (driver, css, name, value, timeoutMs = 15000) =>
  waitUntil(
    async () => (await attribute(driver, css, name)) === value,
    async () =>
      `${css} [${name}] = ${value} (vu : ${await attribute(driver, css, name)})`,
    timeoutMs,
  );
/** Valeur d'un champ, posée comme React l'attend. */
const setValue = (driver, css, value) =>
  driver.execute(
    `const el = document.querySelector(arguments[0]);
     const proto = el instanceof HTMLTextAreaElement
       ? HTMLTextAreaElement.prototype : el instanceof HTMLSelectElement
       ? HTMLSelectElement.prototype : HTMLInputElement.prototype;
     Object.getOwnPropertyDescriptor(proto, "value").set.call(el, arguments[1]);
     el.dispatchEvent(new Event(el instanceof HTMLSelectElement ? "change" : "input", { bubbles: true }));
     return el.value;`,
    [css, value],
  );

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

/** Captures claire puis sombre de l'écran où `css` est montré. */
async function shootBoth(driver, css, name) {
  await delay(400);
  await driver.screenshot(`.tools/shots/workshop-${name}.png`);
  await driver.invoke("update_settings", { patch: { theme: "dark" } });
  await reload(driver);
  await driver.waitFor(css, { timeoutMs: 15000 });
  await delay(400);
  await driver.screenshot(`.tools/shots/workshop-${name}-dark.png`);
  await driver.invoke("update_settings", { patch: { theme: "light" } });
  await reload(driver);
  await driver.waitFor(css, { timeoutMs: 15000 });
}

const VIEW = "[data-testid=workshop-project-view]";
const partRow = (id) => `[data-testid=workshop-part-row][data-part='${id}']`;

// Notifications seulement : ni zone de notification, ni portail.
const desk = await startDesktopServices({
  notifications: true,
  watcher: false,
  portal: null,
});
const notified = (summary) =>
  desk.notifications().filter((n) => n.summary === summary);

try {
  await withApp(
    binary,
    {
      name: "workshop",
      env: {
        CHATONLM_E2E_WORKSHOP: SCRIPT,
        CHATONLM_E2E_PICK_FILE: PICKS,
        PATH: `${fakeBin}:${process.env.PATH}`,
      },
    },
    async (driver) => {
      await driver.waitFor("[data-testid=new-chat]");
      await driver.execute(WATCH_PAGE);
      // Thème clair d'abord (celui du « système » de la session est sombre) ;
      // les écrans importants sont pris aussi en sombre (`shootBoth`).
      await driver.invoke("update_settings", { patch: { theme: "light" } });
      await reload(driver);
      await driver.waitFor("[data-testid=new-chat]");
      await driver.click(
        await driver.waitFor("button[aria-label='Show sidebar']"),
      );
      await driver.click(await driver.waitFor("nav a[href$='/tools']"));
      await driver.click(
        await driver.waitFor("[data-testid=tool-card-workshop]"),
      );
      await driver.waitFor("[data-testid=workshop-home]");
      await driver.waitFor("[data-testid=workshop-empty]");
      await shootBoth(driver, "[data-testid=workshop-empty]", "home-empty");

      // 1. Nouveau projet.
      await driver.click(await driver.waitFor("[data-testid=workshop-new]"));
      await driver.waitFor("[data-testid=workshop-form]");
      // Modèle choisi par l'Atelier (Auto) et son avertissement.
      await waitAttribute(
        driver,
        "[data-testid=workshop-model-check]",
        "data-model",
        "tomato-writer:27b",
      );
      assert.ok(
        await exists(driver, "[data-testid=workshop-model-warning]"),
        "avertissement du modèle (small)",
      );
      await shootBoth(
        driver,
        "[data-testid=workshop-model-warning]",
        "new-empty",
      );
      // Pièce jointe par le sélecteur (crochet).
      await driver.click(await driver.waitFor("[data-testid=workshop-attach]"));
      await waitUntil(
        async () =>
          (await text(driver, "[data-testid=workshop-attachment]"))?.includes(
            "notes.md",
          ),
        "pastille notes.md",
      );
      // Tâche tapée au vrai clavier, puis Maj+Tab dans la zone : le niveau
      // d'autonomie suivant, le focus reste dans la zone.
      await driver.execute(
        "document.querySelector('[data-testid=workshop-task]').focus()",
      );
      await pressKeys(chordsOf(TASK), { settleMs: 200 });
      assert.equal(
        await driver.execute(
          "return document.querySelector('[data-testid=workshop-task]').value",
        ),
        TASK,
      );
      const before = await attribute(
        driver,
        "[data-testid=workshop-autonomy]",
        "data-level",
      );
      const helpBefore = await text(
        driver,
        "[data-testid=workshop-autonomy-help]",
      );
      await pressKeys("shift+tab", { settleMs: 300 });
      const after = await attribute(
        driver,
        "[data-testid=workshop-autonomy]",
        "data-level",
      );
      assert.notEqual(after, before, "Maj+Tab change l'autonomie");
      assert.notEqual(
        await text(driver, "[data-testid=workshop-autonomy-help]"),
        helpBefore,
        "ligne d'aide du niveau",
      );
      assert.ok(
        await driver.execute(
          "return document.activeElement === document.querySelector('[data-testid=workshop-task]')",
        ),
        "focus resté dans la zone de la tâche",
      );
      // Encore trois fois : le tour complet ramène au niveau de départ.
      await pressKeys(["shift+tab", "shift+tab", "shift+tab"], {
        settleMs: 300,
      });
      assert.equal(
        await attribute(
          driver,
          "[data-testid=workshop-autonomy]",
          "data-level",
        ),
        before,
        "quatre niveaux, en boucle",
      );
      await pressKeys("shift+tab", { settleMs: 300 });
      await driver.screenshot(".tools/shots/workshop-new.png");
      await driver.click(await driver.waitFor("[data-testid=workshop-create]"));

      // 2. Vue du projet, jusqu'à la question du plan.
      await driver.waitFor(VIEW, { timeoutMs: 15000 });
      const id = Number(await attribute(driver, VIEW, "data-project"));
      assert.ok(id > 0, "identifiant du projet");
      const detail = await driver.invoke("workshop_get", { id });
      assert.equal(detail.project.autonomy, after, "autonomie choisie gardée");
      assert.deepEqual(detail.attachments, ["notes.md"]);
      const folder = detail.project.folder;
      assert.ok(
        realpathSync(folder).startsWith(runs),
        `dossier du projet hors de la session : ${folder}`,
      );
      await driver.waitFor("[data-testid=workshop-phases]");
      await driver.waitFor("[data-testid=workshop-need-you]", {
        timeoutMs: 20000,
      });
      await waitAttribute(driver, VIEW, "data-state", "waiting");
      assert.equal(
        await attribute(
          driver,
          "[data-testid=workshop-need-you]",
          "data-question",
        ),
        "approvePlan",
      );
      assert.ok(
        await exists(driver, "[data-testid=workshop-need-you] [role=alert]"),
        "« J'ai besoin de toi » annoncé (role=alert)",
      );
      assert.equal(
        await driver.execute(
          "return document.querySelectorAll('[data-testid=workshop-plan-part]').length",
        ),
        4,
      );
      // Phases : collecte faite, plan en cours.
      assert.equal(
        await attribute(
          driver,
          "[data-testid=workshop-phase][data-phase=collect]",
          "data-mark",
        ),
        "done",
      );
      await waitUntil(
        () => notified("ChatonLM needs you").length === 1,
        () =>
          `notification de la question (reçues : ${JSON.stringify(desk.notifications())})`,
      );
      assert.equal(
        notified("ChatonLM needs you")[0].body,
        `${TASK}: a question is waiting in the Workshop.`,
      );
      await shootBoth(driver, "[data-testid=workshop-question-plan]", "plan");
      // Titre de la partie 2 modifié, plan approuvé.
      await setValue(
        driver,
        "[data-testid=workshop-plan-part][data-part='2'] [data-testid=workshop-plan-part-title]",
        EDITED_TITLE,
      );
      await driver.click(
        await driver.waitFor("[data-testid=workshop-plan-approve]"),
      );

      // 3. Construction : le plan de la vue suit le plan modifié.
      await driver.waitFor(partRow(3), { timeoutMs: 15000 });
      await waitUntil(
        async () => (await text(driver, partRow(2)))?.includes(EDITED_TITLE),
        async () =>
          `titre modifié dans le plan (vu : ${await text(driver, partRow(2))})`,
      );
      await waitAttribute(driver, partRow(2), "data-state", "done");
      await waitAttribute(driver, partRow(3), "data-state", "fixing");
      // Panneau de la partie 2 : texte et vérifications.
      await driver.click(await driver.waitFor(partRow(2)));
      await waitAttribute(
        driver,
        "[data-testid=workshop-part-panel]",
        "data-part",
        "2",
      );
      await waitUntil(
        async () =>
          (await text(driver, "[data-testid=workshop-part-text]"))?.includes(
            "Tiny Tim",
          ),
        "texte de la partie 2",
      );
      assert.equal(
        await driver.execute(
          "return document.querySelectorAll('[data-testid=workshop-part-panel] [data-testid=workshop-check]').length",
        ),
        PASSING_2.length,
      );
      assert.ok(
        await exists(
          driver,
          "[data-testid=workshop-part-panel] [data-testid=workshop-check][data-method=factJudge]",
        ),
        "vérification du juge d'exactitude",
      );
      await shootBoth(driver, VIEW, "running");

      // 4. Pause pendant la correction de la partie 3, puis reprise.
      await driver.click(await driver.waitFor("[data-testid=workshop-pause]"));
      await waitAttribute(driver, VIEW, "data-state", "paused");
      await driver.waitFor("[data-testid=workshop-resume]");
      await delay(300);
      await driver.screenshot(".tools/shots/workshop-paused.png");
      writeFileSync(SCRIPT, JSON.stringify(script(300)));
      await driver.click(await driver.waitFor("[data-testid=workshop-resume]"));

      // 5. Livraison et rapport.
      await waitAttribute(driver, VIEW, "data-state", "done", 30000);
      await driver.waitFor("[data-testid=workshop-report]", {
        timeoutMs: 15000,
      });
      assert.match(
        (await text(driver, "[data-testid=workshop-report-file]")) ?? "",
        new RegExp(DELIVERABLE.replace(/\./g, "\\.")),
      );
      assert.equal(
        await attribute(
          driver,
          "[data-testid=workshop-report-part][data-state=toReview]",
          "data-state",
        ),
        "toReview",
      );
      await waitUntil(
        () => notified("Project finished").length === 1,
        () =>
          `notification de la fin (reçues : ${JSON.stringify(desk.notifications())})`,
      );
      assert.equal(
        notified("Project finished")[0].body,
        `${TASK}: the deliverable is ready in its folder.`,
      );
      await shootBoth(driver, "[data-testid=workshop-report]", "report");
      // Livrable et rapport écrits dans le dossier du projet.
      assert.match(
        readFileSync(path.join(folder, DELIVERABLE), "utf8"),
        /Tiny Tim/,
      );
      assert.ok(existsSync(path.join(folder, ".atelier", "rapport.md")));
      // Parties relues de l'état sauvé : titre modifié, partie 3 à revoir.
      const parts = await driver.invoke("workshop_parts", { id });
      assert.deepEqual(
        parts.map((part) => [part.id, part.state]),
        [
          [1, "done"],
          [2, "done"],
          [3, "toReview"],
          [4, "done"],
        ],
      );
      assert.equal(parts[1].title, EDITED_TITLE);

      // 6. Ouvrir le dossier (menu ⋯ et rapport), puis le livrable : faux
      // xdg-open, aucune fenêtre.
      const openedCount = (target) =>
        opened().filter((t) => t === target).length;
      await driver.click(await driver.waitFor("[data-testid=workshop-more]"));
      await driver.click(
        await driver.waitFor("[data-testid=workshop-open-folder]"),
      );
      await waitUntil(
        () => openedCount(folder) === 1,
        () => `dossier ouvert par le menu (vu : ${opened().join(", ")})`,
      );
      await driver.click(
        await driver.waitFor("[data-testid=workshop-report-open]"),
      );
      await waitUntil(
        () => openedCount(folder) === 2,
        () => `dossier ouvert par le rapport (vu : ${opened().join(", ")})`,
      );
      await driver.click(
        await driver.waitFor("[data-testid=workshop-report-open-file]"),
      );
      await waitUntil(
        () => openedCount(path.join(folder, DELIVERABLE)) === 1,
        () => `livrable ouvert (vu : ${opened().join(", ")})`,
      );
      assert.equal(
        await exists(driver, "[data-testid=workshop-open-error]"),
        false,
        "aucune erreur d'ouverture",
      );

      // 7. Accueil : le projet fini dans la liste.
      await driver.click(await driver.waitFor("[data-testid=workshop-back]"));
      await driver.waitFor(
        `[data-testid=workshop-projects] [data-project='${id}'][data-state=done]`,
      );
      await delay(300);
      await driver.screenshot(".tools/shots/workshop-home.png");

      await collect(driver);
      assert.deepEqual(violations, [], "violations de la CSP");
      assert.deepEqual(consoleErrors, [], "erreurs dans la console");
    },
  );
} finally {
  await desk.stop();
}
console.log("✓ Atelier : création, plan approuvé, pause, livraison, rapport");
