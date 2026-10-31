// @vitest-environment node
// Serveur MCP côté extension (lot 4b, 2 ; corrections 97) : coupé par
// défaut, jamais activé par un dossier ; déclaration stdio sans secret (ni en
// argument ni en variable d'environnement) ; le fichier privé (0600) ne
// porte jamais les justificatifs de l'éditeur, mais un accès propre aux
// agents demandé à l'app (`agents.issue`), une seule fois pour toutes les
// fenêtres, redemandé après un nouvel appairage ; réglage coupé : accès
// retiré dans l'app (`agents.revoke`, à la prochaine connexion s'il n'y a
// pas de liaison) puis fichier effacé ; désappairage ; fichier de la fenêtre
// (`window-<séance>.json`) ; appels des agents montrés au panneau, arrêt et
// coupure depuis le panneau ; Cursor.
import { promises as fs } from "node:fs";
import path from "node:path";
import {
  afterAll,
  afterEach,
  beforeAll,
  beforeEach,
  describe,
  expect,
  it,
  vi,
} from "vitest";
import type { HostMessage } from "../shared/protocol";
import { Agents, MCP_PROVIDER, REVOKE_PENDING } from "./agents";
import { type Credentials, Link } from "./bridge/link";
import {
  activityFile,
  controlFile,
  CREDENTIALS,
  windowFile,
  writePrivate,
} from "./mcp/files";
import { FakeBridge, until } from "./test/fakeBridge";
import {
  cursor,
  McpStdioServerDefinition,
  resetTestState,
  testState,
  workspace,
} from "./test/vscode";

const scratch = path.resolve(import.meta.dirname, "../../.tools/tmp");
const SESSION = "cd".repeat(16);
let root = "";
beforeAll(async () => {
  await fs.mkdir(scratch, { recursive: true });
  root = await fs.mkdtemp(path.join(scratch, "vscode-agents-"));
});
afterAll(async () => {
  if (root) await fs.rm(root, { recursive: true, force: true });
});
beforeEach(() => resetTestState());
const disposers: (() => void)[] = [];
afterEach(() => {
  for (const dispose of disposers.splice(0)) dispose();
});

type Access = {
  clientId: string;
  secret: string;
  bridgeDir: string | null;
  editor: string;
};

let count = 0;
/**
 * Une fenêtre : liaison de l'éditeur avec le faux pont (pas encore
 * démarrée), et ses agents. `storageDir` et `fake` partagés : une autre
 * fenêtre du même éditeur.
 */
function setup(
  options: {
    paired?: boolean;
    storageDir?: string;
    fake?: FakeBridge;
    session?: string;
  } = {},
) {
  const fake = options.fake ?? new FakeBridge();
  const editor = fake.clients.has("client-1")
    ? null
    : fake.addClient("client-1");
  const state: { credentials: Credentials | null } = {
    credentials: options.paired === false ? null : editor,
  };
  const link = new Link({
    dir: () => "/run",
    readDiscovery: async () => fake.discovery,
    connect: fake.connect,
    secrets: {
      get: async () => state.credentials,
      set: async (c) => void (state.credentials = c),
      delete: async () => void (state.credentials = null),
    },
    client: { name: "t", version: "0", editor: "VS Code" },
    watch: () => null,
    appRunning: async () => false,
  });
  const storageDir = options.storageDir ?? path.join(root, `mcp-${++count}`);
  const posted: HostMessage[] = [];
  const pair = vi.fn(async () => {});
  const agents = new Agents({
    post: (message) => posted.push(message),
    reveal: async () => {},
    credentials: async () => state.credentials,
    link,
    bridgeDir: () => "/run/user/1000/com.chatonlm.chatonlm",
    storageDir,
    script: "/ext/dist/mcp.cjs",
    execPath: "/usr/share/codium/codium",
    version: "0.1.0",
    pair,
    session: options.session ?? SESSION,
  });
  disposers.push(() => {
    agents.dispose();
    link.dispose();
  });
  const file = path.join(storageDir, CREDENTIALS);
  const exists = (name = CREDENTIALS) =>
    fs.stat(path.join(storageDir, name)).then(
      () => true,
      () => false,
    );
  const access = async () =>
    JSON.parse(await fs.readFile(file, "utf8")) as Access;
  /** Liaison et agents démarrés, accès écrit (s'il y a lieu). */
  const startAll = async () => {
    await link.start();
    await agents.start();
    await agents.sync();
  };
  return {
    fake,
    editor,
    link,
    agents,
    posted,
    storageDir,
    file,
    exists,
    access,
    state,
    pair,
    startAll,
  };
}

const enable = (value = true) =>
  workspace.getConfiguration("chatonlm").update("mcp.enabled", value);

describe("MCP server setting", () => {
  it("is off by default: no server, no access asked, no file", async () => {
    const env = setup();
    env.agents.register();
    await env.startAll();
    expect(testState.mcpProviders.map((p) => p.id)).toEqual([MCP_PROVIDER]);
    expect(env.agents.definitions()).toEqual([]);
    expect(await env.exists()).toBe(false);
    expect(env.fake.calls("agents.issue")).toEqual([]);
    expect(env.fake.calls("agents.revoke")).toEqual([]);
  });

  it("can't be turned on by an opened folder", async () => {
    testState.workspaceConfig["chatonlm.mcp.enabled"] = true;
    const env = setup();
    await env.startAll();
    expect(env.agents.enabled).toBe(false);
    expect(env.agents.definitions()).toEqual([]);
    expect(await env.exists()).toBe(false);
  });

  it("declares a stdio server with no secret in its arguments or environment", async () => {
    await enable();
    const env = setup();
    // Accès écrit : il ne passe pas pour autant dans la commande.
    await env.startAll();
    const [definition] = env.agents.definitions();
    expect(definition).toBeInstanceOf(McpStdioServerDefinition);
    expect(definition.command).toBe("/usr/share/codium/codium");
    expect(definition.args).toEqual([
      "/ext/dist/mcp.cjs",
      "--state",
      env.storageDir,
      "--session",
      SESSION,
    ]);
    expect(definition.env).toEqual({ ELECTRON_RUN_AS_NODE: "1" });
    const written = await env.access();
    expect(JSON.stringify(definition)).not.toContain(written.secret);
    expect(JSON.stringify(definition)).not.toContain(written.clientId);
    expect(JSON.stringify(definition)).not.toContain(
      env.state.credentials?.secret,
    );
  });

  it("writes an access of the agents' own, never the editor's credentials", async () => {
    await enable();
    const env = setup();
    await env.startAll();
    expect(env.fake.calls("agents.issue")).toHaveLength(1);
    const stat = await fs.stat(env.file);
    expect(stat.mode & 0o777).toBe(0o600);
    expect((await fs.stat(env.storageDir)).mode & 0o777).toBe(0o700);
    const written = await env.access();
    expect(written).toEqual({
      clientId: "agents-1",
      secret: expect.stringMatching(/^[0-9a-f]{64}$/),
      bridgeDir: "/run/user/1000/com.chatonlm.chatonlm",
      editor: "client-1",
    });
    expect(written.secret).not.toBe(env.state.credentials?.secret);
    expect(env.fake.isAgents(written.clientId)).toBe(true);
  });

  it("keeps the access it has: one for every window and restart", async () => {
    await enable();
    const first = setup();
    await first.startAll();
    const written = await first.access();
    // Autre fenêtre (même stockage), puis relance de la première.
    const second = setup({
      fake: first.fake,
      storageDir: first.storageDir,
      session: "ef".repeat(16),
    });
    second.state.credentials = first.state.credentials;
    await second.startAll();
    await first.agents.sync();
    expect(first.fake.calls("agents.issue")).toHaveLength(1);
    expect(await first.access()).toEqual(written);
  });

  it("asks for one access only when several windows see the setting turned on", async () => {
    const first = setup();
    const others = [1, 2].map((n) => {
      const env = setup({
        fake: first.fake,
        storageDir: first.storageDir,
        session: `${n}`.repeat(32),
      });
      env.state.credentials = first.state.credentials;
      return env;
    });
    const all = [first, ...others];
    for (const env of all) await env.startAll();
    // Toutes les fenêtres reçoivent le changement du réglage en même temps.
    await enable();
    await until(() => all.every((env) => env.agents.enabled));
    await Promise.all(all.map((env) => env.agents.sync()));
    await until(() => first.fake.issued.length > 0);
    await Promise.all(all.map((env) => env.agents.sync()));
    expect(first.fake.issued).toHaveLength(1);
    // L'accès écrit est celui que l'app connaît encore.
    expect(first.fake.clients.has((await first.access()).clientId)).toBe(true);
  });

  it("asks for the access once the link with the app is there", async () => {
    await enable();
    const env = setup();
    await env.agents.start();
    await env.agents.sync();
    expect(await env.exists()).toBe(false);
    await env.link.start();
    await until(() => env.fake.issued.length === 1);
    await env.agents.sync();
    expect((await env.access()).clientId).toBe("agents-1");
  });

  it("asks for a new access after a new pairing", async () => {
    await enable();
    const env = setup();
    await env.startAll();
    expect((await env.access()).editor).toBe("client-1");
    env.state.credentials = env.fake.addClient("client-2");
    await env.link.secretsChanged();
    await until(() => env.fake.issued.length === 2);
    await env.agents.sync();
    const written = await env.access();
    expect(written.editor).toBe("client-2");
    expect(written.clientId).toBe("agents-2");
  });

  it("withdraws the access in the app, then removes the file, when turned off", async () => {
    await enable();
    const env = setup();
    await env.startAll();
    const { clientId } = await env.access();
    await enable(false);
    await until(() => !env.agents.enabled);
    await env.agents.sync();
    expect(await env.exists()).toBe(false);
    expect(env.fake.revoked).toBe(1);
    expect(env.fake.clients.has(clientId)).toBe(false);
    expect(await env.exists(REVOKE_PENDING)).toBe(false);
  });

  it("withdraws it at the next connection when the link was down", async () => {
    await enable();
    const env = setup();
    await env.startAll();
    const { clientId } = await env.access();
    env.fake.refuse = true;
    env.fake.dropAll();
    await until(() => !env.link.connected);
    await enable(false);
    await until(() => !env.agents.enabled);
    await env.agents.sync();
    // Fichier effacé tout de suite ; retrait noté.
    expect(await env.exists()).toBe(false);
    expect(await env.exists(REVOKE_PENDING)).toBe(true);
    expect(env.fake.clients.has(clientId)).toBe(true);
    env.fake.refuse = false;
    await env.link.retryNow();
    await until(() => env.fake.revoked === 1);
    await env.agents.sync();
    expect(env.fake.clients.has(clientId)).toBe(false);
    expect(await env.exists(REVOKE_PENDING)).toBe(false);
  });

  it("does not withdraw a new access when turned back on before the link came back", async () => {
    await enable();
    const env = setup();
    await env.startAll();
    env.fake.refuse = true;
    env.fake.dropAll();
    await until(() => !env.link.connected);
    await enable(false);
    await until(() => !env.agents.enabled);
    await env.agents.sync();
    await enable(true);
    await until(() => env.agents.enabled);
    env.fake.refuse = false;
    await env.link.retryNow();
    await until(() => env.fake.issued.length === 2);
    await env.agents.sync();
    expect(env.fake.calls("agents.revoke")).toEqual([]);
    expect(env.fake.clients.has((await env.access()).clientId)).toBe(true);
    expect(await env.exists(REVOKE_PENDING)).toBe(false);
  });

  it("withdraws the access and removes the file when unpaired", async () => {
    await enable();
    const env = setup();
    await env.startAll();
    const { clientId } = await env.access();
    await env.agents.forget();
    expect(await env.exists()).toBe(false);
    expect(env.fake.clients.has(clientId)).toBe(false);
    // Plus d'appairage : rien n'est réécrit.
    env.state.credentials = null;
    await env.agents.sync();
    expect(await env.exists()).toBe(false);
  });

  it("says so when the app is too old to give agents an access", async () => {
    await enable();
    const env = setup();
    delete env.fake.handlers["agents.issue"];
    await env.startAll();
    expect(await env.exists()).toBe(false);
    expect(testState.shown.at(-1)?.text).toMatch(/Update ChatonLM/);
  });

  it("asks for pairing before starting the server of an unpaired editor", async () => {
    await enable();
    const env = setup({ paired: false });
    await env.startAll();
    const [definition] = env.agents.definitions();
    expect(await env.agents.resolve(definition)).toBeUndefined();
    expect(testState.shown.at(-1)?.text).toMatch(/paired/);
    testState.choice = "Pair";
    env.pair.mockImplementation(async () => {
      env.state.credentials = env.editor;
      await env.link.secretsChanged();
    });
    expect(await env.agents.resolve(definition)).toBe(definition);
    expect(await env.exists()).toBe(true);
  });

  it("registers with Cursor's API when it is there", async () => {
    const registered: unknown[] = [];
    const unregistered: string[] = [];
    cursor.mcp = {
      registerServer: (config) => registered.push(config),
      unregisterServer: (name) => unregistered.push(name),
    };
    const env = setup();
    env.agents.register();
    await env.startAll();
    expect(registered).toEqual([]);
    await enable();
    await until(() => registered.length === 1);
    expect(JSON.stringify(registered[0])).not.toContain(
      env.state.credentials?.secret,
    );
    await enable(false);
    await until(() => unregistered.length === 1);
  });
});

describe("window file", () => {
  it("is written while the window is open, and removed when it closes", async () => {
    const env = setup();
    await env.startAll();
    const name = windowFile(SESSION);
    const file = path.join(env.storageDir, name);
    expect((await fs.stat(file)).mode & 0o777).toBe(0o600);
    expect(JSON.parse(await fs.readFile(file, "utf8"))).toEqual({
      pid: process.pid,
    });
    env.agents.dispose();
    expect(await env.exists(name)).toBe(false);
  });

  it("is removed at start for a window that closed abruptly", async () => {
    const env = setup();
    const dead = windowFile("0f".repeat(16));
    const alive = windowFile("1e".repeat(16));
    await writePrivate(
      path.join(env.storageDir, dead),
      JSON.stringify({ pid: 0x3fffff0 }),
    );
    await writePrivate(
      path.join(env.storageDir, alive),
      JSON.stringify({ pid: process.pid }),
    );
    await env.startAll();
    expect(await env.exists(dead)).toBe(false);
    expect(await env.exists(alive)).toBe(true);
  });
});

describe("agent calls in the panel", () => {
  async function withActivity() {
    await enable();
    const env = setup();
    await env.agents.start();
    const log = path.join(env.storageDir, activityFile(SESSION));
    const append = (record: object) =>
      fs.appendFile(log, `${JSON.stringify(record)}\n`);
    return { ...env, append };
  }

  const views = (posted: HostMessage[]) =>
    posted.flatMap((m) => (m.type === "agents" ? [m.agents] : []));

  it("shows each call, who made it and how it ended", async () => {
    const env = await withActivity();
    await env.append({
      id: "0123456789abcdef",
      tool: "chatonlm_ask",
      agent: "Copilot",
      state: "running",
      at: 1000,
      detail: "Why is the build red?",
    });
    await env.agents.read();
    expect(views(env.posted).at(-1)?.calls).toEqual([
      {
        id: "0123456789abcdef",
        tool: "chatonlm_ask",
        agent: "Copilot",
        detail: "Why is the build red?",
        state: "running",
        startedAt: 1000,
        endedAt: null,
      },
    ]);
    const status = testState.statusItems.find(
      (item) => item.id === "chatonlm.agents",
    );
    expect(status?.visible).toBe(true);
    await env.append({
      id: "0123456789abcdef",
      tool: "chatonlm_ask",
      agent: "Copilot",
      state: "done",
      at: 2000,
      detail: "Why is the build red?",
    });
    // Ligne illisible ou outil hors liste : ignorée.
    await fs.appendFile(
      path.join(env.storageDir, activityFile(SESSION)),
      'garbage\n{"id":"ffffffffffffffff","tool":"rm_rf","state":"done","at":1}\n',
    );
    await env.agents.read();
    const calls = views(env.posted).at(-1)?.calls ?? [];
    expect(calls).toHaveLength(1);
    expect(calls[0].state).toBe("done");
    expect(calls[0].endedAt).toBe(2000);
    expect(status?.visible).toBe(false);
  });

  it("asks the server to stop a running call", async () => {
    const env = await withActivity();
    await env.append({
      id: "0123456789abcdef",
      tool: "chatonlm_generate_image",
      agent: null,
      state: "running",
      at: 1000,
      detail: "cat",
    });
    await env.agents.read();
    await env.agents.command("stop", "0123456789abcdef");
    const control = path.join(env.storageDir, controlFile(SESSION));
    expect(JSON.parse(await fs.readFile(control, "utf8"))).toEqual({
      stop: ["0123456789abcdef"],
    });
    expect((await fs.stat(control)).mode & 0o777).toBe(0o600);
  });

  it("turns the server off for every agent from the panel", async () => {
    const env = await withActivity();
    await env.agents.command("turnOff");
    expect(testState.config["chatonlm.mcp.enabled"]).toBe(false);
    expect(env.agents.definitions()).toEqual([]);
    expect(await env.exists()).toBe(false);
  });
});
