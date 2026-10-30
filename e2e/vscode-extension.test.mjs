// L'extension dans un vrai éditeur (VSCodium de .tools/vscodium, essai
// d'extension officiel : --extensionDevelopmentPath, --extensionTestsPath)
// face à l'app réelle (build e2e) et au faux Ollama :
// - l'app ouvre le pont et montre un code ; la commande « ChatonLM: Pair »
//   l'emploie, l'éditeur apparaît connecté dans l'app ;
// - « Explain » sur la sélection d'un fichier du dossier ouvert : la
//   conversation arrive dans l'app, marquée « VS Code », avec la réponse du
//   modèle ;
// - VSCodium se ferme sans erreur (l'essai intérieur rend son verdict).
// Prérequis : builds de l'extension (vscode/dist) ; VSCodium téléchargé par
// scripts/fetch-vscodium.sh.
import assert from "node:assert/strict";
import { execFileSync, spawn } from "node:child_process";
import {
  cpSync,
  existsSync,
  mkdirSync,
  openSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { withApp } from "./lib/app.mjs";
import { findDiscovery } from "./lib/bridge.mjs";
import { startFake } from "./lib/fake.mjs";

const ROOT = fileURLToPath(new URL("..", import.meta.url));
// Version épinglée (packaging/vscodium.env), téléchargée au besoin.
const CODIUM = execFileSync(join(ROOT, "scripts/fetch-vscodium.sh"))
  .toString()
  .trim();
const binary = process.argv[2];
const MODEL = "qwen3:0.6b";

for (const needed of [
  CODIUM,
  join(ROOT, "vscode/dist/extension.cjs"),
  join(ROOT, "vscode/dist/webview/index.js"),
]) {
  assert.ok(existsSync(needed), `missing ${needed}`);
}

const delay = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

/**
 * Environnement de la session d'essai sans ce que le VS Code du développeur
 * transmet (ELECTRON_RUN_AS_NODE ferait de VSCodium un simple Node ;
 * VSCODE_IPC_HOOK* et compagnie le relieraient à l'éditeur réel).
 */
function isolatedEnv() {
  const env = { ...process.env };
  for (const key of Object.keys(env)) {
    if (/^(ELECTRON_|VSCODE_|CHROME_|GIT_ASKPASS|TERM_PROGRAM)/.test(key)) {
      delete env[key];
    }
  }
  return env;
}

async function waitUntil(check, what, timeoutMs = 60000) {
  const deadline = Date.now() + timeoutMs;
  for (;;) {
    const value = await check();
    if (value) return value;
    if (Date.now() > deadline) throw new Error(`Délai dépassé : ${what}`);
    await delay(250);
  }
}

const fake = await startFake("ollama");
const run = join(ROOT, ".tools/e2e-runs", `vscode-${process.pid}`);
rmSync(run, { recursive: true, force: true });
const workspace = join(run, "workspace");
const userData = join(run, "user-data");
mkdirSync(workspace, { recursive: true });
mkdirSync(join(userData, "User"), { recursive: true });
writeFileSync(
  join(workspace, "sample.js"),
  "function add(a, b) {\n  return a + b;\n}\n",
);
const doneFile = join(run, "verdict.txt");

try {
  await withApp(
    binary,
    { name: "vscode-extension", env: { OLLAMA_HOST: fake.url } },
    async (driver) => {
      await driver.waitFor("[data-testid=new-chat]");
      await driver.invoke("update_settings", {
        patch: { selectedModel: MODEL },
      });
      await driver.invoke("bridge_set_enabled", { enabled: true });
      const { pairing } = await driver.invoke("bridge_pair");
      const found = await waitUntil(findDiscovery, "bridge.json");
      writeFileSync(
        join(userData, "User", "settings.json"),
        JSON.stringify({
          "chatonlm.bridgeDir": found.dir,
          "security.workspace.trust.enabled": false,
          "telemetry.telemetryLevel": "off",
          "update.mode": "none",
          "extensions.autoCheckUpdates": false,
        }),
      );

      const log = openSync(join(ROOT, ".tools/logs/e2e-vscodium.log"), "w");
      const codium = spawn(
        CODIUM,
        [
          `--extensionDevelopmentPath=${join(ROOT, "vscode")}`,
          `--extensionTestsPath=${join(ROOT, "e2e/vscode/extension-tests.cjs")}`,
          `--user-data-dir=${userData}`,
          `--extensions-dir=${join(run, "extensions")}`,
          `--logsPath=${join(run, "logs")}`,
          "--password-store=basic",
          "--no-sandbox",
          "--disable-gpu",
          "--skip-welcome",
          "--skip-release-notes",
          workspace,
        ],
        {
          env: {
            ...isolatedEnv(),
            CHATONLM_TEST_PAIR_CODE: pairing.code,
            CHATONLM_TEST_DONE_FILE: doneFile,
          },
          stdio: ["ignore", log, log],
        },
      );
      const exited = new Promise((resolve) => codium.on("exit", resolve));

      let verdict = "ok";
      try {
        await waitUntil(
          async () =>
            (await driver.invoke("bridge_state")).clients.some(
              (client) => client.connected,
            ),
          "éditeur connecté dans l'app",
        );
        const chat = await waitUntil(async () => {
          const chats = await driver.invoke("list_chats");
          return chats.find((c) => c.source === "vscode");
        }, "conversation marquée VS Code");
        assert.match(chat.userExcerpt, /add/);
        const read = await waitUntil(async () => {
          const full = await driver.invoke("get_chat", { id: chat.id });
          return full.messages.length === 2 && full.messages[1].content
            ? full
            : null;
        }, "réponse du modèle enregistrée");
        assert.match(read.messages[0].content, /return a \+ b/);
        assert.match(read.messages[0].content, /sample\.js/);
        // Capture de la session virtuelle (jamais du bureau réel) : panneau
        // ouvert, réponse affichée.
        await delay(8000);
        execFileSync(join(ROOT, "scripts/e2e-session.sh"), [
          "--shot",
          "vscode-panel",
        ]);
      } catch (error) {
        verdict = String(error?.stack ?? error);
      }
      writeFileSync(doneFile, verdict);
      const code = await Promise.race([
        exited,
        delay(60000).then(() => {
          codium.kill("SIGKILL");
          return "timeout";
        }),
      ]);
      assert.equal(verdict, "ok");
      assert.equal(
        code,
        0,
        "VSCodium exited with an error (see e2e-vscodium.log)",
      );
    },
  );
  console.log("✓ vscode-extension");
} finally {
  await fake.stop();
  // Journaux de VSCodium gardés (sortie « ChatonLM » : erreurs du panneau).
  const logs = join(run, "logs");
  if (existsSync(logs)) {
    const kept = join(ROOT, ".tools/logs/vscodium-logs");
    rmSync(kept, { recursive: true, force: true });
    cpSync(logs, kept, { recursive: true });
  }
  rmSync(run, { recursive: true, force: true });
}
