// @vitest-environment node
// Autocomplétion locale (lot 3, X) : coupée par défaut, jamais de requête sans
// modèle prêt, pause jusqu'au prochain plan (pas de boucle), serveur distant
// ou modèle cloud → rien, fichier sensible → rien, annulation à la frappe,
// une seule requête en vol, anti-rebond, cache, autre complétion active,
// barre d'état, carte du panneau, installation par l'app ; réglages changés
// dans l'éditeur des réglages, retour d'inactivité, fichiers où rien n'est
// proposé, « trop lent », coupe sûre en UTF-16, délai côté client, CRLF.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { CompletionView, PanelMessage } from "../shared/protocol";
import {
  Completion,
  CompletionCache,
  DEBOUNCE_MS,
  PREFIX_CHARS,
  PREFIX_STEP_LINES,
  promptWindow,
  SLOW_AFTER,
  SLOW_DEBOUNCE_FACTOR,
  SUFFIX_CHARS,
  WAKE_INTERVAL_MS,
} from "./completion";
import { deferred, until } from "./test/fakeBridge";
import { cleanUp, ofType, openEditor, setupController } from "./test/setup";
import {
  CancellationTokenSource,
  env as vscodeEnv,
  InlineCompletionItem,
  Position,
  resetTestState,
  Selection,
  TextDocument,
  testState,
  workspace,
} from "./test/vscode";

// Anti-rebond réel (250 ms, plus long quand « trop lent ») et plusieurs
// contrôleurs dans un même essai.
vi.setConfig({ testTimeout: 30_000 });

beforeEach(() => resetTestState());
afterEach(() => cleanUp());

const MODEL = "qwen2.5-coder:1.5b-base";
const CODE = "function add(a, b) {\n  \n}\n";
const CONTEXT = { triggerKind: 1, selectedCompletionInfo: undefined };
const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));

type Env = Awaited<ReturnType<typeof setupController>>;

/** Proposition demandée au curseur (ligne 2, après l'indentation). */
function ask(
  env: Env,
  options: {
    file?: string;
    text?: string;
    at?: [number, number];
    token?: CancellationTokenSource;
  } = {},
) {
  const editor = openEditor(
    options.file ?? "/work/src/add.ts",
    options.text ?? CODE,
  );
  const [line, character] = options.at ?? [1, 2];
  editor.selection = new Selection(
    new Position(line, character),
    new Position(line, character),
  );
  const token = options.token ?? new CancellationTokenSource();
  return env.controller.completion.provideInlineCompletionItems(
    editor.document as never,
    new Position(line, character) as never,
    CONTEXT as never,
    token.token as never,
  );
}

const texts = (items: unknown[]) =>
  (items as InlineCompletionItem[]).map((item) => item.insertText);

const viewOf = (env: Env): CompletionView | null =>
  ofType(env.posted, "completion").at(-1)?.completion ?? null;

/** Item de la barre d'état de l'autocomplétion. */
const statusItem = () =>
  testState.statusItems.find(
    (item) => item.command === "chatonlm.completionMenu",
  );

/** Autocomplétion activée par l'utilisateur (le modèle est préparé). */
async function enabled(env?: Env) {
  const ready = env ?? (await setupController());
  await ready.controller.completion.setEnabled(true);
  return ready;
}

describe("autocomplete, off by default", () => {
  it("asks nothing, loads nothing and shows the item as off", async () => {
    const env = await setupController();
    expect(testState.config["chatonlm.completion.enabled"]).toBeUndefined();
    expect(texts(await ask(env))).toEqual([]);
    expect(env.fake.calls("complete.fim")).toEqual([]);
    expect(env.fake.calls("completion.prepare")).toEqual([]);
    // Personne ne regarde le plan : pas d'abonnement au chef.
    expect(env.fake.calls("gpu.status")).toEqual([]);
    expect(statusItem()?.visible).toBe(true);
    expect(statusItem()?.text).toContain("$(circle-slash)");
    expect(statusItem()?.tooltip).toContain("Autocomplete is off.");
  });

  it("turns on, prepares the model, then suggests", async () => {
    const env = await enabled();
    expect(testState.config["chatonlm.completion.enabled"]).toBe(true);
    expect(env.fake.calls("completion.prepare")).toEqual([{ model: MODEL }]);
    expect(viewOf(env)?.status).toBe("ready");
    expect(statusItem()?.text).toContain("$(check)");

    const items = await ask(env);
    expect(texts(items)).toEqual(["return a + b;"]);
    const [request] = env.fake.calls("complete.fim");
    expect(request).toEqual({
      model: MODEL,
      prefix: "function add(a, b) {\n  ",
      suffix: "\n}\n",
      language: "typescript",
      path: "src/add.ts",
      multiline: true,
    });
  });

  it("asks for a single line when code follows the cursor, or when set so", async () => {
    const env = await enabled();
    await ask(env, { text: "const x = (a);\n", at: [0, 11] });
    expect(env.fake.calls("complete.fim").at(-1)?.multiline).toBe(false);
    testState.config["chatonlm.completion.multiline"] = false;
    await ask(env, { text: "const y = \n", at: [0, 10] });
    expect(env.fake.calls("complete.fim").at(-1)?.multiline).toBe(false);
  });
});

describe("never asks without a ready model", () => {
  it("sends nothing while the plan says the model is not in memory", async () => {
    const env = await setupController();
    // Mémoire libre inconnue : pas de retour d'inactivité, « Prepare » reste
    // un geste de l'utilisateur.
    env.fake.gpu.mode = "unknown";
    // Activé dans les réglages (au démarrage) : rien n'est chargé tout seul.
    testState.config["chatonlm.completion.enabled"] = true;
    await env.controller.completion.refresh();
    expect(env.fake.calls("completion.prepare")).toEqual([]);
    expect(viewOf(env)).toMatchObject({
      status: "paused",
      reason: "notLoaded",
    });
    expect(texts(await ask(env))).toEqual([]);
    expect(env.fake.calls("complete.fim")).toEqual([]);
    expect(env.fake.calls("completion.prepare")).toEqual([]);
  });

  it("says why the chat model keeps the graphics card", async () => {
    const env = await setupController();
    env.fake.gpu.mode = "exclusive";
    await enabled(env);
    expect(viewOf(env)).toMatchObject({
      status: "paused",
      reason: "exclusive",
    });
    expect(testState.shown.at(-1)?.text).toBe(
      "Paused: the chat model is using the graphics card.",
    );
    expect(texts(await ask(env))).toEqual([]);
    expect(env.fake.calls("complete.fim")).toEqual([]);
  });

  it("stops asking after a pause, until the next plan", async () => {
    const env = await enabled();
    env.fake.gpu.chatBusy = true;
    expect(texts(await ask(env))).toEqual([]);
    expect(env.fake.calls("complete.fim")).toHaveLength(1);
    expect(viewOf(env)).toMatchObject({ status: "paused", reason: "chatBusy" });
    // Frappes suivantes : aucune requête, pas de boucle.
    for (let i = 0; i < 3; i++) {
      expect(texts(await ask(env, { at: [1, 1] }))).toEqual([]);
    }
    expect(env.fake.calls("complete.fim")).toHaveLength(1);
    // Retour dans la fenêtre : plan relu, inchangé, la pause tient.
    testState.events.windowState.fire({ focused: true });
    await until(() => env.fake.calls("gpu.status").length >= 2);
    await sleep(20);
    expect(texts(await ask(env, { at: [1, 1] }))).toEqual([]);
    expect(env.fake.calls("complete.fim")).toHaveLength(1);
    // Fin de la réponse : nouveau plan, une requête repart.
    env.fake.gpu.chatBusy = false;
    env.fake.notify("gpu.changed", { plan: env.fake.plan() });
    await until(() => viewOf(env)?.status === "ready");
    expect(texts(await ask(env, { at: [1, 0] }))).toEqual(["return a + b;"]);
    expect(env.fake.calls("complete.fim")).toHaveLength(2);
  });
});

describe("answers of the app (bridge, lot 3)", () => {
  it("asks for the plan of the chosen model", async () => {
    const env = await enabled();
    expect(env.fake.calls("gpu.status").at(-1)).toEqual({ model: MODEL });
  });

  it("asks nothing while the plan says a reply is being written", async () => {
    const env = await enabled();
    env.fake.gpu.chatBusy = true;
    env.fake.notify("gpu.changed", { plan: env.fake.plan() });
    await until(() => viewOf(env)?.reason === "chatBusy");
    expect(texts(await ask(env))).toEqual([]);
    expect(env.fake.calls("complete.fim")).toEqual([]);
  });

  it("proposes nothing when the first word came too late, and asks again later", async () => {
    const env = await enabled();
    env.fake.fimTimedOut = true;
    expect(texts(await ask(env))).toEqual([]);
    expect(viewOf(env)?.status).toBe("ready");
    env.fake.fimTimedOut = false;
    expect(texts(await ask(env))).toEqual(["return a + b;"]);
    expect(env.fake.calls("complete.fim")).toHaveLength(2);
  });

  it("stops asking a model that can't complete, and says why", async () => {
    const env = await setupController();
    env.fake.gpu.installed.add("chatonlm-nofim");
    testState.config["chatonlm.completion.model"] = "chatonlm-nofim";
    await enabled(env);
    expect(viewOf(env)?.status).toBe("ready");
    expect(texts(await ask(env))).toEqual([]);
    expect(viewOf(env)).toMatchObject({
      status: "unusable",
      error: "This model can't complete code in the middle of a file.",
    });
    expect(statusItem()?.tooltip).toContain(
      "This model can't complete code in the middle of a file.",
    );
    expect(texts(await ask(env, { at: [1, 1] }))).toEqual([]);
    expect(env.fake.calls("complete.fim")).toHaveLength(1);
    // Un autre modèle : les requêtes reprennent.
    await env.controller.handle({ type: "completionModel", model: MODEL });
    expect(texts(await ask(env, { at: [1, 0] }))).toEqual(["return a + b;"]);
  });

  it("reads the models again when the offered one is already installed", async () => {
    const env = await enabled();
    const before = env.fake.calls("completion.suggest").length;
    await env.controller.completion.offerPull("qwen2.5-coder:0.5b-base");
    expect(env.fake.calls("completion.suggest").length).toBe(before + 1);
    env.fake.local = false;
    await env.controller.completion.offerPull("qwen2.5-coder:7b-base");
    expect(testState.shown.at(-1)?.text).toBe(
      "Autocomplete needs a model on this computer.",
    );
  });
});

describe("only on this computer", () => {
  it("sends nothing when the server is on another computer", async () => {
    const env = await setupController();
    env.fake.local = false;
    await enabled(env);
    expect(viewOf(env)?.status).toBe("notLocal");
    expect(testState.shown.at(-1)?.text).toBe(
      "Autocomplete needs a model on this computer.",
    );
    expect(texts(await ask(env))).toEqual([]);
    expect(env.fake.calls("complete.fim")).toEqual([]);
    expect(env.fake.calls("completion.prepare")).toEqual([]);
  });

  it("stops when the app moves to another server", async () => {
    const env = await enabled();
    expect(texts(await ask(env))).toEqual(["return a + b;"]);
    env.fake.local = false;
    // `app.state` relu passé son délai : plus rien ne part.
    const realNow = Date.now;
    Date.now = () => realNow() + 60_000;
    try {
      expect(texts(await ask(env, { at: [1, 1] }))).toEqual([]);
    } finally {
      Date.now = realNow;
    }
    expect(env.fake.calls("complete.fim")).toHaveLength(1);
    expect(viewOf(env)?.status).toBe("notLocal");
  });

  it("sends nothing while the app does not say where the server is", async () => {
    const env = await setupController();
    const state = env.fake.handlers["app.state"];
    env.fake.handlers["app.state"] = (params, context) => {
      const answer = {
        ...(state(params, context) as Record<string, unknown>),
      };
      delete answer.local;
      return answer;
    };
    await enabled(env);
    expect(viewOf(env)?.status).toBe("checking");
    expect(texts(await ask(env))).toEqual([]);
    expect(env.fake.calls("complete.fim")).toEqual([]);
  });

  it("sends nothing to a cloud model", async () => {
    const env = await setupController();
    testState.config["chatonlm.completion.model"] = "qwen3-coder:480b-cloud";
    await enabled(env);
    expect(viewOf(env)?.status).toBe("notLocal");
    expect(texts(await ask(env))).toEqual([]);
    expect(env.fake.calls("complete.fim")).toEqual([]);
  });
});

describe("what is never completed", () => {
  it("leaves files that may hold secrets alone", async () => {
    const env = await enabled();
    for (const file of [
      "/work/.env",
      "/work/certs/server.pem",
      "/work/.npmrc",
    ]) {
      expect(
        texts(await ask(env, { file, text: "TOKEN=\n", at: [0, 6] })),
        file,
      ).toEqual([]);
    }
    expect(env.fake.calls("complete.fim")).toEqual([]);
  });

  it("leaves selections, several cursors and read-only file systems alone", async () => {
    const env = await enabled();
    const editor = openEditor("/work/src/add.ts", CODE);
    const document = editor.document;
    const run = () =>
      env.controller.completion.provideInlineCompletionItems(
        document as never,
        new Position(1, 2) as never,
        CONTEXT as never,
        new CancellationTokenSource().token as never,
      );
    editor.selection = new Selection(new Position(0, 0), new Position(1, 2));
    expect(texts(await run())).toEqual([]);
    editor.selection = new Selection(new Position(1, 2), new Position(1, 2));
    (editor as unknown as { selections: Selection[] }).selections = [
      editor.selection,
      new Selection(new Position(2, 0), new Position(2, 0)),
    ];
    expect(texts(await run())).toEqual([]);
    expect(env.fake.calls("complete.fim")).toEqual([]);

    testState.readOnlySchemes = ["file"];
    expect(texts(await ask(env))).toEqual([]);
    expect(env.fake.calls("complete.fim")).toEqual([]);
    testState.readOnlySchemes = [];
    expect(texts(await ask(env))).toEqual(["return a + b;"]);
  });

  it("leaves files made read-only by the settings alone", async () => {
    const env = await enabled();
    testState.config["files.readonlyInclude"] = {
      "**/vendor/**": true,
      "generated/**": true,
    };
    testState.config["files.readonlyExclude"] = { "**/vendor/own/**": true };
    for (const file of ["/work/lib/vendor/x.ts", "/work/generated/api.ts"]) {
      expect(texts(await ask(env, { file })), file).toEqual([]);
    }
    env.controller.completion.postView();
    expect(viewOf(env)?.status).toBe("notHere");
    expect(env.fake.calls("complete.fim")).toEqual([]);
    expect(
      texts(await ask(env, { file: "/work/lib/vendor/own/x.ts" })),
    ).toEqual(["return a + b;"]);
  });

  it("stays quiet where another inline completion is surely on, and offers a way out", async () => {
    const env = await enabled();
    testState.extensions.set("GitHub.copilot-chat", {});
    // Copilot allumé par un réglage de l'utilisateur : ChatonLM se tait.
    testState.config["github.copilot.enable"] = { "*": true };
    expect(texts(await ask(env))).toEqual([]);
    expect(env.fake.calls("complete.fim")).toEqual([]);
    env.controller.completion.postView();
    expect(viewOf(env)).toMatchObject({
      status: "otherProvider",
      otherProvider: "GitHub Copilot",
    });
    expect(statusItem()?.tooltip).toContain(
      "GitHub Copilot already suggests code in this file",
    );
    // Copilot coupé pour ce langage : ChatonLM propose.
    testState.config["github.copilot.enable"] = {
      "*": true,
      typescript: false,
    };
    expect(texts(await ask(env))).toEqual(["return a + b;"]);
    // De nouveau allumé, puis « Always suggest with ChatonLM » du menu.
    testState.config["github.copilot.enable"] = { typescript: true };
    expect(texts(await ask(env, { at: [1, 1] }))).toEqual([]);
    testState.pick = (items) =>
      items.find((item) => item.label.includes("Always suggest with ChatonLM"));
    await env.controller.completion.menu();
    expect(testState.config["chatonlm.completion.ignoreOthers"]).toBe(true);
    expect(texts(await ask(env, { at: [1, 1] }))).toEqual(["return a + b;"]);
    expect(viewOf(env)).toMatchObject({
      status: "ready",
      otherProvider: null,
      alsoSuggests: null,
    });
  });

  it("suggests anyway where another extension may suggest, and says so", async () => {
    const env = await enabled();
    // Copilot installé, réglages par défaut (peut-être sans compte) : on ne
    // peut pas savoir s'il propose ; Tabnine n'a pas de réglage connu.
    for (const id of ["GitHub.copilot-chat", "TabNine.tabnine-vscode"]) {
      testState.extensions.clear();
      testState.extensions.set(id, {});
      expect(texts(await ask(env)), id).toEqual(["return a + b;"]);
    }
    testState.extensions.set("GitHub.copilot-chat", {});
    env.controller.completion.postView();
    expect(viewOf(env)).toMatchObject({
      status: "ready",
      alsoSuggests: "GitHub Copilot",
    });
    expect(statusItem()?.tooltip).toContain(
      "GitHub Copilot may also suggest code here.",
    );
    testState.pick = () => undefined;
    await env.controller.completion.menu();
    const always = (
      testState.quickPicks.at(-1) as { label: string; detail?: string }[]
    ).find((item) => item.label.includes("Always suggest with ChatonLM"));
    expect(always?.detail).toBe("GitHub Copilot may also suggest code here.");
    await env.controller.handle({
      type: "completion",
      command: "ignoreOthers",
    });
    expect(testState.config["chatonlm.completion.ignoreOthers"]).toBe(true);
    expect(viewOf(env)?.alsoSuggests).toBeNull();
  });

  it("reads the settings of Continue and Windsurf", async () => {
    const env = await enabled();
    testState.extensions.set("Continue.continue", {});
    // Continue gardé pour sa conversation, sa complétion coupée.
    testState.config["continue.enableTabAutocomplete"] = false;
    expect(texts(await ask(env))).toEqual(["return a + b;"]);
    env.controller.completion.postView();
    expect(viewOf(env)?.alsoSuggests).toBeNull();
    testState.config["continue.enableTabAutocomplete"] = true;
    expect(texts(await ask(env, { at: [1, 1] }))).toEqual([]);
    testState.extensions.clear();
    testState.extensions.set("Codeium.codeium", {});
    testState.config["codeium.enableConfig"] = { "*": true, typescript: false };
    expect(texts(await ask(env, { at: [1, 0] }))).toEqual(["return a + b;"]);
    testState.config["codeium.enableConfig"] = { "*": true };
    expect(texts(await ask(env, { at: [1, 1] }))).toEqual([]);
    env.controller.completion.postView();
    expect(viewOf(env)).toMatchObject({
      status: "otherProvider",
      otherProvider: "Windsurf",
    });
  });
});

describe("one request at a time", () => {
  it("cancels the request when typing goes on", async () => {
    const env = await enabled();
    const answer = deferred<{ text: string }>();
    env.fake.handlers["complete.fim"] = () => answer.promise;
    const typing = new CancellationTokenSource();
    const pending = ask(env, { token: typing });
    await until(() => env.fake.calls("complete.fim").length === 1);
    const id = env.fake.requests.length;
    typing.cancel();
    await until(() => env.fake.cancelled.length === 1);
    answer.resolve({ text: "late" });
    expect(texts(await pending)).toEqual([]);
    expect(env.fake.cancelled[0]).toBeGreaterThan(0);
    expect(id).toBeGreaterThan(0);
  });

  it("cancels the one in flight when a newer one starts", async () => {
    const env = await enabled();
    const first = deferred<{ text: string }>();
    let calls = 0;
    env.fake.handlers["complete.fim"] = () =>
      ++calls === 1 ? first.promise : { text: "second" };
    const one = ask(env, { at: [1, 1] });
    await until(() => calls === 1);
    const two = ask(env, { at: [1, 2] });
    await until(() => calls === 2);
    expect(env.fake.cancelled).toHaveLength(1);
    first.resolve({ text: "first" });
    expect(texts(await two)).toEqual(["second"]);
    await one;
  });

  it("waits for typing to settle before asking", async () => {
    const env = await enabled();
    const one = ask(env, { at: [1, 0] });
    await sleep(DEBOUNCE_MS / 5);
    const two = ask(env, { at: [1, 1] });
    await sleep(DEBOUNCE_MS / 5);
    const three = ask(env, { at: [1, 2] });
    expect(texts(await one)).toEqual([]);
    expect(texts(await two)).toEqual([]);
    expect(texts(await three)).toEqual(["return a + b;"]);
    expect(env.fake.calls("complete.fim")).toHaveLength(1);
  });

  it("shows a kept suggestion again without asking, and follows what is typed", async () => {
    const env = await enabled();
    expect(texts(await ask(env))).toEqual(["return a + b;"]);
    expect(texts(await ask(env))).toEqual(["return a + b;"]);
    // « ret » tapé : le reste de la proposition, sans requête.
    expect(
      texts(
        await ask(env, {
          text: "function add(a, b) {\n  ret\n}\n",
          at: [1, 5],
        }),
      ),
    ).toEqual(["urn a + b;"]);
    expect(env.fake.calls("complete.fim")).toHaveLength(1);
  });
});

describe("status bar, menu and panel", () => {
  it("drives everything from the status bar menu", async () => {
    const env = await setupController();
    testState.pick = (items) =>
      items.find((item) => item.label.includes("Turn on autocomplete"));
    await env.controller.completion.menu();
    expect(testState.config["chatonlm.completion.enabled"]).toBe(true);
    expect(viewOf(env)?.status).toBe("ready");

    testState.pick = (items) =>
      items.find((item) => item.label.includes("Measure speed"));
    await env.controller.completion.menu();
    expect(testState.shown.at(-1)?.text).toBe(
      "Smooth: first word in 120 ms, 85 tokens per second.",
    );
    expect(viewOf(env)?.measure).toEqual({
      state: "done",
      result: {
        rating: "smooth",
        firstTokenMs: 120,
        tokensPerSecond: 85,
        partialGpu: false,
      },
    });

    testState.pick = (items) =>
      items.find((item) => item.label.includes("Turn off autocomplete"));
    await env.controller.completion.menu();
    expect(testState.config["chatonlm.completion.enabled"]).toBe(false);
    expect(statusItem()?.text).toContain("$(circle-slash)");
  });

  it("lists the models with the non-commercial licence of the 3B", async () => {
    const env = await enabled();
    testState.pick = () => undefined;
    await env.controller.completion.chooseModel();
    const items = testState.quickPicks.at(-1) as {
      label: string;
      description?: string;
      detail?: string;
    }[];
    const threeB = items.find((i) => i.label.includes("3b"));
    expect(threeB?.detail).toContain("Non-commercial use only");
    expect(threeB?.description).toContain("not installed");
    const current = items.find((i) => i.label.includes("1.5b"));
    expect(current?.label).toBe(`$(check) ${MODEL}`);
    expect(current?.description).toBe("recommended");
  });

  it("asks the app to offer a model it suggests, and nothing else", async () => {
    const env = await enabled();
    const handle = (m: PanelMessage) => env.controller.handle(m);
    await handle({ type: "offerPull", model: "qwen2.5-coder:7b-base" });
    await handle({ type: "offerPull", model: "llama3:70b" });
    expect(env.fake.offered).toEqual(["qwen2.5-coder:7b-base"]);
  });

  it("never downloads a model itself: a missing one is offered by the app", async () => {
    const env = await enabled();
    testState.choice = "Install in ChatonLM";
    await env.controller.handle({
      type: "completionModel",
      model: "qwen2.5-coder:3b-base",
    });
    expect(testState.config["chatonlm.completion.model"]).toBe(
      "qwen2.5-coder:3b-base",
    );
    expect(viewOf(env)?.status).toBe("missing");
    expect(env.fake.offered).toEqual(["qwen2.5-coder:3b-base"]);
    expect(
      env.fake.calls("completion.prepare").map((p) => p.model),
    ).not.toContain("qwen2.5-coder:3b-base");
    expect(texts(await ask(env))).toEqual([]);
    expect(env.fake.calls("complete.fim")).toEqual([]);
  });

  it("keeps following the recommended model when it is chosen", async () => {
    const env = await enabled();
    await env.controller.handle({
      type: "completionModel",
      model: "qwen2.5-coder:0.5b-base",
    });
    expect(testState.config["chatonlm.completion.model"]).toBe(
      "qwen2.5-coder:0.5b-base",
    );
    expect(env.fake.calls("completion.prepare").at(-1)).toEqual({
      model: "qwen2.5-coder:0.5b-base",
    });
    await env.controller.handle({ type: "completionModel", model: MODEL });
    expect(testState.config["chatonlm.completion.model"]).toBe("auto");
  });

  it("answers the panel: turn on, measure, refresh the card", async () => {
    const env = await setupController();
    await env.controller.handle({ type: "ready" });
    expect(viewOf(env)).toMatchObject({ enabled: false, status: "off" });
    await env.controller.handle({ type: "completion", command: "refresh" });
    expect(env.fake.calls("gpu.status")).toHaveLength(1);
    expect(viewOf(env)?.plan?.mode).toBe("together");
    expect(viewOf(env)?.options.map((o) => o.model)).toContain(
      "qwen2.5-coder:3b-base",
    );
    await env.controller.handle({ type: "completion", command: "enable" });
    expect(viewOf(env)?.status).toBe("ready");
    await env.controller.handle({ type: "completion", command: "measure" });
    expect(viewOf(env)?.measure?.state).toBe("done");
    await env.controller.handle({ type: "completion", command: "disable" });
    expect(viewOf(env)?.enabled).toBe(false);
  });

  it("says why a measure was refused", async () => {
    const env = await setupController();
    testState.config["chatonlm.completion.enabled"] = true;
    await env.controller.completion.refresh();
    await env.controller.completion.measure(false);
    expect(viewOf(env)?.measure).toEqual({
      state: "failed",
      message:
        "Paused: the autocomplete model is not loaded. Prepare it to start.",
    });
  });

  it("shows the graphics card in the panel on request, once", async () => {
    const env = await setupController();
    await env.controller.completion.showCard();
    expect(env.deps.reveal).toHaveBeenCalled();
    expect(ofType(env.posted, "revealGpu")).toHaveLength(1);
    expect(viewOf(env)?.plan).not.toBeNull();
    // Panneau recréé ensuite : l'état seul, pas de nouvelle demande.
    await env.controller.handle({ type: "ready" });
    expect(ofType(env.posted, "revealGpu")).toHaveLength(1);
  });

  it("forgets the plan when the link drops", async () => {
    const env = await enabled();
    env.fake.dropAll();
    await until(() => viewOf(env)?.status === "disconnected");
    expect(viewOf(env)?.plan).toBeNull();
    expect(texts(await ask(env))).toEqual([]);
  });
});

describe("prompt window", () => {
  const doc = (text: string) => new TextDocument("/work/a.ts", text);

  it("cuts the prefix at whole lines, from a stable start", () => {
    const lines = Array.from({ length: 400 }, (_, i) =>
      `line ${i}`.padEnd(39, "."),
    );
    const document = doc(lines.join("\n"));
    const at = (line: number, character: number) =>
      promptWindow(document as never, new Position(line, character) as never);
    const first = at(300, 5);
    expect(first.prefix.length).toBeLessThanOrEqual(PREFIX_CHARS);
    const start = first.prefix.split("\n")[0];
    expect(lines.indexOf(start) % PREFIX_STEP_LINES).toBe(0);
    // Quelques frappes plus loin : même début (cache de préfixe).
    expect(at(300, 9).prefix.split("\n")[0]).toBe(start);
    expect(at(300, 9).prefix.startsWith(first.prefix.slice(0, -5))).toBe(true);
    expect(at(10, 0).prefix.split("\n")[0]).toBe(lines[0]);
    expect(at(300, 5).suffix.length).toBeLessThanOrEqual(SUFFIX_CHARS);
    expect(at(300, 5).suffix.startsWith(lines[300].slice(5))).toBe(true);
  });

  it("keeps the end of a very long line", () => {
    const long = "x".repeat(PREFIX_CHARS * 2);
    const window = promptWindow(
      doc(long) as never,
      new Position(0, long.length) as never,
    );
    expect(window.prefix).toHaveLength(PREFIX_CHARS);
    expect(window.restOfLineEmpty).toBe(true);
  });
});

describe("cache", () => {
  it("is per model and forgets the oldest", () => {
    const cache = new CompletionCache();
    cache.set("a", true, "p", "s", "one");
    expect(cache.get("b", true, "p", "s")).toBeUndefined();
    expect(cache.get("a", true, "p", "s")).toBe("one");
    for (let i = 0; i < 40; i++) cache.set("a", true, `p${i}`, "s", "x");
    expect(cache.get("a", true, "p", "s")).toBeUndefined();
  });

  it("keeps several lines and a single line apart", () => {
    const cache = new CompletionCache();
    cache.set("a", true, "p", "s", "one\ntwo");
    expect(cache.get("a", false, "p", "s")).toBeUndefined();
    expect(cache.get("a", false, "p" + "o", "s")).toBeUndefined();
    expect(cache.get("a", true, "p", "s")).toBe("one\ntwo");
  });

  it("asks again when the multi-line setting changes", async () => {
    const env = await enabled();
    expect(texts(await ask(env))).toEqual(["return a + b;"]);
    testState.config["chatonlm.completion.multiline"] = false;
    await ask(env);
    expect(env.fake.calls("complete.fim").map((c) => c.multiline)).toEqual([
      true,
      false,
    ]);
  });
});

describe("settings changed in the Settings editor", () => {
  it("turning it on there prepares the model, like the toggle", async () => {
    const env = await setupController();
    // Case cochée dans l'éditeur des réglages : seul l'évènement arrive.
    await workspace
      .getConfiguration("chatonlm")
      .update("completion.enabled", true);
    await until(() => viewOf(env)?.status === "ready");
    expect(env.fake.calls("completion.prepare")).toEqual([{ model: MODEL }]);
    expect(texts(await ask(env))).toEqual(["return a + b;"]);
  });

  it("turning it on with the toggle prepares the model once", async () => {
    const env = await enabled();
    await sleep(20);
    expect(env.fake.calls("completion.prepare")).toHaveLength(1);
  });

  it("a model typed there reads its plan again, and is prepared", async () => {
    const env = await enabled();
    await workspace
      .getConfiguration("chatonlm")
      .update("completion.model", "qwen2.5-coder:0.5b-base");
    await until(
      () =>
        env.fake.calls("completion.prepare").at(-1)?.model ===
        "qwen2.5-coder:0.5b-base",
    );
    expect(env.fake.calls("gpu.status").at(-1)).toEqual({
      model: "qwen2.5-coder:0.5b-base",
    });
    await until(() => viewOf(env)?.status === "ready");
    // Un nom pas encore fini (frappe en cours) : rien n'est montré.
    const shown = testState.shown.length;
    await workspace
      .getConfiguration("chatonlm")
      .update("completion.model", "qwen2.5-co");
    await until(
      () => env.fake.calls("gpu.status").at(-1)?.model === "qwen2.5-co",
    );
    await sleep(20);
    expect(testState.shown).toHaveLength(shown);
  });

  it("turning it off there stops the request in flight", async () => {
    const env = await enabled();
    env.fake.handlers["complete.fim"] = () => new Promise(() => {});
    const pending = ask(env);
    await until(() => env.fake.calls("complete.fim").length === 1);
    await workspace
      .getConfiguration("chatonlm")
      .update("completion.enabled", false);
    await until(() => env.fake.cancelled.length === 1);
    void pending;
  });
});

describe("back after a while without typing (decided by the plan)", () => {
  /** Modèle évincé par Ollama (10 min sans frappe) : nouveau plan. */
  async function evicted(env: Env) {
    env.fake.gpu.completion = null;
    env.fake.notify("gpu.changed", { plan: env.fake.plan() });
    await until(() => viewOf(env)?.reason === "notLoaded");
  }

  it("prepares the model again on typing when both fit together", async () => {
    const env = await enabled();
    await evicted(env);
    expect(texts(await ask(env))).toEqual([]);
    await until(() => env.fake.calls("completion.prepare").length === 2);
    await until(() => viewOf(env)?.status === "ready");
    expect(texts(await ask(env, { at: [1, 1] }))).toEqual(["return a + b;"]);
    // Aucun avis : c'est un retour d'inactivité.
    expect(testState.shown).toEqual([]);
  });

  it("asks at most once every 30 s", async () => {
    const env = await enabled();
    await evicted(env);
    await ask(env);
    await until(() => viewOf(env)?.status === "ready");
    await evicted(env);
    for (let i = 0; i < 3; i++) await ask(env, { at: [1, i] });
    await sleep(20);
    expect(env.fake.calls("completion.prepare")).toHaveLength(2);
    const realNow = Date.now;
    Date.now = () => realNow() + WAKE_INTERVAL_MS + 1000;
    try {
      await ask(env, { at: [1, 0] });
    } finally {
      Date.now = realNow;
    }
    await until(() => env.fake.calls("completion.prepare").length === 3);
  });

  it("never when one model at a time, too big, unknown or busy", async () => {
    const cases: [string, (env: Env) => void][] = [
      // L'un après l'autre, conversation pas en mémoire : la raison du plan
      // est « pas chargé », mais seul l'utilisateur prépare.
      [
        "exclusive",
        (env) => {
          env.fake.gpu.mode = "exclusive";
          env.fake.gpu.chatLoaded = false;
        },
      ],
      ["doesNotFit", (env) => void (env.fake.gpu.mode = "doesNotFit")],
      ["unknown", (env) => void (env.fake.gpu.mode = "unknown")],
      ["chatBusy", (env) => void (env.fake.gpu.chatBusy = true)],
    ];
    for (const [name, set] of cases) {
      resetTestState();
      const env = await setupController();
      testState.config["chatonlm.completion.enabled"] = true;
      set(env);
      await env.controller.completion.refresh();
      expect(viewOf(env)?.status, name).toBe("paused");
      for (let i = 0; i < 3; i++) await ask(env, { at: [1, i] });
      await sleep(20);
      expect(env.fake.calls("completion.prepare"), name).toEqual([]);
      cleanUp();
    }
  });
});

describe("where nothing is ever suggested", () => {
  it("says so instead of « Ready »", async () => {
    const env = await enabled();
    openEditor("/work/.env", "TOKEN=\n");
    testState.events.activeEditor.fire(undefined);
    expect(viewOf(env)?.status).toBe("notHere");
    expect(statusItem()?.tooltip).toContain(
      "Autocomplete is not available in this file.",
    );
    // Fenêtre WSL, SSH ou conteneur : fichiers hors de cette machine.
    vscodeEnv.remoteName = "wsl";
    openEditor("vscode-remote://wsl+Ubuntu/home/a.ts", CODE);
    testState.events.activeEditor.fire(undefined);
    expect(viewOf(env)?.status).toBe("remoteWindow");
    openEditor("/work/src/add.ts", CODE);
    testState.events.activeEditor.fire(undefined);
    expect(viewOf(env)?.status).toBe("ready");
  });
});

describe("status bar", () => {
  it("gives its state to screen readers", async () => {
    await enabled();
    expect(statusItem()?.accessibilityInformation?.label).toBe(
      `ChatonLM autocomplete: Ready, with ${MODEL}.`,
    );
  });

  it("offers Prepare only when it can work", async () => {
    const env = await setupController();
    env.fake.gpu.mode = "doesNotFit";
    testState.config["chatonlm.completion.enabled"] = true;
    await env.controller.completion.refresh();
    testState.pick = () => undefined;
    await env.controller.completion.menu();
    const labels = () =>
      (testState.quickPicks.at(-1) as { label: string }[]).map((i) => i.label);
    expect(labels().some((l) => l.includes("Prepare the model"))).toBe(false);
    env.fake.gpu.mode = "together";
    await env.controller.completion.refresh();
    await env.controller.completion.menu();
    expect(labels().some((l) => l.includes("Prepare the model"))).toBe(true);
  });

  it("says when suggestions keep coming too late, and waits longer", async () => {
    const env = await enabled();
    env.fake.fimTimedOut = true;
    for (let i = 0; i < SLOW_AFTER; i++) {
      expect(texts(await ask(env, { at: [1, i] }))).toEqual([]);
    }
    expect(viewOf(env)?.status).toBe("slow");
    expect(statusItem()?.tooltip).toContain("Too slow");
    testState.pick = () => undefined;
    await env.controller.completion.menu();
    expect(
      (testState.quickPicks.at(-1) as { label: string }[]).some((i) =>
        i.label.includes("Measure speed"),
      ),
    ).toBe(true);
    // Toujours demandé, mais après un anti-rebond plus long.
    env.fake.fimTimedOut = false;
    const started = Date.now();
    expect(texts(await ask(env, { at: [1, 0] }))).toEqual(["return a + b;"]);
    expect(Date.now() - started).toBeGreaterThanOrEqual(
      DEBOUNCE_MS * SLOW_DEBOUNCE_FACTOR - 20,
    );
    expect(viewOf(env)?.status).toBe("ready");
  });
});

describe("what is sent", () => {
  it("sends only the file name of a file outside the open folder", async () => {
    const env = await enabled();
    await ask(env, { file: "/home/someone/notes/add.ts" });
    expect(env.fake.calls("complete.fim").at(-1)?.path).toBe("add.ts");
  });

  it("sends the same window from a CRLF document (Windows)", async () => {
    const env = await enabled();
    await ask(env, { text: CODE.replaceAll("\n", "\r\n") });
    const [request] = env.fake.calls("complete.fim");
    expect(request).toMatchObject({
      prefix: "function add(a, b) {\n  ",
      suffix: "\n}\n",
    });
    // Même fenêtre qu'en LF : la proposition gardée resert.
    expect(texts(await ask(env))).toEqual(["return a + b;"]);
    expect(env.fake.calls("complete.fim")).toHaveLength(1);
  });

  it("gives up on a request the app never answers", async () => {
    const env = await enabled();
    env.fake.handlers["complete.fim"] = () => new Promise(() => {});
    const completion = new Completion({
      link: env.link,
      post: () => {},
      reveal: async () => {},
      debounceMs: 10,
      fimTimeoutMs: 100,
    });
    await completion.refresh();
    const editor = openEditor("/work/src/add.ts", CODE);
    const items = await completion.provideInlineCompletionItems(
      editor.document as never,
      new Position(1, 2) as never,
      CONTEXT as never,
      new CancellationTokenSource().token as never,
    );
    expect(items).toEqual([]);
    await until(() => env.fake.cancelled.length === 1);
    completion.dispose();
  });
});

describe("prompt window, UTF-16", () => {
  const doc = (text: string) => new TextDocument("/work/a.ts", text);
  const wellFormed = (text: string) => {
    // encodeURIComponent refuse une moitié de paire seule.
    expect(() => encodeURIComponent(text)).not.toThrow();
  };

  it("never cuts an emoji in half", () => {
    const emoji = "\u{1F600}";
    const before = `${emoji.repeat(2500)}x`;
    const prefix = promptWindow(
      doc(before) as never,
      new Position(0, before.length) as never,
    ).prefix;
    wellFormed(prefix);
    expect(prefix.length).toBe(PREFIX_CHARS - 1);
    const after = `x${emoji.repeat(600)}`;
    const suffix = promptWindow(
      doc(after) as never,
      new Position(0, 0) as never,
    ).suffix;
    wellFormed(suffix);
    expect(suffix.length).toBe(SUFFIX_CHARS - 1);
  });
});
