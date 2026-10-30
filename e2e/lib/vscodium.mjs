// VSCodium pour les essais de l'extension dans un vrai éditeur (version
// épinglée : packaging/vscodium.env, téléchargée par scripts/fetch-vscodium.sh).
// Lancé avec l'essai d'extension officiel (--extensionDevelopmentPath,
// --extensionTestsPath = e2e/vscode/extension-tests.cjs), un profil neuf sous
// .tools/e2e-runs, et un scénario choisi par CHATONLM_TEST_SCENARIO. L'essai
// intérieur et l'essai extérieur se parlent par deux fichiers du dossier de
// l'essai : `step.txt` (étape atteinte dans l'éditeur) et `verdict.txt`
// (« ok » ou la raison de l'échec, écrit par l'essai extérieur).
import assert from "node:assert/strict";
import { execFileSync, spawn } from "node:child_process";
import {
  cpSync,
  existsSync,
  mkdirSync,
  openSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

export const ROOT = fileURLToPath(new URL("../..", import.meta.url));

const delay = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

/**
 * Environnement de la session d'essai sans ce que le VS Code du développeur
 * transmet (ELECTRON_RUN_AS_NODE ferait de VSCodium un simple Node ;
 * VSCODE_IPC_HOOK* et compagnie le relieraient à l'éditeur réel).
 */
export function isolatedEnv() {
  const env = { ...process.env };
  for (const key of Object.keys(env)) {
    if (/^(ELECTRON_|VSCODE_|CHROME_|GIT_ASKPASS|TERM_PROGRAM)/.test(key)) {
      delete env[key];
    }
  }
  return env;
}

export async function waitUntil(check, what, timeoutMs = 60000) {
  const deadline = Date.now() + timeoutMs;
  for (;;) {
    const value = await check();
    if (value) return value;
    if (Date.now() > deadline) throw new Error(`Délai dépassé : ${what}`);
    await delay(250);
  }
}

/**
 * Capture de la session virtuelle (jamais du bureau réel). Spectacle rend
 * parfois une image vide : trois essais.
 */
export async function shot(name) {
  for (let attempt = 1; ; attempt += 1) {
    try {
      execFileSync(join(ROOT, "scripts/e2e-session.sh"), ["--shot", name]);
      return;
    } catch (error) {
      if (attempt >= 3) throw error;
      await delay(1000);
    }
  }
}

/**
 * Prépare un essai : dossier ouvert (`files` : nom → contenu), profil neuf.
 * Rend `{ run, workspace, step(), done(verdict), start(env), keepLogs() }`.
 */
export function prepare(name, files) {
  const codium = execFileSync(join(ROOT, "scripts/fetch-vscodium.sh"))
    .toString()
    .trim();
  for (const needed of [
    codium,
    join(ROOT, "vscode/dist/extension.cjs"),
    join(ROOT, "vscode/dist/webview/index.js"),
  ]) {
    assert.ok(existsSync(needed), `missing ${needed}`);
  }
  const run = join(ROOT, ".tools/e2e-runs", `${name}-${process.pid}`);
  rmSync(run, { recursive: true, force: true });
  const workspace = join(run, "workspace");
  const userData = join(run, "user-data");
  mkdirSync(workspace, { recursive: true });
  mkdirSync(join(userData, "User"), { recursive: true });
  for (const [file, content] of Object.entries(files)) {
    writeFileSync(join(workspace, file), content);
  }
  const stepFile = join(run, "step.txt");
  const verdictFile = join(run, "verdict.txt");

  return {
    run,
    workspace,
    /** Étape écrite par l'essai intérieur, ou null. */
    step: () =>
      existsSync(stepFile) ? readFileSync(stepFile, "utf8").trim() : null,
    /** Demande `name` à l'essai intérieur (fichier `step.txt.<name>`). */
    ask: (name) => writeFileSync(`${stepFile}.${name}`, ""),
    /** Verdict de l'essai extérieur : l'essai intérieur le lit et finit. */
    done: (verdict) => writeFileSync(verdictFile, verdict),
    /** Lance VSCodium (réglages de l'utilisateur, scénario, variables). */
    start({ bridgeDir, scenario, env = {} }) {
      writeFileSync(
        join(userData, "User", "settings.json"),
        JSON.stringify({
          "chatonlm.bridgeDir": bridgeDir,
          "security.workspace.trust.enabled": false,
          "telemetry.telemetryLevel": "off",
          "update.mode": "none",
          "extensions.autoCheckUpdates": false,
        }),
      );
      const log = openSync(
        join(ROOT, `.tools/logs/e2e-${name}-codium.log`),
        "w",
      );
      const codiumProcess = spawn(
        codium,
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
            ...env,
            CHATONLM_TEST_SCENARIO: scenario,
            CHATONLM_TEST_STEP_FILE: stepFile,
            CHATONLM_TEST_DONE_FILE: verdictFile,
          },
          stdio: ["ignore", log, log],
        },
      );
      const exited = new Promise((resolve) =>
        codiumProcess.on("exit", resolve),
      );
      /** Code de sortie de VSCodium (60 s au plus, puis tué). */
      return () =>
        Promise.race([
          exited,
          delay(60000).then(() => {
            codiumProcess.kill("SIGKILL");
            return "timeout";
          }),
        ]);
    },
    /** Journaux de VSCodium gardés (sortie « ChatonLM »), dossier retiré. */
    finish() {
      const logs = join(run, "logs");
      if (existsSync(logs)) {
        const kept = join(ROOT, `.tools/logs/${name}-codium-logs`);
        rmSync(kept, { recursive: true, force: true });
        cpSync(logs, kept, { recursive: true });
      }
      rmSync(run, { recursive: true, force: true });
    },
  };
}
