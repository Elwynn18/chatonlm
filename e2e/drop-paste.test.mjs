// Glisser-déposer et collage de fichiers dans l'app réelle (build e2e :
// frontend embarqué, CSP de production), avec le faux serveur, de vraies
// touches du clavier (e2e/lib/keyboard.mjs) et un vrai presse-papiers de la
// session (e2e/lib/clipboard-writer.py : une autre application GTK 3 qui
// copie). Fichiers créés dans le dossier de l'essai (.tools/e2e-runs/run.*/
// Drop Files : chemin avec une espace et des majuscules).
// - Dépôt (évènements tauri://drag-* émis vers la fenêtre « main », comme le
//   fait Tauri à partir du dépôt natif GTK : même cible, même charge) :
//   drag-enter sans chemin (glisser de texte) : aucun voile ; avec des
//   chemins : voile officiel sur toute la fenêtre (rectangle = fenêtre, par-
//   dessus la barre latérale, son bouton et la zone de saisie), texte « Drop
//   files here or paste from clipboard to add them to your message », icône,
//   inactif à la souris ; captures drop-overlay-light.png et -dark.png
//   (couleurs du voile différentes) ; drag-over le garde ; drag-leave le
//   retire sans rien joindre ; drag-drop le retire et joint les fichiers dans
//   l'ordre, un dossier donnant ses fichiers de premier niveau triés par nom
//   (sous-dossier ignoré), report.docx refusé « File type not supported »,
//   vignette de l'image chargée (chatonlm://localhost/staged/<id>). Sur la
//   page de nouvelle conversation et dans une conversation existante ; page
//   Settings (pas de zone de saisie) : aucun voile. Un évènement émis par la
//   page n'autorise aucun chemin (seul le dépôt natif intercepté en Rust le
//   fait) : chaque dépôt simulé remet d'abord ses chemins par la commande
//   e2e_allow_paths ; sans elle, ou une seconde fois avec un chemin déjà
//   préparé, pastille « Error reading file », rien de joint.
// - Vrai glisser-déposer natif (e2e/lib/drag.mjs) : une source GTK 3 offre
//   notes.txt et le dossier en text/uri-list, le pointeur de KWin (interface
//   EIS, e2e/lib/keyboard.py) la saisit et la lâche sur l'app : voile sur
//   toute la fenêtre pendant le survol, retiré au dépôt, pastilles dans
//   l'ordre (capture drop-native.png), sans e2e_allow_paths (chemins remis
//   par le dépôt natif lui-même).
// - Collage réel (Ctrl+V, vraie touche, focus dans la zone de saisie) :
//   texte seul : collé dans la zone, aucune pastille ; presse-papiers d'un
//   gestionnaire de fichiers (text/uri-list d'un fichier existant +
//   text/plain de son chemin) : WebKitGTK n'expose que le type
//   text/uri-list, sans données ni fichier, et colle le chemin : texte collé,
//   aucune pastille (l'officiel colle aussi le texte) ; image seule (type
//   image/png brut, ou GtkClipboard.set_image) : WebKitGTK 2.52 n'expose
//   rien à la page (aucun type, élément ni fichier) ; l'app demande alors
//   l'image au presse-papiers GTK (stage_clipboard_image : le propriétaire
//   reçoit une demande image/png) : pastille « image.png », vignette chargée,
//   rien de collé dans la zone de saisie.
// - Collage d'un fichier (évènement paste de la page, DataTransfer construit
//   comme celui d'un navigateur qui expose l'image) : image sans nom →
//   pastille « image.png », vignette chargée, collage empêché ; fichier de
//   10 Mio + 1 octet → « File too large » (octets non lus) ; envoi par une
//   vraie touche Entrée à chatonlm-vision : l'image en base64 dans la requête
//   (GET /fake/requests), vignette du message envoyé, pièce jointe en base.
// - Aucune violation de la CSP ni erreur ou avertissement dans la console.
import assert from "node:assert/strict";
import { mkdirSync, realpathSync, writeFileSync } from "node:fs";
import path from "node:path";
import { crc32, deflateSync } from "node:zlib";
import { ROOT, withApp } from "./lib/app.mjs";
import { holdClipboard } from "./lib/clipboard.mjs";
import { DRAG_SOURCE_CAPTION, startDragSource } from "./lib/drag.mjs";
import { startFake } from "./lib/fake.mjs";
import { PENDING_CALLS } from "./lib/page.mjs";
import { pressKeys } from "./lib/keyboard.mjs";
import { forWindows, runKWinScript } from "./lib/kwin.mjs";

const binary = process.argv[2];

const delay = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

// Fichiers de l'essai : à côté du HOME de la session (.tools/e2e-runs/run.*,
// supprimé à la fin de la session), jamais ailleurs.
const home = realpathSync(process.env.HOME ?? "/");
const runs = path.join(realpathSync(ROOT), ".tools", "e2e-runs") + path.sep;
assert.ok(home.startsWith(runs), `HOME hors de .tools/e2e-runs : ${home}`);
const FILES = path.join(path.dirname(home), "Drop Files");
const FOLDER = path.join(FILES, "Folder A");
mkdirSync(path.join(FOLDER, "sub"), { recursive: true });
const file = (name) => path.join(FILES, name);

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
writeFileSync(file("notes.txt"), NOTES);
writeFileSync(file("red.png"), png(4, 4, [255, 0, 0]));
writeFileSync(file("report.docx"), "PK\u0003\u0004 not a real document");
// Jamais remis : un dépôt simulé sans e2e_allow_paths ne le lit pas.
writeFileSync(file("private.md"), "# never handed over\n");
// Dossier : fichiers de premier niveau triés par nom (octets : majuscules
// d'abord), sous-dossier ignoré.
writeFileSync(path.join(FOLDER, "b.txt"), "b");
writeFileSync(path.join(FOLDER, "a.md"), "# a\n");
writeFileSync(path.join(FOLDER, "C.txt"), "C");
writeFileSync(path.join(FOLDER, "sub", "inner.txt"), "inner");
const FOLDER_FILES = ["C.txt", "a.md", "b.txt"];
/** Image du presse-papiers (collage réel et collage de la page). */
const PASTED = png(3, 2, [0, 128, 255]);
const MAX_FILE_SIZE = 10 * 1024 * 1024;

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
// rechargement et à la fin. Chaque collage est aussi noté (capture, avant
// l'app) : ce que WebKitGTK expose dans clipboardData.
const violations = [];
const consoleErrors = [];
const consoleWarnings = [];
const WATCH_PAGE = `
  if (!window.__csp) {
    window.__csp = [];
    window.__errors = [];
    window.__warnings = [];
    window.__pastes = [];
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
    const describe = (file) => file && { name: file.name, type: file.type, size: file.size };
    window.addEventListener("paste", (event) => {
      const data = event.clipboardData;
      window.__pastes.push({
        types: [...data.types],
        items: [...data.items].map((item) => ({
          kind: item.kind,
          type: item.type,
          file: item.kind === "file" ? describe(item.getAsFile()) : null,
        })),
        files: [...data.files].map(describe),
        text: data.getData("text/plain"),
        uris: data.getData("text/uri-list"),
        target: event.target.getAttribute?.("aria-label") ?? event.target.nodeName,
      });
    }, true);
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
 * Appels au backend en cours : rappels de __TAURI_INTERNALS__.callbacks, sauf
 * ceux des écouteurs d'évènements, qui restent tant qu'ils écoutent (dépôt de
 * fichiers d'une zone de saisie montée). Tauri note chaque écouteur dans
 * window.__internal_unstable_listeners_object_id__[évènement][id].handlerId
 * (propriétés non énumérables : event/mod.rs, listen_js_script).
 */
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

/**
 * Thème clair ou sombre, enregistré et appliqué aussitôt comme applyTheme
 * (src/hooks/useTheme.ts) ; voir thinking.test.mjs.
 */
async function setTheme(driver, theme) {
  await driver.invoke("update_settings", { patch: { theme } });
  await driver.execute(
    `const root = document.documentElement;
     root.classList.toggle("dark", arguments[0] === "dark");
     root.style.colorScheme = arguments[0];`,
    [theme],
  );
  // Fin des transitions de couleur.
  await delay(400);
}

/** Identifiant de la conversation affichée. */
const currentChatId = (driver) =>
  driver.execute(
    "return (location.hash + location.pathname).match(/\\/c\\/([^/?#]+)/)?.[1] ?? null",
  );

const TEXTAREA = "textarea[aria-label='Message']";
const OVERLAY = "[data-testid=drop-overlay]";
const OVERLAY_TEXT =
  "Drop files here or paste from clipboard to add them to your message";

const textareaValue = (driver) =>
  driver.execute("return document.querySelector(arguments[0]).value", [
    TEXTAREA,
  ]);

const focusTextarea = (driver) =>
  driver.execute("document.querySelector(arguments[0]).focus()", [TEXTAREA]);

/** Zone de saisie : pastilles (nom, vignette), erreurs, lectures en cours. */
const composerState = (driver) =>
  driver.execute(
    `return {
       files: [...document.querySelectorAll("[data-testid=composer-file]")].map((chip) => {
         const img = chip.querySelector("img");
         return {
           name: chip.querySelector("span").textContent,
           image: img && {
             src: img.getAttribute("src"),
             alt: img.alt,
             loaded: img.complete && img.naturalWidth > 0,
           },
         };
       }),
       errors: [...document.querySelectorAll("[data-testid=composer-file-error]")]
         .map((chip) => [...chip.querySelectorAll("span")].map((s) => s.textContent)),
       reading: document.querySelectorAll("[data-testid=composer-file-reading]").length,
     };`,
  );

/** Attend un état de la zone de saisie qui vérifie `check`, et le renvoie. */
async function waitComposer(driver, check, what) {
  let state;
  await waitUntil(
    async () => {
      state = await composerState(driver);
      return check(state);
    },
    () => `${what} : ${JSON.stringify(state)}`,
  );
  return state;
}

const names = (state) => state.files.map((f) => f.name);
const STAGED = /^chatonlm:\/\/localhost\/staged\/[0-9a-f-]{36}$/;
const ATTACHMENT = /^chatonlm:\/\/localhost\/attachment\/(\d+)$/;

/** Retire toutes les pastilles et erreurs (« Remove <nom> »). */
async function removeAll(driver) {
  for (const button of await driver.findAll(
    "[data-testid=composer-file] button, [data-testid=composer-file-error] button",
  )) {
    await driver.click(button);
  }
  await waitComposer(
    driver,
    (s) => s.files.length === 0 && s.errors.length === 0,
    "pastilles retirées",
  );
}

/**
 * Évènement de dépôt de Tauri, émis vers la fenêtre « main » comme le fait
 * Tauri pour une WebviewWindow (manager/window.rs : emit_to sur
 * EventTarget::labeled, charge DragDropPayload) à partir du dépôt natif GTK.
 */
const POSITION = { x: 600, y: 400 };
const emitDrag = (driver, event, payload) =>
  driver.invoke("plugin:event|emit_to", {
    target: { kind: "AnyLabel", label: "main" },
    event: `tauri://${event}`,
    payload,
  });
const dragEnter = (driver, paths) =>
  emitDrag(driver, "drag-enter", { paths, position: POSITION });
const dragOver = (driver) =>
  emitDrag(driver, "drag-over", { position: POSITION });
/**
 * Dépôt simulé : ses chemins sont d'abord remis par e2e_allow_paths, comme
 * le ferait le dépôt natif intercepté en Rust (sauf `allow: false`).
 */
async function dragDrop(driver, paths, { allow = true } = {}) {
  if (allow) await driver.invoke("e2e_allow_paths", { paths });
  await emitDrag(driver, "drag-drop", { paths, position: POSITION });
}
const dragLeave = (driver) => emitDrag(driver, "drag-leave", null);

/**
 * Voile de dépôt : rectangle, fenêtre, texte, icône, réaction à la souris,
 * couleur du panneau, et s'il passe par-dessus ces éléments (point au centre
 * de chacun, voile rendu un instant sensible à la souris pour le test).
 */
const overlayState = (driver) =>
  driver.execute(
    `const overlay = document.querySelector(arguments[0]);
     if (!overlay) return null;
     const rect = overlay.getBoundingClientRect();
     const covers = (selector) => {
       const target = document.querySelector(selector);
       if (!target) return null;
       const r = target.getBoundingClientRect();
       if (r.width === 0 || r.height === 0) return null;
       overlay.style.pointerEvents = "auto";
       const hit = document.elementFromPoint(r.left + r.width / 2, r.top + r.height / 2);
       overlay.style.pointerEvents = "";
       return overlay.contains(hit);
     };
     return {
       rect: [rect.left, rect.top, rect.width, rect.height],
       window: [0, 0, innerWidth, innerHeight],
       text: overlay.textContent,
       icon: overlay.querySelector("svg") !== null,
       pointerEvents: getComputedStyle(overlay).pointerEvents,
       panel: getComputedStyle(overlay.lastElementChild).backgroundColor,
       covers: {
         sidebarToggle: covers("button[title='Hide sidebar']"),
         settingsLink: covers("nav a[href$='settings']"),
         textarea: covers(arguments[1]),
       },
     };`,
    [OVERLAY, TEXTAREA],
  );

/** Attend le voile, vérifie qu'il couvre toute la fenêtre, et le renvoie. */
async function expectOverlay(driver) {
  let state;
  await waitUntil(
    async () => (state = await overlayState(driver)),
    "voile de dépôt",
  );
  assert.deepEqual(state.rect, state.window, "voile pas sur toute la fenêtre");
  assert.equal(state.text, OVERLAY_TEXT);
  assert.equal(state.icon, true);
  assert.equal(state.pointerEvents, "none");
  assert.deepEqual(state.covers, {
    sidebarToggle: true,
    settingsLink: true,
    textarea: true,
  });
  return state;
}

/** Vérifie qu'aucun voile n'est affiché, `ms` après l'évènement. */
async function expectNoOverlay(driver, ms = 500) {
  await delay(ms);
  // Booléen : un élément renvoyé par le script ne se compare pas à null.
  assert.equal(
    await driver.execute(
      "return document.querySelector(arguments[0]) !== null",
      [OVERLAY],
    ),
    false,
    "voile affiché",
  );
}

/**
 * Dépôt complet sur la zone de saisie montée : glisser de texte (sans
 * chemin), survol puis sortie, survol puis dépôt de `paths` ; pastilles et
 * refus attendus. `onOverlay(state)` : appelé voile affiché (captures).
 */
async function dropFlow(driver, paths, expected, onOverlay = async () => {}) {
  // Glisser de texte ou de lien : pas de chemins, pas de voile.
  await dragEnter(driver, []);
  await expectNoOverlay(driver);
  await dragLeave(driver);

  // Survol avec des fichiers, puis sortie : voile, puis rien de joint.
  await dragEnter(driver, paths);
  const state = await expectOverlay(driver);
  await dragOver(driver);
  await delay(200);
  assert.ok(await overlayState(driver), "voile retiré par drag-over");
  await onOverlay(state);
  await dragLeave(driver);
  await waitUntil(
    async () => (await overlayState(driver)) === null,
    "voile toujours là après drag-leave",
  );
  await delay(300);
  let composer = await composerState(driver);
  assert.deepEqual(composer.files, []);
  assert.deepEqual(composer.errors, []);
  assert.equal(composer.reading, 0);

  // Survol puis dépôt : voile retiré, fichiers joints dans l'ordre.
  await dragEnter(driver, paths);
  await expectOverlay(driver);
  await dragOver(driver);
  await dragDrop(driver, paths);
  await waitUntil(
    async () => (await overlayState(driver)) === null,
    "voile toujours là après drag-drop",
  );
  composer = await waitComposer(
    driver,
    (s) =>
      s.reading === 0 &&
      s.files.length === expected.files.length &&
      s.errors.length === expected.errors.length,
    "pastilles du dépôt",
  );
  assert.deepEqual(names(composer), expected.files);
  assert.deepEqual(composer.errors, expected.errors);
  // Vignette de l'image déposée, servie par chatonlm:// sous la CSP.
  composer = await waitComposer(
    driver,
    (s) => s.files.every((f) => f.image === null || f.image.loaded),
    "vignettes du dépôt chargées",
  );
  for (const chip of composer.files.filter((f) => f.image)) {
    assert.match(chip.image.src, STAGED);
    assert.equal(chip.image.alt, chip.name);
  }
  return composer;
}

/**
 * Vrai collage : `content` mis dans le presse-papiers de la session par une
 * autre application, focus rendu à l'app, focus dans la zone de saisie, vraie
 * touche Ctrl+V. Renvoie les types proposés, ceux que l'app a demandés au
 * propriétaire du presse-papiers, et ce que la page a vu (clipboardData).
 */
async function realPaste(driver, content) {
  const held = await holdClipboard(content);
  try {
    await waitUntil(
      () => driver.execute("return document.hasFocus()"),
      "focus pas rendu à l'app après la copie",
    );
    await focusTextarea(driver);
    await driver.execute("window.__pastes = []");
    await pressKeys("ctrl+v", { settleMs: 300 });
    const pastes = await waitUntil(async () => {
      const seen = await driver.execute("return window.__pastes");
      return seen.length > 0 && seen;
    }, "aucun évènement paste après Ctrl+V");
    assert.equal(pastes.length, 1);
    // Le temps d'une éventuelle lecture des fichiers collés.
    await delay(700);
    return {
      offered: held.targets,
      requested: [...held.requested],
      seen: pastes[0],
    };
  } finally {
    await held.release();
  }
}

/**
 * Collage d'un fichier par un évènement paste de la page, avec un
 * DataTransfer qui expose le fichier comme le ferait un navigateur (élément
 * « file », sans texte). Renvoie vrai si l'app a empêché le collage.
 */
const pasteFile = (driver, { name, type, base64 = "", size = null }) =>
  driver.execute(
    `const [name, type, base64, size] = arguments;
     const bytes = size === null
       ? Uint8Array.from(atob(base64), (c) => c.charCodeAt(0))
       : new Uint8Array(size);
     const data = new DataTransfer();
     data.items.add(new File([bytes], name, { type }));
     const textarea = document.querySelector(arguments[4]);
     textarea.focus();
     const event = new ClipboardEvent("paste", {
       clipboardData: data,
       bubbles: true,
       cancelable: true,
     });
     if (event.clipboardData !== data) {
       Object.defineProperty(event, "clipboardData", { value: data });
     }
     textarea.dispatchEvent(event);
     return event.defaultPrevented;`,
    [name, type, base64, size, TEXTAREA],
  );

/** Messages envoyés affichés : images (adresse, chargée), texte. */
const sentMessages = (driver) =>
  driver.execute(`
    return [...document.querySelectorAll("[data-message-index]")]
      .filter((item) => item.querySelector("[data-role=user]"))
      .map((item) => ({
        images: [...item.querySelectorAll("img")].map((img) => ({
          src: img.getAttribute("src"),
          alt: img.alt,
          loaded: img.complete && img.naturalWidth > 0,
        })),
        text: item.querySelector("[data-role=user] .whitespace-pre-wrap")?.textContent ?? null,
      }));`);

/** Texte de la dernière réponse affichée. */
const lastReply = (driver) =>
  driver.execute(
    "return [...document.querySelectorAll('[data-role=assistant]')].at(-1)?.textContent ?? null",
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

const PASTED_TEXT = "Hello from the clipboard";
const MESSAGE = "What is in this screenshot?";
/** Collages réels observés : rapportés à la fin. */
const observed = {};

try {
  await withApp(
    binary,
    { name: "drop-paste", env: { OLLAMA_HOST: fake.url } },
    shotOnFailure("drop-paste", async (driver) => {
      await driver.waitFor("[data-testid=new-chat]");
      // Modèle de secours écrit par la page (session neuve) avant tout choix.
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
      // Barre latérale ouverte : le voile doit passer par-dessus.
      await driver.click(
        await driver.waitFor("button[aria-label='Show sidebar']"),
      );
      await driver.waitFor("nav a[href$='settings']");
      await delay(400);

      // Dépôt sur la page de nouvelle conversation ; voile capturé en clair
      // et en sombre.
      await dropFlow(
        driver,
        [file("notes.txt"), file("red.png"), FOLDER, file("report.docx")],
        {
          files: ["notes.txt", "red.png", ...FOLDER_FILES],
          errors: [["report.docx", "• File type not supported"]],
        },
        async () => {
          await setTheme(driver, "light");
          const light = await expectOverlay(driver);
          await driver.screenshot(".tools/shots/drop-overlay-light.png");
          await setTheme(driver, "dark");
          const dark = await expectOverlay(driver);
          assert.notEqual(dark.panel, light.panel, "voile sombre = clair");
          await driver.screenshot(".tools/shots/drop-overlay-dark.png");
          await setTheme(driver, "light");
        },
      );
      await driver.screenshot(".tools/shots/drop-composer.png");

      // Page Settings : pas de zone de saisie, pas de voile ; en quittant la
      // conversation, les fichiers déposés sont libérés. Rien n'y est
      // préparé : chemin non remis, sans quoi la remise inutilisée masquerait
      // celle du dépôt natif de notes.txt plus bas.
      await driver.click(await driver.find("nav a[href='/settings']"));
      await driver.waitFor("[data-testid=settings]");
      await dragEnter(driver, [file("notes.txt")]);
      await expectNoOverlay(driver);
      await dragDrop(driver, [file("notes.txt")], { allow: false });
      await delay(300);
      await driver.click((await driver.findAll("nav a"))[0]);
      await driver.waitFor("[data-testid=new-chat]");
      let state = await composerState(driver);
      assert.deepEqual(state.files, []);
      assert.deepEqual(state.errors, []);

      // Vrai collage de texte seul : collé dans la zone, aucune pastille.
      let paste = await realPaste(driver, { text: PASTED_TEXT });
      observed.text = paste;
      assert.deepEqual(paste.seen.types, ["text/plain"]);
      assert.equal(paste.seen.text, PASTED_TEXT);
      assert.deepEqual(paste.seen.files, []);
      assert.equal(await textareaValue(driver), PASTED_TEXT);
      state = await composerState(driver);
      assert.deepEqual(state, { files: [], errors: [], reading: 0 });
      await driver.clear(await driver.find(TEXTAREA));

      // Presse-papiers d'un gestionnaire de fichiers (fichier copié) :
      // WebKitGTK n'expose que text/uri-list, sans données ni fichier, et
      // colle le chemin (text/plain) : texte collé, aucune pastille.
      const notesUri = `file://${encodeURI(file("notes.txt"))}`;
      paste = await realPaste(driver, {
        text: file("notes.txt"),
        data: { "text/uri-list": Buffer.from(`${notesUri}\r\n`) },
      });
      observed.fileManager = paste;
      assert.ok(paste.requested.includes("text/uri-list"));
      assert.deepEqual(paste.seen.types, ["text/uri-list"]);
      assert.deepEqual(paste.seen.files, []);
      assert.ok(paste.seen.items.every((item) => item.kind === "string"));
      assert.equal(await textareaValue(driver), file("notes.txt"));
      state = await composerState(driver);
      assert.deepEqual(state, { files: [], errors: [], reading: 0 });
      await driver.clear(await driver.find(TEXTAREA));

      // Vraie image seule (capture d'écran copiée), par un type image/png
      // brut puis par GtkClipboard.set_image. WebKitGTK 2.52 (et celui
      // d'Ubuntu 22.04, embarqué dans l'AppImage) n'expose rien à la page : le
      // backend lit l'image dans le presse-papiers GTK. WebKitGTK 2.54
      // l'expose comme un fichier « image.png » : chemin des fichiers collés.
      // Dans les deux cas, une seule pastille.
      for (const [kind, content] of [
        ["imagePng", { data: { "image/png": PASTED } }],
        ["gtkSetImage", { image: PASTED }],
      ]) {
        paste = await realPaste(driver, content);
        observed[kind] = paste;
        assert.equal(paste.seen.target, "Message");
        const seen = {
          types: paste.seen.types,
          items: paste.seen.items,
          files: paste.seen.files,
        };
        if (seen.files.length === 0) {
          assert.deepEqual(
            seen,
            { types: [], items: [], files: [] },
            `${kind} : ni fichier ni rien d'autre exposé`,
          );
        } else {
          assert.deepEqual(seen.types, ["Files"], `${kind} : types exposés`);
          assert.deepEqual(
            seen.files.map((f) => [f.name, f.type]),
            [["image.png", "image/png"]],
            `${kind} : fichier exposé`,
          );
        }
        // set_image : GTK sert l'image lui-même, sans demande visible ici.
        if (kind === "imagePng") {
          assert.ok(
            paste.requested.includes("image/png"),
            `image jamais demandée au presse-papiers (${paste.requested})`,
          );
        }
        assert.equal(await textareaValue(driver), "");
        state = await waitComposer(
          driver,
          (s) => s.files.length === 1 && s.files[0].image?.loaded,
          `${kind} : pastille de l'image collée`,
        );
        assert.deepEqual(names(state), ["image.png"]);
        assert.deepEqual(state.errors, []);
        assert.match(state.files[0].image.src, STAGED);
        // Jamais les deux chemins à la fois (fichier exposé ET lecture GTK).
        await delay(700);
        assert.deepEqual(
          names(await composerState(driver)),
          ["image.png"],
          `${kind} : une seule pastille`,
        );
        await driver.click(
          await driver.find(
            "[data-testid=composer-file] button[aria-label='Remove image.png']",
          ),
        );
        await waitComposer(
          driver,
          (s) => s.files.length === 0,
          `${kind} : pastille retirée`,
        );
      }

      // Fichier collé tel qu'un navigateur l'expose : image sans nom →
      // « image.png », collage empêché, vignette chargée.
      assert.equal(
        await pasteFile(driver, {
          name: "",
          type: "image/png",
          base64: PASTED.toString("base64"),
        }),
        true,
        "collage d'un fichier non empêché",
      );
      state = await waitComposer(
        driver,
        (s) => s.files.length === 1 && s.reading === 0 && s.files[0].image,
        "pastille de l'image collée",
      );
      assert.deepEqual(names(state), ["image.png"]);
      assert.deepEqual(state.errors, []);
      state = await waitComposer(
        driver,
        (s) => s.files[0].image.loaded,
        "vignette de l'image collée",
      );
      assert.match(state.files[0].image.src, STAGED);
      assert.equal(await textareaValue(driver), "");

      // Trop gros : refusé sans lire ses octets.
      assert.equal(
        await pasteFile(driver, {
          name: "big.txt",
          type: "text/plain",
          size: MAX_FILE_SIZE + 1,
        }),
        true,
      );
      state = await waitComposer(
        driver,
        (s) => s.errors.length === 1 && s.reading === 0,
        "refus du gros fichier collé",
      );
      assert.deepEqual(state.errors, [["big.txt", "• File too large"]]);
      await driver.click(
        await driver.find(
          "[data-testid=composer-file-error] button[aria-label='Remove big.txt']",
        ),
      );
      await waitComposer(driver, (s) => s.errors.length === 0, "refus retiré");

      // Envoi par une vraie touche Entrée : image en base64 dans la requête.
      await driver.type(await driver.waitFor(TEXTAREA), MESSAGE);
      await focusTextarea(driver);
      await pressKeys("enter");
      await driver.waitFor("[data-testid=chat]");
      await waitUntil(
        async () => (await lastReply(driver)) === '"Hi!"',
        "réponse de chatonlm-vision",
      );
      await driver.waitFor("button[aria-label='Send message']");
      const chatId = await currentChatId(driver);
      assert.ok(chatId, "conversation créée");
      const requests = await requestsOf("chatonlm-vision");
      assert.equal(requests.length, 1);
      const sent = requests[0].messages.at(-1);
      assert.equal(sent.content, MESSAGE);
      assert.deepEqual(sent.images, [PASTED.toString("base64")]);
      let messages;
      await waitUntil(
        async () => {
          messages = await sentMessages(driver);
          const last = messages.at(-1);
          return last?.text === MESSAGE && last.images[0]?.loaded;
        },
        () => `message envoyé : ${JSON.stringify(messages)}`,
      );
      assert.equal(messages.at(-1).images.length, 1);
      assert.equal(messages.at(-1).images[0].alt, "image.png");
      assert.match(messages.at(-1).images[0].src, ATTACHMENT);
      const saved = (await driver.invoke("get_chat", { id: chatId })).messages;
      assert.deepEqual(
        saved[0].attachments.map(({ filename, kind, mime, size }) => ({
          filename,
          kind,
          mime,
          size,
        })),
        [
          {
            filename: "image.png",
            kind: "image",
            mime: "image/png",
            size: PASTED.length,
          },
        ],
      );

      // Dépôt dans la conversation existante.
      await dropFlow(driver, [file("red.png"), FOLDER, file("report.docx")], {
        files: ["red.png", ...FOLDER_FILES],
        errors: [["report.docx", "• File type not supported"]],
      });
      assert.equal(await currentChatId(driver), chatId);
      await removeAll(driver);

      // Dépôt simulé sans e2e_allow_paths (un tauri://drag-drop émis par la
      // page n'autorise rien) : private.md, jamais remis, et red.png, déjà
      // préparé (remise consommée). « Error reading file », rien de joint,
      // fichiers pourtant lisibles.
      await dragEnter(driver, [file("private.md"), file("red.png")]);
      await expectOverlay(driver);
      await dragDrop(driver, [file("private.md"), file("red.png")], {
        allow: false,
      });
      state = await waitComposer(
        driver,
        (s) => s.reading === 0 && s.errors.length === 2,
        "refus du dépôt non remis",
      );
      assert.deepEqual(state.files, []);
      assert.deepEqual(state.errors, [
        ["private.md", "• Error reading file"],
        ["red.png", "• Error reading file"],
      ]);
      // Un à un : retirer un refus redessine les autres (bouton périmé).
      for (const name of ["private.md", "red.png"]) {
        await driver.click(
          await driver.find(
            `[data-testid=composer-file-error] button[aria-label='Remove ${name}']`,
          ),
        );
      }
      await waitComposer(driver, (s) => s.errors.length === 0, "refus retirés");

      // Vrai glisser-déposer natif : source GTK à gauche, app à droite,
      // pointeur de KWin (bouton gauche appuyé sur la source, déplacé sur
      // l'app, relâché). Voile pendant le survol, pastilles au dépôt.
      const source = await startDragSource([file("notes.txt"), FOLDER]);
      try {
        // Fenêtre tout juste affichée : KWin ne la place pas encore. Placée
        // deux fois (sans effet si elle l'est déjà).
        const place = `${forWindows(
          DRAG_SOURCE_CAPTION,
          "w.frameGeometry = { x: 20, y: 380, width: 220, height: 200 };",
        )}
          ${forWindows(
            "ChatonLM",
            "w.frameGeometry = { x: 300, y: 20, width: 1040, height: 900 };",
          )}`;
        for (const wait of [500, 300]) {
          await delay(wait);
          await runKWinScript(place, { settleMs: 200 });
        }
        const [x0, y0] = [130, 480];
        const [x1, y1] = [820, 470];
        const path = [];
        for (let i = 1; i <= 20; i++) {
          const x = Math.round(x0 + ((x1 - x0) * i) / 20);
          const y = Math.round(y0 + ((y1 - y0) * i) / 20);
          path.push(`pointer:${x},${y}`, "wait:30");
        }
        let hovering = null;
        let moved = false;
        // Un seul appel : KWin relâche le bouton quand le pointeur EIS
        // disparaît (fin de keyboard.py).
        const gesture = pressKeys(
          [
            `pointer:${x0},${y0}`,
            "wait:200",
            "button:down",
            "wait:100",
            ...[3, 8, 15, 25].map((dx) => `pointer:${x0 + dx},${y0}`),
            ...path,
            "wait:1500",
            "button:up",
          ],
          { settleMs: 300, timeoutMs: 30000 },
        ).finally(() => (moved = true));
        while (!moved && hovering === null) {
          hovering = await overlayState(driver);
          if (hovering === null) await delay(50);
        }
        await gesture;
        assert.ok(
          hovering,
          `voile jamais vu (source : ${source.events} ${source.errors()})`,
        );
        assert.deepEqual(hovering.rect, hovering.window);
        assert.equal(hovering.text, OVERLAY_TEXT);
        await waitUntil(
          async () => (await overlayState(driver)) === null,
          "voile toujours là après le dépôt natif",
        );
        const dropped = await waitComposer(
          driver,
          (s) => s.reading === 0 && s.files.length === 1 + FOLDER_FILES.length,
          "pastilles du dépôt natif",
        );
        assert.deepEqual(names(dropped), ["notes.txt", ...FOLDER_FILES]);
        assert.deepEqual(dropped.errors, []);
        assert.ok(source.events.includes("data-get"), `${source.events}`);
        await driver.screenshot(".tools/shots/drop-native.png");
      } finally {
        await source.stop();
      }
      await collectViolations(driver);
    }),
  );
} finally {
  await fake.stop();
}

// Ce que WebKitGTK a exposé aux vrais collages (journal de l'essai).
console.log(JSON.stringify({ pastes: observed }, null, 1));

assert.deepEqual(violations, [], "violations de la CSP");
assert.deepEqual(consoleErrors, [], "erreurs dans la console");
assert.deepEqual(consoleWarnings, [], "avertissements dans la console");
