// Autocomplétion locale dans un vrai éditeur (VSCodium, e2e/lib/vscodium.mjs)
// face à l'app réelle (build e2e) et au faux Ollama (modèle chatonlm-fim, à
// capacité « insert ») :
// - coupée par défaut (rien n'est demandé avant la bascule) ; activée, le
//   modèle est chargé avec ses options figées (keep_alive court, contexte
//   4 096), et chaque proposition reprend ces options ;
// - la proposition est demandée avec le code autour du curseur (préfixe et
//   suffixe), affichée en gris (capture .tools/shots/vscode-completion.png)
//   puis acceptée dans le fichier ;
// - rien ne part depuis un fichier sensible (.env) ni après la coupure ;
// - le nombre de requêtes reste borné (pas de boucle).
// Prérequis : builds de l'extension (vscode/dist).
import assert from "node:assert/strict";
import { withApp } from "./lib/app.mjs";
import { findDiscovery } from "./lib/bridge.mjs";
import { startFake } from "./lib/fake.mjs";
import { chooseModel, prepare, shot, waitUntil } from "./lib/vscodium.mjs";

const binary = process.argv[2];
const delay = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

const fake = await startFake("ollama");
const codium = prepare("vscode-completion", {
  "completion.js": "function add(a, b) {\n  \n}\n",
  ".env": "API_KEY=\n",
});
const fakeGet = async (path) => (await fetch(new URL(path, fake.url))).json();
/** Propositions demandées (corps de /api/generate avec un prompt). */
const generates = () => fakeGet("/fake/generates");
/** Chargements et déchargements demandés, dans l'ordre. */
const loads = () => fakeGet("/fake/loads");

try {
  await withApp(
    binary,
    { name: "vscode-completion", env: { OLLAMA_HOST: fake.url } },
    async (driver) => {
      await driver.waitFor("[data-testid=new-chat]");
      await chooseModel(driver, "qwen3:0.6b");
      await driver.invoke("bridge_set_enabled", { enabled: true });
      const { pairing } = await driver.invoke("bridge_pair");
      const found = await waitUntil(findDiscovery, "bridge.json");
      const exited = codium.start({
        bridgeDir: found.dir,
        scenario: "completion",
        settings: { "chatonlm.completion.model": "chatonlm-fim" },
        env: { CHATONLM_TEST_PAIR_CODE: pairing.code },
      });

      let verdict = "ok";
      try {
        // Coupée par défaut : une demande de proposition ne fait rien
        // partir et ne charge rien.
        await waitUntil(() => codium.step() === "ready", "éditeur prêt", 90000);
        assert.deepEqual(await generates(), [], "rien avant l'activation");
        assert.deepEqual(
          (await loads()).filter((l) => l.model === "chatonlm-fim"),
          [],
          "aucun chargement avant l'activation",
        );
        codium.ask("enable");
        await waitUntil(() => codium.step() === "enabled", "activée", 30000);
        await waitUntil(
          async () =>
            (await loads()).some(
              (l) => l.model === "chatonlm-fim" && l.kind === "load",
            ),
          "modèle d'autocomplétion chargé",
          30000,
        );
        await delay(1000);
        codium.ask("trigger");
        await waitUntil(
          () => codium.step() === "suggested",
          "proposition demandée",
          60000,
        );
        // Chargé une fois, avec ses options figées, avant toute proposition.
        const loaded = (await loads()).filter(
          (l) => l.model === "chatonlm-fim",
        );
        assert.deepEqual(loaded[0], {
          model: "chatonlm-fim",
          kind: "load",
          // 80 % de la durée de la conversation (défaut du serveur : 5 min) :
          // Ollama évince l'autocomplétion en premier.
          keep_alive: 240,
          num_ctx: 4096,
        });
        const sentAll = await waitUntil(
          async () => {
            const all = await generates();
            return all.length > 0 && all;
          },
          "proposition demandée",
          15000,
        );
        const sent = sentAll[0];
        assert.equal(sent.model, "chatonlm-fim");
        assert.match(sent.prompt, /function add\(a, b\) \{/);
        assert.ok(sent.suffix.startsWith("\n}"), JSON.stringify(sent.suffix));
        assert.equal(sent.options?.num_ctx, 4096);
        assert.equal(sent.keep_alive, 240);
        await delay(500);
        await shot("vscode-completion");
        codium.ask("shot");
        const accepted = await waitUntil(
          () => (codium.step() ?? "").startsWith("accepted:") && codium.step(),
          "proposition acceptée",
          60000,
        );
        assert.ok(accepted.length > "accepted:".length, accepted);
        codium.ask("secret");
        await waitUntil(() => codium.step() === "off", "autocomplétion coupée");
        const atOff = (await generates()).length;
        await delay(2000);
        const after = await generates();
        // Rien du fichier sensible n'est parti (une relance sur completion.js
        // après l'acceptation est permise).
        for (const request of after) {
          assert.doesNotMatch(
            `${request.prompt}${request.suffix}`,
            /API_KEY/,
            "rien depuis .env",
          );
        }
        assert.equal(after.length, atOff, "rien après la coupure");
        assert.ok(after.length <= 40, `requêtes bornées (${after.length})`);
      } catch (error) {
        verdict = String(error?.stack ?? error);
        // État de l'éditeur au moment de l'échec (barre d'état, menus).
        await shot("vscode-completion-failure").catch(() => {});
      }
      codium.done(verdict);
      const code = await exited();
      assert.equal(verdict, "ok");
      assert.equal(code, 0, "VSCodium exited with an error (see its log)");
    },
  );
  console.log("✓ vscode-completion");
} finally {
  await fake.stop();
  codium.finish();
}
