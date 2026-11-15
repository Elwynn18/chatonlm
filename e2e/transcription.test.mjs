// Outil Transcription dans l'app réelle (session virtuelle) :
// - carte « Transcription » de la page Tools, accueil vide (capture) ;
// - fichier local « choisi » (crochet CHATONLM_E2E_PICK_MEDIA : jfk.wav,
//   11 s, échantillon de la dictée), vrai modèle Whisper tiny (sur le processeur,
//   comme la dictée), langue anglaise : décodage, transcription, texte
//   (« ask not »), horodatage, .txt enregistré (crochet
//   CHATONLM_E2E_SAVE_FILE), capture ;
// - Synthétiser avec le faux moteur (scénario `chatonlm-synthesis`) :
//   document écrit, enregistré en .md et .docx ;
// - adresse d'une vidéo, faux GitHub et faux yt-dlp/Deno : boîte d'accord
//   (licences, GPL/LGPL, responsabilité ; capture), installation, puis la
//   transcription part d'elle-même (titre de la fiche, texte) ;
// - vidéo privée : l'échec dit sa raison (code `fetch`, `private`) ;
// - historique : les trois transcriptions réussies, la plus récente d'abord.
import assert from "node:assert/strict";
import {
  copyFileSync,
  existsSync,
  mkdirSync,
  readFileSync,
  realpathSync,
  symlinkSync,
} from "node:fs";
import path from "node:path";
import { ROOT, withApp } from "./lib/app.mjs";
import { poll } from "./lib/desktop.mjs";
import { fakeModel, startEngine, startFakeGithub } from "./lib/fake.mjs";

const binary = process.argv[2];
const DATA = process.env.CHATONLM_DATA_DIR;
assert.ok(
  DATA,
  "CHATONLM_DATA_DIR absent : lancer dans scripts/e2e-session.sh",
);
const TINY = `${ROOT}.tools/speech-models/whisper/ggml-tiny.bin`;
assert.ok(existsSync(TINY), `modèle d'essai absent : ${TINY}`);

// Fichiers de l'essai : à côté du HOME de la session (.tools/e2e-runs/run.*).
const home = realpathSync(process.env.HOME ?? "/");
const runs = path.join(realpathSync(ROOT), ".tools", "e2e-runs") + path.sep;
assert.ok(home.startsWith(runs), `HOME hors de .tools/e2e-runs : ${home}`);
const FILES = path.join(path.dirname(home), "Transcription Files");
mkdirSync(FILES, { recursive: true });
// Discours de Kennedy entier (11 s, domaine public) : les fixtures de
// media-audio n'en gardent que 4 s.
const JFK = `${ROOT}.tools/speech-samples/jfk.wav`;
assert.ok(existsSync(JFK), `échantillon absent : ${JFK}`);
const MEDIA = path.join(FILES, "jfk.wav");
copyFileSync(JFK, MEDIA);
const SAVED = path.join(FILES, "saved");

// Modèle Whisper tiny « installé » (lien vers la copie de l'essai).
mkdirSync(`${DATA}/speech-models/whisper-tiny`, { recursive: true });
symlinkSync(TINY, `${DATA}/speech-models/whisper-tiny/ggml-tiny.bin`);

const SYNTHESIS = fakeModel("synthesis");
const engine = startEngine([SYNTHESIS]);
const github = await startFakeGithub();

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
/** Clique le bouton (ou lien) dont le texte est `label` ; faux s'il manque. */
const clickLabeled = (driver, label, scope = "body") =>
  driver.execute(
    `const target = [...document.querySelector(arguments[1]).querySelectorAll("button, a")]
       .find((el) => el.textContent.trim() === arguments[0] && !el.disabled);
     target?.click();
     return Boolean(target);`,
    [label, scope],
  );
/** Valeur d'un champ ou d'une liste, posée comme React l'attend. */
const setValue = (driver, css, value) =>
  driver.execute(
    `const el = document.querySelector(arguments[0]);
     const proto = el instanceof HTMLSelectElement
       ? HTMLSelectElement.prototype : HTMLInputElement.prototype;
     Object.getOwnPropertyDescriptor(proto, "value").set.call(el, arguments[1]);
     el.dispatchEvent(new Event(el instanceof HTMLSelectElement ? "change" : "input", { bubbles: true }));
     return el.value;`,
    [css, value],
  );
const waitText = (driver, css, predicate, what, timeoutMs = 20000) =>
  poll(
    () => text(driver, css),
    (value) => value !== null && predicate(value),
    {
      what,
      timeoutMs,
    },
  );

async function openHome(driver) {
  if (!(await clickLabeled(driver, "All transcriptions"))) {
    await driver.click(await driver.waitFor("nav a[href$='/tools']"));
    await driver.click(
      await driver.waitFor("[data-testid=tool-card-transcription]"),
    );
  }
  await driver.waitFor("[data-testid=transcription-form]");
}

/** Transcription finie : son texte (paragraphes). */
async function finishedText(driver) {
  await driver.waitFor("[data-testid=transcription-transcript]", {
    timeoutMs: 180000,
  });
  return text(driver, "[data-testid=transcription-text]");
}

try {
  await withApp(
    binary,
    {
      name: "transcription",
      env: {
        ...engine.env,
        ...github.env,
        CHATONLM_E2E_PICK_MEDIA: MEDIA,
        CHATONLM_E2E_SAVE_FILE: SAVED,
      },
    },
    async (driver) => {
      await driver.waitFor("[data-testid=new-chat]");
      await driver.click(
        await driver.waitFor("button[aria-label='Show sidebar']"),
      );
      await openHome(driver);
      await driver.waitFor("[data-testid=transcription-empty]");
      await driver.screenshot(".tools/shots/transcription-home.png");

      // 1. Fichier local, Whisper tiny, anglais.
      await driver.click(
        await driver.waitFor("[data-testid=transcription-pick]"),
      );
      await waitText(
        driver,
        "[data-testid=transcription-file]",
        (value) => value.includes("jfk.wav") && /0:1\d/.test(value),
        "fichier choisi et lu",
      );
      assert.equal(
        await setValue(
          driver,
          "[data-testid=transcription-model]",
          "whisper-tiny",
        ),
        "whisper-tiny",
      );
      await setValue(driver, "[data-testid=transcription-language]", "en");
      await driver.click(
        await driver.waitFor("[data-testid=transcription-start]"),
      );
      await driver.waitFor("[data-testid=transcription-view]");
      const spoken = await finishedText(driver);
      assert.match(spoken, /ask not what your country/i);
      assert.equal(
        await text(driver, "[data-testid=transcription-title]"),
        "jfk.wav",
      );
      await driver.click(
        await driver.waitFor("[data-testid=transcription-timestamps]"),
      );
      await driver.waitFor("[data-testid=transcription-stamp]");
      await driver.screenshot(".tools/shots/transcription-done.png");
      await driver.click(
        await driver.waitFor("[data-testid=transcription-save-txt]"),
      );
      await waitText(
        driver,
        "[data-testid=transcription-saved]",
        (value) => value.includes(`${SAVED}.txt`),
        ".txt enregistré",
      );
      const txt = readFileSync(`${SAVED}.txt`, "utf8");
      assert.match(txt, /^\[00:00\] /);
      assert.match(txt, /ask not what your country/i);

      // 2. Synthétiser (faux moteur), puis enregistrer.
      await setValue(
        driver,
        "[data-testid=transcription-synthesis-model]",
        SYNTHESIS,
      );
      await driver.click(
        await driver.waitFor("[data-testid=transcription-synthesize]"),
      );
      await poll(
        () =>
          attribute(
            driver,
            "[data-testid=transcription-synthesis]",
            "data-stage",
          ),
        (stage) => stage === "done",
        { what: "document écrit", timeoutMs: 60000 },
      );
      const documentText = await waitText(
        driver,
        "[data-testid=transcription-document]",
        (value) => value.length > 20,
        "document affiché",
      );
      assert.match(documentText, /Summary/);
      await driver.screenshot(".tools/shots/transcription-synthesis.png");
      await driver.click(
        await driver.waitFor("[data-testid=transcription-document-md]"),
      );
      await waitText(
        driver,
        "[data-testid=transcription-document-saved]",
        (value) => value.includes(`${SAVED}.md`),
        ".md enregistré",
      );
      assert.match(readFileSync(`${SAVED}.md`, "utf8"), /^# Summary/m);
      await driver.click(
        await driver.waitFor("[data-testid=transcription-document-docx]"),
      );
      await waitText(
        driver,
        "[data-testid=transcription-document-saved]",
        (value) => value.includes(`${SAVED}.docx`),
        ".docx enregistré",
      );
      assert.equal(
        readFileSync(`${SAVED}.docx`).subarray(0, 2).toString(),
        "PK",
      );

      // 3. Adresse : accord, outils installés, transcription lancée seule.
      await openHome(driver);
      await setValue(
        driver,
        "[data-testid=transcription-address]",
        "https://video.test/ok?title=Test%20talk&d=11",
      );
      await driver.waitFor("[data-testid=transcription-tools-install]");
      await driver.click(
        await driver.waitFor("[data-testid=transcription-start]"),
      );
      const consent = await waitText(
        driver,
        "[data-testid=transcription-consent]",
        (value) => value.includes("Unlicense"),
        "boîte d'accord",
      );
      assert.match(
        consent,
        /yt-dlp \(Unlicense\) and Deno \(MIT\) from GitHub/,
      );
      assert.match(consent, /GPL or the LGPL/);
      assert.match(consent, /you have the right to copy for your personal use/);
      // Accord explicite : le focus est sur « Cancel », pas sur l'accord (un
      // Entrée de plus ne télécharge rien).
      assert.equal(
        await driver.execute(
          "return document.activeElement?.closest('[role=alertdialog]') ? document.activeElement.textContent.trim() : null",
        ),
        "Cancel",
      );
      await driver.screenshot(".tools/shots/transcription-consent.png");
      assert.ok(await clickLabeled(driver, "Accept and download"));
      await driver
        .waitFor("[data-testid=transcription-view]", { timeoutMs: 60000 })
        .catch(async (error) => {
          // Diagnostic : état des outils et du formulaire au moment de l'échec.
          console.error(
            "formulaire :",
            await text(driver, "[data-testid=transcription-form]"),
          );
          await driver.screenshot(".tools/shots/transcription-tools-stuck.png");
          throw error;
        });
      // Audio du faux yt-dlp : les 4 premières secondes du discours.
      const fetched = await finishedText(driver);
      assert.match(fetched, /fellow Americans/i);
      assert.equal(
        await text(driver, "[data-testid=transcription-title]"),
        "Test talk",
      );
      const requests = await github.requests();
      assert.ok(
        JSON.stringify(requests).includes("yt-dlp_linux.zip"),
        `archive de yt-dlp non demandée : ${JSON.stringify(requests)}`,
      );

      // 4. Vidéo privée : la raison, d'après le code.
      await openHome(driver);
      await setValue(
        driver,
        "[data-testid=transcription-address]",
        "https://video.test/private",
      );
      // Outils déjà là : plus de boîte d'accord.
      await waitText(
        driver,
        "[data-testid=transcription-tools]",
        (value) => value.includes("are installed"),
        "outils installés",
      );
      await driver.click(
        await driver.waitFor("[data-testid=transcription-start]"),
      );
      await driver.waitFor("[data-testid=transcription-error]", {
        timeoutMs: 60000,
      });
      assert.equal(
        await attribute(
          driver,
          "[data-testid=transcription-error]",
          "data-fetch",
        ),
        "private",
      );
      assert.match(
        await text(driver, "[data-testid=transcription-error]"),
        /The video couldn't be retrieved/,
      );
      await driver.screenshot(".tools/shots/transcription-error.png");

      // 5. Historique : les deux réussies (l'échec sans texte n'y est pas).
      await openHome(driver);
      const titles = await poll(
        () =>
          driver.execute(
            `return [...document.querySelectorAll("[data-testid=transcription-item]")]
               .map((row) => row.querySelector("a span span").textContent)`,
          ),
        (list) => list.length === 2,
        { what: "historique" },
      );
      assert.deepEqual(titles, ["Test talk", "jfk.wav"]);
    },
  );
} finally {
  await github.stop();
}
console.log("transcription : OK");
