// Chef de la carte graphique et autocomplétion par le lien VS Code (lot 3),
// dans l'app réelle (build e2e), avec le faux moteur (fake-llama lancé par
// l'app sur des modèles inscrits) et le client Node du pont
// (e2e/lib/bridge.mjs) :
// - une proposition (complete.fim) ne charge JAMAIS le modèle : en pause
//   (`notLoaded`), aucun processus lancé ;
// - gpu.status rend le plan (machine simulée, modèles), puis gpu.changed
//   suit « Préparer » et un chargement fait par la page (bouton Load) ;
// - completion.prepare lance le modèle avec son contexte imposé (4 096,
//   `-c`), puis complete.fim répond par `/infill` (préfixe et suffixe
//   transmis) ;
// - une seule proposition en vol par connexion : la nouvelle arrête
//   l'ancienne (-32800) ;
// - pendant une réponse de conversation, ou un chargement demandé par la
//   page : en pause (`chatBusy`), rien d'envoyé ; reprise ensuite ;
// - completion.measure et completion.suggest (modèles d'autocomplétion de
//   Hugging Face, machine simulée : 24 Gio) ;
// - models.offerPull : la page propose le téléchargement (file du ModelHub),
//   qui ne part qu'à « Download » ; un modèle non proposé est refusé ;
// - aucune violation de la CSP ni erreur dans la console.
import assert from "node:assert/strict";
import { withApp } from "./lib/app.mjs";
import { BridgeClient, readDiscovery } from "./lib/bridge.mjs";
import { fakeModel, startEngine } from "./lib/fake.mjs";

const binary = process.argv[2];
const MODEL = fakeModel("simple");
const FIM = fakeModel("fim");
/** Proposition qui attend 300 ms avant de répondre (une seule en vol). */
const SLOW_FIM = fakeModel("fim-slow");
const HANG = fakeModel("hang");
const LONGLOAD = fakeModel("longload");
const engine = startEngine([
  MODEL,
  { id: FIM, base: true },
  { id: SLOW_FIM, base: true },
  HANG,
  LONGLOAD,
]);

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

/** Propositions reçues par l'instance de `id` (`/infill`). */
const infills = async (id) =>
  (await engine.requests(id))
    .filter((request) => request.path === "/infill")
    .map((request) => request.body);
/** Contexte de lancement (`-c`) de l'instance de `id`. */
const ctxOf = async (id) => {
  const args = await engine.args(id);
  return args ? args[args.indexOf("-c") + 1] : null;
};

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

/** Prochaine notification `method` reçue après `from` (index). */
async function notification(client, method, from, check = () => true) {
  return waitUntil(
    () =>
      client.notifications
        .slice(from)
        .find((message) => message.method === method && check(message.params))
        ?.params,
    `notification ${method}`,
  );
}

const paused = (answer) => {
  assert.equal(answer.error?.code, -32007, JSON.stringify(answer));
  return answer.error.data.reason;
};

/** Requête envoyée sans attendre : rend { id, answer }. */
function start(client, method, params) {
  const id = client.next;
  return { id, answer: client.request(method, params) };
}

{
  await withApp(
    binary,
    { name: "vscode-gpu", env: engine.env },
    async (driver) => {
      await driver.waitFor("[data-testid=new-chat]");
      await driver.execute(WATCH_PAGE);
      await driver.invoke("update_settings", {
        patch: { selectedModel: MODEL },
      });
      await driver.invoke("bridge_set_enabled", { enabled: true });
      const paired = await driver.invoke("bridge_pair");
      const discovery = await waitUntil(readDiscovery, "bridge.json");
      const pairing = await BridgeClient.connect(discovery.endpoint);
      await pairing.initialize();
      const { clientId, secret } = await pairing.pair(
        paired.pairing.code,
        "VS Code e2e",
      );
      await pairing.close();
      const client = await BridgeClient.connect(discovery.endpoint);
      await client.initialize();
      assert.ok((await client.auth(clientId, secret)).result);

      // Jamais de chargement par une proposition.
      const fim = { model: FIM, prefix: "def f(", suffix: ")\n" };
      assert.equal(
        paused(await client.request("complete.fim", fim)),
        "notLoaded",
      );
      assert.deepEqual(engine.servers(), [], "aucun processus lancé");

      // Plan : machine simulée (24 Gio de VRAM), rien en mémoire.
      const status = await client.request("gpu.status", { model: FIM });
      const plan = status.result;
      assert.equal(plan.completion.model, FIM, JSON.stringify(status));
      assert.equal(plan.completion.loaded, false);
      assert.equal(plan.chat.model, MODEL);
      assert.equal(plan.chat.loaded, false);
      assert.equal(plan.memory.total, 24 * 2 ** 30);
      assert.equal(plan.memory.gpu, true);
      assert.equal(plan.reason, "notLoaded");

      // Préparer : chargé avec les options figées ; le plan suit.
      let from = client.notifications.length;
      const prepared = await client.request("completion.prepare", {
        model: FIM,
      });
      assert.equal(
        prepared.result?.plan.completion.loaded,
        true,
        JSON.stringify(prepared),
      );
      assert.equal(await ctxOf(FIM), "4096");
      assert.deepEqual(
        engine.servers().map((entry) => entry.alias),
        [FIM],
        "seul le modèle d'autocomplétion est lancé",
      );
      await notification(
        client,
        "gpu.changed",
        from,
        (params) => params.plan.completion?.loaded,
      );
      const answer = await client.request("complete.fim", fim);
      assert.equal(answer.result?.text, "return a + b", JSON.stringify(answer));
      assert.equal(answer.result.timedOut, false);
      const [sent] = await infills(FIM);
      assert.equal(sent.input_prefix, "def f(");
      assert.equal(sent.input_suffix, ")\n");

      // Préparé avant que la page charge la conversation.
      assert.ok(
        (await client.request("completion.prepare", { model: SLOW_FIM }))
          .result,
      );
      assert.equal(await ctxOf(SLOW_FIM), "4096");

      // Chargement par la page (Load) : le plan suit aussitôt.
      from = client.notifications.length;
      await driver.invoke("load_model", { model: MODEL });
      const changed = await notification(
        client,
        "gpu.changed",
        from,
        (params) => params.plan.chat?.loaded,
      );
      assert.equal(changed.plan.mode, "together", JSON.stringify(changed));

      // Une seule proposition en vol : la nouvelle arrête l'ancienne
      // (`chatonlm-fim-slow` attend 300 ms avant de répondre).
      const before = (await infills(SLOW_FIM)).length;
      const first = start(client, "complete.fim", {
        model: SLOW_FIM,
        prefix: "a",
      });
      await waitUntil(
        async () => (await infills(SLOW_FIM)).length === before + 1,
        "première proposition envoyée",
      );
      const second = start(client, "complete.fim", {
        model: SLOW_FIM,
        prefix: "ab",
      });
      assert.equal((await first.answer).error?.code, -32800);
      const done = await second.answer;
      assert.equal(done.result?.text, "return a + b", JSON.stringify(done));
      assert.equal((await infills(SLOW_FIM)).at(-1).input_prefix, "ab");

      // Pendant une réponse de conversation : en pause, rien d'envoyé.
      const chatting = start(client, "chat.send", {
        content: "Wait",
        model: HANG,
      });
      await notification(
        client,
        "chat.event",
        0,
        (params) => params.event.type === "userMessage",
      );
      const sentBefore = (await infills(FIM)).length;
      assert.equal(
        paused(await client.request("complete.fim", fim)),
        "chatBusy",
      );
      assert.equal(
        paused(await client.request("completion.measure", { model: FIM })),
        "chatBusy",
      );
      assert.equal((await infills(FIM)).length, sentBefore);
      client.send({ method: "$/cancelRequest", params: { id: chatting.id } });
      assert.equal((await chatting.answer).result?.outcome, "cancelled");

      // Chargement lent demandé par la page : en pause aussi, puis reprise.
      await driver.execute(
        `window.__loading = window.__TAURI_INTERNALS__
           .invoke("load_model", { model: arguments[0] })
           .then(() => "done", (error) => String(error));
         return true;`,
        [LONGLOAD],
      );
      await waitUntil(
        () => engine.server(LONGLOAD) !== null,
        "chargement lent demandé",
      );
      const quick = { model: SLOW_FIM, prefix: "b", maxTokens: 1 };
      assert.equal(
        paused(await client.request("complete.fim", quick)),
        "chatBusy",
      );
      const loadedSlowly = await waitUntil(
        () =>
          driver.execute(
            `window.__loading.then((outcome) => { window.__loaded = outcome; });
             return window.__loaded ?? null;`,
          ),
        "chargement lent fini",
      );
      assert.equal(loadedSlowly, "done");
      assert.ok(
        (await client.request("complete.fim", quick)).result,
        "reprise après le chargement",
      );

      // Mesure et propositions. Le moteur garde 3 modèles au plus
      // (`MAX_LOADED`, src-tauri/src/engine/mod.rs) : FIM, le moins
      // récemment servi, a laissé sa place au modèle de la conversation
      // (HANG) ; une mesure ne charge jamais, « Préparer » le relance.
      assert.ok(
        !engine.servers().some((entry) => entry.alias === FIM),
        "FIM toujours en mémoire après deux autres chargements",
      );
      assert.equal(
        paused(await client.request("completion.measure", { model: FIM })),
        "notLoaded",
      );
      assert.ok(
        (await client.request("completion.prepare", { model: FIM })).result,
        "FIM préparé de nouveau",
      );
      const measure = await client.request("completion.measure", {
        model: FIM,
      });
      assert.ok(
        ["smooth", "usable", "tooSlow"].includes(measure.result?.rating),
        JSON.stringify(measure),
      );
      assert.ok(measure.result.tokensPerSecond > 0);
      // Modèles d'autocomplétion proposés : dépôts GGUF de Hugging Face
      // (`auteur/dépôt:QUANT`), le conseillé parmi eux.
      const suggested = await client.request("completion.suggest");
      const HF_ID = /^[\w.-]+\/[\w.-]+:[\w.-]+$/;
      const ids = suggested.result?.options.map((option) => option.model) ?? [];
      assert.ok(
        ids.length > 0 && ids.every((id) => HF_ID.test(id)),
        JSON.stringify(suggested),
      );
      assert.ok(ids.includes(suggested.result.recommended));
      assert.ok(
        suggested.result.options.every(
          (option) => typeof option.commercialUse === "boolean",
        ),
      );

      // Téléchargement proposé dans l'app, seulement après « Download ».
      const refused = await client.request("models.offerPull", {
        model: MODEL,
      });
      assert.equal(refused.error?.code, -32602);
      const pulled = ids.at(-1);
      const offered = await client.request("models.offerPull", {
        model: pulled,
      });
      assert.equal(offered.result?.installed, false, JSON.stringify(offered));
      const repoName = pulled
        .slice(pulled.indexOf("/") + 1, pulled.lastIndexOf(":"))
        .replace(/-GGUF$/i, "");
      const title = await waitUntil(
        () =>
          driver.execute(
            `const dialog = document.querySelector("[role=alertdialog]");
             const id = dialog?.getAttribute("aria-labelledby");
             return id ? document.getElementById(id)?.textContent ?? null : null;`,
          ),
        "proposition de téléchargement",
      );
      assert.ok(title.toLowerCase().includes(repoName.toLowerCase()), title);
      assert.deepEqual(
        await driver.invoke("hub_downloads"),
        [],
        "nothing before Download",
      );
      await driver.execute(
        `[...document.querySelectorAll("[role=alertdialog] button")]
           .find((b) => b.textContent.trim() === "Download").click();`,
      );
      await waitUntil(
        async () =>
          (await driver.invoke("hub_downloads")).some(
            (download) => download.model === pulled,
          ),
        "téléchargement mis en file",
      );

      await client.close();
      const problems = await driver.execute(
        "return { csp: window.__csp, errors: window.__errors }",
      );
      assert.deepEqual(problems.csp, [], "no CSP violation");
      assert.deepEqual(problems.errors, [], "no console error");
    },
  );
  console.log("vscode-gpu : OK");
}
