// Autocomplétion locale dans un vrai éditeur (VSCodium, e2e/lib/vscodium.mjs)
// face à l'app réelle (build e2e) et au faux moteur (modèle chatonlm-fim :
// jetons FIM, modèle de base ; `/infill` du faux llama-server) :
// - coupée par défaut (rien n'est demandé ni chargé avant la bascule) ;
//   activée, le modèle est lancé avec son contexte imposé (4 096, `-c`), et
//   chaque proposition passe par `/infill` ;
// - la proposition est demandée avec le code autour du curseur (préfixe et
//   suffixe), affichée en gris (capture .tools/shots/vscode-completion.png)
//   puis acceptée dans le fichier ;
// - rien ne part depuis un fichier sensible (.env) ni après la coupure ;
// - le nombre de requêtes reste borné (pas de boucle).
// Prérequis : builds de l'extension (vscode/dist).
import assert from "node:assert/strict";
import { withApp } from "./lib/app.mjs";
import { findDiscovery } from "./lib/bridge.mjs";
import { fakeModel, startEngine } from "./lib/fake.mjs";
import { chooseModel, prepare, shot, waitUntil } from "./lib/vscodium.mjs";

const binary = process.argv[2];
const delay = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

const CHAT = fakeModel("simple");
const FIM = fakeModel("fim");
const engine = startEngine([CHAT, { id: FIM, base: true }]);
const codium = prepare("vscode-completion", {
  "completion.js": "function add(a, b) {\n  \n}\n",
  ".env": "API_KEY=\n",
});
/** Propositions demandées (corps de `/infill` du modèle FIM). */
const infills = async () =>
  (await engine.requests(FIM))
    .filter((request) => request.path === "/infill")
    .map((request) => request.body);

try {
  await withApp(
    binary,
    { name: "vscode-completion", env: engine.env },
    async (driver) => {
      await driver.waitFor("[data-testid=new-chat]");
      await chooseModel(driver, CHAT);
      await driver.invoke("bridge_set_enabled", { enabled: true });
      const { pairing } = await driver.invoke("bridge_pair");
      const found = await waitUntil(findDiscovery, "bridge.json");
      const exited = codium.start({
        bridgeDir: found.dir,
        scenario: "completion",
        settings: { "chatonlm.completion.model": FIM },
        env: { CHATONLM_TEST_PAIR_CODE: pairing.code },
      });

      let verdict = "ok";
      try {
        // Coupée par défaut : une demande de proposition ne fait rien
        // partir et ne charge rien.
        await waitUntil(() => codium.step() === "ready", "éditeur prêt", 90000);
        assert.equal(
          engine.server(FIM),
          null,
          "aucun chargement avant l'activation",
        );
        codium.ask("enable");
        await waitUntil(() => codium.step() === "enabled", "activée", 30000);
        await waitUntil(
          () => engine.server(FIM) !== null,
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
        // Lancé une fois, avec son contexte imposé, avant toute proposition.
        const args = await engine.args(FIM);
        const ctx = args[args.indexOf("-c") + 1];
        assert.equal(ctx, "4096", JSON.stringify(args));
        assert.equal(
          engine.servers().filter((entry) => entry.alias === FIM).length,
          1,
          "un seul processus pour l'autocomplétion",
        );
        const sentAll = await waitUntil(
          async () => {
            const all = await infills();
            return all.length > 0 && all;
          },
          "proposition demandée",
          15000,
        );
        const sent = sentAll[0];
        assert.match(sent.input_prefix, /function add\(a, b\) \{/);
        assert.ok(
          sent.input_suffix.startsWith("\n}"),
          JSON.stringify(sent.input_suffix),
        );
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
        const atOff = (await infills()).length;
        await delay(2000);
        const after = await infills();
        // Rien du fichier sensible n'est parti (une relance sur completion.js
        // après l'acceptation est permise).
        for (const request of after) {
          assert.doesNotMatch(
            `${request.input_prefix}${request.input_suffix}${JSON.stringify(request.input_extra ?? [])}`,
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
  codium.finish();
}
