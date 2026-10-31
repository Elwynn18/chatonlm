// @vitest-environment node
// Serveur MCP de ChatonLM (lot 4b, 2 ; corrections 97) contre le faux pont,
// avec un accès agents (limité par le faux pont comme par l'app) : protocole
// (initialize, tools/list, erreurs), liste blanche des cinq outils et leurs
// `readOnlyHint`, entrées bornées (champs hérités refusés), justificatifs lus
// seulement dans le fichier privé de l'extension (effacement vérifié),
// séance sans fenêtre refusée, accès retiré, modèles du cloud et Ollama
// d'une autre machine refusés, correction `localOnly`, conversation
// éphémère effacée, journal des appels (sans caractères bidirectionnels) et
// arrêts (agent, panneau, réglage coupé, même au repos).
import { promises as fs } from "node:fs";
import path from "node:path";
import { PassThrough } from "node:stream";
import { afterAll, afterEach, beforeAll, describe, expect, it } from "vitest";
import { deferred, FakeBridge, RpcFailure, until } from "../test/fakeBridge";
import {
  activityFile,
  controlFile,
  CREDENTIALS,
  eraseCredentials,
  oneLine,
  parseActivity,
  TOOL_NAMES,
  windowFile,
  writePrivate,
} from "./files";
import {
  NO_WINDOW_TEXT,
  OFF_TEXT,
  parseArgs,
  REVOKED_TEXT,
  runMcp,
  unreachableText,
} from "./run";
import {
  type BridgeLike,
  isCloud,
  LIMITS,
  NOT_LOCAL_TEXT,
  toolList,
  validate,
} from "./tools";

const scratch = path.resolve(import.meta.dirname, "../../../.tools/tmp");
const SESSION = "ab".repeat(16);
let root = "";
beforeAll(async () => {
  await fs.mkdir(scratch, { recursive: true });
  root = await fs.mkdtemp(path.join(scratch, "vscode-mcp-"));
});
afterAll(async () => {
  if (root) await fs.rm(root, { recursive: true, force: true });
});

type Line = {
  id?: unknown;
  result?: {
    content?: { type: string; text?: string; data?: string }[];
    isError?: boolean;
    tools?: { name: string }[];
    protocolVersion?: string;
  };
  error?: { code: number; message: string };
};

const stops: (() => void)[] = [];
afterEach(() => {
  for (const stop of stops.splice(0)) stop();
});

let count = 0;
async function start(
  options: {
    credentials?: "private" | "loose" | "none";
    /** Fenêtre de VS Code de la séance : ouverte (défaut), absente, ou
     * fichier resté d'une fenêtre fermée brutalement. */
    window?: "open" | "none" | "dead";
    appRunning?: boolean | null;
    discovery?: boolean;
  } = {},
) {
  const fake = new FakeBridge();
  const stateDir = path.join(root, `state-${++count}`);
  await fs.mkdir(stateDir, { recursive: true });
  // Éditeur appairé, et l'accès propre aux agents que l'app lui a donné :
  // c'est lui que l'extension écrit pour le serveur.
  const editor = fake.addClient();
  const credentials = fake.addAgents(editor.clientId);
  const file = path.join(stateDir, CREDENTIALS);
  const mode = options.credentials ?? "private";
  if (mode !== "none") {
    await writePrivate(
      file,
      JSON.stringify({
        ...credentials,
        bridgeDir: "/run/fake",
        editor: editor.clientId,
      }),
    );
    if (mode === "loose") await fs.chmod(file, 0o644);
  }
  const windowMode = options.window ?? "open";
  if (windowMode !== "none") {
    await writePrivate(
      path.join(stateDir, windowFile(SESSION)),
      JSON.stringify({ pid: windowMode === "open" ? process.pid : 0x3fffff0 }),
    );
  }
  const input = new PassThrough();
  const lines: Line[] = [];
  const mcp = runMcp(
    {
      version: "1.0.0",
      platform: "linux",
      env: {},
      input,
      write: (line) => lines.push(JSON.parse(line) as Line),
      connect: fake.connect,
      readDiscovery: async () =>
        options.discovery === false ? null : fake.discovery,
      appRunning: async () => options.appRunning ?? false,
    },
    { stateDir, session: SESSION },
  );
  stops.push(() => mcp.stop());
  let next = 100;
  const send = (message: unknown) =>
    input.write(`${JSON.stringify(message)}\n`);
  const request = (method: string, params?: unknown) => {
    const id = next++;
    send({ jsonrpc: "2.0", id, method, params });
    return {
      id,
      reply: async () => {
        await until(() => lines.some((l) => l.id === id), 4000);
        return lines.find((l) => l.id === id) as Line;
      },
    };
  };
  const call = (name: string, args: unknown = {}) =>
    request("tools/call", { name, arguments: args });
  const activity = async () => {
    await mcp.flushed();
    const text = await fs
      .readFile(path.join(stateDir, activityFile(SESSION)), "utf8")
      .catch(() => "");
    return text
      .split("\n")
      .map(parseActivity)
      .filter((r) => r !== null);
  };
  return {
    fake,
    editor,
    credentials,
    stateDir,
    file,
    input,
    lines,
    mcp,
    send,
    request,
    call,
    activity,
  };
}

const textOf = (line: Line) =>
  line.result?.content?.map((c) => c.text ?? "").join("") ?? "";

describe("MCP protocol", () => {
  it("answers initialize and tools/list without any link to the app", async () => {
    const env = await start({ credentials: "none" });
    const init = await env
      .request("initialize", {
        protocolVersion: "2025-06-18",
        capabilities: {},
        clientInfo: { name: "Copilot", version: "1" },
      })
      .reply();
    expect(init.result?.protocolVersion).toBe("2025-06-18");
    const list = await env.request("tools/list").reply();
    expect(list.result?.tools?.map((t) => t.name)).toEqual([...TOOL_NAMES]);
    expect(env.fake.sessions).toHaveLength(0);
  });

  it("offers exactly the five tools of the contract, with bounded inputs", () => {
    expect(toolList().map((t) => t.name)).toEqual([
      "chatonlm_ask",
      "chatonlm_generate_image",
      "chatonlm_speak",
      "chatonlm_proofread",
      "chatonlm_models",
    ]);
    for (const tool of toolList()) {
      expect(tool.inputSchema.additionalProperties).toBe(false);
      for (const property of Object.values(tool.inputSchema.properties)) {
        const p = property as { type: string; maxLength?: number };
        if (p.type === "string") expect(p.maxLength).toBeGreaterThan(0);
      }
    }
  });

  it("marks as read-only only the tools with no effect the user would want to confirm", () => {
    // VS Code ne demande pas de confirmation pour un outil en lecture seule :
    // voix à haute voix, carte graphique, modèle chargé la demandent.
    expect(
      Object.fromEntries(
        toolList().map((tool) => [tool.name, tool.annotations.readOnlyHint]),
      ),
    ).toEqual({
      chatonlm_ask: false,
      chatonlm_generate_image: false,
      chatonlm_speak: false,
      chatonlm_proofread: true,
      chatonlm_models: true,
    });
    for (const tool of toolList()) {
      expect(tool.annotations.destructiveHint).toBe(false);
      expect(tool.annotations.openWorldHint).toBe(false);
    }
  });

  it("refuses arguments inherited from Object.prototype", () => {
    for (const key of [
      "toString",
      "constructor",
      "__proto__",
      "hasOwnProperty",
    ]) {
      expect(() =>
        validate("chatonlm_ask", JSON.parse(`{"question":"hi","${key}":5}`)),
      ).toThrow(`Unknown argument: ${key}`);
    }
    expect(() => validate("chatonlm_models", { constructor: "x" })).toThrow(
      "Unknown argument: constructor",
    );
  });

  it("refuses a tool outside the list, and unknown methods, without asking the app", async () => {
    const env = await start();
    const other = await env.call("chatonlm_read_history").reply();
    expect(other.error?.code).toBe(-32602);
    const method = await env.request("resources/list").reply();
    expect(method.error?.code).toBe(-32601);
    expect(env.fake.requests.filter((r) => r.method !== "initialize")).toEqual(
      [],
    );
    expect(await env.activity()).toEqual([]);
  });

  it("refuses unknown arguments and inputs out of bounds", async () => {
    const env = await start();
    const cases: [string, unknown][] = [
      ["chatonlm_ask", { question: "hi", history: true }],
      ["chatonlm_ask", { question: "x".repeat(LIMITS.question + 1) }],
      ["chatonlm_ask", {}],
      ["chatonlm_generate_image", { prompt: "cat", width: 300 }],
      ["chatonlm_generate_image", { prompt: "cat", height: 4096 }],
      ["chatonlm_speak", { text: "x".repeat(LIMITS.speech + 1) }],
      ["chatonlm_speak", { text: "hi", language: "de" }],
      ["chatonlm_proofread", { text: "hi", mode: "translate" }],
      ["chatonlm_models", { all: true }],
    ];
    for (const [name, args] of cases) {
      const reply = await env.call(name, args).reply();
      expect(reply.result?.isError, `${name} ${JSON.stringify(args)}`).toBe(
        true,
      );
    }
    expect(env.fake.sessions).toHaveLength(0);
  });

  it("rejects lines that are not JSON-RPC", async () => {
    const env = await start({ credentials: "none" });
    env.input.write("not json\n");
    await until(() => env.lines.some((l) => l.error?.code === -32700));
  });
});

describe("credentials", () => {
  it("reads them only from the extension's private file", async () => {
    const env = await start();
    const reply = await env.call("chatonlm_models").reply();
    expect(reply.result?.isError).toBeFalsy();
    expect(env.fake.authenticated).toHaveLength(1);
  });

  it("stays off without the file, and with a file others can read", async () => {
    for (const credentials of ["none", "loose"] as const) {
      const env = await start({ credentials });
      const reply = await env.call("chatonlm_models").reply();
      expect(reply.result?.isError).toBe(true);
      expect(textOf(reply)).toBe(OFF_TEXT);
      expect(env.fake.sessions).toHaveLength(0);
    }
  });

  it("refuses every tool when no VS Code window shows this session", async () => {
    for (const window of ["none", "dead"] as const) {
      const env = await start({ window });
      const reply = await env.call("chatonlm_models").reply();
      expect(reply.result?.isError, window).toBe(true);
      expect(textOf(reply)).toBe(NO_WINDOW_TEXT);
      // Rien n'est demandé à l'app.
      expect(env.fake.sessions).toHaveLength(0);
    }
  });

  it("stops its calls and leaves the app when the window closes", async () => {
    const env = await start();
    await env.call("chatonlm_models").reply();
    expect(env.fake.authenticated).toHaveLength(1);
    await fs.rm(path.join(env.stateDir, windowFile(SESSION)));
    await until(() => env.fake.authenticated.length === 0, 8000);
    const reply = await env.call("chatonlm_models").reply();
    expect(textOf(reply)).toBe(NO_WINDOW_TEXT);
  });

  it("tells the agent when its access was withdrawn in the app", async () => {
    const env = await start();
    expect((await env.call("chatonlm_models").reply()).result?.isError).toBe(
      undefined,
    );
    // Réglage coupé dans une fenêtre dont le fichier n'a pas encore disparu,
    // ou éditeur retiré dans l'app : l'app ne connaît plus l'accès.
    env.fake.removeClient(env.editor.clientId);
    await until(() => env.fake.authenticated.length === 0);
    const reply = await env.call("chatonlm_models").reply();
    expect(reply.result?.isError).toBe(true);
    expect(textOf(reply)).toBe(REVOKED_TEXT);
    expect(REVOKED_TEXT).toMatch(/^ChatonLM access for agents was turned off/);
  });

  it("drops its link once the credentials are gone, even with no call running", async () => {
    const env = await start();
    await env.call("chatonlm_models").reply();
    expect(env.fake.authenticated).toHaveLength(1);
    await fs.rm(env.file);
    await until(() => env.fake.authenticated.length === 0, 8000);
  });

  it("tells a link turned off in the app from a closed app", async () => {
    const off = await start({ discovery: false, appRunning: true });
    expect(textOf(await off.call("chatonlm_models").reply())).toBe(
      unreachableText({ connection: "bridgeOff" }),
    );
    const closed = await start({ discovery: false, appRunning: false });
    expect(textOf(await closed.call("chatonlm_models").reply())).toBe(
      unreachableText({ connection: "appClosed" }),
    );
  });

  it("checks that the credentials are really gone once erased", async () => {
    const file = path.join(root, "erase-credentials.json");
    const content = JSON.stringify({ clientId: "agents-1", secret: "ab" });
    let disk: string | null = content;
    const ops = {
      // Fichier tenu par un autre programme (Windows) : pas supprimé.
      rm: async () => {
        throw Object.assign(new Error("busy"), { code: "EBUSY" });
      },
      overwrite: async (_file: string, text: string) => void (disk = text),
      read: async () => disk,
    };
    await eraseCredentials(file, ops);
    expect(disk).toBe("{}");
    // Ni supprimé ni réécrit : erreur (montrée par l'extension).
    disk = content;
    await expect(
      eraseCredentials(file, {
        ...ops,
        overwrite: async () => {
          throw new Error("busy");
        },
      }),
    ).rejects.toThrow();
    // Cas ordinaire, sur le disque.
    await writePrivate(file, content);
    await eraseCredentials(file);
    await expect(fs.stat(file)).rejects.toThrow();
  });

  it("takes no secret from its arguments", () => {
    expect(
      parseArgs([
        "--state",
        "/state",
        "--session",
        SESSION,
        "--secret",
        "00ff",
        "--client-id",
        "client-1",
      ]),
    ).toEqual({ stateDir: "/state", session: SESSION });
    expect(parseArgs(["--state", "relative", "--session", SESSION])).toBeNull();
    expect(parseArgs(["--state", "/state", "--session", "x"])).toBeNull();
  });
});

describe("tools", () => {
  it("lists only the models that run on this computer", async () => {
    const env = await start();
    const reply = await env.call("chatonlm_models").reply();
    const listed = JSON.parse(textOf(reply)) as {
      default: string | null;
      models: { name: string }[];
    };
    expect(listed.models.map((m) => m.name)).toEqual(["qwen3:0.6b"]);
    expect(listed.default).toBe("qwen3:0.6b");
  });

  it("asks a local model in an ephemeral chat, then forgets it", async () => {
    const env = await start();
    env.fake.handlers["chat.send"] = (_params, { id, notify }) => {
      notify("chat.event", {
        requestId: id,
        event: { type: "chatCreated", chatId: "chat-9" },
      });
      notify("chat.event", {
        requestId: id,
        event: { type: "content", text: "Forty" },
      });
      notify("chat.event", {
        requestId: id,
        event: { type: "content", text: "-two." },
      });
      return { chatId: "chat-9", outcome: "done" };
    };
    const reply = await env
      .call("chatonlm_ask", { question: "Meaning of life?" })
      .reply();
    expect(textOf(reply)).toBe("Forty-two.");
    expect(env.fake.calls("chat.send")).toEqual([
      { content: "Meaning of life?", model: "qwen3:0.6b", ephemeral: true },
    ]);
    await until(() => env.fake.forgotten.includes("chat-9"));
    const records = await env.activity();
    expect(records.map((r) => r.state)).toEqual(["running", "done"]);
    expect(records[0].tool).toBe("chatonlm_ask");
    expect(records[0].detail).toBe("Meaning of life?");
  });

  it("refuses a cloud model, by name or as the app reports it", async () => {
    const env = await start();
    env.fake.handlers["chat.send"] = () => ({ chatId: null, outcome: "done" });
    const byName = await env
      .call("chatonlm_ask", { question: "hi", model: "gpt-oss:120b-cloud" })
      .reply();
    expect(byName.result?.isError).toBe(true);
    env.fake.handlers["models.details"] = (params) => ({
      model: params.model,
      remote: true,
    });
    const reported = await env
      .call("chatonlm_ask", { question: "hi", model: "remote-model" })
      .reply();
    expect(reported.result?.isError).toBe(true);
    expect(textOf(reported)).toMatch(/runs in the cloud/);
    expect(env.fake.calls("chat.send")).toEqual([]);
  });

  it("refuses an Ollama on another computer, for questions and the model list", async () => {
    const env = await start();
    env.fake.local = false;
    env.fake.handlers["chat.send"] = () => ({ chatId: null, outcome: "done" });
    for (const args of [
      { question: "hi" },
      { question: "hi", model: "qwen3:0.6b" },
    ]) {
      const reply = await env.call("chatonlm_ask", args).reply();
      expect(reply.result?.isError).toBe(true);
      expect(textOf(reply)).toBe(NOT_LOCAL_TEXT);
    }
    const models = await env.call("chatonlm_models").reply();
    expect(models.result?.isError).toBe(true);
    expect(textOf(models)).toBe(NOT_LOCAL_TEXT);
    expect(env.fake.calls("chat.send")).toEqual([]);
    expect(env.fake.calls("models.list")).toEqual([]);
  });

  it("counts every model as off this computer when the app's Ollama is elsewhere", async () => {
    const stub = (local: unknown): BridgeLike => ({
      call: <T>(method: string) => ({
        id: 1,
        result: Promise.resolve(
          (method === "app.state" ? { local } : { remote: false }) as T,
        ),
      }),
      cancel: () => {},
      onNotification: () => () => {},
    });
    expect(await isCloud(stub(true), "qwen3:0.6b")).toBe(false);
    expect(await isCloud(stub(false), "qwen3:0.6b")).toBe(true);
    // Ce que l'app ne dit pas : une autre machine.
    expect(await isCloud(stub(undefined), "qwen3:0.6b")).toBe(true);
  });

  it("passes on a refusal of agents by the app", async () => {
    const env = await start();
    env.fake.handlers["image.generate"] = () => {
      throw new RpcFailure(-32037, "not allowed");
    };
    const reply = await env
      .call("chatonlm_generate_image", { prompt: "cat" })
      .reply();
    expect(textOf(reply)).toBe("ChatonLM does not let agents do this.");
  });

  it("returns a generated image as MCP image content", async () => {
    const env = await start();
    env.fake.handlers["image.generate"] = () => ({
      pngBase64: "iVBORw0KGgoAAAA=",
      seed: 7,
      model: "sd-turbo",
      width: 512,
      height: 512,
    });
    const reply = await env
      .call("chatonlm_generate_image", { prompt: "A red kitten", width: 512 })
      .reply();
    expect(reply.result?.content?.[0]).toEqual({
      type: "image",
      data: "iVBORw0KGgoAAAA=",
      mimeType: "image/png",
    });
    expect(env.fake.calls("image.generate")).toEqual([
      { prompt: "A red kitten", width: 512 },
    ]);
  });

  it("reads aloud and answers once the text has been read", async () => {
    const env = await start();
    env.fake.handlers["speech.read"] = (_params, { notify }) => {
      setTimeout(() => {
        notify("speech.event", {
          readId: "r1",
          type: "started",
          segmentId: "agent",
        });
        notify("speech.event", {
          readId: "r1",
          type: "finished",
          segmentId: "agent",
        });
      }, 20);
      return { readId: "r1" };
    };
    const reply = await env
      .call("chatonlm_speak", { text: "Build passed.", language: "en" })
      .reply();
    expect(reply.result?.isError).toBeFalsy();
    expect(env.fake.calls("speech.read")).toEqual([
      { segments: [{ id: "agent", text: "Build passed." }], language: "en" },
    ]);
  });

  it("proofreads a text", async () => {
    const env = await start();
    env.fake.handlers["proofread.run"] = (params) => ({
      text: String(params.text).replace("teh", "the"),
      model: "qwen3:0.6b",
    });
    const reply = await env
      .call("chatonlm_proofread", { text: "teh cat" })
      .reply();
    expect(textOf(reply)).toBe("the cat");
    // Seulement sur cette machine : l'app refuse un modèle du cloud.
    expect(env.fake.calls("proofread.run")).toEqual([
      { text: "teh cat", mode: "correct", localOnly: true },
    ]);
  });

  it("passes on the app's refusal of a proofreading model off this computer", async () => {
    const env = await start();
    env.fake.handlers["proofread.run"] = (params) => {
      if (params.localOnly === true) {
        throw new RpcFailure(-32020, "refused", {
          code: "notLocal",
          message: "The proofreading model doesn't run on this computer.",
        });
      }
      return { text: "sent to the cloud" };
    };
    const reply = await env
      .call("chatonlm_proofread", { text: "teh cat" })
      .reply();
    expect(reply.result?.isError).toBe(true);
    expect(textOf(reply)).toBe(
      "The proofreading model doesn't run on this computer.",
    );
  });

  it("removes bidirectional marks from what it shows in the panel", async () => {
    expect(oneLine("x\u202eevil\u2066y\u200f", 80)).toBe("xevily");
    const env = await start();
    await env
      .request("initialize", {
        protocolVersion: "2025-06-18",
        capabilities: {},
        clientInfo: { name: "Agent\u202elivE" },
      })
      .reply();
    env.fake.handlers["proofread.run"] = () => ({ text: "ok" });
    await env.call("chatonlm_proofread", { text: "a\u2067b" }).reply();
    const [record] = await env.activity();
    expect(record.agent).toBe("AgentlivE");
    expect(record.detail).toBe("ab");
  });

  it("passes the app's refusal on to the agent", async () => {
    const env = await start();
    env.fake.handlers["image.generate"] = () => {
      throw new RpcFailure(-32036, "no image", {
        reason: "noModel",
        message: "Choose an image model in ChatonLM first.",
      });
    };
    const reply = await env
      .call("chatonlm_generate_image", { prompt: "cat" })
      .reply();
    expect(reply.result?.isError).toBe(true);
    expect(textOf(reply)).toBe("Choose an image model in ChatonLM first.");
    expect((await env.activity()).at(-1)?.state).toBe("failed");
  });
});

describe("stopping a call", () => {
  /** `proofread.run` qui ne répond qu'à son annulation. */
  async function pending() {
    const env = await start();
    const asked = deferred<number>();
    const release = deferred();
    env.fake.handlers["proofread.run"] = async (_params, { id }) => {
      asked.resolve(id);
      await release.promise;
      throw new RpcFailure(-32800, "cancelled");
    };
    const call = env.call("chatonlm_proofread", { text: "teh cat" });
    const requestId = await asked.promise;
    const running = (await env.activity()).find((r) => r.state === "running");
    expect(running).toBeDefined();
    return { env, call, requestId, release, callId: running?.id ?? "" };
  }

  it("stops when the panel asks for it (control file)", async () => {
    const { env, call, requestId, release, callId } = await pending();
    await writePrivate(
      path.join(env.stateDir, controlFile(SESSION)),
      JSON.stringify({ stop: [callId] }),
    );
    await until(() => env.fake.cancelled.includes(requestId), 4000);
    release.resolve();
    const reply = await call.reply();
    expect(reply.result?.isError).toBe(true);
    expect(textOf(reply)).toMatch(/Stopped/);
    expect((await env.activity()).at(-1)?.state).toBe("stopped");
  });

  it("stops when the setting is turned off (credentials removed)", async () => {
    const { env, call, release } = await pending();
    await fs.rm(env.file);
    await until(() => env.fake.authenticated.length === 0, 4000);
    // Arrêté avant toute réponse de l'app (le faux `proofread.run` ne
    // répond qu'une fois relâché).
    const reply = await call.reply();
    expect(textOf(reply)).toMatch(/Stopped/);
    expect((await env.activity()).at(-1)?.state).toBe("stopped");
    release.resolve();
  });

  it("stops when the agent cancels the request", async () => {
    const { env, call, requestId, release } = await pending();
    env.send({
      jsonrpc: "2.0",
      method: "notifications/cancelled",
      params: { requestId: call.id },
    });
    await until(() => env.fake.cancelled.includes(requestId));
    release.resolve();
    expect((await call.reply()).result?.isError).toBe(true);
  });
});
