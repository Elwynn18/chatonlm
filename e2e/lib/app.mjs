// Lancement de l'app réelle pour un test : environnement choisi par le test,
// serveur WebDriver sur un port libre, journal dans .tools/logs/.
import { spawn } from "node:child_process";
import { openSync, readFileSync, readdirSync } from "node:fs";
import { createServer } from "node:net";
import { fileURLToPath } from "node:url";
import { WebDriver } from "./webdriver.mjs";

/** Racine du dépôt. */
export const ROOT = fileURLToPath(new URL("../../", import.meta.url));

/** Variables de l'environnement du développeur à ne pas transmettre. */
const DROPPED_ENV = [
  "OLLAMA_HOST",
  "CHATONLM_SEED_DEMO",
  "CHATONLM_E2E_HOST_FACTS",
  // Ollama intégré (binaire de .tools/runtimes) et adresse par défaut
  // remplacée : un test qui s'en sert les donne lui-même.
  "CHATONLM_OLLAMA_BIN",
  "CHATONLM_E2E_DEFAULT_URL",
  "CHATONLM_REGISTRY_URL",
  "CHATONLM_OLLAMA_COM_URL",
  "CHATONLM_HF_URL",
  "CHATONLM_HF_STATUS_URL",
  "CHATONLM_E2E_HARDWARE",
  "CHATONLM_E2E_MICROPHONE",
  "CHATONLM_E2E_SETUP_DELAY_MS",
  "CHATONLM_SPEECH_MODELS_URL",
  "CHATONLM_VOICE_MODELS_URL",
  "CHATONLM_E2E_AUDIO_OUT",
  // Serveur des mises à jour (builds e2e) : un test qui s'en sert le donne
  // lui-même.
  "CHATONLM_UPDATE_URL",
  // Mode conversation vocale : faux micro et mesures, donnés par un test qui
  // s'en sert.
  "CHATONLM_E2E_VOICE_MIC",
  "CHATONLM_VOICE_TIMINGS",
];

/**
 * Machine du serveur imposée par défaut (ModelHub : pastilles de place,
 * rubrique « Best for your machine ») : un GPU de 24 Gio, 32 Gio de RAM, pour
 * des essais identiques sur toute machine (sans lancer nvidia-smi). Un test la
 * remplace par `env.CHATONLM_E2E_HARDWARE` (JSON d'un `HubHardware` sans
 * `context`, ou `none` : serveur jugé distant).
 */
export const E2E_HARDWARE = JSON.stringify({
  gpus: [
    {
      name: "NVIDIA GeForce RTX 3090",
      vramTotal: 24 * 2 ** 30,
      vramFree: 22 * 2 ** 30,
    },
  ],
  ramTotal: 32 * 2 ** 30,
  ramAvailable: 24 * 2 ** 30,
});

/** Un port TCP libre sur 127.0.0.1 (rien n'y écoute au retour). */
export function freePort() {
  return new Promise((resolve, reject) => {
    const server = createServer();
    server.once("error", reject);
    server.listen(0, "127.0.0.1", () => {
      const { port } = server.address();
      server.close(() => resolve(port));
    });
  });
}

const delay = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

/** Vrai si `promise` se termine dans le délai. */
const settlesWithin = (promise, ms) =>
  Promise.race([promise.then(() => true), delay(ms).then(() => false)]);

/** Descendants d'un processus (processus de WebKit), d'après /proc. */
function descendantsOf(root) {
  const children = new Map();
  for (const entry of readdirSync("/proc")) {
    if (!/^\d+$/.test(entry)) continue;
    try {
      const stat = readFileSync(`/proc/${entry}/stat`, "utf8");
      // Le nom du programme, entre parenthèses, peut contenir des espaces.
      const ppid = Number(stat.slice(stat.lastIndexOf(")") + 2).split(" ")[1]);
      if (!children.has(ppid)) children.set(ppid, []);
      children.get(ppid).push(Number(entry));
    } catch {
      // Processus terminé entre-temps.
    }
  }
  const found = [];
  const pending = [root];
  while (pending.length > 0) {
    for (const child of children.get(pending.pop()) ?? []) {
      found.push(child);
      pending.push(child);
    }
  }
  return found;
}

const isAlive = (pid) => {
  try {
    process.kill(pid, 0);
    return true;
  } catch {
    return false;
  }
};

/**
 * Ferme l'app comme un utilisateur (fermeture de la fenêtre, ou commande
 * `quit_app` comme Ctrl+Q si `quit` vaut "command"), avec SIGTERM puis SIGKILL
 * en dernier recours. Ses processus WebKit sont ensuite attendus, puis tués
 * s'ils traînent : arrêtée brutalement, l'app peut laisser un WebKitWebProcess
 * dont le thread de rendu GPU ne finit pas (NVIDIA).
 */
async function shutDown(app, exited, driver, quit, name) {
  const helpers = descendantsOf(app.pid);
  if (quit === "command") {
    // Sans attendre la réponse : l'app se termine pendant l'appel.
    await driver
      .execute("window.__TAURI_INTERNALS__.invoke('quit_app'); return true")
      .catch(() => {});
  } else {
    // Le plugin WebDriver détruit la fenêtre (sans CloseRequested) : l'app
    // quitte même quand une zone de notification la garderait.
    await driver.closeWindow().catch(() => {});
  }
  if (!(await settlesWithin(exited, 5000))) {
    app.kill("SIGTERM");
    if (!(await settlesWithin(exited, 3000))) {
      app.kill("SIGKILL");
      await exited;
    }
  }
  for (let i = 0; i < 30 && helpers.some(isAlive); i++) await delay(100);
  for (const pid of helpers.filter(isAlive)) {
    let program = "?";
    try {
      program = readFileSync(`/proc/${pid}/cmdline`, "utf8").split("\0")[0];
    } catch {
      // Terminé entre-temps.
    }
    console.error(
      `Processus de l'app encore actif 3 s après sa fin, tué : ${pid} (${program}, ${name})`,
    );
    try {
      process.kill(pid, "SIGKILL");
    } catch {
      // Déjà terminé.
    }
  }
}

/**
 * Lance l'app, ouvre une session WebDriver, exécute `fn(driver, app)`, puis
 * ferme tout. Échoue si la fenêtre a été affichée par le filet de secours du
 * backend plutôt que par le frontend. Options : `args` (arguments de l'app,
 * ex. ["--dictate"]), `quit` ("window" par défaut, ou "command" : fermeture
 * par la commande `quit_app`, comme Ctrl+Q ou « Quit ChatonLM » du menu de la
 * zone de notification). `app` donne `exited` (promesse du code de sortie)
 * et `pid`, pour un test qui fait quitter l'app lui-même.
 */
export async function withApp(
  binary,
  { name, env = {}, args = [], quit = "window" },
  fn,
) {
  const log = `${ROOT}.tools/logs/e2e-${name}.log`;
  const port = await freePort();
  const output = openSync(log, "w");
  const inherited = { ...process.env };
  for (const key of DROPPED_ENV) delete inherited[key];
  const app = spawn(binary, args, {
    env: {
      ...inherited,
      // Adresse locale standard fermée : un Ollama de la machine n'est jamais
      // proposé comme adresse de remplacement.
      CHATONLM_E2E_STANDARD_URL: "http://127.0.0.1:1",
      // Registre fermé (ollama.com jamais contacté) ; un test qui cherche un
      // modèle au registre donne le faux serveur.
      CHATONLM_REGISTRY_URL: "http://127.0.0.1:1",
      // ollama.com fermé (image du compte, page de connexion) ; un test du
      // compte donne le faux serveur, qui joue aussi ollama.com.
      CHATONLM_OLLAMA_COM_URL: "http://127.0.0.1:1",
      // Hugging Face et sa page d'état fermés (ModelHub hors ligne) ; un test
      // du ModelHub donne l'adresse du faux Hugging Face (startFakeHf) aux
      // deux.
      CHATONLM_HF_URL: "http://127.0.0.1:1",
      CHATONLM_HF_STATUS_URL: "http://127.0.0.1:1",
      CHATONLM_E2E_HARDWARE: E2E_HARDWARE,
      // Modèles de transcription (Hugging Face) fermés ; un test de la
      // dictée copie un modèle dans le dossier des données.
      CHATONLM_SPEECH_MODELS_URL: "http://127.0.0.1:1",
      // Voix de la lecture à voix haute (release GitHub, Hugging Face)
      // fermées ; un test de la lecture lie les voix de .tools/voice-models
      // dans le dossier des données. Sortie audio factice : la session de
      // test n'a pas de carte son.
      CHATONLM_VOICE_MODELS_URL: "http://127.0.0.1:1",
      CHATONLM_E2E_AUDIO_OUT: "null",
      // Mode conversation vocale sans micro : jamais celui de la machine ; un
      // test du mode vocal donne son faux micro (liste de WAV en JSON).
      CHATONLM_E2E_VOICE_MIC: "none",
      // Interface en anglais (langue « System » des essais), quelle que soit
      // la langue du bureau du développeur ; un test des langues donne la
      // sienne (LANG, LANGUAGE).
      LANG: "en_US.UTF-8",
      LANGUAGE: "",
      LC_ALL: "",
      LC_MESSAGES: "",
      ...env,
      TAURI_WEBDRIVER_PORT: String(port),
    },
    stdio: ["ignore", output, output],
  });
  // Code de sortie, ou nom du signal qui a arrêté l'app.
  const exited = new Promise((resolve) => {
    app.once("exit", (code, signal) => resolve(code ?? signal));
    app.once("error", () => resolve(null));
  });
  const crashed = exited.then((code) => {
    throw new Error(`L'app s'est arrêtée (code ${code}) ; voir ${log}`);
  });
  crashed.catch(() => {});

  const driver = new WebDriver(port);
  try {
    await Promise.race([driver.start(), crashed]);
    await fn(driver, { exited, pid: app.pid });
  } finally {
    await shutDown(app, exited, driver, quit, name);
  }
  if (readFileSync(log, "utf8").includes("startup fallback")) {
    throw new Error(`Fenêtre affichée par le filet de secours ; voir ${log}`);
  }
}
