// Pièces jointes dans l'app réelle (build e2e : frontend embarqué, CSP de
// production), avec le faux serveur et de vraies touches du clavier
// (e2e/lib/keyboard.mjs). Fichiers créés par le test dans le dossier de
// l'essai (.tools/e2e-runs/run.*/Test Files : chemin avec une espace et des
// majuscules) :
// - « + » sans dialogue (crochet CHATONLM_E2E_PICK_FILE : chemins « choisis »
//   relus dans un fichier à chaque clic, fichier absent = annulation) :
//   notes.txt, red.png (vraie image PNG), brief.pdf (PDF avec du texte),
//   extra.md et blue.png en pastilles, dans l'ordre ; vignettes chargées
//   (chatonlm://localhost/staged/<id>, sous la CSP de production) ; refus
//   officiels : report.docx « File type not supported », big.txt de 10 Mio +
//   1 octet « File too large », locked.txt sans droits « Error reading
//   file » ; avec une erreur, Send désactivé et une vraie touche Entrée
//   n'envoie rien (aucune requête, aucune conversation, texte gardé) ;
//   « Remove <nom> » retire pastilles et erreurs (image retirée : plus
//   servie par chatonlm://) ; sélecteur annulé : rien ;
// - chatonlm-vision (capacité vision) : aucun avertissement ; envoi par une
//   vraie touche Entrée ; requête /api/chat (GET /fake/requests) : texte de
//   notes.txt et de brief.pdf encadré (« --- File: <nom> --- » … « --- End of
//   <nom> --- ») dans content, red.png en base64 dans images ; message
//   envoyé : vignette de 64 px au-dessus de la bulle, chargée
//   (chatonlm://localhost/attachment/<id> de la pièce jointe en base), pastilles
//   des autres fichiers dans la bulle ; pièces jointes en base (get_chat) ;
//   pareil après un rechargement de la page ;
// - chatonlm-plain (capacité completion seule) : pastille de l'image en rouge
//   avec « This model does not support images », envoi permis ; à l'envoi,
//   aucune image dans la requête, ni dans le message affiché ni en base, et
//   l'image n'est plus servie (libérée) ; le fichier texte part ; après un
//   rechargement de la page, l'image préparée n'est plus servie (zone vidée) ;
// - vrai sélecteur GTK (sans le crochet) : « + » ouvre la fenêtre « Select
//   Files » (cherchée par KWin), qui prend le focus ; Échap la ferme sans
//   pastille ; chemin complet tapé après Ctrl+L (vraies touches), Entrée : la
//   pastille du fichier choisi apparaît ;
// - aucune violation de la CSP ni erreur ou avertissement dans la console.
import assert from "node:assert/strict";
import {
  chmodSync,
  mkdirSync,
  realpathSync,
  rmSync,
  truncateSync,
  writeFileSync,
} from "node:fs";
import path from "node:path";
import { crc32, deflateSync } from "node:zlib";
import { ROOT, withApp } from "./lib/app.mjs";
import { startFake } from "./lib/fake.mjs";
import { PENDING_CALLS } from "./lib/page.mjs";
import { chordsOf, pressKeys } from "./lib/keyboard.mjs";
import { windowCaptions } from "./lib/kwin.mjs";

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

/**
 * PDF d'une page portant ces lignes (ASCII), en Helvetica, comme
 * src-tauri/tests/it/samples.rs.
 */
function pdf(lines) {
  const text = lines
    .map((line, index) => `${index > 0 ? "T* " : ""}(${line}) Tj`)
    .join(" ");
  const content = `BT /F1 12 Tf 16 TL 72 720 Td ${text} ET`;
  const objects = [
    "<< /Type /Catalog /Pages 2 0 R >>",
    "<< /Type /Pages /Kids [4 0 R] /Count 1 >>",
    "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>",
    "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] " +
      "/Resources << /Font << /F1 3 0 R >> >> /Contents 5 0 R >>",
    `<< /Length ${content.length} >>\nstream\n${content}\nendstream`,
  ];
  let out = "%PDF-1.4\n";
  const offsets = [];
  objects.forEach((object, index) => {
    offsets.push(out.length);
    out += `${index + 1} 0 obj\n${object}\nendobj\n`;
  });
  const xref = out.length;
  out += `xref\n0 ${objects.length + 1}\n0000000000 65535 f \n`;
  for (const offset of offsets) {
    out += `${String(offset).padStart(10, "0")} 00000 n \n`;
  }
  out += `trailer\n<< /Size ${objects.length + 1} /Root 1 0 R >>\nstartxref\n${xref}\n%%EOF\n`;
  return Buffer.from(out, "latin1");
}

const NOTES = "Line 1\nLine 2";
const RED = png(4, 4, [255, 0, 0]);
const PDF_LINE = "Quarterly figures rose.";
const MAX_FILE_SIZE = 10 * 1024 * 1024;
writeFileSync(file("notes.txt"), NOTES);
writeFileSync(file("red.png"), RED);
writeFileSync(file("blue.png"), png(4, 4, [0, 0, 255]));
writeFileSync(file("brief.pdf"), pdf([PDF_LINE]));
writeFileSync(file("extra.md"), "# Extra\n");
writeFileSync(file("report.docx"), "PK\u0003\u0004 not a real document");
writeFileSync(file("big.txt"), "");
truncateSync(file("big.txt"), MAX_FILE_SIZE + 1);
writeFileSync(file("locked.txt"), "secret");
chmodSync(file("locked.txt"), 0o000);
// Choisi dans le vrai sélecteur GTK, chemin tapé au clavier.
const TYPED = "Picked 2.md";
writeFileSync(file(TYPED), "Picked in the GTK dialog.\n");

/** Fichiers « choisis » au prochain clic sur « + » (crochet e2e). */
const pick = (...names) =>
  writeFileSync(PICKS, `${names.map(file).join("\n")}\n`);
/** Prochain clic sur « + » : sélecteur annulé. */
const cancelPick = () => rmSync(PICKS, { force: true });

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

const exists = (driver, css) =>
  driver.execute("return document.querySelector(arguments[0]) !== null", [css]);

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
const WARNING = "This model does not support images";

const textareaValue = (driver) =>
  driver.execute("return document.querySelector(arguments[0]).value", [
    TEXTAREA,
  ]);

const focusTextarea = (driver) =>
  driver.execute("document.querySelector(arguments[0]).focus()", [TEXTAREA]);

/** Clic sur « + » (bouton officiel : titre et nom identiques). */
async function clickPlus(driver) {
  const plus = await driver.waitFor(PLUS);
  assert.equal(
    await driver.execute("return document.querySelector(arguments[0]).title", [
      PLUS,
    ]),
    "Upload multiple files",
  );
  await driver.click(plus);
}

/**
 * Zone de saisie : pastilles (nom, avertissement, rouge, vignette), erreurs
 * (textes), lectures en cours, avertissements dans toute la page, bouton
 * d'envoi.
 */
const composerState = (driver) =>
  driver.execute(
    `const warning = arguments[0];
     return {
       files: [...document.querySelectorAll("[data-testid=composer-file]")].map((chip) => {
         const img = chip.querySelector("img");
         return {
           name: chip.querySelector("span").textContent,
           warning: chip.textContent.includes(warning),
           red: chip.classList.contains("border-red-200") && chip.classList.contains("bg-red-50"),
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
       warnings: [...document.querySelectorAll("span")]
         .filter((span) => span.textContent === warning).length,
       sendDisabled: document.querySelector("button[aria-label='Send message']")?.disabled ?? null,
     };`,
    [WARNING],
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

/**
 * Messages envoyés affichés : images (adresse, chargée, au-dessus de la
 * bulle et hors d'elle, hauteur), vignettes vides, pastilles de la bulle,
 * texte.
 */
const sentMessages = (driver) =>
  driver.execute(`
    return [...document.querySelectorAll("[data-message-index]")]
      .filter((item) => item.querySelector("[data-role=user]"))
      .map((item) => {
        const bubble = item.querySelector("[data-role=user]");
        const top = bubble.getBoundingClientRect().top;
        return {
          images: [...item.querySelectorAll("img")].map((img) => ({
            src: img.getAttribute("src"),
            alt: img.alt,
            loaded: img.complete && img.naturalWidth > 0,
            above: !bubble.contains(img) && img.getBoundingClientRect().bottom <= top,
            height: img.getBoundingClientRect().height,
          })),
          placeholders: item.querySelectorAll("[data-testid=thumbnail-placeholder]").length,
          files: [...bubble.querySelectorAll(".rounded-lg span")].map((s) => s.textContent),
          text: bubble.querySelector(".whitespace-pre-wrap")?.textContent ?? null,
        };
      });`);

async function waitSent(driver, check, what) {
  let messages;
  await waitUntil(
    async () => {
      messages = await sentMessages(driver);
      return messages.length > 0 && check(messages.at(-1));
    },
    () => `${what} : ${JSON.stringify(messages)}`,
  );
  return messages.at(-1);
}

/** Texte de la dernière réponse affichée. */
const lastReply = (driver) =>
  driver.execute(
    "return [...document.querySelectorAll('[data-role=assistant]')].at(-1)?.textContent ?? null",
  );

async function waitReply(driver, text) {
  let shown;
  await waitUntil(
    async () => {
      shown = await lastReply(driver);
      return shown === text;
    },
    () =>
      `réponse ${JSON.stringify(text)} (affiché : ${JSON.stringify(shown)})`,
  );
  await driver.waitFor("button[aria-label='Send message']");
}

/** Fenêtres « Select Files » (sélecteur natif) de la session. */
const pickerWindows = () =>
  windowCaptions("Select Files").filter(
    (caption) => caption === "Select Files",
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

/** Message envoyé à chatonlm-vision, et son texte attendu dans la requête. */
const VISION_TEXT = "Describe these files.";
const PLAIN_TEXT = "What is in the files?";
const notesFrame = `\n--- File: notes.txt ---\n${NOTES}\n--- End of notes.txt ---`;

/** Message envoyé à chatonlm-vision tel qu'affiché, avant et après relecture. */
function isVisionMessage(message) {
  return (
    message.text === VISION_TEXT &&
    message.placeholders === 0 &&
    message.images.length === 1 &&
    ATTACHMENT.test(message.images[0].src) &&
    message.images[0].loaded &&
    JSON.stringify(message.files) === JSON.stringify(["notes.txt", "brief.pdf"])
  );
}

try {
  await withApp(
    binary,
    {
      name: "attachments",
      env: { OLLAMA_HOST: fake.url, CHATONLM_E2E_PICK_FILE: PICKS },
    },
    shotOnFailure("attachments", async (driver) => {
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

      // Fichiers acceptés, en pastilles dans l'ordre du choix ; vignettes
      // chargées par chatonlm:// sous la CSP de production.
      pick("notes.txt", "red.png", "brief.pdf", "extra.md", "blue.png");
      await clickPlus(driver);
      let state = await waitComposer(
        driver,
        (s) => s.files.length === 5 && s.reading === 0,
        "cinq pastilles",
      );
      assert.deepEqual(names(state), [
        "notes.txt",
        "red.png",
        "brief.pdf",
        "extra.md",
        "blue.png",
      ]);
      assert.deepEqual(state.errors, []);
      state = await waitComposer(
        driver,
        (s) => s.files.filter((f) => f.image?.loaded).length === 2,
        "vignettes chargées",
      );
      const [red, blue] = state.files.filter((f) => f.image);
      assert.equal(red.image.alt, "red.png");
      assert.match(red.image.src, STAGED);
      assert.equal(blue.image.alt, "blue.png");
      assert.match(blue.image.src, STAGED);
      // Les autres : icône de document, pas de vignette.
      assert.deepEqual(
        state.files.filter((f) => f.image === null).map((f) => f.name),
        ["notes.txt", "brief.pdf", "extra.md"],
      );
      // Modèle avec la vision : aucun avertissement, rien en rouge.
      assert.equal(state.warnings, 0);
      assert.ok(state.files.every((f) => !f.red && !f.warning));
      // Rien d'écrit : Send inactif sans texte.
      assert.equal(state.sendDisabled, true);

      // Refus officiels, après les pastilles, dans l'ordre du choix.
      pick("report.docx", "big.txt", "locked.txt");
      await clickPlus(driver);
      state = await waitComposer(
        driver,
        (s) => s.errors.length === 3 && s.reading === 0,
        "trois refus",
      );
      assert.deepEqual(state.errors, [
        ["report.docx", "• File type not supported"],
        ["big.txt", "• File too large"],
        ["locked.txt", "• Error reading file"],
      ]);
      assert.equal(state.files.length, 5);
      await driver.screenshot(".tools/shots/attachments-composer.png");

      // Avec une erreur : Send désactivé, et une vraie touche Entrée dans la
      // zone de saisie n'envoie rien (texte gardé, pas de saut de ligne).
      await driver.type(await driver.waitFor(TEXTAREA), VISION_TEXT);
      state = await composerState(driver);
      assert.equal(state.sendDisabled, true, "Send actif malgré une erreur");
      await focusTextarea(driver);
      await pressKeys("enter", { settleMs: 1000 });
      assert.equal((await requestsOf("chatonlm-vision")).length, 0);
      assert.ok(await exists(driver, "[data-testid=new-chat]"));
      assert.deepEqual(await driver.invoke("list_chats"), []);
      assert.equal(await textareaValue(driver), VISION_TEXT);

      // « Remove <nom> » retire les erreurs une à une : Send revient.
      for (const name of ["big.txt", "report.docx", "locked.txt"]) {
        await driver.click(
          await driver.find(
            `[data-testid=composer-file-error] button[aria-label='Remove ${name}']`,
          ),
        );
      }
      state = await waitComposer(
        driver,
        (s) => s.errors.length === 0,
        "erreurs retirées",
      );
      assert.equal(state.sendDisabled, false);

      // Pastilles retirées : extra.md, puis blue.png, dont la vignette n'est
      // plus servie (fichier libéré par le backend).
      assert.equal(await imageLoads(driver, blue.image.src), true);
      for (const name of ["extra.md", "blue.png"]) {
        await driver.click(
          await driver.find(
            `[data-testid=composer-file] button[aria-label='Remove ${name}']`,
          ),
        );
      }
      state = await waitComposer(
        driver,
        (s) => s.files.length === 3,
        "pastilles retirées",
      );
      assert.deepEqual(names(state), ["notes.txt", "red.png", "brief.pdf"]);
      await waitUntil(
        async () => !(await imageLoads(driver, blue.image.src)),
        "vignette de blue.png toujours servie après Remove",
      );
      assert.equal(await imageLoads(driver, red.image.src), true);

      // Sélecteur annulé : rien ne change, aucun message d'erreur.
      cancelPick();
      await clickPlus(driver);
      await delay(1000);
      state = await composerState(driver);
      assert.deepEqual(names(state), ["notes.txt", "red.png", "brief.pdf"]);
      assert.deepEqual(state.errors, []);
      assert.equal(state.reading, 0);
      assert.equal(
        await driver.execute(
          "return document.body.textContent.includes('Failed to select files')",
        ),
        false,
      );

      // Envoi par une vraie touche Entrée.
      await focusTextarea(driver);
      await pressKeys("enter");
      await driver.waitFor("[data-testid=chat]");
      await waitReply(driver, '"Hi!"');
      const chatId = await currentChatId(driver);
      assert.ok(chatId, "conversation créée");

      // Requête reçue par Ollama : texte des fichiers encadré dans content,
      // image en base64 dans images.
      const visionRequests = await requestsOf("chatonlm-vision");
      assert.equal(visionRequests.length, 1);
      const sent = visionRequests[0].messages.at(-1);
      assert.equal(sent.role, "user");
      const head = `${VISION_TEXT}${notesFrame}\n--- File: brief.pdf ---\n`;
      assert.ok(
        sent.content.startsWith(head),
        `content : ${JSON.stringify(sent.content)}`,
      );
      const pdfText = sent.content.slice(head.length);
      assert.ok(
        pdfText.endsWith("\n--- End of brief.pdf ---"),
        `content : ${JSON.stringify(sent.content)}`,
      );
      assert.equal(
        pdfText.slice(0, -"\n--- End of brief.pdf ---".length).trim(),
        PDF_LINE,
      );
      assert.deepEqual(sent.images, [RED.toString("base64")]);

      // Message affiché : vignette chargée au-dessus de la bulle, pastilles
      // des autres fichiers dans la bulle ; pièces jointes en base.
      let message = await waitSent(driver, isVisionMessage, "message envoyé");
      assert.equal(message.images[0].alt, "red.png");
      assert.ok(message.images[0].above, "vignette pas au-dessus de la bulle");
      assert.equal(message.images[0].height, 64);
      const saved = (await driver.invoke("get_chat", { id: chatId })).messages;
      assert.equal(saved.length, 2);
      assert.deepEqual(
        saved[0].attachments.map(({ filename, kind, mime, size }) => ({
          filename,
          kind,
          mime,
          size,
        })),
        [
          {
            filename: "notes.txt",
            kind: "text",
            mime: "text/plain",
            size: NOTES.length,
          },
          {
            filename: "red.png",
            kind: "image",
            mime: "image/png",
            size: RED.length,
          },
          {
            filename: "brief.pdf",
            kind: "text",
            mime: "application/pdf",
            size: pdf([PDF_LINE]).length,
          },
        ],
      );
      const redId = saved[0].attachments[1].id;
      assert.equal(ATTACHMENT.exec(message.images[0].src)[1], String(redId));
      state = await composerState(driver);
      assert.deepEqual(state.files, []);
      assert.equal(state.warnings, 0);
      await driver.screenshot(".tools/shots/attachments-sent.png");

      // Après un rechargement (get_chat) : toujours là.
      await reload(driver);
      await driver.waitFor("[data-testid=chat]");
      assert.equal(await currentChatId(driver), chatId);
      message = await waitSent(driver, isVisionMessage, "message relu");
      assert.ok(message.images[0].above);
      assert.equal(
        message.images[0].src,
        `chatonlm://localhost/attachment/${redId}`,
      );

      // Modèle sans vision (capacité completion seule) : image en rouge avec
      // l'avertissement officiel ; l'envoi reste permis.
      await newChat(driver);
      await useModel(driver, "chatonlm-plain");
      pick("notes.txt", "red.png");
      await clickPlus(driver);
      state = await waitComposer(
        driver,
        (s) =>
          s.files.length === 2 && s.reading === 0 && s.files[1].image?.loaded,
        "pastilles sans vision",
      );
      assert.deepEqual(names(state), ["notes.txt", "red.png"]);
      assert.deepEqual(
        state.files.map((f) => [f.red, f.warning]),
        [
          [false, false],
          [true, true],
        ],
      );
      assert.equal(state.warnings, 1);
      const unsupported = state.files[1].image.src;
      await driver.type(await driver.waitFor(TEXTAREA), PLAIN_TEXT);
      state = await composerState(driver);
      assert.equal(state.sendDisabled, false, "image sans vision bloquante");
      await driver.click(
        await driver.find("button[aria-label='Send message']"),
      );
      await driver.waitFor("[data-testid=chat]");
      await waitReply(driver, "Received think=absent.");
      const plainRequests = await requestsOf("chatonlm-plain");
      assert.equal(plainRequests.length, 1);
      const plainSent = plainRequests[0].messages.at(-1);
      assert.equal(plainSent.content, `${PLAIN_TEXT}${notesFrame}`);
      assert.equal(plainSent.images, undefined, "image envoyée sans vision");
      message = await waitSent(
        driver,
        (m) =>
          m.text === PLAIN_TEXT &&
          JSON.stringify(m.files) === JSON.stringify(["notes.txt"]),
        "message sans image",
      );
      assert.deepEqual(message.images, []);
      assert.equal(message.placeholders, 0);
      const plainChat = await driver.invoke("get_chat", {
        id: await currentChatId(driver),
      });
      assert.deepEqual(
        plainChat.messages[0].attachments.map((a) => a.filename),
        ["notes.txt"],
      );
      // Image retirée à l'envoi : libérée, plus servie.
      await waitUntil(
        async () => !(await imageLoads(driver, unsupported)),
        "image sans vision toujours servie après l'envoi",
      );
      state = await composerState(driver);
      assert.deepEqual(state.files, []);
      assert.equal(state.warnings, 0);

      // Rechargement de la page : la zone de préparation est vidée, l'image
      // préparée n'est plus servie (son composer a disparu).
      pick("red.png");
      await clickPlus(driver);
      state = await waitComposer(
        driver,
        (s) => s.files.length === 1 && s.files[0].image?.loaded,
        "pastille avant le rechargement",
      );
      const beforeReload = state.files[0].image.src;
      await reload(driver);
      await waitUntil(
        async () => !(await imageLoads(driver, beforeReload)),
        "image préparée toujours servie après le rechargement",
      );
      await collectViolations(driver);
    }),
  );

  // Vrai sélecteur GTK : pas de crochet. Même base : chatonlm-plain relu.
  await withApp(
    binary,
    { name: "attachments-picker", env: { OLLAMA_HOST: fake.url } },
    shotOnFailure("attachments-picker", async (driver) => {
      await driver.waitFor("[data-testid=new-chat]");
      await waitUntil(
        async () => (await shownModel(driver)) === "chatonlm-plain",
        "modèle choisi relu",
      );
      await driver.execute(WATCH_PAGE);
      assert.deepEqual(pickerWindows(), []);
      const pageHasFocus = () => driver.execute("return document.hasFocus()");
      assert.equal(await pageHasFocus(), true);

      // « + » ouvre la fenêtre « Select Files », qui prend le focus ; Échap
      // la ferme sans rien choisir.
      await clickPlus(driver);
      await waitUntil(
        () => pickerWindows().length === 1,
        "fenêtre Select Files",
      );
      await waitUntil(
        async () => !(await pageHasFocus()),
        "focus resté dans la page",
      );
      await delay(300);
      await pressKeys("escape");
      await waitUntil(
        () => pickerWindows().length === 0,
        "Select Files ouverte après Échap",
      );
      await delay(500);
      let state = await composerState(driver);
      assert.deepEqual(state.files, []);
      assert.deepEqual(state.errors, []);
      assert.equal(state.reading, 0);

      // Chemin complet tapé (Ctrl+L : champ d'emplacement), Entrée : la
      // pastille du fichier choisi apparaît.
      await clickPlus(driver);
      await waitUntil(
        () => pickerWindows().length === 1,
        "fenêtre Select Files rouverte",
      );
      await waitUntil(
        async () => !(await pageHasFocus()),
        "focus resté dans la page",
      );
      await delay(300);
      await pressKeys("ctrl+l", { settleMs: 300 });
      await pressKeys(chordsOf(file(TYPED)), {
        settleMs: 500,
        timeoutMs: 60000,
      });
      await pressKeys("enter");
      await waitUntil(
        () => pickerWindows().length === 0,
        "Select Files ouverte après Entrée",
      );
      state = await waitComposer(
        driver,
        (s) => s.files.length === 1 && s.reading === 0,
        "pastille du fichier choisi",
      );
      assert.deepEqual(names(state), [TYPED]);
      assert.deepEqual(state.errors, []);
      await collectViolations(driver);
    }),
  );
} finally {
  await fake.stop();
  chmodSync(file("locked.txt"), 0o600);
}

assert.deepEqual(violations, [], "violations de la CSP");
assert.deepEqual(consoleErrors, [], "erreurs dans la console");
assert.deepEqual(consoleWarnings, [], "avertissements dans la console");
