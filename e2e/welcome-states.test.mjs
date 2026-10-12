// Écran d'accueil selon l'installation d'Ollama, imposée par
// CHATONLM_E2E_HOST_FACTS (build de test) pour ne pas dépendre de la machine :
// non installé (commande d'installation, lien de téléchargement ouvert par un
// faux xdg-open, Retry), service arrêté et désactivé, service qui redémarre
// en boucle.
import assert from "node:assert/strict";
import {
  existsSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { join } from "node:path";
import { ROOT, freePort, withApp } from "./lib/app.mjs";
import { startFake } from "./lib/fake.mjs";

const binary = process.argv[2];

const texts = (driver, css) =>
  driver.execute(
    "return [...document.querySelectorAll(arguments[0])].map((e) => e.textContent)",
    [css],
  );

// Faux xdg-open : note le lien, sort avec le code écrit dans « exit ».
const fakeBin = mkdtempSync(join(ROOT, ".tools/tmp/xdg-open-"));
process.on("exit", () => rmSync(fakeBin, { recursive: true, force: true }));
writeFileSync(
  join(fakeBin, "xdg-open"),
  `#!/bin/sh\necho "$1" >> "${fakeBin}/opened"\nexit "$(cat "${fakeBin}/exit")"\n`,
  { mode: 0o755 },
);
const opened = () =>
  existsSync(join(fakeBin, "opened"))
    ? readFileSync(join(fakeBin, "opened"), "utf8").trim().split("\n")
    : [];
const clickText = (driver, css, text) =>
  driver.execute(
    "[...document.querySelectorAll(arguments[0])].find((e) => e.textContent === arguments[1]).click()",
    [css, text],
  );

// Non installé : adresse locale fermée, ni binaire ni service.
const port = await freePort();
await withApp(
  binary,
  {
    name: "welcome-not-installed",
    env: {
      OLLAMA_HOST: `127.0.0.1:${port}`,
      CHATONLM_E2E_HOST_FACTS: "binary=no;unit=none",
      PATH: `${fakeBin}:${process.env.PATH}`,
    },
  },
  async (driver) => {
    assert.equal(
      await driver.text(await driver.waitFor("h1")),
      "Ollama is not installed",
    );
    assert.deepEqual(await texts(driver, "code"), [
      "curl -fsSL https://ollama.com/install.sh | sh",
    ]);
    const href = await driver.execute(
      "return [...document.querySelectorAll('a')].find((a) => a.textContent === 'Download from ollama.com')?.href",
    );
    assert.equal(href, "https://ollama.com/download");
    await driver.screenshot(".tools/shots/welcome-not-installed.png");

    // Lien ouvert par xdg-open ; puis échec de xdg-open (code 3) : message et
    // adresse à recopier.
    writeFileSync(join(fakeBin, "exit"), "0");
    await clickText(driver, "a", "Download from ollama.com");
    for (let i = 0; i < 50 && opened().length === 0; i++) {
      await new Promise((resolve) => setTimeout(resolve, 100));
    }
    assert.deepEqual(opened(), ["https://ollama.com/download"]);
    writeFileSync(join(fakeBin, "exit"), "3");
    await clickText(driver, "a", "Download from ollama.com");
    assert.equal(
      await driver.text(await driver.waitFor("[role=alert]")),
      "Couldn't open the browser (no application is set to open web links (xdg-open exited with 3)). Go to https://ollama.com/download",
    );

    // Ollama installé et démarré entre-temps : Retry ouvre l'app.
    const fake = await startFake("ollama", port);
    try {
      await driver.execute(
        "[...document.querySelectorAll('button')].find((b) => b.textContent === 'Retry').click()",
      );
      await driver.waitFor("[data-testid=new-chat]", { timeoutMs: 5000 });
    } finally {
      await fake.stop();
    }
  },
);

// Service système arrêté et désactivé, à l'adresse standard (127.0.0.2 : un
// Ollama de la machine, lié à 127.0.0.1, n'y répond pas).
await withApp(
  binary,
  {
    name: "welcome-service-stopped",
    env: {
      OLLAMA_HOST: "127.0.0.2:11434",
      CHATONLM_E2E_HOST_FACTS: "binary=yes;unit=system:inactive:dead:disabled",
    },
  },
  async (driver) => {
    assert.equal(
      await driver.text(await driver.waitFor("h1")),
      "Ollama is not running",
    );
    assert.deepEqual(await texts(driver, "code"), [
      "sudo systemctl start ollama",
      "sudo systemctl enable ollama",
    ]);
    await driver.screenshot(".tools/shots/welcome-service-stopped.png");
  },
);

// Service utilisateur qui redémarre en boucle (Restart=always).
await withApp(
  binary,
  {
    name: "welcome-crash-loop",
    env: {
      OLLAMA_HOST: "127.0.0.2:11434",
      CHATONLM_E2E_HOST_FACTS:
        "binary=yes;unit=user:activating:auto-restart:enabled",
    },
  },
  async (driver) => {
    assert.equal(
      await driver.text(await driver.waitFor("h1")),
      "Ollama failed to start",
    );
    assert.deepEqual(await texts(driver, "code"), [
      "systemctl --user status ollama",
      "journalctl --user -e -u ollama",
    ]);
  },
);
