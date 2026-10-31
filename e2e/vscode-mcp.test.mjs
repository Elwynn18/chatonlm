// Serveur MCP de ChatonLM : VSCodium appairé avec `chatonlm.mcp.enabled`
// (réglage de l'utilisateur) demande à l'app un accès propre aux agents
// (`agents.issue`) et l'écrit (0600) dans son stockage, jamais les
// justificatifs de l'éditeur ; sa fenêtre écrit `window-<séance>.json`. Un
// client MCP minimal en Node lance vscode/dist/mcp.cjs avec une séance à lui :
// sans fichier de fenêtre, tout outil est refusé ; ce fichier écrit comme le
// ferait une fenêtre, `initialize`, `tools/list` (les cinq outils),
// `tools/call chatonlm_models` (le modèle local, pas le cloud), journal
// d'activité écrit. Couper le réglage retire l'accès dans l'app
// (`agents.revoke`) puis efface le fichier : l'outil est refusé, et le même
// accès remis sur le disque est refusé par l'app.
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { randomBytes } from "node:crypto";
import {
  existsSync,
  readdirSync,
  readFileSync,
  rmSync,
  statSync,
  writeFileSync,
} from "node:fs";
import { join } from "node:path";
import { createInterface } from "node:readline";
import { ROOT, withApp } from "./lib/app.mjs";
import { findDiscovery } from "./lib/bridge.mjs";
import { startFake } from "./lib/fake.mjs";
import { chooseModel, prepare, waitUntil } from "./lib/vscodium.mjs";

const binary = process.argv[2];

/** Client MCP minimal : une ligne JSON par message. */
function mcpClient(stateDir, session) {
  const child = spawn(
    process.execPath,
    [
      join(ROOT, "vscode/dist/mcp.cjs"),
      "--state",
      stateDir,
      "--session",
      session,
    ],
    { stdio: ["pipe", "pipe", "inherit"], env: { PATH: process.env.PATH } },
  );
  const waiting = new Map();
  createInterface({ input: child.stdout }).on("line", (line) => {
    const message = JSON.parse(line);
    waiting.get(message.id)?.(message);
  });
  let id = 0;
  const request = (method, params) =>
    new Promise((resolve, reject) => {
      const n = ++id;
      const timer = setTimeout(
        () => reject(new Error(`${method}: no answer`)),
        30_000,
      );
      waiting.set(n, (message) => {
        clearTimeout(timer);
        resolve(message);
      });
      child.stdin.write(
        `${JSON.stringify({ jsonrpc: "2.0", id: n, method, params })}\n`,
      );
    });
  return {
    request,
    close: () => child.stdin.end(),
    exited: new Promise((r) => child.on("exit", r)),
  };
}

const fake = await startFake("ollama");
const codium = prepare("vscode-mcp", { "sample.js": "// mcp\n" });
const stateDir = join(
  codium.run,
  "user-data/User/globalStorage/chatonlm.chatonlm/mcp",
);
const credentials = join(stateDir, "credentials.json");
try {
  await withApp(
    binary,
    { name: "vscode-mcp", env: { OLLAMA_HOST: fake.url } },
    async (driver) => {
      await driver.waitFor("[data-testid=new-chat]");
      await chooseModel(driver, "qwen3:0.6b");
      await driver.invoke("bridge_set_enabled", { enabled: true });
      const { pairing } = await driver.invoke("bridge_pair");
      const found = await waitUntil(findDiscovery, "bridge.json");
      const exited = codium.start({
        bridgeDir: found.dir,
        scenario: "mcp",
        env: { CHATONLM_TEST_PAIR_CODE: pairing.code },
        settings: { "chatonlm.mcp.enabled": true },
      });
      let verdict = "ok";
      try {
        await waitUntil(() => codium.step() === "paired", "appairé");
        await waitUntil(() => existsSync(credentials), "justificatifs écrits");
        assert.equal(statSync(credentials).mode & 0o777, 0o600);
        // Un accès propre aux agents, donné à l'éditeur appairé : jamais
        // les justificatifs de l'éditeur lui-même.
        const access = readFileSync(credentials, "utf8");
        const parsed = JSON.parse(access);
        assert.match(parsed.secret, /^[0-9a-f]{64}$/);
        assert.equal(typeof parsed.editor, "string");
        assert.notEqual(parsed.clientId, parsed.editor);
        // La fenêtre de VSCodium montre les appels de sa séance.
        assert.ok(
          readdirSync(stateDir).some((f) =>
            /^window-[0-9a-f]{32}\.json$/.test(f),
          ),
          "fichier de la fenêtre",
        );
        const session = randomBytes(16).toString("hex");
        const client = mcpClient(stateDir, session);
        const init = await client.request("initialize", {
          protocolVersion: "2025-06-18",
          capabilities: {},
          clientInfo: { name: "e2e-agent", version: "1" },
        });
        assert.equal(init.result.serverInfo.name, "chatonlm");
        // Séance qu'aucune fenêtre ne montre : refusée, rien demandé à l'app.
        const unseen = await client.request("tools/call", {
          name: "chatonlm_models",
          arguments: {},
        });
        assert.equal(unseen.result.isError, true);
        assert.match(unseen.result.content[0].text, /No VS Code window/);
        // Fichier de fenêtre écrit comme le ferait une fenêtre (processus
        // vivant : celui de l'essai).
        const windowFile = join(stateDir, `window-${session}.json`);
        writeFileSync(windowFile, JSON.stringify({ pid: process.pid }), {
          mode: 0o600,
        });
        const list = await client.request("tools/list");
        assert.deepEqual(
          list.result.tools.map((t) => t.name),
          [
            "chatonlm_ask",
            "chatonlm_generate_image",
            "chatonlm_speak",
            "chatonlm_proofread",
            "chatonlm_models",
          ],
        );
        const models = await client.request("tools/call", {
          name: "chatonlm_models",
          arguments: {},
        });
        assert.ok(!models.result.isError, JSON.stringify(models));
        const listed = JSON.parse(models.result.content[0].text);
        assert.ok(listed.models.some((m) => m.name === "qwen3:0.6b"));
        assert.ok(listed.models.every((m) => !/cloud$/.test(m.name)));
        const log = readdirSync(stateDir).find(
          (f) => f === `activity-${session}.jsonl`,
        );
        assert.ok(log, "journal d'activité");
        assert.match(
          readFileSync(join(stateDir, log), "utf8"),
          /"state":"done"/,
        );
        // Coupure du réglage : justificatifs effacés, outil refusé.
        codium.ask("off");
        await waitUntil(() => codium.step() === "off", "réglage coupé");
        await waitUntil(
          () => !existsSync(credentials),
          "justificatifs effacés",
        );
        const refused = await client.request("tools/call", {
          name: "chatonlm_models",
          arguments: {},
        });
        assert.equal(refused.result.isError, true);
        assert.match(refused.result.content[0].text, /turned off/);
        // Le même accès remis sur le disque : l'app l'a retiré.
        writeFileSync(credentials, access, { mode: 0o600 });
        const revoked = await client.request("tools/call", {
          name: "chatonlm_models",
          arguments: {},
        });
        assert.equal(revoked.result.isError, true);
        assert.match(
          revoked.result.content[0].text,
          /^ChatonLM access for agents was turned off/,
        );
        rmSync(credentials, { force: true });
        rmSync(windowFile, { force: true });
        client.close();
        await client.exited;
      } catch (error) {
        verdict = String(error?.stack ?? error);
      }
      codium.done(verdict);
      const code = await exited();
      assert.equal(verdict, "ok");
      assert.equal(code, 0);
    },
  );
  console.log("✓ vscode-mcp");
} finally {
  await fake.stop();
  codium.finish();
}
