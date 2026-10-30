// Lien VS Code dans l'app réelle (build e2e), avec le faux Ollama et un client
// Node qui fait ce que fera l'extension (e2e/lib/bridge.mjs) :
// - coupé par défaut : aucun point d'accès, aucun fichier de découverte ;
// - Settings > VS Code : la bascule ouvre le socket (dossier 0700, socket 0600)
//   et écrit bridge.json ;
// - « Pair an editor » montre un code ; tapé par le client, il l'appaire ; une
//   nouvelle connexion se prouve dans les deux sens, l'éditeur apparaît
//   « Connected » dans l'onglet ;
// - chat.send : la réponse du modèle arrive en flux, la conversation apparaît
//   dans la barre latérale sans rechargement ;
// - « Remove » ferme la connexion et l'éditeur n'est plus reconnu ;
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
