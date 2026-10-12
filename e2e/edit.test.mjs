// Modifier un message dans l'app réelle (build e2e : frontend embarqué, CSP de
// production), avec le faux serveur et de vraies touches du clavier
// (e2e/lib/keyboard.mjs : Maj+Tab, Entrée, Échap). Fichiers créés par le test
// dans le dossier de l'essai (.tools/e2e-runs/run.*/Test Files), joints par
// « + » sans dialogue (crochet CHATONLM_E2E_PICK_FILE). Vérifié :
// - conversation à deux échanges avec chatonlm-vision : premier message avec
//   notes.txt et red.png (vraie image PNG), second message sans pièce jointe ;
// - bouton « edit » : sous chaque message utilisateur, absent des réponses ;
//   invisible (opacity 0) sans survol ni focus ; atteint par de vraies
//   touches Maj+Tab depuis la zone de saisie (second message, puis premier),
//   focus visible (:focus-visible, opacity 1) ;
// - clic (JS) sur celui du premier message : zone de saisie remplie avec son
//   texte et focalisée, « Press ESC to cancel editing », pastilles gardées
//   [notes.txt, red.png] (vignette chargée depuis
//   chatonlm://localhost/attachment/<id>), le premier message et tous les
//   suivants à opacity 0.5 (style calculé), leurs boutons « edit » à 0.3 ;
// - vraie touche Échap : édition annulée (zone vide, pastilles retirées, plus
//   d'indication ni d'estompage, l'image reste servie) ; édition reprise au
//   clavier (Maj+Tab jusqu'au bouton, vraie touche Entrée) ;
// - pastille de l'image gardée retirée, todo.md ajouté par « + », texte
//   changé, vraie touche Entrée : dès l'envoi (MutationObserver sur la liste
//   des messages), ni le second échange ni l'ancienne réponse ne sont jamais
//   affichés ; requête /api/chat (GET /fake/requests) : le seul message
//   modifié (texte nouveau, notes.txt puis todo.md encadrés « --- File: »),
//   aucune image ; nouvelle réponse ; get_chat : deux messages (le modifié,
//   même id, pièces jointes [notes.txt (même id), todo.md], et la réponse) ;
//   l'image retirée n'est plus servie ; même affichage après un rechargement ;
// - pendant une réponse (chatonlm-markdown-slow, environ 8 s) : édition du
//   message en cours de réponse ; vraie touche Entrée : rien n'est envoyé
//   (édition et texte gardés) ; Échap annule l'édition sans arrêter la
//   réponse (elle s'allonge, Stop reste, aucun flux abandonné) ; une seconde
//   Échap l'arrête (flux abandonné, Send revient, texte figé) ;
// - aucune violation de la CSP ni erreur ou avertissement dans la console.
import assert from "node:assert/strict";
import { mkdirSync, realpathSync, writeFileSync } from "node:fs";
import path from "node:path";
import { crc32, deflateSync } from "node:zlib";
import { ROOT, withApp } from "./lib/app.mjs";
import { startFake } from "./lib/fake.mjs";
import { PENDING_CALLS } from "./lib/page.mjs";
import { pressKeys } from "./lib/keyboard.mjs";

const binary = process.argv[2];

const delay = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

// Fichiers de l'essai : à côté du HOME de la session (.tools/e2e-runs/run.*,
// supprimé à la fin de la session), jamais ailleurs.
const home = realpathSync(process.env.HOME ?? "/");
const runs = path.join(realpathSync(ROOT), ".tools", "e2e-runs") + path.sep;
assert.ok(home.startsWith(runs), `HOME hors de .tools/e2e-runs : ${home}`);
const FILES = path.join(path.dirname(home), "Test Files");
mkdirSync(FILES, { recursive: true });
const file = (name) => path.join(FILES, name);
const PICKS = file("picks.txt");

/** Image PNG d'une couleur (RVB), `width` × `height` pixels. */
function png(width, height, [r, g, b]) {
  const row = Buffer.alloc(1 + 3 * width);
  for (let x = 0; x < width; x++) row.set([r, g, b], 1 + 3 * x);
  const raw = Buffer.concat(Array.from({ length: height }, () => row));
  const chunk = (type, data) => {
    const body = Buffer.concat([Buffer.from(type, "ascii"), data]);
    const length = Buffer.alloc(4);
    length.writeUInt32BE(data.length);
    const crc = Buffer.alloc(4);
    crc.writeUInt32BE(crc32(body));
    return Buffer.concat([length, body, crc]);
  };
  const header = Buffer.alloc(13);
  header.writeUInt32BE(width, 0);
  header.writeUInt32BE(height, 4);
  header.set([8, 2, 0, 0, 0], 8); // 8 bits, RVB
  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk("IHDR", header),
    chunk("IDAT", deflateSync(raw)),
    chunk("IEND", Buffer.alloc(0)),
  ]);
}

const NOTES = "Line 1\nLine 2";
const TODO = "- buy milk\n";
const RED = png(4, 4, [255, 0, 0]);
writeFileSync(file("notes.txt"), NOTES);
writeFileSync(file("red.png"), RED);
writeFileSync(file("todo.md"), TODO);

/** Fichiers « choisis » au prochain clic sur « + » (crochet e2e). */
const pick = (...names) =>
  writeFileSync(PICKS, `${names.map(file).join("\n")}\n`);

const FIRST_TEXT = "Describe these files.";
const SECOND_TEXT = "And a second question.";
const EDITED_TEXT = "Only summarize the text files.";
const SLOW_TEXT = "Write a long answer.";
const REPLY = '"Hi!"';
const HINT = "Press ESC to cancel editing";
const frame = (name, text) =>
  `\n--- File: ${name} ---\n${text}\n--- End of ${name} ---`;

const fake = await startFake("ollama");

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

/** Corps des /api/chat reçus par le faux serveur, d'un modèle. */
async function requestsOf(model) {
  const response = await fetch(new URL("/fake/requests", fake.url));
  return (await response.json()).filter((request) => request.model === model);
}

// Violations de la CSP, erreurs et avertissements de la console : notés par
// la page (écouteurs reposés après chaque rechargement), relevés avant chaque
// rechargement et à la fin.
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

/**
 * Recharge la page et attend la nouvelle (écouteurs reposés), seulement quand
 * aucun appel au backend n'est en cours (voir thinking.test.mjs).
 */
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

/** Choisit un modèle (absent du serveur) par le réglage, puis recharge. */
async function useModel(driver, model) {
  assert.ok(await shownModel(driver), "aucun modèle affiché avant useModel");
  await driver.invoke("update_settings", { patch: { selectedModel: model } });
  await reload(driver);
  await waitUntil(
    async () => (await shownModel(driver)) === model,
    `modèle ${model}`,
  );
}

/** Nouvelle conversation, par le bouton « New chat ». */
async function newChat(driver) {
  await driver.click(await driver.waitFor("a[title='New chat']"));
  await driver.waitFor("[data-testid=new-chat]");
}

/** Identifiant de la conversation affichée. */
const currentChatId = (driver) =>
  driver.execute(
    "return (location.hash + location.pathname).match(/\\/c\\/([^/?#]+)/)?.[1] ?? null",
  );

const TEXTAREA = "textarea[aria-label='Message']";
const PLUS = "button[aria-label='Upload multiple files']";

const focusTextarea = (driver) =>
  driver.execute("document.querySelector(arguments[0]).focus()", [TEXTAREA]);

/** Clic sur « + » : fichiers choisis par le crochet (`pick`). */
async function clickPlus(driver) {
  await driver.click(await driver.waitFor(PLUS));
}

/**
 * Zone de saisie : texte, focus, indication de l'édition, pastilles (nom,
 * vignette), lectures en cours, erreurs, bouton d'envoi (Send ou Stop).
 */
const composerState = (driver) =>
  driver.execute(
    `const textarea = document.querySelector(arguments[0]);
     const hint = arguments[1];
     return {
       value: textarea.value,
       focused: document.activeElement === textarea,
       hint: [...document.querySelectorAll("p")]
         .filter((p) => p.textContent === hint).length,
       files: [...document.querySelectorAll("[data-testid=composer-file]")].map((chip) => {
         const img = chip.querySelector("img");
         return {
           name: chip.querySelector("span").textContent,
           image: img && {
             src: img.getAttribute("src"),
             loaded: img.complete && img.naturalWidth > 0,
           },
         };
       }),
       reading: document.querySelectorAll("[data-testid=composer-file-reading]").length,
       errors: document.querySelectorAll("[data-testid=composer-file-error]").length,
       button: document.querySelector(
         "button[aria-label='Send message'], button[aria-label='Stop']",
       )?.getAttribute("aria-label") ?? null,
     };`,
    [TEXTAREA, HINT],
  );

/** Attend un état de la zone de saisie qui vérifie `check`, et le renvoie. */
async function waitComposer(driver, check, what, timeoutMs) {
  let state;
  await waitUntil(
    async () => {
      state = await composerState(driver);
      return check(state);
    },
    () => `${what} : ${JSON.stringify(state)}`,
    timeoutMs,
  );
  return state;
}

/**
 * Messages affichés : rôle, opacité calculée du message, texte, pastilles de
 * la bulle, images, bouton « edit » (texte, opacité calculée, focus, focus
 * visible, bulle survolée).
 */
const messagesState = (driver) =>
  driver.execute(`
    return [...document.querySelectorAll("[data-message-index]")].map((item) => {
      const bubble = item.querySelector("[data-role=user]");
      const edit = item.querySelector(".edit-button");
      return {
        role: bubble ? "user" : "assistant",
        opacity: getComputedStyle(item.firstElementChild).opacity,
        text: bubble
          ? bubble.querySelector(".whitespace-pre-wrap")?.textContent ?? null
          : item.querySelector("[data-role=assistant]")?.textContent ?? null,
        files: bubble
          ? [...bubble.querySelectorAll(".rounded-lg span")].map((s) => s.textContent)
          : [],
        images: [...item.querySelectorAll("img")].map((img) => ({
          src: img.getAttribute("src"),
          loaded: img.complete && img.naturalWidth > 0,
        })),
        edit: edit && {
          text: edit.textContent,
          opacity: getComputedStyle(edit).opacity,
          focused: document.activeElement === edit,
          focusVisible: edit.matches(":focus-visible"),
          hovered: bubble.matches(":hover"),
        },
      };
    });`);

/** Attend des messages affichés qui vérifient `check`, et les renvoie. */
async function waitMessages(driver, check, what, timeoutMs) {
  let messages;
  await waitUntil(
    async () => {
      messages = await messagesState(driver);
      return check(messages);
    },
    () => `${what} : ${JSON.stringify(messages)}`,
    timeoutMs,
  );
  return messages;
}

/** Bouton « edit » du message de cet index (élément WebDriver). */
const editButton = (driver, index) =>
  driver.waitFor(`[data-message-index='${index}'] .edit-button`);

/**
 * Vraies touches Maj+Tab depuis la zone de saisie, une à une, jusqu'au bouton
 * « edit » du message `index` (au plus `max` touches) ; renvoie les éléments
 * traversés (pour le diagnostic).
 */
async function shiftTabToEdit(driver, index, max = 6) {
  await focusTextarea(driver);
  const path = [];
  for (let i = 0; i < max; i++) {
    await pressKeys("shift+tab", { settleMs: 100 });
    const step = await driver.execute(
      `const active = document.activeElement;
       const item = active?.closest("[data-message-index]");
       return {
         name: active?.getAttribute("aria-label") ?? active?.textContent ?? null,
         index: item ? Number(item.dataset.messageIndex) : null,
         edit: active?.classList.contains("edit-button") ?? false,
       };`,
    );
    path.push(step);
    if (step.edit && step.index === index) return path;
  }
  throw new Error(
    `Maj+Tab n'atteint pas le bouton « edit » du message ${index} : ${JSON.stringify(path)}`,
  );
}

/**
 * L'image de cette adresse chatonlm:// se charge-t-elle maintenant ? Adresse
 * neuve (requête ignorée par le protocole) : jamais l'image déjà en cache.
 */
const imageLoads = (driver, src) =>
  driver.executeAsync(
    `const done = arguments[arguments.length - 1];
     const image = new Image();
     image.onload = () => done(image.naturalWidth > 0);
     image.onerror = () => done(false);
     image.src = arguments[0] + "?check=" + Math.random();`,
    [src],
  );

/** Texte de la dernière réponse affichée. */
const lastReply = (driver) =>
  driver.execute(
    "return [...document.querySelectorAll('[data-role=assistant]')].at(-1)?.textContent ?? ''",
  );

/** fn(driver), avec une capture de la fenêtre en cas d'échec. */
const shotOnFailure = (name, fn) => async (driver) => {
  try {
    await fn(driver);
  } catch (error) {
    await driver.screenshot(`.tools/shots/${name}-failure.png`).catch(() => {});
    throw error;
  }
};

/** Mode édition du premier message (texte, focus, pastilles gardées). */
async function waitEditingFirst(driver, redUrl) {
  const state = await waitComposer(
    driver,
    (s) =>
      s.value === FIRST_TEXT &&
      s.hint === 1 &&
      s.files.length === 2 &&
      s.files[1].image?.loaded,
    "mode édition du premier message",
  );
  assert.equal(state.focused, true, "focus hors de la zone de saisie");
  assert.deepEqual(
    state.files.map((f) => f.name),
    ["notes.txt", "red.png"],
  );
  assert.equal(state.files[0].image, null);
  assert.equal(state.files[1].image.src, redUrl);
  assert.equal(state.errors, 0);
  assert.equal(state.reading, 0);
  return state;
}

/**
 * Snapshots de la liste des messages (textes des bulles, nombre de
 * réponses) à chaque mutation, à partir de maintenant.
 */
const OBSERVE_LIST = `
  window.__listObserver?.disconnect();
  window.__snapshots = [];
  const list = document.querySelector("[data-role=message-list]");
  const snapshot = () => {
    const items = [...list.querySelectorAll("[data-message-index]")];
    window.__snapshots.push({
      users: items
        .filter((item) => item.querySelector("[data-role=user]"))
        .map((item) =>
          item.querySelector("[data-role=user] .whitespace-pre-wrap")?.textContent ?? null),
      replies: items.filter((item) => !item.querySelector("[data-role=user]")).length,
    });
  };
  window.__listObserver = new MutationObserver(snapshot);
  window.__listObserver.observe(list, { childList: true, subtree: true, characterData: true });`;

try {
  await withApp(
    binary,
    {
      name: "edit",
      env: { OLLAMA_HOST: fake.url, CHATONLM_E2E_PICK_FILE: PICKS },
    },
    shotOnFailure("edit", async (driver) => {
      await driver.waitFor("[data-testid=new-chat]");
      await waitUntil(
        async () => (await shownModel(driver)) === "qwen3:0.6b",
        "modèle par défaut",
      );

      // Écouteur de la CSP, vérifié sur un script en ligne refusé.
      await driver.execute(WATCH_PAGE);
      await driver.execute(`
        const script = document.createElement("script");
        script.textContent = "window.__inline = true";
        document.head.append(script);`);
      await waitUntil(
        async () => (await driver.execute("return window.__csp.length")) > 0,
        "violation témoin de la CSP",
      );
      assert.equal(
        await driver.execute("return typeof window.__inline"),
        "undefined",
      );
      await driver.execute("window.__csp = []");

      await useModel(driver, "chatonlm-vision");

      // 1. Deux échanges : le premier avec notes.txt et red.png.
      pick("notes.txt", "red.png");
      await clickPlus(driver);
      await waitComposer(
        driver,
        (s) =>
          s.files.length === 2 && s.reading === 0 && s.files[1].image?.loaded,
        "pastilles du premier message",
      );
      await driver.type(await driver.waitFor(TEXTAREA), FIRST_TEXT);
      await focusTextarea(driver);
      await pressKeys("enter");
      await driver.waitFor("[data-testid=chat]");
      await waitMessages(
        driver,
        (m) =>
          m.length === 2 &&
          m[1].text === REPLY &&
          m[0].edit !== null &&
          m[0].images[0]?.loaded,
        "premier échange",
      );
      await waitComposer(
        driver,
        (s) => s.button === "Send message",
        "fin de la première réponse",
      );
      const chatId = await currentChatId(driver);
      assert.ok(chatId, "conversation créée");

      await driver.type(await driver.waitFor(TEXTAREA), SECOND_TEXT);
      await focusTextarea(driver);
      await pressKeys("enter");
      let messages = await waitMessages(
        driver,
        (m) =>
          m.length === 4 &&
          m[2].text === SECOND_TEXT &&
          m[3].text === REPLY &&
          m[2].edit !== null,
        "second échange",
      );
      await waitComposer(
        driver,
        (s) => s.button === "Send message",
        "fin de la seconde réponse",
      );
      let saved = (await driver.invoke("get_chat", { id: chatId })).messages;
      assert.deepEqual(
        saved.map((m) => [m.role, m.content]),
        [
          ["user", FIRST_TEXT],
          ["assistant", REPLY],
          ["user", SECOND_TEXT],
          ["assistant", REPLY],
        ],
      );
      const firstId = saved[0].id;
      const [notes, red] = saved[0].attachments;
      assert.deepEqual(
        [notes.filename, red.filename],
        ["notes.txt", "red.png"],
      );
      const redUrl = `chatonlm://localhost/attachment/${red.id}`;
      assert.equal(messages[0].images[0].src, redUrl);
      assert.equal((await requestsOf("chatonlm-vision")).length, 2);

      // 2. Bouton « edit » : sous chaque message utilisateur, jamais sous une
      // réponse ; invisible sans survol ni focus.
      assert.deepEqual(
        messages.map((m) => m.role),
        ["user", "assistant", "user", "assistant"],
      );
      for (const message of messages) {
        if (message.role === "assistant") {
          assert.equal(message.edit, null, "bouton « edit » sous une réponse");
          continue;
        }
        assert.equal(message.edit.text, "edit");
        assert.equal(message.edit.hovered, false, "bulle survolée");
        assert.equal(message.edit.focused, false);
        assert.equal(message.edit.opacity, "0", "« edit » visible au repos");
        assert.equal(message.opacity, "1");
      }
      assert.equal(
        await driver.execute(
          `return [...document.querySelectorAll(".edit-button")]
             .filter((button) => !button.closest("[data-role=user]")).length`,
        ),
        0,
        "bouton « edit » hors d'une bulle",
      );

      // Atteint au clavier : vraies touches Maj+Tab depuis la zone de saisie
      // (copie de la seconde réponse, puis « edit » du second message) ;
      // focus visible.
      await shiftTabToEdit(driver, 2);
      messages = await messagesState(driver);
      assert.equal(messages[2].edit.focused, true);
      assert.equal(messages[2].edit.focusVisible, true);
      assert.equal(
        messages[2].edit.opacity,
        "1",
        "« edit » invisible au focus",
      );
      assert.equal(messages[0].edit.opacity, "0");
      // Puis celui du premier message.
      await shiftTabToEdit(driver, 0);
      messages = await messagesState(driver);
      assert.equal(messages[0].edit.focused, true);
      assert.equal(messages[0].edit.focusVisible, true);
      assert.equal(
        messages[0].edit.opacity,
        "1",
        "« edit » invisible au focus",
      );
      assert.equal(messages[2].edit.opacity, "0");
      await driver.screenshot(".tools/shots/edit-focus.png");

      // Clic (JS) sur « edit » du premier message : zone de saisie remplie et
      // focalisée, indication, pastilles gardées avec la vignette de l'image
      // enregistrée ; lui et tous les suivants estompés.
      await driver.click(await editButton(driver, 0));
      await waitEditingFirst(driver, redUrl);
      messages = await waitMessages(
        driver,
        (m) =>
          m.length === 4 && m.every((message) => message.opacity === "0.5"),
        "messages estompés",
      );
      assert.equal(messages[0].edit.opacity, "0.3");
      assert.equal(messages[2].edit.opacity, "0.3");
      assert.equal(
        messages[0].text,
        FIRST_TEXT,
        "message modifié avant l'envoi",
      );
      await driver.screenshot(".tools/shots/edit-editing.png");

      // 3. Vraie touche Échap : édition annulée.
      await pressKeys("escape");
      let state = await waitComposer(
        driver,
        (s) => s.value === "" && s.hint === 0 && s.files.length === 0,
        "édition annulée",
      );
      assert.equal(state.focused, true);
      messages = await waitMessages(
        driver,
        (m) => m.length === 4 && m.every((message) => message.opacity === "1"),
        "messages plus estompés",
      );
      assert.equal(messages[0].edit.opacity, "0");
      assert.equal(messages[0].text, FIRST_TEXT);
      // Rien n'a changé : l'image du message reste servie.
      assert.equal(await imageLoads(driver, redUrl), true);
      assert.equal((await requestsOf("chatonlm-vision")).length, 2);

      // Édition reprise au clavier : Maj+Tab jusqu'au bouton, vraie Entrée.
      await shiftTabToEdit(driver, 0);
      await pressKeys("enter");
      await waitEditingFirst(driver, redUrl);
      await waitMessages(
        driver,
        (m) =>
          m.length === 4 && m.every((message) => message.opacity === "0.5"),
        "messages estompés (édition reprise)",
      );

      // 4. Image gardée retirée, todo.md ajouté, texte changé.
      await driver.click(
        await driver.find(
          "[data-testid=composer-file] button[aria-label='Remove red.png']",
        ),
      );
      await waitComposer(
        driver,
        (s) => s.files.length === 1 && s.files[0].name === "notes.txt",
        "image gardée retirée",
      );
      pick("todo.md");
      await clickPlus(driver);
      state = await waitComposer(
        driver,
        (s) => s.files.length === 2 && s.reading === 0,
        "todo.md ajouté",
      );
      assert.deepEqual(
        state.files.map((f) => f.name),
        ["notes.txt", "todo.md"],
      );
      assert.equal(state.hint, 1);
      await driver.clear(await driver.find(TEXTAREA));
      await driver.type(await driver.find(TEXTAREA), EDITED_TEXT);
      state = await composerState(driver);
      assert.equal(state.value, EDITED_TEXT);
      assert.equal(state.button, "Send message");

      // Envoi par une vraie touche Entrée, la liste des messages observée.
      await driver.execute(OBSERVE_LIST);
      await focusTextarea(driver);
      await pressKeys("enter");
      await waitMessages(
        driver,
        (m) =>
          m.length === 2 &&
          m[0].text === EDITED_TEXT &&
          m[1].text === REPLY &&
          m[0].edit !== null,
        "message modifié et nouvelle réponse",
      );
      state = await waitComposer(
        driver,
        (s) => s.button === "Send message",
        "fin de la réponse au message modifié",
      );
      assert.equal(state.value, "");
      assert.equal(state.hint, 0, "indication d'édition après l'envoi");
      assert.deepEqual(state.files, []);
      const snapshots = await driver.execute(
        "window.__listObserver.disconnect(); return window.__snapshots;",
      );
      assert.ok(snapshots.length > 0, "liste des messages jamais modifiée");
      // Premier rendu après l'envoi, avant toute réponse : le message
      // envoyé seul à la place des quatre.
      assert.deepEqual(
        snapshots[0],
        { users: [EDITED_TEXT], replies: 0 },
        `premier rendu après l'envoi : ${JSON.stringify(snapshots)}`,
      );
      for (const snapshot of snapshots) {
        assert.ok(
          !snapshot.users.includes(SECOND_TEXT) &&
            !snapshot.users.includes(FIRST_TEXT) &&
            snapshot.replies <= 1,
          `messages remplacés encore affichés après l'envoi : ${JSON.stringify(snapshot)}`,
        );
      }
      assert.ok(
        snapshots.some((s) => s.users.includes(EDITED_TEXT)),
        `message modifié jamais affiché : ${JSON.stringify(snapshots)}`,
      );

      // Requête : le seul message modifié, texte des fichiers encadré dans
      // l'ordre (gardé puis nouveau), aucune image.
      const visionRequests = await requestsOf("chatonlm-vision");
      assert.equal(visionRequests.length, 3);
      const request = visionRequests[2];
      assert.equal(
        request.messages.length,
        1,
        JSON.stringify(request.messages),
      );
      assert.equal(request.messages[0].role, "user");
      assert.equal(
        request.messages[0].content,
        `${EDITED_TEXT}${frame("notes.txt", NOTES)}${frame("todo.md", TODO)}`,
      );
      assert.equal(request.messages[0].images, undefined, "image envoyée");
      const raw = JSON.stringify(request);
      assert.ok(!raw.includes(SECOND_TEXT), "second échange envoyé");
      assert.ok(!raw.includes(RED.toString("base64")), "image envoyée");

      // En base : le message modifié (même id, notes.txt gardé avec son id,
      // puis todo.md) et la nouvelle réponse, rien d'autre.
      saved = (await driver.invoke("get_chat", { id: chatId })).messages;
      assert.deepEqual(
        saved.map((m) => [m.role, m.content]),
        [
          ["user", EDITED_TEXT],
          ["assistant", REPLY],
        ],
      );
      assert.equal(saved[0].id, firstId, "message modifié : autre id");
      assert.deepEqual(
        saved[0].attachments.map((a) => a.filename),
        ["notes.txt", "todo.md"],
      );
      assert.equal(saved[0].attachments[0].id, notes.id);
      // L'image retirée n'est plus servie (supprimée de la base).
      await waitUntil(
        async () => !(await imageLoads(driver, redUrl)),
        "image retirée toujours servie",
      );

      // Affichage : celui de la base, sans estompage.
      const isEditedView = (m) =>
        m.length === 2 &&
        m[0].role === "user" &&
        m[0].text === EDITED_TEXT &&
        JSON.stringify(m[0].files) ===
          JSON.stringify(["notes.txt", "todo.md"]) &&
        m[0].images.length === 0 &&
        m[0].edit?.opacity === "0" &&
        m[1].role === "assistant" &&
        m[1].text === REPLY &&
        m[1].edit === null &&
        m.every((message) => message.opacity === "1");
      await waitMessages(driver, isEditedView, "conversation modifiée");
      await driver.screenshot(".tools/shots/edit-sent.png");

      // Après un rechargement : même affichage.
      await reload(driver);
      await driver.waitFor("[data-testid=chat]");
      assert.equal(await currentChatId(driver), chatId);
      await waitMessages(driver, isEditedView, "conversation modifiée relue");

      // 5. Pendant une réponse (chatonlm-markdown-slow, environ 8 s).
      const disconnectsBefore = await fake.disconnects();
      await newChat(driver);
      await useModel(driver, "chatonlm-markdown-slow");
      await driver.type(await driver.waitFor(TEXTAREA), SLOW_TEXT);
      await focusTextarea(driver);
      await pressKeys("enter");
      await driver.waitFor("[data-testid=chat]");
      await waitMessages(
        driver,
        (m) =>
          m.length === 2 &&
          m[0].text === SLOW_TEXT &&
          m[0].edit !== null &&
          (m[1].text ?? "").length > 0,
        "réponse lente commencée",
      );
      assert.equal((await composerState(driver)).button, "Stop");

      // Édition du message en cours de réponse.
      await driver.click(await editButton(driver, 0));
      state = await waitComposer(
        driver,
        (s) => s.value === SLOW_TEXT && s.hint === 1,
        "mode édition pendant la réponse",
      );
      assert.equal(state.focused, true);
      assert.equal(state.button, "Stop");

      // Vraie touche Entrée : rien n'est envoyé, l'édition reste.
      await pressKeys("enter", { settleMs: 400 });
      state = await composerState(driver);
      assert.equal(state.value, SLOW_TEXT, "texte changé par Entrée");
      assert.equal(state.hint, 1, "édition quittée par Entrée");
      assert.equal(state.button, "Stop", "réponse finie trop tôt (Entrée)");
      assert.equal((await requestsOf("chatonlm-markdown-slow")).length, 1);
      messages = await messagesState(driver);
      assert.equal(messages.length, 2);
      assert.equal(messages[0].text, SLOW_TEXT);

      // Échap : édition annulée, la réponse continue.
      const beforeEscape = (await lastReply(driver)).length;
      await pressKeys("escape");
      state = await waitComposer(
        driver,
        (s) => s.value === "" && s.hint === 0,
        "édition annulée pendant la réponse",
      );
      assert.equal(state.button, "Stop", "réponse arrêtée par la 1re Échap");
      await waitUntil(
        async () => (await lastReply(driver)).length > beforeEscape,
        "réponse figée après la 1re Échap",
      );
      assert.equal((await composerState(driver)).button, "Stop");
      assert.equal(await fake.disconnects(), disconnectsBefore);

      // Seconde Échap : la réponse s'arrête (flux abandonné par l'app).
      await pressKeys("escape");
      await waitComposer(
        driver,
        (s) => s.button === "Send message",
        "réponse pas arrêtée par la 2e Échap",
      );
      await waitUntil(
        async () => (await fake.disconnects()) === disconnectsBefore + 1,
        "flux pas abandonné après la 2e Échap",
      );
      const stopped = await lastReply(driver);
      await delay(600);
      assert.equal(await lastReply(driver), stopped, "réponse pas figée");
      assert.equal((await requestsOf("chatonlm-markdown-slow")).length, 1);
      await collectViolations(driver);
    }),
  );
} finally {
  await fake.stop();
}

assert.deepEqual(violations, [], "violations de la CSP");
assert.deepEqual(consoleErrors, [], "erreurs dans la console");
assert.deepEqual(consoleWarnings, [], "avertissements dans la console");
