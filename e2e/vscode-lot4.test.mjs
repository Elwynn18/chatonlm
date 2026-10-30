// Lot 4a dans un vrai éditeur (VSCodium, e2e/lib/vscodium.mjs) face à l'app
// réelle (build e2e), au faux Ollama, au faux Telegram, au faux Hugging Face
// et au faux moteur d'images :
// - « notify » : une commande du terminal (intégration du shell de bash), avec
//   des arguments dont un mot de passe, finit en échec ; le compte Telegram lié
//   reçoit un message spontané avec le nom du programme et l'échec, SANS ses
//   arguments, sa sortie ni un chemin ;
// - « proofread » : correction des commentaires d'un fichier : vue de
//   différences ouverte, code inchangé dans la proposition, fichier intact,
//   la chaîne du code jamais envoyée au modèle ;
// - « image » : « Generate this image » sur une image absente d'une page :
//   PNG écrit à l'emplacement attendu, rien d'autre.
// Prérequis : builds de l'extension (vscode/dist), fake-sd-server et
// fake-messengers construits (scripts/e2e-build.sh).
import assert from "node:assert/strict";
import { chmodSync, existsSync, readdirSync } from "node:fs";
import { join } from "node:path";
import { ROOT as APP_ROOT, withApp } from "./lib/app.mjs";
import { findDiscovery } from "./lib/bridge.mjs";
import { startFake, startFakeHf } from "./lib/fake.mjs";
import { FAKE_TOKENS, startFakeMessenger } from "./lib/messenger.mjs";
import { chooseModel, prepare, waitUntil } from "./lib/vscodium.mjs";

const binary = process.argv[2];
const KLEIN = "chatonlm/klein-test-GGUF";
const FAMILIES = `${APP_ROOT}src-tauri/crates/fake-ollama/fixtures/hf/image-families.json`;
const SD_SERVER = `${APP_ROOT}.tools/e2e-target/debug/fake-sd-server`;
assert.ok(existsSync(SD_SERVER), `faux moteur d'images absent : ${SD_SERVER}`);

/** Relie le bot Telegram et y lie le compte `user`. */
async function linkTelegram(driver, telegram, user) {
  await driver.invoke("easy_link_connect", {
    platform: "telegram",
    token: FAKE_TOKENS.telegram,
  });
  await waitUntil(
    async () =>
      (await driver.invoke("easy_link_state")).bots[0].link.state ===
      "connected",
    "Telegram connecté",
  );
  const paired = await driver.invoke("easy_link_pair", {
    platform: "telegram",
  });
  await telegram.message({ user, name: "Ana", text: paired.pairing.code });
  await waitUntil(
    async () => (await driver.invoke("easy_link_state")).accounts.length === 1,
    "compte lié",
  );
}

/** Installe le modèle d'images d'essai et le choisit. */
async function installImageModel(driver) {
  const detail = await driver.invoke("image_repo", { repo: KLEIN });
  const file = detail.files.find((f) => f.recommended) ?? detail.files[0];
  const id = `${KLEIN}/${file.file}`;
  await driver.invoke("image_download", {
    repo: KLEIN,
    file: file.file,
    revision: detail.revision,
  });
  await waitUntil(
    async () =>
      (await driver.invoke("image_downloads")).find((d) => d.id === id)
        ?.state === "done",
    "modèle d'images installé",
    60000,
  );
  await driver.invoke("image_use_model", { id });
}

/** Lance un scénario dans VSCodium et rend son verdict. */
async function scenario(driver, name, files, { settings = {}, check }) {
  const codium = prepare(`vscode-${name}`, files);
  try {
    if (name === "notify") chmodSync(join(codium.workspace, "build.sh"), 0o755);
    const { pairing } = await driver.invoke("bridge_pair");
    const found = await waitUntil(findDiscovery, "bridge.json");
    const exited = codium.start({
      bridgeDir: found.dir,
      scenario: name,
      settings,
      env: { CHATONLM_TEST_PAIR_CODE: pairing.code },
    });
    let verdict = "ok";
    try {
      await check(codium);
    } catch (error) {
      verdict = String(error?.stack ?? error);
    }
    codium.done(verdict);
    const code = await exited();
    assert.equal(verdict, "ok", name);
    assert.equal(code, 0, `${name} : VSCodium exited with an error`);
  } finally {
    codium.finish();
  }
}

const fake = await startFake("ollama");
const hf = await startFakeHf("hf");
const telegram = await startFakeMessenger("telegram");

try {
  await withApp(
    binary,
    {
      name: "vscode-lot4",
      env: {
        OLLAMA_HOST: fake.url,
        CHATONLM_TELEGRAM_URL: telegram.url,
        CHATONLM_HF_URL: hf.url,
        CHATONLM_HF_STATUS_URL: hf.url,
        CHATONLM_IMAGE_FAMILIES: FAMILIES,
        CHATONLM_SD_SERVER_BIN: SD_SERVER,
      },
    },
    async (driver) => {
      await driver.waitFor("[data-testid=new-chat]");
      await chooseModel(driver, "qwen3:0.6b");
      await driver.invoke("update_settings", {
        patch: { proofreadModel: "chatonlm-proofread" },
      });
      await driver.invoke("bridge_set_enabled", { enabled: true });
      await linkTelegram(driver, telegram, "1001");
      await driver.invoke("easy_link_set_vscode_notify", { enabled: true });
      await installImageModel(driver);

      // 1. Fin d'une commande du terminal.
      await scenario(
        driver,
        "notify",
        {
          "build.sh":
            "#!/bin/bash\necho SORTIE-SECRETE /home/someone/project\nsleep 2\nexit 3\n",
        },
        {
          settings: {
            "chatonlm.notify.enabled": true,
            "chatonlm.notify.minDurationSeconds": 0,
          },
          async check(codium) {
            await waitUntil(
              () => (codium.step() ?? "").startsWith("ended:"),
              "commande finie",
              60000,
            );
            const message = await waitUntil(
              async () =>
                (await telegram.sent()).find(
                  (action) =>
                    action.kind === "send" && /build/.test(action.text),
                ),
              "message spontané reçu",
              30000,
            );
            assert.match(message.text, /build\.sh/);
            assert.match(message.text, /Failed/);
            assert.doesNotMatch(message.text, /SORTIE-SECRETE/);
            assert.doesNotMatch(message.text, /\/home\//);
            // Jamais d'argument : ni la cible, ni le mot de passe tapé.
            assert.doesNotMatch(message.text, /Hunter2Summer|production/);
          },
        },
      );

      // 2. Correction des commentaires.
      await scenario(
        driver,
        "proofread",
        {
          "src/a.ts":
            '// Retrys teh request.\nconst s = "teh-server";\nexport { s };\n',
        },
        {
          async check(codium) {
            await waitUntil(
              () => codium.step() === "diff",
              "différences",
              90000,
            );
            const sent = await (
              await fetch(new URL("/fake/requests", fake.url))
            ).json();
            assert.ok(
              sent.some((r) => r.model === "chatonlm-proofread"),
              "correction demandée",
            );
            assert.doesNotMatch(JSON.stringify(sent), /teh-server/);
          },
        },
      );

      // 3. Image écrite dans le dossier.
      await scenario(
        driver,
        "image",
        {
          "site/index.html": '<img src="img/hero.png" alt="A red kitten">\n',
        },
        {
          async check(codium) {
            await waitUntil(
              () => codium.step() === "written",
              "image écrite",
              150000,
            );
            const img = readdirSync(join(codium.workspace, "site/img"));
            assert.deepEqual(img, ["hero.png"]);
          },
        },
      );
    },
  );
  console.log("✓ vscode-lot4");
} finally {
  await fake.stop();
  await hf.stop();
  await telegram.stop();
}
