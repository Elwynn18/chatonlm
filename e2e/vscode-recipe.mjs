// Recette pilotée pas à pas de l'extension VS Code : l'app réelle (build e2e)
// et VSCodium dans la session virtuelle, face au vrai moteur (llama-server
// épinglé) et aux vrais modèles d'essai (scripts/test-llama.sh fetch, rôles
// de models.json, verrou des vrais serveurs tenu jusqu'à la fin), voix de
// lecture et modèles de transcription de .tools. Ce n'est pas un essai :
// check.sh ne le lance pas. Rien n'est scénarisé ici : le banc attend des
// commandes dans .tools/recipe/ctl, envoyées par e2e/lib/recipe-ctl.mjs :
//   - `codium/<n>.js` : corps d'une fonction async (vscode, h) exécutée dans
//     l'hôte d'extensions (scénario « recipe » de e2e/vscode/extension-tests.cjs) ;
//   - `app/<n>.js` : corps d'une fonction async (ctx) exécutée ici, avec le
//     pilote WebDriver de l'app, les captures, le vrai clavier et la souris de
//     la session (pressKeys), KWin, le faux Telegram ;
//   - `end` : fin de la recette.
// Lancement (4 h au plus) :
//   scripts/test-llama.sh fetch
//   E2E_TIMEOUT=14400 scripts/e2e-session.sh e2e/run.sh \
//     e2e/vscode-recipe.mjs "$(scripts/e2e-build.sh)"
// Variables : RECIPE_ROLES (rôles de models.json, séparés par des virgules ;
// « text,fim » par défaut : conversation, puis autocomplétion). Un modèle plus
// grand se donne au rôle « text » de models.json.
// Prérequis : builds de l'extension (vscode/dist).
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import {
  chmodSync,
  existsSync,
  mkdirSync,
  readFileSync,
  readdirSync,
  renameSync,
  rmSync,
  symlinkSync,
  writeFileSync,
} from "node:fs";
import { join } from "node:path";
import { ROOT, withApp } from "./lib/app.mjs";
import { findDiscovery } from "./lib/bridge.mjs";
import { holdClipboard, readClipboard } from "./lib/clipboard.mjs";
import { holdRealLock, startFakeHf, startRealEngine } from "./lib/fake.mjs";
import { pressKeys } from "./lib/keyboard.mjs";
import { forWindows, runKWinScript, windowCaptions } from "./lib/kwin.mjs";
import { FAKE_TOKENS, startFakeMessenger } from "./lib/messenger.mjs";
import { PHRASES, linkParakeet, linkVoice, writePhrase } from "./lib/voice.mjs";
import { chooseModel, prepare, shot, waitUntil } from "./lib/vscodium.mjs";

const binary = process.argv[2];
const CTL = `${ROOT}.tools/recipe/ctl`;
const DATA = process.env.CHATONLM_DATA_DIR;
assert.ok(
  DATA,
  "CHATONLM_DATA_DIR absent : lancer dans scripts/e2e-session.sh",
);
const delay = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

const releaseLock = await holdRealLock();
const engine = startRealEngine(
  (process.env.RECIPE_ROLES || "text,fim").split(",").map((r) => r.trim()),
);
if (!engine) {
  releaseLock();
  throw new Error(
    "Vrai moteur ou modèles absents : scripts/build-llama-server.sh, scripts/test-llama.sh fetch",
  );
}
const [MODEL, COMPLETION = null] = engine.ids;

rmSync(CTL, { recursive: true, force: true });
mkdirSync(`${CTL}/codium`, { recursive: true });
mkdirSync(`${CTL}/app`, { recursive: true });

// Voix, Parakeet (séance vocale), Whisper tiny (dictée).
for (const voice of ["pocket-tts-en", "pocket-tts-fr", "pocket-tts-es"]) {
  linkVoice(DATA, voice);
}
linkParakeet(DATA);
mkdirSync(`${DATA}/speech-models/whisper-tiny`, { recursive: true });
symlinkSync(
  `${ROOT}.tools/speech-models/whisper/ggml-tiny.bin`,
  `${DATA}/speech-models/whisper-tiny/ggml-tiny.bin`,
);
const phrase = writePhrase(
  PHRASES.first,
  `${ROOT}.tools/tmp/vscode-recipe-${process.pid}`,
);

// Un petit projet réaliste : dépôt git (visite des changements, message de
// commit), fichiers à expliquer, corriger, compléter, relire.
const codium = prepare("vscode-recipe", {
  "package.json": `{
  "name": "shop",
  "version": "1.0.0",
  "private": true,
  "type": "module",
  "scripts": { "test": "node --test" }
}
`,
  "README.md": `# Shop

A tiny shop backend. It calcualte the cart total, retry the payement
when the bank dont answer, and turn product titles into URL slugs.

## Usage

Run \`npm test\` to launch the tests. Their is no server yet.
`,
  "src/slugify.ts": `/** Turns a title into a URL-friendly slug. */
export function slugify(title: string, maxLength = 60): string {
  const slug = title
    .normalize("NFKD")
    .replace(/[\\u0300-\\u036f]/g, "")
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "");
  if (slug.length <= maxLength) return slug;
  const cut = slug.slice(0, maxLength);
  const lastDash = cut.lastIndexOf("-");
  return lastDash > 0 ? cut.slice(0, lastDash) : cut;
}
`,
  "src/retry.ts": `/** Runs a task again after a failure, waiting longer each time. */
export async function retry<T>(
  task: () => Promise<T>,
  attempts = 3,
  delayMs = 500,
): Promise<T> {
  let lastError: unknown;
  for (let attempt = 1; attempt <= attempts; attempt++) {
    try {
      return await task();
    } catch (error) {
      lastError = error;
      if (attempt < attempts) {
        const wait = delayMs * 2 ** (attempt - 1);
        await new Promise((resolve) => setTimeout(resolve, wait));
      }
    }
  }
  throw lastError;
}
`,
  "src/cart.ts": `interface CartItem {
  name: string;
  price: number;
  quantity: number;
}

export function cartTotal(items: CartItem[]): number {

}
`,
  "src/discount.js": `// Aplly a discount, in percent, to a price. The result is rounded to
// the cent, and never go below zero.
export function applyDiscount(price, percent) {
  const discounted = price - price * percent;
  return Math.round(discounted * 100) / 100;
}
`,
  "src/main.c": `#include <stdio.h>

int main() {

}
`,
  "notes.md": "Quote: \n",
  "site/index.html": `<!doctype html>
<html lang="en">
  <head><title>Shop</title></head>
  <body>
    <h1>Welcome to the shop</h1>
    <img src="img/hero.png" alt="A cosy shop window at dusk, warm light, a ginger cat sleeping on the counter" width="768" height="432">
  </body>
</html>
`,
  "build.sh": `#!/usr/bin/env bash
echo "Building $1..."
sleep 3
echo "error: missing configuration" >&2
exit 2
`,
  ".env": "API_KEY=sk-recipe-secret-0000\n",
  ".gitignore": ".env\n",
});
chmodSync(join(codium.workspace, "build.sh"), 0o755);
const git = (...args) =>
  execFileSync(
    "git",
    [
      "-c",
      "user.name=Recipe",
      "-c",
      "user.email=recipe@example.invalid",
      ...args,
    ],
    { cwd: codium.workspace },
  );
git("init", "-q", "-b", "main");
git("add", "-A");
git("commit", "-q", "-m", "Initial shop");
// Changement non commité : remise appliquée deux fois, comme un vrai bogue.
writeFileSync(
  join(codium.workspace, "src/retry.ts"),
  readFileSync(join(codium.workspace, "src/retry.ts"), "utf8")
    .replace(
      "attempts = 3,",
      "attempts = 5,\n  // Waits are capped so a long outage does not freeze the caller.\n  maxDelayMs = 4000,",
    )
    .replace(
      "const wait = delayMs * 2 ** (attempt - 1);",
      "const wait = Math.min(delayMs * 2 ** (attempt - 1), maxDelayMs);",
    ),
);

/** Exécute les commandes `<dossier>/<n>.js` dans l'ordre, jusqu'à `end`. */
async function serve(dir, run) {
  const AsyncFunction = (async () => {}).constructor;
  for (let n = 1; ; n += 1) {
    const input = `${dir}/${n}.js`;
    while (!existsSync(input)) {
      if (existsSync(`${CTL}/end`)) return;
      await delay(200);
    }
    let result;
    try {
      const body = readFileSync(input, "utf8");
      const value = await run(AsyncFunction, body);
      result = { ok: true, value: value ?? null };
    } catch (error) {
      result = { ok: false, error: String(error?.stack ?? error) };
    }
    let text;
    try {
      text = JSON.stringify(result, null, 2);
    } catch {
      text = JSON.stringify({ ok: result.ok, value: String(result.value) });
    }
    writeFileSync(`${dir}/${n}.tmp`, text);
    renameSync(`${dir}/${n}.tmp`, `${dir}/${n}.json`);
  }
}

const hf = await startFakeHf("hf");
const telegram = await startFakeMessenger("telegram");

try {
  await withApp(
    binary,
    {
      name: "vscode-recipe",
      env: {
        ...engine.env,
        CHATONLM_TELEGRAM_URL: telegram.url,
        CHATONLM_HF_URL: hf.url,
        CHATONLM_HF_STATUS_URL: hf.url,
        CHATONLM_IMAGE_FAMILIES: `${ROOT}src-tauri/crates/fake-services/fixtures/hf/image-families.json`,
        CHATONLM_SD_SERVER_BIN: `${ROOT}.tools/e2e-target/debug/fake-sd-server`,
        CHATONLM_E2E_MICROPHONE: `${ROOT}.tools/speech-samples/jfk.wav`,
        CHATONLM_E2E_VOICE_MIC: JSON.stringify([
          { on: "listening", wav: phrase.wav, delayMs: 300 },
        ]),
        CHATONLM_NO_GLOBAL_SHORTCUTS: "1",
      },
    },
    async (driver) => {
      await driver.waitFor("[data-testid=new-chat]");
      await chooseModel(driver, MODEL);
      // Modèle chargé d'avance (premier envoi sans attente).
      await driver.invoke("load_model", { model: MODEL });
      await driver.invoke("update_dictation_settings", {
        patch: { speechModel: "whisper-tiny", language: "en", polishModel: "" },
      });
      await driver.invoke("bridge_set_enabled", { enabled: true });
      const { pairing } = await driver.invoke("bridge_pair");
      const found = await waitUntil(findDiscovery, "bridge.json");
      const exited = codium.start({
        bridgeDir: found.dir,
        scenario: "recipe",
        settings: {
          "git.openRepositoryInParentFolders": "never",
          "workbench.startupEditor": "none",
          "workbench.tips.enabled": false,
          "editor.minimap.enabled": false,
        },
        env: {
          CHATONLM_TEST_PAIR_CODE: pairing.code,
          CHATONLM_RECIPE_DIR: CTL,
        },
      });
      const ctx = {
        driver,
        shot: (name) => shot(`recipe-${name}`),
        pressKeys,
        runKWinScript,
        forWindows,
        windowCaptions,
        waitUntil,
        delay,
        fetch,
        MODEL,
        COMPLETION,
        engine,
        DATA,
        codium,
        workspace: codium.workspace,
        telegram,
        FAKE_TOKENS,
        readClipboard,
        holdClipboard,
        readdirSync,
        readFileSync,
        existsSync,
        git,
      };
      writeFileSync(
        `${CTL}/ready`,
        JSON.stringify({ workspace: codium.workspace, data: DATA }),
      );
      let verdict = "ok";
      try {
        await serve(`${CTL}/app`, (AsyncFunction, body) =>
          new AsyncFunction("ctx", body)(ctx),
        );
      } catch (error) {
        verdict = String(error?.stack ?? error);
      }
      codium.done(verdict);
      const code = await exited();
      writeFileSync(`${CTL}/exited`, String(code));
    },
  );
  console.log("✓ vscode-recipe");
} finally {
  codium.finish();
  // Faux serveurs arrêtés et verrou rendu : sinon Node reste vivant et la
  // session aussi.
  await Promise.all([hf.stop(), telegram.stop()]);
  releaseLock();
}
