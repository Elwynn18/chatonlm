// Lien VS Code dans l'app réelle (build e2e), avec le faux Ollama et un client
// Node qui fait ce que fera l'extension (e2e/lib/bridge.mjs) :
// - coupé par défaut : aucun point d'accès, aucun fichier de découverte ;
// - Settings > VS Code : la bascule ouvre le socket (dossier 0700, socket 0600)
//   et écrit bridge.json ;
// - « Pair an editor » montre un code ; tapé par le client, il l'appaire ; une
//   nouvelle connexion se prouve dans les deux sens, l'éditeur apparaît
//   « Connected » dans l'onglet ;
// - chat.send : la réponse du modèle arrive en flux, la conversation apparaît
//   dans la barre latérale sans rechargement, marquée « VS Code » ;
// - chat.get rend la conversation, chat.list les seules conversations de
//   l'éditeur (pas celle écrite dans l'app), models.details dit où tourne le
//   modèle et son contexte, app.state si le serveur est sur la machine ;
// - conversation éphémère de l'éditeur : hors de l'historique ; chat.keep la
//   fait apparaître dans la barre latérale, chat.forget l'efface ;
// - « Remove » ferme la connexion, efface l'éphémère restée de l'éditeur, et
//   l'éditeur n'est plus reconnu ;
// - coupé de nouveau : socket et bridge.json retirés ;
// - aucune violation de la CSP ni erreur dans la console.
import assert from "node:assert/strict";
import { existsSync, statSync } from "node:fs";
import { withApp } from "./lib/app.mjs";
import { BridgeClient, findDiscovery, readDiscovery } from "./lib/bridge.mjs";
import { startFake } from "./lib/fake.mjs";

const binary = process.argv[2];
const fake = await startFake("ollama");
const MODEL = "qwen3:0.6b";

const delay = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

async function waitUntil(check, what, timeoutMs = 20000) {
  const deadline = Date.now() + timeoutMs;
  for (;;) {
    const value = await check();
    if (value) return value;
    if (Date.now() > deadline) throw new Error(`Délai dépassé : ${what}`);
    await delay(100);
  }
}

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

const navigate = (driver, path) =>
  driver.execute(
    `const index = (history.state?.__TSR_index ?? 0) + 1;
     const key = Math.random().toString(36).slice(2);
     history.pushState({ key, __TSR_key: key, __TSR_index: index }, "", arguments[0]);`,
    [path],
  );

/** Clique le bouton de l'onglet dont le texte est `label`. */
const press = (driver, label) =>
  driver.execute(
    `const button = [...document.querySelectorAll("[data-testid=vscode-settings] button")]
       .find((b) => b.textContent.trim() === arguments[0]);
     if (!button || button.disabled) return false;
     button.click();
     return true;`,
    [label],
  );

const toggle = (driver) =>
  driver.execute(
    `const toggle = document.querySelector("[data-testid=vscode-settings] [role=switch]");
     if (!toggle) return null;
     toggle.click();
     return toggle.getAttribute("aria-checked");`,
  );

const status = (driver) =>
  driver.execute(
    `return document.querySelector("[data-testid=vscode-status]")?.dataset.state ?? null`,
  );

const editors = (driver) =>
  driver.execute(
    `return [...document.querySelectorAll("[data-testid=vscode-editor]")].map((row) => row.textContent)`,
  );

/** Envoie un message depuis la page ; rend la conversation créée, répondue. */
async function sendFromPage(driver, text) {
  const textarea = await driver.waitFor("textarea[aria-label='Message']");
  await driver.type(textarea, text);
  await driver.execute(
    `document.querySelector("textarea[aria-label='Message']").dispatchEvent(
       new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true }));`,
  );
  const id = await waitUntil(async () => {
    const shown = await driver.execute("return location.pathname");
    return shown.startsWith("/c/") && shown !== "/c/new" && shown.slice(3);
  }, "conversation créée par la page");
  await waitUntil(
    () =>
      driver.execute(
        `return !!document.querySelector("button[aria-label='Send message']")`,
      ),
    "fin de la réponse",
  );
  return id;
}

const sidebarTitles = (driver) =>
  driver.execute(
    `return [...document.querySelectorAll("a[href^='/c/']:not([href='/c/new'])")].map((a) => a.textContent)`,
  );

try {
  await withApp(
    binary,
    { name: "vscode-bridge", env: { OLLAMA_HOST: fake.url } },
    async (driver) => {
      await driver.waitFor("[data-testid=new-chat]");
      await driver.execute(WATCH_PAGE);
      await driver.invoke("update_settings", {
        patch: { selectedModel: MODEL },
      });

      // Coupé par défaut.
      assert.equal(findDiscovery(), null, "no discovery file while off");
      await navigate(driver, "/settings?section=vscode");
      await driver.waitFor("[data-testid=vscode-settings] [role=switch]");
      assert.equal(await status(driver), "off");
      assert.equal(
        await press(driver, "Pair an editor"),
        false,
        "pairing needs the bridge",
      );

      // Activé : socket privé, fichier de découverte.
      await toggle(driver);
      await waitUntil(
        async () => (await status(driver)) === "listening",
        "pont ouvert",
      );
      const found = await waitUntil(findDiscovery, "bridge.json");
      const discovery = readDiscovery();
      assert.deepEqual(discovery.protocol, [1, 1]);
      assert.equal(statSync(found.dir).mode & 0o777, 0o700);
      assert.equal(statSync(discovery.endpoint).mode & 0o777, 0o600);
      assert.equal(statSync(found.file).mode & 0o777, 0o600);

      // Appairage par le code montré.
      await waitUntil(() => press(driver, "Pair an editor"), "Pair an editor");
      const shown = await waitUntil(
        () =>
          driver.execute(
            `return document.querySelector("[data-testid=vscode-code]")?.textContent ?? null`,
          ),
        "code d'appairage",
      );
      const code = shown.replace(/\D/g, "");
      assert.match(code, /^\d{6}$/, shown);

      const pairing = await BridgeClient.connect(discovery.endpoint);
      // Rien d'autre que la poignée de main avant la preuve.
      const early = await pairing.request("chat.send", { content: "Hi" });
      assert.equal(early.error?.code, -32001);
      await pairing.initialize();
      const wrong = await pairing.pair(code === "000000" ? "000001" : "000000");
      assert.equal(wrong.error?.code, -32005);
      const { clientId, secret } = await pairing.pair(code, "VS Code e2e");
      assert.equal(secret.length, 32);
      await pairing.close();
      await waitUntil(
        async () =>
          !(await driver.execute(
            `return !!document.querySelector("[data-testid=vscode-code]")`,
          )),
        "code consommé, retiré de l'onglet",
      );

      const client = await BridgeClient.connect(discovery.endpoint);
      await client.initialize();
      const authed = await client.auth(clientId, secret);
      assert.ok(authed.result, JSON.stringify(authed));
      await waitUntil(
        async () =>
          (await editors(driver)).some(
            (row) => row.includes("VS Code e2e") && row.includes("Connected"),
          ),
        "éditeur connecté dans l'onglet",
      );

      // Conversation par le pont.
      const state = await client.request("app.state");
      assert.equal(state.result.selectedModel, MODEL);
      // Faux Ollama sur 127.0.0.1 : sur cette machine ; contexte du serveur.
      assert.equal(state.result.local, true, JSON.stringify(state));
      assert.equal(state.result.contextLength, 0);
      const models = await client.request("models.list");
      assert.ok(Array.isArray(models.result), JSON.stringify(models));
      // La page est sur une conversation vide : la barre latérale suit
      // l'évènement du moteur, sans rechargement.
      await navigate(driver, "/c/new");
      await driver.waitFor("[data-testid=new-chat]");
      await driver.click(
        await driver.waitFor("button[aria-label='Show sidebar']"),
      );
      assert.deepEqual(await sidebarTitles(driver), []);
      const sent = await client.request("chat.send", {
        content: "Hello from VS Code",
      });
      assert.equal(sent.result?.outcome, "done", JSON.stringify(sent));
      const types = client.notifications
        .filter((n) => n.method === "chat.event")
        .map((n) => n.params.event.type);
      assert.equal(types[0], "chatCreated");
      assert.ok(types.includes("content"), types.join(","));
      assert.equal(types.at(-1), "done");
      await waitUntil(
        async () => (await sidebarTitles(driver)).length > 0,
        "conversation dans la barre latérale",
      );
      const chat = await driver.invoke("get_chat", {
        id: sent.result.chatId,
      });
      assert.equal(chat.messages[0].content, "Hello from VS Code");
      assert.ok(chat.messages[1].content.length > 0);
      // Une conversation écrite dans l'app, à côté : sans marque, et absente
      // de chat.list.
      const own = await sendFromPage(driver, "Hello from the app");
      await waitUntil(
        async () => (await sidebarTitles(driver)).length === 2,
        "deux conversations dans la barre latérale",
      );
      // Marque « VS Code » devant son titre, et seulement devant le sien.
      const marks = await waitUntil(async () => {
        const found = await driver.execute(
          `return [...document.querySelectorAll("a[href^='/c/']")]
             .filter((a) => a.querySelector("[data-testid=chat-source]"))
             .map((a) => {
               const mark = a.querySelector("[data-testid=chat-source]");
               return { href: a.getAttribute("href"), label: mark.getAttribute("aria-label"),
                        title: mark.title, svg: !!mark.querySelector("svg") };
             })`,
        );
        return found.length > 0 ? found : null;
      }, "marque VS Code dans la barre latérale");
      assert.deepEqual(marks, [
        {
          href: `/c/${sent.result.chatId}`,
          label: "VS Code chat",
          title: "VS Code chat",
          svg: true,
        },
      ]);

      // chat.get : la même conversation que la page lit.
      const got = await client.request("chat.get", {
        chatId: sent.result.chatId,
      });
      assert.equal(got.result?.id, sent.result.chatId, JSON.stringify(got));
      assert.deepEqual(
        got.result.messages.map((m) => [m.role, m.content]),
        chat.messages.map((m) => [m.role, m.content]),
      );
      const unknown = await client.request("chat.get", { chatId: "nowhere" });
      assert.equal(unknown.error?.code, -32020, JSON.stringify(unknown));

      // chat.list : les conversations de l'éditeur seulement.
      const listed = await client.request("chat.list");
      assert.deepEqual(
        listed.result?.map((c) => [c.id, c.source]),
        [[sent.result.chatId, "vscode"]],
        JSON.stringify(listed),
      );
      assert.ok(!listed.result.some((c) => c.id === own));

      // models.details : modèle local, pas de cloud.
      const details = await client.request("models.details", { model: MODEL });
      assert.equal(details.result?.model, MODEL, JSON.stringify(details));
      assert.equal(details.result.remote, false);
      assert.ok(
        "contextLength" in details.result,
        "contexte employé (null : défaut du serveur)",
      );

      // Conversations éphémères de l'éditeur.
      const exists = (id) =>
        driver.invoke("get_chat", { id }).then(
          () => true,
          () => false,
        );
      const ephemeral = async (content) => {
        const sent = await client.request("chat.send", {
          content,
          ephemeral: true,
        });
        assert.equal(sent.result?.outcome, "done", JSON.stringify(sent));
        return sent.result.chatId;
      };
      const kept = await ephemeral("Keep me");
      assert.equal(
        (await driver.invoke("get_chat", { id: kept })).ephemeral,
        true,
      );
      assert.equal((await sidebarTitles(driver)).length, 2, "hors historique");
      const keep = await client.request("chat.keep", { chatId: kept });
      assert.deepEqual(keep.result, {}, JSON.stringify(keep));
      await waitUntil(
        async () => (await sidebarTitles(driver)).length === 3,
        "conversation gardée dans la barre latérale",
      );
      assert.ok(
        (await client.request("chat.list")).result.some((c) => c.id === kept),
      );
      const refusedKeep = await client.request("chat.keep", { chatId: own });
      assert.equal(refusedKeep.error?.code, -32020);
      const forgotten = await ephemeral("Forget me");
      const forget = await client.request("chat.forget", {
        chatId: forgotten,
      });
      assert.deepEqual(forget.result, { forgotten: true });
      assert.equal(await exists(forgotten), false, "effacée");
      const notMine = await client.request("chat.forget", { chatId: own });
      assert.deepEqual(notMine.result, { forgotten: false });
      assert.equal(await exists(own), true);
      // Restée ouverte dans l'éditeur : effacée au retrait.
      const left = await ephemeral("Left open");
      assert.equal(await exists(left), true);

      // Retrait : connexion fermée, éditeur oublié.
      await navigate(driver, "/settings?section=vscode");
      await driver.waitFor("[data-testid=vscode-editor]");
      await waitUntil(() => press(driver, "Remove"), "Remove");
      await Promise.race([
        client.whenClosed,
        delay(5000).then(() => {
          throw new Error("connexion restée ouverte après Remove");
        }),
      ]);
      await waitUntil(
        async () => (await editors(driver)).length === 0,
        "éditeur retiré de l'onglet",
      );
      assert.equal(await exists(left), false, "éphémère effacée au retrait");
      assert.equal(await exists(kept), true, "conversation gardée intacte");
      const again = await BridgeClient.connect(discovery.endpoint);
      await again.initialize();
      const refused = await again.auth(clientId, secret);
      assert.equal(refused.error?.code, -32002);
      await again.close();

      // Coupé : plus de socket ni de fichier de découverte.
      await toggle(driver);
      await waitUntil(
        async () => (await status(driver)) === "off",
        "pont coupé",
      );
      await waitUntil(
        () => !existsSync(found.file) && !existsSync(discovery.endpoint),
        "socket et bridge.json retirés",
      );

      const watched = await driver.execute(
        `return { csp: window.__csp ?? [], errors: window.__errors ?? [] }`,
      );
      assert.deepEqual(watched.csp, [], "CSP");
      assert.deepEqual(watched.errors, [], "console.error");
    },
  );
  console.log("✓ vscode-bridge");
} finally {
  await fake.stop();
}
