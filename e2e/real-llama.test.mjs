// Conditions réelles : l'app lance le vrai llama-server
// (.tools/runtimes/llama-server) sur un vrai modèle d'essai (Qwen3 0.6B,
// scripts/test-llama.sh fetch, lié dans le dossier des modèles de l'essai),
// et télécharge depuis le vrai Hugging Face. scripts/check.sh --full le lance
// (un seul vrai serveur à la fois : verrou .tools/locks/llama-real.lock pris
// par ce test). `CHATONLM_LLAMA_TEST_CPU=1` : processeur seulement.
// - le sélecteur montre le modèle de la base locale ;
// - Context length réglé à 8k par un clic (vrai pointeur) : le processus est
//   lancé avec un contexte de 8192 (`running_models`) ;
// - deux messages tapés au clavier dans la même conversation : deux vraies
//   réponses, non vides, sans erreur, enregistrées dans l'historique ; le
//   premier (modèle pas encore lancé) montre « Loading model... », pas le
//   second ;
// - page Proofread (modèle du chat) : Correct puis Rephrase au clavier sur
//   deux paragraphes, revue sans erreur ;
// - onglet Study (modèle du chat) : cours de 10 min créé par la page, plan
//   écrit par le vrai modèle (JSON contraint) et montré, Start : premier
//   palier rendu ; à chaque étape, un refus propre est accepté à la place
//   (code `invalid`, message montré : un modèle de 0,6 milliard rate parfois
//   son plan) ; cours supprimé ;
// - onglet Models des réglages : le modèle lancé listé avec son contexte (8k)
//   et sa part GPU/CPU ; Unload : processus arrêté, ligne retirée ;
// - ModelHub avec le vrai Hugging Face (réseau) : recherche « SmolLM2-135M »
//   (« / » et frappe au vrai clavier), fiche du dépôt, version Q4_K_M,
//   Download : progression vue, Installed ; Use in chat : nouvelle
//   conversation avec ce modèle, un échange au clavier sur le vrai moteur ;
//   puis suppression par l'onglet Installed (boîte de confirmation) : fichiers
//   retirés du dossier des modèles de l'essai.
// Les réponses d'un vrai modèle varient : seule leur forme est vérifiée.
import assert from "node:assert/strict";
import { existsSync } from "node:fs";
import { join } from "node:path";
import { setTimeout as delay } from "node:timers/promises";
import { withApp } from "./lib/app.mjs";
import { holdRealLock, startRealEngine } from "./lib/fake.mjs";
import { chordsOf, pressKeys } from "./lib/keyboard.mjs";
import { pointerOffset } from "./lib/pointer.mjs";

const binary = process.argv[2];

// Un seul vrai serveur à la fois sur la machine : verrou commun aux contrats
// et aux essais à la main (.tools/locks/llama-real.lock), tenu par un
// processus gardien jusqu'à la fin du test (son entrée se ferme avec ce
// processus, même s'il échoue).
await holdRealLock();

const engine = startRealEngine(["text"]);
if (!engine) {
  throw new Error(
    "Vrai moteur ou modèle absent : scripts/build-llama-server.sh, scripts/test-llama.sh fetch",
  );
}
const MODEL = engine.ids[0];
const TITLE = MODEL.slice(
  MODEL.indexOf("/") + 1,
  MODEL.lastIndexOf(":"),
).replace(/-GGUF$/i, "");

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

/** Centre (coordonnées de la page) de la zone du cran `label` du curseur. */
const notch = (driver, label) =>
  driver.execute(
    `const text = [...document.querySelectorAll("[data-testid=settings] div")]
       .find((e) => e.children.length === 0 && e.textContent === arguments[0]);
     const r = text.previousElementSibling.getBoundingClientRect();
     return [Math.round(r.left + r.width / 2), Math.round(r.top + r.height / 2)];`,
    [label],
  );

const replies = (driver) =>
  driver.execute(
    "return [...document.querySelectorAll('[data-role=assistant]')].map((e) => e.textContent)",
  );

/** Tape un message au clavier et l'envoie (Entrée), puis attend la fin. */
async function ask(driver, text, count) {
  await driver.click(await driver.waitFor("textarea[aria-label='Message']"));
  await pressKeys([...chordsOf(text), "enter"]);
  // Envoyé : Stop affiché, ou réponse déjà finie (petit modèle rapide).
  await waitUntil(
    async () =>
      (await exists(driver, "button[aria-label='Stop']")) ||
      (await replies(driver)).length >= count,
    `envoi du message ${count}`,
  );
  // Fin : plus de bouton Stop pendant une seconde entière (un vrai modèle
  // peut d'abord raisonner longtemps).
  let since = null;
  await waitUntil(
    async () => {
      if (await exists(driver, "button[aria-label='Stop']")) since = null;
      else since ??= Date.now();
      return since !== null && Date.now() - since > 1000;
    },
    `fin de la réponse ${count}`,
    180_000,
  );
  const texts = await replies(driver);
  assert.equal(texts.length, count, "une réponse par message");
  assert.ok(texts.at(-1).trim().length > 0, "réponse vide");
  assert.equal(
    await exists(driver, "[data-testid=chat] [role=alert]"),
    false,
    "erreur affichée dans la conversation",
  );
}

await withApp(
  binary,
  { name: "real-llama", env: engine.env },
  async (driver) => {
    await driver.waitFor("[data-testid=new-chat]");

    // Sélecteur : le modèle de la base locale, puis MODEL choisi.
    await driver.click(await driver.find("[title='Select model']"));
    const options = await waitUntil(async () => {
      // Identifiant de chaque ligne (`data-model-id`), indépendant du nom
      // court affiché.
      const texts = await driver.execute(
        "return [...document.querySelectorAll('[role=option]')].map((o) => o.dataset.modelId)",
      );
      return texts.includes(MODEL) && texts;
    }, `${MODEL} dans le sélecteur`);
    assert.ok(options.includes(MODEL));
    await driver.execute(
      `[...document.querySelectorAll("[role=option]")]
         .find((o) => o.dataset.modelId === arguments[0]).click()`,
      [MODEL],
    );
    await waitUntil(
      async () => (await driver.invoke("get_settings")).selectedModel === MODEL,
      `${MODEL} enregistré`,
    );

    // Context length : 8k, par un clic sur le cran.
    await pressKeys("ctrl+comma");
    await driver.waitFor("[data-testid=settings]");
    const [dx, dy] = await pointerOffset(driver);
    const [x8, y8] = await notch(driver, "8k");
    await pressKeys(
      [
        `pointer:${x8 + dx},${y8 + dy}`,
        "wait:100",
        "button:down",
        "wait:100",
        "button:up",
      ],
      { settleMs: 300 },
    );
    await waitUntil(
      async () => (await driver.invoke("get_settings")).contextLength === 8192,
      "contexte de 8192 enregistré",
    );
    if (await exists(driver, "button[aria-label='Show sidebar']")) {
      await driver.click(
        await driver.find("button[aria-label='Show sidebar']"),
      );
    }
    await driver.click(await driver.waitFor("nav a[href$='/c/new']"));
    await driver.waitFor("[data-testid=new-chat]");

    // Deux échanges réels dans la même conversation ; ligne de chargement
    // notée par la page (elle ne dure que le chargement, ~1 s après les
    // 750 ms d'attente).
    await driver.execute(
      `window.__loadingSeen = false;
       new MutationObserver(() => {
         if (document.querySelector("[data-testid=loading-model]")) window.__loadingSeen = true;
       }).observe(document.body, { subtree: true, childList: true });`,
    );
    const loadingSeen = () =>
      driver.execute(
        "const seen = window.__loadingSeen; window.__loadingSeen = false; return seen;",
      );
    await ask(driver, "Name the color of the sky in one short sentence.", 1);
    assert.equal(
      await loadingSeen(),
      true,
      "« Loading model... » au 1er envoi",
    );
    await ask(driver, "Now name the color of grass in one short sentence.", 2);
    assert.equal(await loadingSeen(), false, "aucun chargement au 2e envoi");

    const id = (await driver.invoke("list_chats"))[0].id;
    const chat = await driver.invoke("get_chat", { id });
    assert.deepEqual(
      chat.messages.map((m) => m.role),
      ["user", "assistant", "user", "assistant"],
    );
    for (const message of chat.messages) {
      assert.ok(message.content.trim().length > 0, `${message.role} vide`);
      if (message.role === "assistant") assert.equal(message.model, MODEL);
    }

    const loaded = (await driver.invoke("running_models")).find(
      (m) => m.id === MODEL,
    );
    assert.equal(loaded?.contextLength, 8192, "contexte du processus lancé");

    // Page Proofread en « Auto » (le modèle du chat, chargé) : Correct par
    // Ctrl+Entrée puis Rephrase par Ctrl+Maj+Entrée (vraies touches), deux
    // passages, revue sans erreur ; réponses variables : forme seulement.
    if (await exists(driver, "button[aria-label='Show sidebar']")) {
      await driver.click(
        await driver.find("button[aria-label='Show sidebar']"),
      );
    }
    await driver.click(await driver.waitFor("nav a[href$='/proofread']"));
    await driver.waitFor("[data-testid=proofread]");
    const PROOFREAD_TEXT =
      "Je suis allez au cinema hier soir avec mes ami.\n\nI recieve the letter yesterday and I was very happy.";
    await driver.execute(
      `const area = document.querySelector("textarea[aria-label='Text to correct']");
       area.focus();
       area.select();
       document.execCommand("insertText", false, arguments[0]);`,
      [PROOFREAD_TEXT],
    );
    const proofreadDone = async (what) => {
      await driver.waitFor("[data-testid=proofread-review]", {
        timeoutMs: 180000,
      });
      const shown = await driver.execute(
        `const review = document.querySelector("[data-testid=proofread-review]");
         const copy = review.cloneNode(true);
         copy.querySelectorAll("[data-skip]").forEach((e) => e.remove());
         return {
           text: copy.textContent,
           status: document.querySelector("[data-testid=proofread-status]")?.textContent ?? "",
           alert: document.querySelector("[data-testid=proofread] [role=alert]")?.textContent ?? null,
         };`,
      );
      assert.equal(shown.alert, null, `${what} : erreur ${shown.alert}`);
      assert.ok(shown.text.includes("\n\n"), `${what} : deux passages`);
      assert.ok(shown.text.trim().length > 20, `${what} : texte rendu`);
      assert.ok(shown.status.includes(TITLE), `${what} : ${shown.status}`);
      return shown;
    };
    await pressKeys("ctrl+enter");
    const corrected = await proofreadDone("Correct");
    console.log(`  Correct (${MODEL}) : ${JSON.stringify(corrected.text)}`);
    await pressKeys("ctrl+shift+enter");
    await driver.waitFor(
      "[data-testid=proofread-review][aria-label='Rephrased text']",
      {
        timeoutMs: 180000,
      },
    );
    const rephrased = await proofreadDone("Rephrase");
    console.log(`  Rephrase (${MODEL}) : ${JSON.stringify(rephrased.text)}`);

    // Onglet Study : un cours écrit par le vrai modèle.
    if (await exists(driver, "button[aria-label='Show sidebar']")) {
      await driver.click(
        await driver.find("button[aria-label='Show sidebar']"),
      );
    }
    await driver.click(await driver.waitFor("nav a[href$='/study']"));
    await driver.waitFor("[data-testid=study-form] textarea");
    await driver.execute(
      `const area = document.querySelector("[data-testid=study-form] textarea");
       area.focus();
       document.execCommand("insertText", false, "Photosynthesis");
       [...document.querySelectorAll("[data-testid=study-form] label")]
         .find((label) => label.textContent.trim() === "10 min").click();`,
    );
    await driver.click(await driver.waitFor("[data-testid=study-create]"));
    const planned = await waitUntil(
      () =>
        driver.execute(
          `const plan = document.querySelector("[data-testid=study-plan]");
           const error = document.querySelector("[data-testid=study-error]");
           return plan ? { plan: plan.textContent } : error ? { error: error.textContent } : null;`,
        ),
      "plan du cours",
      240000,
    );
    const studyCourse = await driver.execute(
      "return location.pathname.split('/').pop()",
    );
    const prepared = await driver.invoke("study_get", {
      courseId: studyCourse,
    });
    if (planned.error !== undefined) {
      // Refus propre d'un plan inutilisable (réponses d'un modèle de 0,6
      // milliard, redemandées une fois) : code `invalid`, message montré.
      assert.equal(prepared.status, "failed");
      assert.equal(prepared.error.code, "invalid", planned.error);
      console.log(`  Plan (${MODEL}) refusé : ${planned.error}`);
    } else {
      assert.match(planned.plan, /2 levels/);
      const plan = prepared.plan;
      assert.equal(plan.levels.length, 2);
      console.log(`  Plan (${MODEL}) : ${JSON.stringify(plan.title)}`);
      assert.ok(
        await driver.execute(
          `const start = [...document.querySelectorAll("button")]
             .find((b) => b.textContent.trim() === "Start");
           start?.click();
           return Boolean(start);`,
        ),
      );
      // Le lecteur montre d'abord une carte d'attente : l'état du palier est
      // lu dans le moteur, puis la carte qui lui correspond.
      const level = await waitUntil(
        async () => {
          const course = await driver.invoke("study_get", {
            courseId: studyCourse,
          });
          const first = course.levels[0];
          return ["ready", "failed"].includes(first.state) ? first : null;
        },
        "premier palier",
        300000,
      );
      if (level.state === "ready") {
        assert.ok(level.lesson.sections.length >= 2);
        await driver.waitFor("article[data-testid=study-section]", {
          timeoutMs: 20000,
        });
        console.log(
          `  Palier 1 (${MODEL}) : ${level.attempts[0].questions.length} questions`,
        );
      } else {
        // Refus propre : partie en échec, code `invalid`, message montré.
        assert.equal(level.error.code, "invalid", level.error.message);
        await driver.waitFor("[data-testid=study-error]", { timeoutMs: 20000 });
        console.log(`  Palier 1 (${MODEL}) refusé : ${level.error.message}`);
      }
    }
    await driver.invoke("study_delete", { courseId: studyCourse });

    // Onglet Models : le modèle chargé, puis Unload.
    if (await exists(driver, "button[aria-label='Show sidebar']")) {
      await driver.click(
        await driver.find("button[aria-label='Show sidebar']"),
      );
    }
    await driver.click(await driver.waitFor("nav a[href='/settings']"));
    await driver.click(
      await driver.waitFor("[data-testid=settings-tab-models]"),
    );
    const row = `[data-testid=loaded-model][data-model='${MODEL}']`;
    const rowText = await waitUntil(
      () =>
        driver.execute(
          "return document.querySelector(arguments[0])?.textContent ?? null",
          [row],
        ),
      `${MODEL} dans « Loaded models »`,
    );
    assert.match(rowText, /8k context/);
    assert.match(rowText, /(GPU|CPU)/);
    await driver.click(await driver.find(`${row} [data-testid=unload-model]`));
    await waitUntil(
      async () =>
        !(await driver.invoke("running_models")).some((m) => m.id === MODEL),
      `${MODEL} déchargé`,
    );
    await waitUntil(
      async () => !(await exists(driver, row)),
      `${MODEL} retiré de la liste`,
    );
  },
);

// ModelHub avec le vrai Hugging Face : recherche, fiche, téléchargement,
// conversation sur le vrai moteur, suppression.
const HF_REPO = "bartowski/SmolLM2-135M-Instruct-GGUF";
const HF_MODEL = `${HF_REPO}:Q4_K_M`;
const HF_FILE = join(
  engine.modelsDir,
  HF_REPO,
  "SmolLM2-135M-Instruct-Q4_K_M.gguf",
);
/** Le modèle dans la base locale (onglet Installed). */
const hfInstalled = async (driver) =>
  (await driver.invoke("hub_installed")).some(
    (m) => m.model.toLowerCase() === HF_MODEL.toLowerCase(),
  );

/** Entrée de la barre latérale (ouverte au besoin). */
async function sidebar(driver, label) {
  if (await exists(driver, "button[aria-label='Show sidebar']")) {
    await driver.click(await driver.find("button[aria-label='Show sidebar']"));
  }
  await waitUntil(
    () =>
      driver.execute(
        `const link = [...document.querySelectorAll("a")]
           .find((a) => a.textContent.trim() === arguments[0] && a.querySelector("svg"));
         link?.click();
         return link !== undefined;`,
        [label],
      ),
    `entrée ${label} de la barre latérale`,
  );
}

/** Onglet du ModelHub. */
async function tab(driver, label) {
  await waitUntil(
    () =>
      driver.execute(
        `const link = [...document.querySelectorAll("nav[aria-label=ModelHub] a")]
           .find((a) => a.textContent.trim().startsWith(arguments[0]));
         link?.click();
         return link?.getAttribute("aria-current") === "page";`,
        [label],
      ),
    `onglet ${label}`,
  );
}

/** Bouton de la page dont le texte commence par `start`, cliqué ; son texte. */
const clickButton = (driver, scope, start) =>
  driver.execute(
    `const button = [...document.querySelectorAll(arguments[0] + " button")]
       .find((b) => b.textContent.trim().startsWith(arguments[1]));
     button?.click();
     return button?.textContent.trim() ?? null;`,
    [scope, start],
  );

const WATCH_PROGRESS = `
  window.__seen = [];
  const note = () => {
    for (const bar of document.querySelectorAll("aside [role=progressbar]")) {
      const entry = bar.getAttribute("aria-valuenow") + "|" + bar.getAttribute("aria-valuetext");
      if (!window.__seen.includes(entry)) window.__seen.push(entry);
    }
  };
  new MutationObserver(note).observe(document.body,
    { subtree: true, childList: true, attributes: true, characterData: true });`;

await withApp(
  binary,
  {
    name: "real-llama-hub",
    env: {
      ...startRealEngine([]).env,
      CHATONLM_HF_URL: "https://huggingface.co",
      CHATONLM_HF_STATUS_URL: "https://status.huggingface.co",
    },
  },
  async (driver) => {
    await driver.waitFor("[data-testid=new-chat]");
    await sidebar(driver, "ModelHub");
    await driver.waitFor("nav[aria-label=ModelHub]");

    // Recherche au clavier, puis fiche du dépôt.
    await driver.waitFor("[aria-label='Search Hugging Face models']");
    await driver.execute("document.activeElement?.blur()");
    await pressKeys("slash");
    await pressKeys(chordsOf("SmolLM2-135M"));
    const card = `[data-testid=model-card][href$='/hub/model/${HF_REPO}']`;
    await driver.click(await driver.waitFor(card, { timeoutMs: 30000 }));
    await waitUntil(
      async () =>
        (await driver.execute(
          "return document.querySelector('#model-title')?.textContent",
        )) === "SmolLM2-135M-Instruct",
      "fiche du dépôt",
      30000,
    );
    await driver.waitFor("[data-testid=version]");
    if (!(await exists(driver, "[data-testid=version] input[value=Q4_K_M]"))) {
      assert.match(
        await clickButton(driver, "aside", "Show all"),
        /^Show all \d+ versions$/,
      );
    }
    await driver.execute(
      "document.querySelector('[data-testid=version] input[value=Q4_K_M]').click()",
    );
    await waitUntil(
      async () =>
        (await driver.execute(
          "return document.querySelector('[data-testid=model-id]')?.title",
        )) === HF_MODEL,
      "identifiant de Q4_K_M",
    );

    // Téléchargement : progression, puis Installed.
    await driver.execute(WATCH_PROGRESS);
    assert.match(
      await clickButton(driver, "aside", "Download"),
      /^Download · \d+ MB$/,
    );
    await waitUntil(
      async () =>
        (await driver.execute(
          `return [...document.querySelectorAll("aside button")]
             .some((b) => b.textContent.trim() === "Use in chat")`,
        )) && (await hfInstalled(driver)),
      "téléchargement fini",
      180_000,
    );
    const seen = await driver.execute("return window.__seen");
    assert.ok(
      seen.some((entry) => / of /.test(entry)),
      `progression : ${seen.join(" / ")}`,
    );
    assert.ok(existsSync(HF_FILE), `fichier ${HF_FILE}`);
    await driver.screenshot(".tools/shots/real-hub-model.png");

    // Use in chat : nouvelle conversation avec ce modèle, un échange.
    assert.equal(
      await clickButton(driver, "aside", "Use in chat"),
      "Use in chat",
    );
    await driver.waitFor("[data-testid=new-chat]");
    await waitUntil(
      async () =>
        (await driver.invoke("get_settings")).selectedModel === HF_MODEL,
      "modèle choisi",
    );
    await waitUntil(
      async () =>
        (
          await driver.execute(
            `return [...document.querySelector("[title='Select model']").querySelectorAll("span")]
               .filter((s) => s.children.length === 0).map((s) => s.textContent)`,
          )
        )
          .join()
          .startsWith("SmolLM2-135M-Instruct"),
      "sélecteur : titre et étiquette",
    );
    await ask(driver, "Say hello in one short sentence.", 1);
    const id = (await driver.invoke("list_chats"))[0].id;
    const chat = await driver.invoke("get_chat", { id });
    assert.deepEqual(
      chat.messages.map((m) => m.role),
      ["user", "assistant"],
    );
    assert.equal(chat.messages[1].model, HF_MODEL);
    await driver.screenshot(".tools/shots/real-hub-chat.png");

    // Suppression par l'onglet Installed : ligne et fichiers retirés.
    await sidebar(driver, "ModelHub");
    await tab(driver, "Installed");
    await waitUntil(
      () =>
        driver.execute(
          `const row = [...document.querySelectorAll("[data-testid=installed-model]")]
             .find((r) => r.textContent.includes("SmolLM2-135M-Instruct"));
           const button = row && [...row.querySelectorAll("button")]
             .find((b) => (b.getAttribute("aria-label") ?? "").startsWith("Delete "));
           button?.click();
           return Boolean(button);`,
        ),
      "bouton Delete du modèle",
      20000,
    );
    await driver.waitFor("[role=alertdialog]");
    assert.equal(
      await clickButton(driver, "[role=alertdialog]", "Delete"),
      "Delete",
    );
    await waitUntil(
      async () => !(await hfInstalled(driver)),
      "modèle supprimé",
      30000,
    );
    await waitUntil(() => !existsSync(HF_FILE), "fichier supprimé", 30000);
  },
);
