// Dictée de l'extension dans VSCodium face à l'app réelle : faux micro WAV
// (CHATONLM_E2E_MICROPHONE, jfk.wav), Whisper tiny, texte inséré au curseur
// de notes.md, presse-papiers intact.
import assert from "node:assert/strict";
import { mkdirSync, symlinkSync } from "node:fs";
import { ROOT, withApp } from "./lib/app.mjs";
import { findDiscovery } from "./lib/bridge.mjs";
import { holdClipboard } from "./lib/clipboard.mjs";
import { startFake } from "./lib/fake.mjs";
import { chooseModel, prepare, shot, waitUntil } from "./lib/vscodium.mjs";

const binary = process.argv[2];
const DATA = process.env.CHATONLM_DATA_DIR;
assert.ok(
  DATA,
  "CHATONLM_DATA_DIR absent : lancer dans scripts/e2e-session.sh",
);
const JFK = `${ROOT}.tools/speech-samples/jfk.wav`;
const TINY = `${ROOT}.tools/speech-models/whisper/ggml-tiny.bin`;
mkdirSync(`${DATA}/speech-models/whisper-tiny`, { recursive: true });
symlinkSync(TINY, `${DATA}/speech-models/whisper-tiny/ggml-tiny.bin`);

const fake = await startFake("ollama");
const codium = prepare("vscode-dictation-editor", { "notes.md": "Quote: \n" });
try {
  await withApp(
    binary,
    {
      name: "vscode-dictation-editor",
      env: {
        OLLAMA_HOST: fake.url,
        CHATONLM_E2E_MICROPHONE: JFK,
        CHATONLM_NO_GLOBAL_SHORTCUTS: "1",
      },
    },
    async (driver) => {
      await driver.waitFor("[data-testid=new-chat]");
      await chooseModel(driver, "qwen3:0.6b");
      // Modèle de la dictée : Whisper tiny (comme e2e/vscode-dictation.test.mjs).
      await driver.invoke("update_dictation_settings", {
        patch: { speechModel: "whisper-tiny", language: "en", polishModel: "" },
      });
      await driver.invoke("bridge_set_enabled", { enabled: true });
      const { pairing } = await driver.invoke("bridge_pair");
      const found = await waitUntil(findDiscovery, "bridge.json");
      const held = await holdClipboard({
        text: "copied by another application",
      });
      const exited = codium.start({
        bridgeDir: found.dir,
        scenario: "dictate",
        env: { CHATONLM_TEST_PAIR_CODE: pairing.code },
      });
      let verdict = "ok";
      try {
        await waitUntil(() => codium.step() === "listening", "écoute");
        // Le WAV dure ~11 s.
        await new Promise((resolve) => setTimeout(resolve, 12_000));
        codium.ask("stop");
        const step = await waitUntil(
          () => (codium.step()?.startsWith("inserted:") ? codium.step() : null),
          "texte inséré",
          120_000,
        );
        assert.match(step, /^inserted:Quote: .*country/i);
      } catch (error) {
        verdict = String(error?.stack ?? error);
        await shot("vscode-dictation-failure").catch(() => {});
        console.log(
          "état de la dictée :",
          JSON.stringify(
            await driver.invoke("dictation_status").catch((e) => String(e)),
          ),
        );
      } finally {
        await held.release();
      }
      codium.done(verdict);
      const code = await exited();
      assert.equal(verdict, "ok");
      assert.equal(code, 0);
    },
  );
  console.log("✓ vscode-dictation-editor");
} finally {
  await fake.stop();
  codium.finish();
}
