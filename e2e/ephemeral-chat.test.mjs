// Conversations éphémères (propres à ChatonLM) dans l'app réelle, faux
// serveur : le chaton-fantôme en haut à droite rend la nouvelle conversation
// éphémère (hors de l'historique, avis au-dessus de la zone de saisie) ;
// « Keep » la met dans l'historique ; une conversation éphémère quittée pour
// une nouvelle est effacée ; celle encore ouverte quand l'app quitte aussi, et
// son texte ne reste ni dans la base ni dans son journal.
import assert from "node:assert/strict";
import { readFileSync, readdirSync } from "node:fs";
import { join } from "node:path";
import { withApp } from "./lib/app.mjs";
import { startFake } from "./lib/fake.mjs";

const binary = process.argv[2];
const dataDir = process.env.CHATONLM_DATA_DIR;
assert.ok(dataDir, "CHATONLM_DATA_DIR manquant (session virtuelle ?)");
const dbDir = join(dataDir, "db");
const fake = await startFake("ollama");
const delay = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
/** Texte reconnaissable, cherché dans les fichiers de la base à la fin. */
const SECRET = "zebra-violet-ephemeral-4729";

async function waitUntil(check, what, timeoutMs = 10000) {
  const deadline = Date.now() + timeoutMs;
  for (;;) {
    const value = await check();
    if (value) return value;
    if (Date.now() > deadline) throw new Error(`Délai dépassé : ${what}`);
    await delay(100);
  }
}

const attribute = (driver, css, name) =>
  driver.execute(
    "return document.querySelector(arguments[0])?.getAttribute(arguments[1]) ?? null",
    [css, name],
  );
const exists = (driver, css) =>
  driver.execute("return document.querySelector(arguments[0]) !== null", [css]);
const click = (driver, css) =>
  driver.execute("document.querySelector(arguments[0]).click()", [css]);
const path = (driver) => driver.execute("return location.pathname");

const GHOST = "[data-testid=ephemeral-button]";
const BANNER = "[data-testid=ephemeral-banner]";

/** Envoie un message et attend la page de la conversation créée, répondue. */
async function sendNew(driver, text) {
  const textarea = await driver.waitFor("textarea[aria-label='Message']");
  await driver.type(textarea, text);
  await driver.execute(
    `document.querySelector("textarea[aria-label='Message']").dispatchEvent(
       new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true }));`,
  );
  const id = await waitUntil(async () => {
    const shown = await path(driver);
    return shown.startsWith("/c/") && shown !== "/c/new" && shown.slice(3);
  }, "page de la conversation créée");
  await waitUntil(
    () => exists(driver, "button[aria-label='Send message']"),
    "fin de la réponse",
  );
  return id;
}

const listed = async (driver) =>
  (await driver.invoke("list_chats")).map((chat) => chat.id);

async function gone(driver, id) {
  try {
    await driver.invoke("get_chat", { id });
    return false;
  } catch (error) {
    // Refus de la commande : rendu par le pilote dans `code`.
    const kind = error?.code?.kind;
    if (kind !== undefined) return kind === "notFound";
    return /notFound/.test(String(error?.message ?? error));
  }
}

/** Bouton activé sur la page de nouvelle conversation. */
async function turnOn(driver) {
  await driver.waitFor(GHOST);
  if ((await attribute(driver, GHOST, "aria-pressed")) !== "true") {
    await click(driver, GHOST);
  }
  await waitUntil(
    async () => (await attribute(driver, GHOST, "aria-pressed")) === "true",
    "chaton-fantôme enfoncé",
  );
  assert.ok(await exists(driver, BANNER), "avis de la nouvelle conversation");
}

let lastOpen = null;
let kept = null;
try {
  await withApp(
    binary,
    { name: "ephemeral-chat", env: { OLLAMA_HOST: fake.url } },
    async (driver, app) => {
      await driver.waitFor("[data-testid=new-chat]");
      await driver.invoke("update_settings", {
        patch: { selectedModel: "qwen3:0.6b" },
      });

      // 1. Conversation éphémère : hors de l'historique, avis et Keep.
      await turnOn(driver);
      await driver.screenshot(".tools/shots/ephemeral-new.png");
      kept = await sendNew(driver, "Say hello in one word.");
      assert.equal(
        (await driver.invoke("get_chat", { id: kept })).ephemeral,
        true,
      );
      assert.ok(!(await listed(driver)).includes(kept), "dans l'historique");
      assert.ok(!(await exists(driver, `[data-chat-id='${kept}']`)));
      await driver.waitFor(BANNER);
      assert.equal(
        await attribute(driver, GHOST, "aria-label"),
        "Keep this chat",
      );
      await driver.screenshot(".tools/shots/ephemeral-chat.png");

      // 2. Keep : dans l'historique, plus d'avis.
      await click(driver, `${BANNER} button`);
      await waitUntil(
        async () => (await listed(driver)).includes(kept),
        "Keep",
      );
      await waitUntil(
        async () => !(await exists(driver, BANNER)),
        "avis retiré",
      );
      console.log("conversation gardée par Keep");

      // 3. Chaton-fantôme d'une conversation ordinaire : nouvelle éphémère,
      // effacée en ouvrant une nouvelle conversation.
      await click(driver, GHOST);
      await waitUntil(
        async () => (await path(driver)) === "/c/new",
        "page new",
      );
      await turnOn(driver);
      const left = await sendNew(driver, "Say hello in one word.");
      await click(driver, "a[aria-label='New chat']");
      await waitUntil(() => gone(driver, left), "conversation quittée effacée");
      assert.ok(await exists(driver, "[data-testid=new-chat]"));
      assert.equal(await attribute(driver, GHOST, "aria-pressed"), "false");
      console.log("conversation éphémère quittée : effacée");

      // 4. Conversation éphémère encore ouverte à la fin de l'app.
      await turnOn(driver);
      lastOpen = await sendNew(driver, `${SECRET} ${SECRET}`);
      await driver
        .execute("window.__TAURI_INTERNALS__.invoke('quit_app'); return true")
        .catch(() => {});
      const code = await Promise.race([app.exited, delay(15000)]);
      assert.equal(code, 0, "l'app n'a pas quitté proprement");
    },
  );

  // Après la fin : base relue hors de l'app.
  const { DatabaseSync } = await import("node:sqlite");
  const db = new DatabaseSync(join(dbDir, "chatonlm.sqlite"), {
    readOnly: true,
  });
  const ids = db
    .prepare("SELECT id, ephemeral FROM chats")
    .all()
    .map((row) => [row.id, row.ephemeral]);
  db.close();
  assert.deepEqual(ids, [[kept, 0]], "conversations restées en base");
  for (const name of readdirSync(dbDir)) {
    const bytes = readFileSync(join(dbDir, name));
    assert.ok(
      !bytes.includes(Buffer.from(SECRET)),
      `texte de la conversation éphémère retrouvé dans ${name}`,
    );
  }
  console.log(`fin de l'app : conversation ${lastOpen} effacée sans trace`);
  console.log("ephemeral-chat : OK");
} finally {
  await fake.stop();
}
