// Settings > Model location dans l'app réelle (contrat 38, recherche 36 § 7
// et § 8), sans jamais toucher au vrai service : faux pkexec et faux
// systemctl de src-tauri/src/ollama/models_dir/fake-bin, racine simulée
// (CHATONLM_TEST_ROOT : etc/, proc/, état du faux service) et dossiers de
// modèles dans le dossier de l'essai (chemins avec espace et accent) ;
// faits de la détection imposés (CHATONLM_E2E_MODELS_FACTS, dossier relu dans
// l'état du faux service), dossier « choisi » lu dans un fichier
// (CHATONLM_E2E_PICK_FOLDER) :
// - service système : champ = dossier actuel ; Browse → confirmation (titre,
//   dossiers, nombre et taille des modèles, Cancel · Leave them · Move
//   models) → Move models : progression (« Moving models… x of y », barre,
//   « Restarting Ollama… »), « Saved », champ mis à jour ; modèles copiés
//   dans le nouveau dossier, anciens effacés, drop-in écrit, service arrêté,
//   rechargé puis relancé ;
// - annulation dans l'invite (126) : rien d'affiché ni de changé, systemctl
//   jamais appelé ;
// - pas d'agent d'authentification (127) : message, commandes sudo à
//   copier, « Details » (sortie de pkexec) ;
// - Ollama ne démarre pas avec le nouveau dossier (24) : message « previous
//   one was restored », « Details » (lignes du script), modèles et champ
//   revenus, pas de drop-in ;
// - lancement à la main : commande « OLLAMA_HOST=… OLLAMA_MODELS='…' ollama
//   serve » ; Copy la dépose dans le presse-papiers (lu comme une autre
//   application) ; pkexec jamais lancé ;
// - serveur distant : « On {host} », Browse désactivé, explication ;
// - dossier .ollama du service caché (ProtectHome=yes, HOME sous /home) :
//   alerte sous la ligne, Fix → invite (faux pkexec), drop-in
//   zz-chatonlm-home.conf écrit, .ollama créé, service rechargé et redémarré,
//   alerte retirée (faits relus dans l'état du faux service) et « Saved » ;
// - téléchargement du ModelHub en cours : refus (« Wait for the model
//   downloads to finish… ») sans invite ; une fois arrêté, la confirmation
//   revient ;
// - captures de la ligne, de la confirmation et des messages
//   (.tools/shots/models-dir/) ;
// - aucune violation de la CSP ni erreur ou avertissement dans la console.
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import {
  existsSync,
  mkdirSync,
  readFileSync,
  readdirSync,
  writeFileSync,
} from "node:fs";
import { dirname, join } from "node:path";
import { setTimeout as delay } from "node:timers/promises";
import { ROOT, withApp } from "./lib/app.mjs";
import { readClipboard } from "./lib/clipboard.mjs";
import { startFake } from "./lib/fake.mjs";

const binary = process.argv[2];
const SHOTS = ".tools/shots/models-dir";
mkdirSync(SHOTS, { recursive: true });

// Dossier de l'essai (supprimé par e2e-session.sh à la fin).
const RUN = dirname(process.env.CHATONLM_DATA_DIR);
const CASES = join(RUN, "models dir é");
const FAKE_BIN = join(ROOT, "src-tauri/src/ollama/models_dir/fake-bin");
const USER = execFileSync("id", ["-un"], { encoding: "utf8" }).trim();
const GROUP = execFileSync("id", ["-gn"], { encoding: "utf8" }).trim();
const DROPIN = "etc/systemd/system/ollama.service.d/zz-chatonlm-models.conf";

const fake = await startFake("ollama");

/** Dossier de modèles comme Ollama le laisse (plus un morceau en cours). */
function makeModels(dir) {
  for (const sub of [
    "blobs",
    "manifests/registry.ollama.ai/library/qwen3",
    "manifests/hf.co/bartowski/Some-GGUF",
  ]) {
    mkdirSync(join(dir, sub), { recursive: true });
  }
  writeFileSync(join(dir, "blobs/sha256-aaaa"), "weights-A");
  writeFileSync(join(dir, "blobs/sha256-bbbb"), "weights-B");
  writeFileSync(join(dir, "blobs/sha256-cccc-partial"), "half");
  writeFileSync(
    join(dir, "manifests/registry.ollama.ai/library/qwen3/0.6b"),
    '{"qwen3":"old"}',
  );
  writeFileSync(
    join(dir, "manifests/hf.co/bartowski/Some-GGUF/Q4_K_M"),
    '{"hf":1}',
  );
}

/**
 * Un cas : racine simulée, anciens modèles, faux service actif (PID 4000),
 * nouveau dossier (absent, parent présent) remis par le « sélecteur ».
 */
function makeCase(name) {
  const dir = join(CASES, name);
  const root = join(dir, "root");
  for (const sub of [
    "run",
    "log",
    "state",
    "proc/4000",
    "etc/systemd/system",
  ]) {
    mkdirSync(join(root, sub), { recursive: true });
  }
  const old = join(dir, "var lib ollama");
  makeModels(old);
  const state = (key, value) =>
    writeFileSync(join(root, "state", key), `${value}\n`);
  state("base_models", old);
  state("loaded_models", old);
  state("active", "active");
  state("pid", "4000");
  writeFileSync(join(root, "proc/4000/environ"), `OLLAMA_MODELS=${old}\0`);
  const target = join(dir, "New models é");
  const pick = join(dir, "picked");
  writeFileSync(pick, `${target}\n`);
  return {
    dir,
    root,
    old,
    target,
    log: (file) => {
      const path = join(root, "log", `${file}.log`);
      return existsSync(path) ? readFileSync(path, "utf8") : "";
    },
    env: (facts, extra = {}) => ({
      OLLAMA_HOST: fake.url,
      CHATONLM_E2E_MODELS_FACTS: facts,
      CHATONLM_PKEXEC: join(FAKE_BIN, "pkexec"),
      CHATONLM_TEST_BIN: FAKE_BIN,
      CHATONLM_TEST_ROOT: root,
      CHATONLM_E2E_PICK_FOLDER: pick,
      ...extra,
    }),
    /** Service système ; dossier relu dans l'état du faux service. */
    system(extra) {
      return this.env(
        `launch=system;dir=@${join(root, "state/loaded_models")};user=${USER};group=${GROUP};running=yes;managed=no;protect_home=yes;protect_system=full`,
        extra,
      );
    },
  };
}

/** Fichiers d'un dossier, chemins relatifs triés. */
function filesOf(dir) {
  if (!existsSync(dir)) return [];
  return readdirSync(dir, { recursive: true, withFileTypes: true })
    .filter((entry) => entry.isFile())
    .map((entry) =>
      join(entry.parentPath ?? entry.path, entry.name).slice(dir.length + 1),
    )
    .sort();
}

const MODEL_FILES = [
  "blobs/sha256-aaaa",
  "blobs/sha256-bbbb",
  "blobs/sha256-cccc-partial",
  "manifests/hf.co/bartowski/Some-GGUF/Q4_K_M",
  "manifests/registry.ollama.ai/library/qwen3/0.6b",
];

async function waitUntil(check, what, timeoutMs = 10000) {
  const deadline = Date.now() + timeoutMs;
  for (;;) {
    const value = await check();
    if (value) return value;
    if (Date.now() > deadline) {
      throw new Error(
        `Délai dépassé : ${typeof what === "function" ? what() : what}`,
      );
    }
    await delay(100);
  }
}

/** État de la ligne « Model location » : champ, Browse, messages. */
const row = (driver) =>
  driver.execute(`
    const label = [...document.querySelectorAll("label")]
      .find((l) => l.textContent === "Model location");
    if (!label) return null;
    const input = document.getElementById(label.htmlFor);
    const root = input.parentElement.parentElement;
    const browse = input.parentElement.querySelector("button");
    return {
      value: input.value,
      readOnly: input.readOnly,
      browse: browse.textContent.trim(),
      browseDisabled: browse.disabled,
      lines: [...root.querySelectorAll("p")].map((p) => p.textContent),
      alert: root.querySelector("[role=alert]") !== null,
      commands: root.querySelector("code")?.textContent ?? null,
      details: root.querySelector("details pre")?.textContent ?? null,
      progressbar: root.querySelector("[role=progressbar]") !== null,
    };`);

/** Attend le champ chargé (non vide). */
async function waitRow(driver) {
  let last;
  await waitUntil(
    async () => {
      last = await row(driver);
      return last && last.value !== "" ? last : null;
    },
    () => `ligne Model location : ${JSON.stringify(last)}`,
  );
  return last;
}

/**
 * Une longue commande défile dans son bloc : la page ne s'élargit pas et
 * Browse reste dans la fenêtre.
 */
async function assertFits(driver) {
  const layout = await driver.execute(`
    const page = document.querySelector("[data-testid=settings] > div");
    const label = [...document.querySelectorAll("label")]
      .find((l) => l.textContent === "Model location");
    const input = document.getElementById(label.htmlFor);
    const code = input.parentElement.parentElement.querySelector("code");
    return {
      page: [page.scrollWidth, page.clientWidth],
      browse: input.parentElement.querySelector("button").getBoundingClientRect().right,
      window: innerWidth,
      code: [code.scrollWidth, code.clientWidth],
    };`);
  assert.ok(
    layout.page[0] <= layout.page[1] &&
      layout.browse <= layout.window &&
      layout.code[0] > layout.code[1],
    `commande hors de son bloc : ${JSON.stringify(layout)}`,
  );
}

const clickBrowse = (driver) =>
  driver.execute(`
    const label = [...document.querySelectorAll("label")]
      .find((l) => l.textContent === "Model location");
    document.getElementById(label.htmlFor).parentElement.querySelector("button").click();`);

/** Boîte de confirmation : titre, lignes, boutons ; null si absente. */
const dialog = (driver) =>
  driver.execute(`
    const box = document.querySelector("[role=alertdialog]");
    if (!box) return null;
    return {
      title: box.querySelector("h2")?.textContent ?? null,
      lines: [...box.querySelectorAll("p")].map((p) => p.textContent),
      buttons: [...box.querySelectorAll("button")].map((b) => b.textContent),
      focused: document.activeElement?.textContent ?? null,
    };`);

async function clickDialog(driver, label) {
  const found = await driver.execute(
    `const button = [...document.querySelectorAll("[role=alertdialog] button")]
       .find((b) => b.textContent === arguments[0]);
     button?.click();
     return button !== undefined;`,
    [label],
  );
  assert.ok(found, `bouton ${label}`);
  await waitUntil(
    async () => (await dialog(driver)) === null,
    "boîte de confirmation fermée",
  );
}

const savedShown = (driver) =>
  driver.execute(
    `return [...document.querySelectorAll("[role=status]")]
       .some((e) => e.textContent === "Saved")`,
  );

/**
 * Note dans la page les textes de progression de la ligne et la barre,
 * pendant le changement.
 */
const WATCH_PROGRESS = `
  window.__moves?.disconnect();
  window.__moveTexts = [];
  window.__moveBars = [];
  const note = () => {
    // Seule l'étape est annoncée (aria-live) ; le paragraphe qui la contient
    // ajoute les octets.
    for (const live of document.querySelectorAll("[data-testid=settings] [aria-live=polite]")) {
      const line = live.closest("p");
      if (line && !window.__moveTexts.includes(line.textContent)) window.__moveTexts.push(line.textContent);
    }
    const bar = document.querySelector("[role=progressbar][aria-label='Moving models']");
    if (bar) {
      const value = bar.getAttribute("aria-valuetext");
      if (!window.__moveBars.includes(value)) window.__moveBars.push(value);
    }
  };
  window.__moves = new MutationObserver(note);
  window.__moves.observe(document.body,
    { subtree: true, childList: true, attributes: true, characterData: true });`;

const violations = [];
const consoleErrors = [];
const consoleWarnings = [];
const WATCH_PAGE = `
  if (!window.__csp) {
    window.__csp = [];
    window.__errors = [];
    window.__warnings = [];
    document.addEventListener("securitypolicyviolation", (event) =>
      window.__csp.push(event.violatedDirective + " " + event.blockedURI));
    const error = console.error;
    console.error = (...args) => {
      window.__errors.push(args.map(String).join(" "));
      error.apply(console, args);
    };
    const warn = console.warn;
    console.warn = (...args) => {
      window.__warnings.push(args.map(String).join(" "));
      warn.apply(console, args);
    };
  }`;

async function collectViolations(driver) {
  const found = await driver.execute(
    `return { csp: window.__csp ?? [], errors: window.__errors ?? [],
       warnings: window.__warnings ?? [] };`,
  );
  violations.push(...found.csp);
  consoleErrors.push(...found.errors);
  consoleWarnings.push(...found.warnings);
}

/** Lance l'app sur la page Settings, relève la page à la fin. */
async function launch(name, env, fn) {
  await withApp(binary, { name, env }, async (driver) => {
    try {
      await driver.waitFor("[data-testid=new-chat]", { timeoutMs: 20000 });
      await driver.execute(WATCH_PAGE);
      if (
        await driver.execute(
          "return document.querySelector(\"button[aria-label='Show sidebar']\") !== null",
        )
      ) {
        await driver.click(
          await driver.find("button[aria-label='Show sidebar']"),
        );
      }
      await driver.click(await driver.waitFor("nav a[href$='/settings']"));
      await driver.waitFor("[data-testid=settings]");
      await fn(driver);
      await collectViolations(driver);
    } catch (error) {
      await driver.screenshot(`${SHOTS}/failure-${name}.png`).catch(() => {});
      throw error;
    }
  });
}

async function shoot(driver, name) {
  await delay(500);
  await driver.screenshot(`${SHOTS}/${name}.png`);
}

try {
  // Succès : déplacement par copie (CHATONLM_TEST_FORCE_COPY), progression,
  // Saved, champ mis à jour.
  const done = makeCase("done");
  await launch(
    "models-dir",
    done.system({ CHATONLM_TEST_FORCE_COPY: "1" }),
    async (driver) => {
      // Captures en clair (le système de la session est sombre).
      const light = await driver.execute(
        `const label = [...document.querySelectorAll("label")]
           .find((l) => l.textContent.trim() === "Light");
         label?.click();
         return label !== undefined;`,
      );
      assert.ok(light);
      await waitUntil(
        () =>
          driver.execute(
            "return !document.documentElement.classList.contains('dark')",
          ),
        "thème clair",
      );
      let state = await waitRow(driver);
      assert.equal(state.value, done.old);
      assert.equal(state.readOnly, true);
      assert.equal(state.browse, "Browse");
      assert.equal(state.browseDisabled, false);
      assert.deepEqual(state.lines, []);
      await waitUntil(async () => !(await savedShown(driver)), "Saved retiré");
      await shoot(driver, "row");

      await clickBrowse(driver);
      let box;
      await waitUntil(
        async () => (box = await dialog(driver)),
        "boîte de confirmation",
      );
      assert.equal(box.title, "Move models to the new location?");
      assert.equal(
        box.lines[0],
        `Ollama will restart and store new models in ${done.target}.`,
      );
      assert.match(
        box.lines[1],
        new RegExp(
          `^Your 2 models \\(\\d+(\\.\\d)? GB\\) are in ${done.old.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}\\.$`,
        ),
      );
      assert.equal(box.lines.length, 2);
      assert.deepEqual(box.buttons, ["Cancel", "Leave them", "Move models"]);
      assert.equal(box.focused, "Move models");
      // Rien n'est lancé avant le choix.
      assert.equal(done.log("pkexec"), "");
      await shoot(driver, "confirm");

      await driver.execute(WATCH_PROGRESS);
      await clickDialog(driver, "Move models");
      await waitUntil(() => savedShown(driver), "badge Saved", 20000);
      state = await waitRow(driver);
      await waitUntil(
        async () => (state = await row(driver)).value === done.target,
        () => `champ mis à jour : ${state.value}`,
      );
      assert.deepEqual(state.lines, [], "message après un succès");
      const texts = await driver.execute("return window.__moveTexts");
      const bars = await driver.execute("return window.__moveBars");
      assert.ok(
        texts.some((t) => /^Moving models…( .* of .*)?$/.test(t)),
        `progression : ${texts.join(" / ")}`,
      );
      assert.ok(
        texts.includes("Restarting Ollama…"),
        `redémarrage : ${texts.join(" / ")}`,
      );
      assert.ok(
        bars.some((b) => /^Moving models… \d+ B of \d+ B$/.test(b)),
        `barre : ${bars.join(" / ")}`,
      );
      await shoot(driver, "saved");

      // Ce que le script a fait (racine simulée).
      assert.deepEqual(
        filesOf(done.target).filter((f) => !f.endsWith("-partial")),
        MODEL_FILES.filter((f) => !f.endsWith("-partial")),
      );
      assert.deepEqual(
        filesOf(done.old).filter((f) => !f.endsWith("-partial")),
        [],
        "anciennes copies gardées",
      );
      const dropin = readFileSync(join(done.root, DROPIN), "utf8");
      assert.ok(
        dropin.includes(`Environment="OLLAMA_MODELS=${done.target}"`),
        dropin,
      );
      const calls = done
        .log("systemctl")
        .split("\n")
        .filter((line) => /^(stop|start|daemon-reload)/.test(line));
      assert.deepEqual(calls, [
        "stop -- ollama.service",
        "daemon-reload",
        "start -- ollama.service",
      ]);
      assert.match(done.log("pkexec"), /^--disable-internal-agent /);
    },
  );

  // Annulation dans l'invite : rien.
  const cancel = makeCase("cancel");
  await launch(
    "models-dir-cancel",
    cancel.system({ FAKE_PKEXEC: "cancel" }),
    async (driver) => {
      await waitRow(driver);
      await clickBrowse(driver);
      await waitUntil(() => dialog(driver), "boîte de confirmation");
      await clickDialog(driver, "Move models");
      await waitUntil(
        async () => cancel.log("pkexec") !== "",
        "faux pkexec lancé",
      );
      await waitUntil(
        async () => !(await row(driver)).browseDisabled,
        "fin de l'opération",
      );
      await delay(500);
      const state = await row(driver);
      assert.equal(state.value, cancel.old);
      assert.deepEqual(state.lines, []);
      assert.equal(await savedShown(driver), false);
      assert.equal(cancel.log("systemctl"), "", "systemctl appelé");
      assert.equal(existsSync(cancel.target), false);
      assert.equal(existsSync(join(cancel.root, DROPIN)), false);
      assert.deepEqual(filesOf(cancel.old), MODEL_FILES);
    },
  );

  // Pas d'agent d'authentification : commandes sudo à copier.
  const noAgent = makeCase("no agent");
  await launch(
    "models-dir-noagent",
    noAgent.system({ FAKE_PKEXEC: "noagent" }),
    async (driver) => {
      await waitRow(driver);
      await clickBrowse(driver);
      await waitUntil(() => dialog(driver), "boîte de confirmation");
      await clickDialog(driver, "Leave them");
      let state;
      await waitUntil(
        async () => (state = await row(driver)).alert,
        "message d'erreur",
      );
      assert.equal(
        state.lines[0],
        "No password prompt is available in this session. Run these commands in a terminal:",
      );
      const quoted = `'${noAgent.target}'`;
      assert.ok(
        state.commands.startsWith(
          `sudo mkdir -p ${quoted} && sudo chown ${USER}:${GROUP} ${quoted}\n`,
        ),
        state.commands,
      );
      assert.match(
        state.commands,
        /sudo tee \/etc\/systemd\/system\/ollama\.service\.d\/zz-chatonlm-models\.conf > \/dev\/null\nsudo systemctl daemon-reload && sudo systemctl restart ollama\.service$/,
      );
      assert.match(state.details, /No authentication agent found/);
      assert.equal(state.value, noAgent.old);
      assert.equal(noAgent.log("systemctl"), "");
      await driver.execute(
        "document.querySelector('[data-testid=settings] details').open = true",
      );
      await assertFits(driver);
      await shoot(driver, "no-agent");
    },
  );

  // Ollama ne démarre pas avec le nouveau dossier : tout est remis.
  const crash = makeCase("crash");
  await launch(
    "models-dir-failed",
    crash.system({ FAKE_FAIL: "crash-new" }),
    async (driver) => {
      await waitRow(driver);
      await clickBrowse(driver);
      await waitUntil(() => dialog(driver), "boîte de confirmation");
      await clickDialog(driver, "Move models");
      let state;
      await waitUntil(
        async () => (state = await row(driver)).alert,
        "message d'erreur",
        20000,
      );
      assert.deepEqual(state.lines, [
        "Ollama did not start with the new location, so the previous one was restored. See: journalctl -u ollama",
      ]);
      assert.equal(state.commands, null);
      assert.match(state.details, /^chatonlm: /m);
      assert.equal(await savedShown(driver), false);
      await waitUntil(
        async () => (await row(driver)).value === crash.old,
        "champ revenu à l'ancien dossier",
      );
      assert.deepEqual(filesOf(crash.old), MODEL_FILES);
      assert.equal(existsSync(join(crash.root, DROPIN)), false);
      await driver.execute(
        "document.querySelector('[data-testid=settings] details').open = true",
      );
      await shoot(driver, "failed");
    },
  );

  // Dossier .ollama du service caché par ProtectHome=yes : Fix.
  const home = makeCase("ollama home");
  const serviceHome = join(home.dir, "home", "théo");
  const ollamaDir = join(serviceHome, ".ollama");
  mkdirSync(serviceHome, { recursive: true });
  writeFileSync(join(home.root, "state/home"), `${serviceHome}\n`);
  writeFileSync(join(home.root, "state/loaded_protect_home"), "yes\n");
  await launch(
    "models-dir-home",
    home.env(
      `launch=system;dir=${home.old};user=${USER};group=${GROUP};running=yes;protect_home=@${join(home.root, "state/loaded_protect_home")};protect_system=full;ollama_dir=${ollamaDir};binds=@${join(home.root, "state/loaded_binds")}`,
      { CHATONLM_TEST_HOME: join(home.dir, "home") },
    ),
    async (driver) => {
      const box = () =>
        driver.execute(`
          const box = document.querySelector("[data-testid=ollama-home]");
          if (!box) return null;
          return {
            text: box.querySelector("[role=alert]")?.textContent ?? null,
            button: box.querySelector("button")?.textContent ?? null,
          };`);
      let state;
      await waitUntil(async () => (state = await box()), "alerte .ollama");
      assert.equal(
        state.text,
        `Ollama can't read its own files in ${ollamaDir}: its systemd service hides /home (ProtectHome). Downloads from ollama.com and your Ollama account won't work until this is fixed.`,
      );
      assert.equal(state.button, "Fix");
      await shoot(driver, "ollama-home");
      await driver.execute(
        `document.querySelector("[data-testid=ollama-home] button").click();`,
      );
      await waitUntil(
        async () =>
          (state = await box()) === null && (await savedShown(driver)),
        () => `alerte retirée et Saved : ${JSON.stringify(state)}`,
      );
      assert.equal(
        readFileSync(
          join(
            home.root,
            "etc/systemd/system/ollama.service.d/zz-chatonlm-home.conf",
          ),
          "utf8",
        ),
        `# Written by ChatonLM (Settings > General): Ollama's own folder.\n[Service]\nProtectHome=tmpfs\nBindPaths="${ollamaDir}"\n`,
      );
      assert.ok(existsSync(ollamaDir), ".ollama créé");
      assert.deepEqual(
        home
          .log("systemctl")
          .split("\n")
          .filter((line) => /^(daemon-reload|restart|stop|start)/.test(line)),
        ["daemon-reload", "restart -- ollama.service"],
      );
      assert.ok(
        home
          .log("pkexec")
          .trimEnd()
          .endsWith(` chatonlm-home ollama.service ${ollamaDir}`),
        home.log("pkexec").slice(-300),
      );
      await shoot(driver, "ollama-home-fixed");
    },
  );

  // Lancement à la main : commande à copier, presse-papiers.
  const manual = makeCase("manual");
  await launch(
    "models-dir-manual",
    manual.env(`launch=manual;dir=${manual.old};host_env=127.0.0.1:11435`),
    async (driver) => {
      let state = await waitRow(driver);
      assert.equal(state.value, manual.old);
      assert.equal(state.browseDisabled, false);
      await clickBrowse(driver);
      await waitUntil(
        async () => (state = await row(driver)).commands !== null,
        "commande à copier",
      );
      assert.equal(await dialog(driver), null);
      assert.equal(state.alert, false);
      assert.deepEqual(state.lines, [
        "Ollama was started from a terminal. Stop it, then start it again with:",
      ]);
      const command = `OLLAMA_HOST=127.0.0.1:11435 OLLAMA_MODELS='${manual.target}' ollama serve`;
      assert.equal(state.commands, command);
      await driver.click(
        await driver.find("button[aria-label='Copy command to clipboard']"),
      );
      await waitUntil(
        async () =>
          (await driver.execute(
            "return document.querySelector(\"button[aria-label='Copy command to clipboard']\").textContent",
          )) === "Copied",
        "Copied",
      );
      assert.equal((await readClipboard()).text, command);
      assert.equal(manual.log("pkexec"), "", "pkexec lancé");
      await assertFits(driver);
      await shoot(driver, "manual");
    },
  );

  // Serveur distant : rien à faire d'ici.
  const remote = makeCase("remote");
  await launch(
    "models-dir-remote",
    remote.env("launch=remote;host=192.168.1.20:11434"),
    async (driver) => {
      const state = await waitRow(driver);
      assert.equal(state.value, "On 192.168.1.20:11434");
      assert.equal(state.browseDisabled, true);
      assert.deepEqual(state.lines, [
        "Ollama runs on 192.168.1.20:11434. Change the model location on that computer.",
      ]);
      assert.equal(state.alert, false);
      await shoot(driver, "remote");
    },
  );

  // Téléchargement du ModelHub en cours : refusé sans invite.
  const busy = makeCase("downloads");
  await launch(
    "models-dir-downloads",
    {
      ...busy.system(),
      CHATONLM_HF_URL: fake.url,
      CHATONLM_HF_STATUS_URL: fake.url,
    },
    async (driver) => {
      await waitRow(driver);
      const model = "hf.co/chatonlm/Tiny-Stall-GGUF:Q4_K_M";
      await driver.invoke("hub_download", {
        request: { model, repo: "chatonlm/Tiny-Stall-GGUF", label: "Q4_K_M" },
      });
      await waitUntil(
        async () =>
          (await driver.invoke("hub_downloads")).some(
            (d) => d.model === model && d.state === "downloading",
          ),
        "téléchargement en cours",
      );
      await clickBrowse(driver);
      let state;
      await waitUntil(async () => (state = await row(driver)).alert, "refus");
      assert.deepEqual(state.lines, [
        "Wait for the model downloads to finish, then try again.",
      ]);
      assert.equal(await dialog(driver), null);
      assert.equal(busy.log("pkexec"), "");
      await shoot(driver, "downloads");
      // Arrêté : la confirmation revient.
      await driver.invoke("hub_cancel_download", { model });
      await clickBrowse(driver);
      await waitUntil(() => dialog(driver), "boîte de confirmation");
      await clickDialog(driver, "Cancel");
      assert.deepEqual((await row(driver)).lines, []);
      assert.equal(busy.log("pkexec"), "");
    },
  );

  assert.deepEqual(violations, [], "violations de la CSP");
  assert.deepEqual(consoleErrors, [], "erreurs dans la console");
  assert.deepEqual(consoleWarnings, [], "avertissements dans la console");
} finally {
  await fake.stop();
}
