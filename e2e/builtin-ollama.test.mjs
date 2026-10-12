// Ollama intégré dans l'app réelle (mode hybride, étape 18b), avec le vrai
// binaire de l'archive officielle (scripts/fetch-ollama.sh, donné par
// CHATONLM_OLLAMA_BIN), jamais l'Ollama du système ni l'instance de test :
// - adresse par défaut fermée (CHATONLM_E2E_DEFAULT_URL), ni binaire ni unité
//   d'Ollama (CHATONLM_E2E_HOST_FACTS) : l'intégré démarre seul, sans
//   « Install Ollama » à l'écran ; il écoute sur un port libre de 127.0.0.1
//   (ni 11434 ni 11435), avec HOME et dossier des modèles dans les données de
//   l'essai, et c'est un descendant de l'app ;
// - modèle de test (gemma3:270m) tiré par l'app au premier message : ses
//   couches sont posées par liens physiques depuis .tools/ollama-test une
//   fois le serveur prêt (le ménage d'Ollama au démarrage les effacerait),
//   pour éviter 300 Mo de téléchargement ; le manifeste vient quand même du
//   registre ollama.com (réseau) ;
// - une réponse du vrai modèle, enregistrée dans l'historique ;
// - fermeture (Ctrl+Q, commande quit_app) : aucun processus de l'intégré
//   restant (serveur et runners), rien dans le HOME de la session
//   (~/.ollama), clé dans <données>/ollama/home, journal
//   <données>/logs/ollama.log présent.
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import {
  existsSync,
  linkSync,
  mkdirSync,
  readdirSync,
  readFileSync,
  readlinkSync,
  realpathSync,
  statSync,
} from "node:fs";
import path from "node:path";
import { setTimeout as delay } from "node:timers/promises";
import { ROOT, withApp } from "./lib/app.mjs";
import { chordsOf, pressKeys } from "./lib/keyboard.mjs";
import { PENDING_CALLS } from "./lib/page.mjs";

const binary = process.argv[2];
const MODEL = "gemma3:270m";
const TEST_MODELS = path.join(ROOT, ".tools/ollama-test/models");
const MANIFEST = path.join(
  TEST_MODELS,
  "manifests/registry.ollama.ai/library/gemma3/270m",
);

// Binaire intégré épinglé (déjà extrait : rien n'est retéléchargé).
const ollamaBin = execFileSync(path.join(ROOT, "scripts/fetch-ollama.sh"), {
  encoding: "utf8",
}).trim();
const ollamaRoot = path.dirname(path.dirname(realpathSync(ollamaBin)));
if (!existsSync(MANIFEST)) {
  throw new Error(
    `${MODEL} absent de .tools/ollama-test : scripts/test-ollama.sh pull ${MODEL}`,
  );
}
const manifest = JSON.parse(readFileSync(MANIFEST, "utf8"));
const blobs = [manifest.config, ...manifest.layers].map((layer) =>
  layer.digest.replace(":", "-"),
);

const dataDir = process.env.CHATONLM_DATA_DIR;
if (!dataDir || process.env.CHATONLM_E2E_SESSION !== "1") {
  throw new Error(
    "À lancer dans la session virtuelle (scripts/e2e-session.sh).",
  );
}
const sessionHome = process.env.HOME;

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
    await delay(200);
  }
}

/** Processus dont l'exécutable est un fichier de l'Ollama intégré. */
function builtinProcesses() {
  const found = [];
  for (const entry of readdirSync("/proc")) {
    if (!/^\d+$/.test(entry)) continue;
    try {
      const exe = readlinkSync(`/proc/${entry}/exe`);
      if (exe.startsWith(`${ollamaRoot}/`)) found.push(Number(entry));
    } catch {
      // Terminé entre-temps, ou processus d'un autre utilisateur.
    }
  }
  return found;
}

/** Parent d'un processus (d'après /proc/<pid>/stat). */
function parentOf(pid) {
  const stat = readFileSync(`/proc/${pid}/stat`, "utf8");
  return Number(stat.slice(stat.lastIndexOf(")") + 2).split(" ")[1]);
}

function isDescendant(pid, ancestor) {
  for (let current = pid; current > 1; current = parentOf(current)) {
    if (current === ancestor) return true;
  }
  return false;
}

/** Variables d'environnement d'un processus. */
function environ(pid) {
  return Object.fromEntries(
    readFileSync(`/proc/${pid}/environ`, "utf8")
      .split("\0")
      .filter(Boolean)
      .map((line) => [
        line.slice(0, line.indexOf("=")),
        line.slice(line.indexOf("=") + 1),
      ]),
  );
}

const isAlive = (pid) => {
  try {
    process.kill(pid, 0);
    return true;
  } catch {
    return false;
  }
};

const exists = (driver, css) =>
  driver.execute("return document.querySelector(arguments[0]) !== null", [css]);

const replies = (driver) =>
  driver.execute(
    "return [...document.querySelectorAll('[data-role=assistant]')].map((e) => e.textContent)",
  );

// « Install Ollama » ne doit jamais s'afficher : noté par la page (écouteur
// reposé après le rechargement).
const WATCH_INSTALL = `
  window.__installSeen = false;
  const note = () => {
    if (document.body.textContent.includes("Install Ollama")) window.__installSeen = true;
  };
  note();
  new MutationObserver(note).observe(document.body,
    { subtree: true, childList: true, characterData: true });`;
let installSeen = false;

/**
 * Choisit le modèle (absent du serveur) par le réglage, puis recharge la page
 * quand aucun appel au backend n'est en cours (comme download.test.mjs).
 */
async function useModel(driver, model) {
  await driver.invoke("update_settings", { patch: { selectedModel: model } });
  await waitUntil(
    () =>
      driver.execute(
        `${PENDING_CALLS}
         if (pending.length > 0) return false;
         window.__oldPage = true;
         location.reload();
         return true;`,
      ),
    "appels au backend toujours en cours avant le rechargement",
  );
  await waitUntil(
    async () => !(await driver.execute("return window.__oldPage === true")),
    "rechargement",
  );
  await driver.execute(WATCH_INSTALL);
  await waitUntil(
    async () =>
      (await driver.execute(
        "return document.querySelector('[title=\"Select model\"]')?.textContent ?? null",
      )) === model,
    `modèle ${model} affiché`,
    30_000,
  );
}

// Aucun processus de l'intégré avant l'essai (reste d'un essai précédent).
assert.deepEqual(builtinProcesses(), [], "Ollama intégré déjà lancé");

const seen = new Set();
let runtime = null;

await withApp(
  binary,
  {
    name: "builtin-ollama",
    env: {
      CHATONLM_OLLAMA_BIN: ollamaBin,
      CHATONLM_E2E_DEFAULT_URL: "http://127.0.0.1:1",
      CHATONLM_E2E_HOST_FACTS: "binary=no;unit=none",
    },
    quit: "command",
  },
  async (driver, app) => {
    await driver.execute(WATCH_INSTALL);

    // L'intégré démarre seul et sert.
    runtime = await waitUntil(
      async () => {
        const value = await driver.invoke("ollama_runtime");
        return value.kind === "builtin" && value.builtinState === "running"
          ? value
          : null;
      },
      "Ollama intégré prêt",
      60_000,
    );
    assert.equal(runtime.source, "auto");
    assert.equal(runtime.builtinAvailable, true);
    assert.equal(
      runtime.builtinVersion,
      readFileSync(path.join(ollamaRoot, "VERSION"), "utf8").trim(),
    );
    const url = new URL(runtime.url);
    assert.equal(url.hostname, "127.0.0.1");
    assert.ok(!["11434", "11435"].includes(url.port), `port ${url.port}`);
    const version = await (await fetch(`${runtime.url}/api/version`)).json();
    assert.equal(version.version, runtime.builtinVersion);

    // Le serveur : un seul « ollama serve », enfant de l'app, avec son HOME
    // et ses modèles dans les données de l'essai.
    const servers = builtinProcesses();
    for (const pid of servers) seen.add(pid);
    assert.equal(servers.length, 1, `processus : ${servers.join(", ")}`);
    const [server] = servers;
    assert.ok(isDescendant(server, app.pid), "serveur hors de l'app");
    const env = environ(server);
    assert.equal(env.OLLAMA_HOST, url.host);
    assert.equal(env.HOME, path.join(dataDir, "ollama/home"));
    assert.equal(env.OLLAMA_MODELS, runtime.builtinModelsDir);
    assert.ok(runtime.builtinModelsDir.startsWith(`${dataDir}/`));

    // Couches du modèle posées par liens physiques (après le ménage du
    // démarrage), sans manifeste : le modèle reste à tirer.
    const blobDir = path.join(runtime.builtinModelsDir, "blobs");
    mkdirSync(blobDir, { recursive: true });
    for (const blob of blobs) {
      linkSync(path.join(TEST_MODELS, "blobs", blob), path.join(blobDir, blob));
    }
    const tags = await (await fetch(`${runtime.url}/api/tags`)).json();
    assert.deepEqual(tags.models, [], "aucun modèle avant le pull");

    // Premier message : le modèle est tiré (registre), puis répond.
    await driver.waitFor("[data-testid=new-chat]", { timeoutMs: 30_000 });
    installSeen ||= await driver.execute("return window.__installSeen");
    await useModel(driver, MODEL);
    await driver.click(await driver.waitFor("textarea[aria-label='Message']"));
    await pressKeys([...chordsOf("Say hello in one short sentence."), "enter"]);
    let since = null;
    let shown = "";
    await waitUntil(
      async () => {
        for (const pid of builtinProcesses()) seen.add(pid);
        shown = await driver.execute(
          "return document.querySelector('main')?.innerText ?? document.body.innerText",
        );
        const done =
          !(await exists(driver, "button[aria-label='Stop']")) &&
          (await replies(driver)).length === 1;
        if (done) since ??= Date.now();
        else since = null;
        return since !== null && Date.now() - since > 1000;
      },
      () =>
        `réponse du modèle tiré ; page : ${JSON.stringify(shown.slice(0, 600))}`,
      240_000,
    ).catch(async (error) => {
      await driver.screenshot(".tools/shots/builtin-ollama-failure.png");
      throw error;
    });
    const texts = await replies(driver);
    assert.ok(texts[0].trim().length > 0, "réponse vide");
    assert.equal(
      await exists(driver, "[data-testid=chat] [role=alert]"),
      false,
      "erreur affichée dans la conversation",
    );
    const installed = await (await fetch(`${runtime.url}/api/tags`)).json();
    assert.ok(
      installed.models.some((m) => m.name === MODEL),
      `${MODEL} tiré : ${installed.models.map((m) => m.name).join(", ")}`,
    );
    const id = (await driver.invoke("list_chats"))[0].id;
    const chat = await driver.invoke("get_chat", { id });
    assert.deepEqual(
      chat.messages.map((m) => m.role),
      ["user", "assistant"],
    );
    assert.equal(chat.messages[1].model, MODEL);
    installSeen ||= await driver.execute("return window.__installSeen");
    assert.equal(installSeen, false, "« Install Ollama » affiché");
    // Le runner du modèle (llama-server ou serveur lui-même) a été vu.
    for (const pid of builtinProcesses()) seen.add(pid);
    await driver.screenshot(".tools/shots/builtin-ollama.png");

    // Ctrl+Q : l'app arrête l'intégré avant de se terminer.
    await driver
      .execute("window.__TAURI_INTERNALS__.invoke('quit_app'); return true")
      .catch(() => {});
    await waitUntil(
      () => Promise.race([app.exited.then(() => true), delay(100)]),
      "fin de l'app",
      20_000,
    );
    // Un instant pour les runners tués avec le groupe.
    await waitUntil(
      () => [...seen].every((pid) => !isAlive(pid)),
      () =>
        `processus de l'intégré encore actifs : ${[...seen].filter(isAlive).join(", ")}`,
      2_000,
    );
  },
);

assert.deepEqual(builtinProcesses(), [], "Ollama intégré resté actif");
assert.ok(seen.size >= 1);
assert.equal(
  existsSync(path.join(sessionHome, ".ollama")),
  false,
  "~/.ollama créé dans le HOME de la session",
);
assert.ok(
  existsSync(path.join(dataDir, "ollama/home/.ollama/id_ed25519")),
  "clé de l'intégré dans ses données",
);
assert.equal(runtime.logPath, path.join(dataDir, "logs/ollama.log"));
assert.ok(statSync(runtime.logPath).size > 0, "journal vide");
assert.match(
  readFileSync(runtime.logPath, "utf8"),
  new RegExp(
    `Listening on ${new URL(runtime.url).host.replaceAll(".", "\\.")}`,
  ),
);
console.log(
  `  Ollama intégré ${runtime.builtinVersion} sur ${runtime.url}, ` +
    `${seen.size} processus vus, tous arrêtés.`,
);
