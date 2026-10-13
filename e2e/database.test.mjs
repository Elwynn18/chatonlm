// Base illisible au démarrage : la fenêtre s'ouvre quand même et explique.
// Base d'une version plus récente : mise à jour demandée, rien n'est proposé
// qui pourrait l'abîmer. Fichier abîmé : « Start with a new database » le met
// de côté (renommé) et l'app démarre sur une base neuve.
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { mkdirSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { withApp } from "./lib/app.mjs";
import { startFake } from "./lib/fake.mjs";

const binary = process.argv[2];
const dataDir = process.env.CHATONLM_DATA_DIR;
assert.ok(dataDir, "CHATONLM_DATA_DIR manquant (session virtuelle ?)");
const dbDir = join(dataDir, "db");
const dbFile = join(dbDir, "chatonlm.sqlite");

const buttons = (driver) =>
  driver.execute(
    "return [...document.querySelectorAll('button')].map((b) => b.textContent)",
  );

// Base créée par une version plus récente (schéma 99).
rmSync(dbDir, { recursive: true, force: true });
mkdirSync(dbDir, { recursive: true });
execFileSync(process.execPath, [
  "--no-warnings",
  "-e",
  `const { DatabaseSync } = require("node:sqlite");
   new DatabaseSync(process.argv[1]).exec("PRAGMA user_version = 99");`,
  dbFile,
]);
await withApp(binary, { name: "database-newer" }, async (driver) => {
  assert.equal(
    await driver.text(await driver.waitFor("h1")),
    "ChatonLM needs an update",
  );
  assert.deepEqual(await buttons(driver), []);
  await driver.screenshot(".tools/shots/database-newer.png");
});

// Fichier abîmé.
rmSync(dbDir, { recursive: true, force: true });
mkdirSync(dbDir, { recursive: true });
writeFileSync(dbFile, Buffer.alloc(8192, 0x42));
const fake = await startFake("ollama");
try {
  await withApp(
    binary,
    { name: "database-corrupt", env: { OLLAMA_HOST: fake.url } },
    async (driver) => {
      assert.equal(
        await driver.text(await driver.waitFor("h1")),
        "ChatonLM can't read its database",
      );
      assert.deepEqual(await buttons(driver), [
        "Start with a new database",
        "Retry",
      ]);
      await driver.screenshot(".tools/shots/database-corrupt.png");
      await driver.execute(
        "[...document.querySelectorAll('button')].find((b) => b.textContent === 'Start with a new database').click()",
      );
      await driver.waitFor("[data-testid=new-chat]", { timeoutMs: 10000 });
      const settings = await driver.invoke("get_settings");
      assert.equal(settings.theme, "system");
      // Easy Link, qui n'avait pas pu démarrer sans base, démarre avec la
      // base retrouvée, sans relancer l'app.
      const easyLink = await driver.invoke("easy_link_state");
      assert.deepEqual(
        easyLink.bots.map((bot) => [bot.platform, bot.configured]),
        [
          ["telegram", false],
          ["discord", false],
        ],
      );
    },
  );
} finally {
  await fake.stop();
}
const kept = readdirSync(dbDir).filter((name) =>
  /^chatonlm\.sqlite\.broken-\d+$/.test(name),
);
assert.equal(kept.length, 1, `fichier mis de côté : ${readdirSync(dbDir)}`);
