// « Erase my data » dans l'app réelle (faux moteur : fake-llama,
// chatonlm-simple), propre à ChatonLM :
// - une conversation écrite, un pré-prompt et le thème sombre réglés, la
//   langue choisie ; une copie de la base et des clés distantes posées dans
//   le dossier des données, avec la clé de l'ancien Ollama intégré
//   (`ollama/home/.ollama/id_ed25519`), un de ses modèles (`ollama/models`)
//   et un journal (`logs/x.log`) ; un bot Telegram relié (faux Telegram), le
//   pont de VS Code ouvert, un texte sur la page Proofread ;
// - Settings › General : la boîte liste ce qui est effacé et gardé ;
//   confirmée, l'app reste ouverte, passe à une nouvelle conversation,
//   l'historique est vide et l'annonce « Your data has been erased. » suit ;
// - réglages par défaut (langue et visite guidée gardées), copie de la base
//   et clés effacées, clé de l'ancien Ollama et journal effacés, ses modèles
//   gardés ; bot arrêté et oublié, pont coupé, page Proofread vide ; modèle
//   et son fichier gardés : la conversation suivante répond toujours.
import assert from "node:assert/strict";
import { existsSync, mkdirSync, readdirSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { setTimeout as delay } from "node:timers/promises";
import { withApp } from "./lib/app.mjs";
import { fakeModel, startEngine } from "./lib/fake.mjs";
import { FAKE_TOKENS, startFakeMessenger } from "./lib/messenger.mjs";

const binary = process.argv[2];
const MODEL = fakeModel("simple");
const engine = startEngine([MODEL]);
const DATA = process.env.CHATONLM_DATA_DIR;
const telegram = await startFakeMessenger("telegram");

async function waitUntil(check, what, timeoutMs = 15000) {
  const deadline = Date.now() + timeoutMs;
  for (;;) {
    const value = await check();
    if (value) return value;
    if (Date.now() > deadline) throw new Error(`Délai dépassé : ${what}`);
    await delay(100);
  }
}

const exists = (driver, css) =>
  driver.execute("return document.querySelector(arguments[0]) !== null", [css]);

/** Fichiers `.gguf` sous `dir` (récursif). */
function ggufs(dir) {
  if (!existsSync(dir)) return [];
  return readdirSync(dir, { recursive: true }).filter((name) =>
    String(name).endsWith(".gguf"),
  );
}

async function send(driver, text) {
  await driver.type(
    await driver.waitFor("textarea[aria-label='Message']"),
    text,
  );
  await driver.execute(
    `document.querySelector("textarea[aria-label='Message']").dispatchEvent(
       new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true }));`,
  );
}

async function waitReply(driver, text) {
  await waitUntil(async () => {
    const replies = await driver.execute(
      "return [...document.querySelectorAll('[data-role=assistant]')].map((e) => e.textContent)",
    );
    return (
      replies.at(-1)?.includes(text) &&
      !(await exists(driver, "[aria-label='Stop']"))
    );
  }, `réponse « ${text} »`);
}

/** Barre latérale ouverte. */
async function showSidebar(driver) {
  if (await exists(driver, "button[aria-label='Show sidebar']")) {
    await driver.click(await driver.find("button[aria-label='Show sidebar']"));
  }
}

const clickByText = (driver, css, text) =>
  driver.execute(
    `const found = [...document.querySelectorAll(arguments[0])]
       .find((e) => e.textContent.trim() === arguments[1]);
     found?.click();
     return found !== undefined;`,
    [css, text],
  );

await withApp(
  binary,
  {
    name: "erase-data",
    env: { ...engine.env, CHATONLM_TELEGRAM_URL: telegram.url },
  },
  async (driver) => {
    await driver.waitFor("[data-testid=new-chat]");
    const model = await waitUntil(
      async () => (await driver.invoke("get_settings")).selectedModel,
      "modèle choisi au départ",
    );
    assert.equal(model, MODEL);
    await driver.invoke("update_settings", {
      patch: {
        language: "en",
        theme: "dark",
        systemPrompt: "Answer in verse.",
      },
    });

    // Une conversation.
    await send(driver, "Hello");
    await waitReply(driver, "Hello! You said: Hello");
    await showSidebar(driver);
    await waitUntil(
      () => exists(driver, "[data-chat-id]"),
      "conversation listée",
    );
    assert.equal((await driver.invoke("list_chats")).length, 1);

    // Bot relié, pont ouvert.
    await driver.invoke("easy_link_connect", {
      platform: "telegram",
      token: FAKE_TOKENS.telegram,
    });
    const telegramBot = async () =>
      (await driver.invoke("easy_link_state")).bots.find(
        (bot) => bot.platform === "telegram",
      );
    await waitUntil(
      async () => (await telegramBot()).link.state === "connected",
      "bot Telegram connecté",
    );
    await driver.invoke("bridge_set_enabled", { enabled: true });
    assert.equal((await driver.invoke("bridge_state")).enabled, true);

    // Texte laissé sur la page Proofread.
    const openProofread = async () => {
      await driver.click(await driver.waitFor("a[data-pill=tools]"));
      await driver.click(
        await driver.waitFor("[data-testid=tool-card-proofread]"),
      );
      return driver.waitFor("[data-testid=proofread] textarea");
    };
    await driver.type(await openProofread(), "My secret letter");
    const proofreadText = () =>
      driver.execute(
        "return document.querySelector('[data-testid=proofread] textarea').value",
      );
    assert.equal(await proofreadText(), "My secret letter");

    // Fichiers de l'utilisateur hors de la base.
    const backup = join(DATA, "db", "chatonlm.sqlite.v38.bak");
    const keys = join(DATA, "remote", "keys.json");
    writeFileSync(backup, "old conversations");
    mkdirSync(join(DATA, "remote"), { recursive: true });
    writeFileSync(keys, '{"version":1,"keys":{"x":"secret"}}');
    const ollamaKey = join(DATA, "ollama", "home", ".ollama", "id_ed25519");
    const ollamaModel = join(DATA, "ollama", "models", "blobs", "sha256-x");
    const log = join(DATA, "logs", "x.log");
    for (const file of [ollamaKey, ollamaModel, log]) {
      mkdirSync(dirname(file), { recursive: true });
      writeFileSync(file, "secret");
    }
    const weights = ggufs(join(DATA, "models"));
    assert.ok(weights.length > 0, "fichier du modèle");

    // Settings › General : la zone rouge, puis la boîte.
    await driver.click(await driver.waitFor("a[data-pill=settings]"));
    await driver.waitFor("[data-testid=erase-data]");
    await driver.execute(
      "document.querySelector('[data-testid=erase-data]').scrollIntoView()",
    );
    assert.ok(
      await clickByText(
        driver,
        "[data-testid=erase-data] button",
        "Erase my data",
      ),
    );
    await driver.waitFor("[role=alertdialog]");
    const dialog = await driver.execute(
      "return document.querySelector('[role=alertdialog]').textContent",
    );
    assert.match(
      dialog,
      /Your conversations, with their attachments and images/,
    );
    assert.match(dialog, /Your models, their files/);
    await driver.screenshot(".tools/shots/erase-data-dialog.png");
    assert.ok(
      await clickByText(driver, "[role=alertdialog] button", "Erase my data"),
    );

    // État vide, app ouverte.
    await driver.waitFor("[data-testid=new-chat]");
    await waitUntil(
      () =>
        driver.execute(
          "return document.querySelector('[data-testid=notice]')?.textContent === 'Your data has been erased.'",
        ),
      "annonce de l'effacement",
    );
    await showSidebar(driver);
    await waitUntil(
      async () => !(await exists(driver, "[data-chat-id]")),
      "historique vide",
    );
    await driver.screenshot(".tools/shots/erase-data-done.png");
    assert.deepEqual(await driver.invoke("list_chats"), []);
    const settings = await driver.invoke("get_settings");
    assert.equal(settings.language, "en");
    assert.equal(settings.onboardingDone, true);
    assert.equal(settings.theme, "system");
    assert.equal(settings.systemPrompt, "");
    assert.equal(
      await driver.execute(
        "return document.documentElement.classList.contains('dark')",
      ),
      await driver.execute(
        "return matchMedia('(prefers-color-scheme: dark)').matches",
      ),
    );
    assert.equal(existsSync(backup), false, "copie de la base effacée");
    assert.equal(existsSync(keys), false, "clés distantes effacées");
    assert.equal(
      existsSync(ollamaKey),
      false,
      "clé de l'ancien Ollama effacée",
    );
    assert.equal(existsSync(join(DATA, "ollama", "home")), false);
    assert.equal(existsSync(log), false, "journal effacé");
    assert.equal(
      existsSync(ollamaModel),
      true,
      "modèle de l'ancien Ollama gardé",
    );

    // Bot arrêté et oublié, pont coupé.
    const bot = await telegramBot();
    assert.equal(bot.configured, false);
    assert.equal(bot.link.state, "off");
    assert.equal((await driver.invoke("bridge_state")).enabled, false);

    // Modèles gardés : listés, fichier là, la conversation suivante répond.
    const models = await driver.invoke("list_models");
    assert.deepEqual(
      models.map((entry) => entry.id),
      [MODEL],
    );
    assert.deepEqual(ggufs(join(DATA, "models")), weights);
    await waitUntil(
      async () => (await driver.invoke("get_settings")).selectedModel === MODEL,
      "modèle choisi de nouveau par la page",
    );
    // Page Proofread vide.
    await openProofread();
    assert.equal(await proofreadText(), "");
    await driver.click(await driver.waitFor("a[data-pill=chat]"));
    await driver.waitFor("[data-testid=new-chat]");
    await send(driver, "Hello again");
    await waitReply(driver, "Hello! You said: Hello again");
    assert.equal((await driver.invoke("list_chats")).length, 1);
  },
);
await telegram.stop();
