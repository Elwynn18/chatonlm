// Chef de la carte graphique et autocomplétion par le lien VS Code (lot 3),
// dans l'app réelle (build e2e), avec le faux Ollama et le client Node du pont
// (e2e/lib/bridge.mjs) :
// - une proposition (complete.fim) ne charge JAMAIS le modèle : en pause
//   (`notLoaded`), rien d'envoyé au modèle, rien en mémoire ;
// - gpu.status rend le plan (machine du serveur, modèles), puis gpu.changed
//   suit « Préparer » et un chargement fait par la page (bouton Load) ;
// - completion.prepare charge avec les options figées (contexte 4 096,
//   4 min en mémoire : plus court que la conversation, ici au défaut du
//   serveur, 5 min), puis complete.fim répond avec ces mêmes options ;
//   aucune proposition ne (re)charge un modèle (`byPrompt`) ;
// - une seule proposition en vol par connexion : la nouvelle arrête
//   l'ancienne (-32800) ;
// - pendant une réponse de conversation, ou un chargement demandé par la
//   page : en pause (`chatBusy`), rien d'envoyé ; reprise ensuite ;
// - completion.measure et completion.suggest (machine simulée : 24 Gio) ;
// - models.offerPull : la page propose le téléchargement, qui ne part qu'à
//   « Download » ; un nom non proposé est refusé ;
// - aucune violation de la CSP ni erreur dans la console.
import assert from "node:assert/strict";
import { withApp } from "./lib/app.mjs";
import { BridgeClient, readDiscovery } from "./lib/bridge.mjs";
import { startFake } from "./lib/fake.mjs";

const binary = process.argv[2];
const fake = await startFake("ollama");
const MODEL = "qwen3:0.6b";
const FIM = "chatonlm-fim";
const LONG = "chatonlm-fim-long";

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

const fakeGet = async (path) => (await fetch(new URL(path, fake.url))).json();
const generates = () => fakeGet("/fake/generates");
const loads = () => fakeGet("/fake/loads");
const loaded = async () =>
  (await fakeGet("/api/ps")).models.map((model) => model.name);

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

try {
  await withApp(
    binary,
    { name: "vscode-gpu", env: { OLLAMA_HOST: fake.url } },
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
      assert.deepEqual(await generates(), []);
      assert.deepEqual(await loaded(), []);
      assert.deepEqual(await loads(), []);

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
      assert.deepEqual(await loads(), [
        { model: FIM, kind: "load", keep_alive: 240, num_ctx: 4096 },
      ]);
      await notification(
        client,
        "gpu.changed",
        from,
        (params) => params.plan.completion?.loaded,
      );
      const answer = await client.request("complete.fim", fim);
      assert.equal(answer.result?.text, "value(6|2)\n    second line");
      assert.equal(answer.result.timedOut, false);
      const [sent] = await generates();
      assert.equal(sent.options.num_ctx, 4096);
      assert.equal(sent.keep_alive, 240);

      // Préparé avant que la page charge la conversation (taille apprise :
      // le faux modèle n'est pas dans /api/tags).
      assert.ok(
        (await client.request("completion.prepare", { model: LONG })).result,
      );
      assert.equal(sent.suffix, ")\n");
      assert.equal(sent.think, false);

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

      // Une seule proposition en vol : la nouvelle arrête l'ancienne.
      const before = (await generates()).length;
      const first = start(client, "complete.fim", { model: LONG, prefix: "a" });
      await waitUntil(
        async () => (await generates()).length === before + 1,
        "première proposition envoyée",
      );
      const second = start(client, "complete.fim", {
        model: LONG,
        prefix: "ab",
      });
      assert.equal((await first.answer).error?.code, -32800);
      const done = await second.answer;
      assert.ok(
        done.result?.text.startsWith("value(2|0)"),
        JSON.stringify(done),
      );

      // Pendant une réponse de conversation : en pause, rien d'envoyé.
      const chatting = start(client, "chat.send", {
        content: "Wait",
        model: "chatonlm-hang",
      });
      await notification(
        client,
        "chat.event",
        0,
        (params) => params.event.type === "userMessage",
      );
      const sentBefore = (await generates()).length;
      assert.equal(
        paused(await client.request("complete.fim", fim)),
        "chatBusy",
      );
      assert.equal(
        paused(await client.request("completion.measure", { model: FIM })),
        "chatBusy",
      );
      assert.equal((await generates()).length, sentBefore);
      client.send({ method: "$/cancelRequest", params: { id: chatting.id } });
      assert.equal((await chatting.answer).result?.outcome, "cancelled");

      // Chargement lent demandé par la page : en pause aussi, puis reprise.
      await driver.execute(
        `window.__loading = window.__TAURI_INTERNALS__
           .invoke("load_model", { model: "chatonlm-longload" })
           .then(() => "done", (error) => String(error));
         return true;`,
      );
      await waitUntil(
        async () =>
          (await loads()).some((load) => load.model === "chatonlm-longload"),
        "chargement lent demandé",
      );
      const quick = { model: LONG, prefix: "b", maxTokens: 1 };
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

      // Mesure et propositions.
      const measure = await client.request("completion.measure", {
        model: FIM,
      });
      assert.ok(
        ["smooth", "usable", "tooSlow"].includes(measure.result?.rating),
        JSON.stringify(measure),
      );
      assert.ok(measure.result.tokensPerSecond > 0);
      const suggested = await client.request("completion.suggest");
      assert.equal(suggested.result?.recommended, "qwen2.5-coder:7b-base");
      const three = suggested.result.options.find(
        (option) => option.model === "qwen2.5-coder:3b-base",
      );
      assert.equal(three.commercialUse, false);

      // Téléchargement proposé dans l'app, seulement après « Download ».
      const refused = await client.request("models.offerPull", {
        model: MODEL,
      });
      assert.equal(refused.error?.code, -32602);
      const offered = await client.request("models.offerPull", {
        model: "qwen2.5-coder:1.5b-base",
      });
      assert.equal(offered.result?.installed, false, JSON.stringify(offered));
      const title = await waitUntil(
        () =>
          driver.execute(
            `const dialog = document.querySelector("[role=alertdialog]");
             const id = dialog?.getAttribute("aria-labelledby");
             return id ? document.getElementById(id)?.textContent ?? null : null;`,
          ),
        "proposition de téléchargement",
      );
      assert.match(title, /qwen2\.5-coder:1\.5b-base/);
      assert.deepEqual(await fake.pulls(), [], "nothing before Download");
      await driver.execute(
        `[...document.querySelectorAll("[role=alertdialog] button")]
           .find((b) => b.textContent.trim() === "Download").click();`,
      );
      await waitUntil(
        async () =>
          (await fake.pulls()).some(
            (pull) => pull.model === "qwen2.5-coder:1.5b-base",
          ),
        "téléchargement lancé",
      );

      assert.deepEqual(
        (await loads()).filter((load) => load.kind === "byPrompt"),
        [],
        "no model (re)loaded by a proposal",
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
} finally {
  await fake.stop();
}
