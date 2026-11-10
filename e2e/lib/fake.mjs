// Faux serveurs lancés depuis un test : moteur de texte (fake-llama, lancé par
// l'app sur des modèles inscrits au démarrage) et Hugging Face
// (fake-services).
import { execFileSync, spawn, spawnSync } from "node:child_process";
import {
  copyFileSync,
  existsSync,
  linkSync,
  mkdirSync,
  readFileSync,
  readdirSync,
} from "node:fs";
import { basename, dirname, join } from "node:path";
import { createInterface } from "node:readline";
import { FAKE_LLAMA, ROOT } from "./app.mjs";

export { FAKE_LLAMA };
export const FAKE_SERVICES = `${ROOT}.tools/e2e-target/debug/fake-services`;

/**
 * Racine de l'arbre principal du dépôt git qui contient `dir` (`dir` lui-même
 * hors d'un arbre de travail, ou hors de git), comme `chatonlm_main_root` de
 * scripts/lib/roots.sh : les vrais modèles d'essai et le verrou des vrais
 * serveurs sont ceux de l'arbre principal, communs à tous les arbres de
 * travail (.tools/wt/<x>).
 */
export function mainRootOf(dir) {
  try {
    const common = execFileSync(
      "git",
      ["-C", dir, "rev-parse", "--path-format=absolute", "--git-common-dir"],
      { encoding: "utf8", stdio: ["ignore", "pipe", "ignore"] },
    ).trim();
    if (common && basename(common) === ".git") return dirname(common);
  } catch {
    // Hors de git : le dossier lui-même.
  }
  return join(dir);
}

let mainRoot = null;
/** Racine de l'arbre principal de ce dépôt (calculée une fois). */
function main() {
  mainRoot ??= mainRootOf(ROOT);
  return mainRoot;
}

/**
 * Vrai llama-server : `CHATONLM_LLAMA_TEST_BIN`, sinon celui de cet arbre
 * (scripts/build-llama-server.sh), sinon celui de l'arbre principal (même
 * ordre que scripts/test-llama.sh).
 */
function realLlama() {
  const candidates = [
    join(ROOT, ".tools/runtimes/llama-server/llama-server"),
    join(main(), ".tools/runtimes/llama-server/llama-server"),
  ];
  return (
    process.env.CHATONLM_LLAMA_TEST_BIN ||
    candidates.find((path) => existsSync(path)) ||
    candidates[0]
  );
}

/**
 * Vrais modèles d'essai (scripts/test-llama.sh fetch) :
 * `CHATONLM_LLAMA_TEST_MODELS` (check.sh), sinon
 * `<CHATONLM_LLAMA_TEST_HOME ou .tools/llama-test de l'arbre principal>/models`.
 */
function realModels() {
  return (
    process.env.CHATONLM_LLAMA_TEST_MODELS ||
    join(
      process.env.CHATONLM_LLAMA_TEST_HOME || join(main(), ".tools/llama-test"),
      "models",
    )
  );
}

/** Verrou des vrais serveurs, celui de l'arbre principal. */
export function realLock() {
  return join(main(), ".tools/locks/llama-real.lock");
}

/**
 * Identifiant d'un modèle du faux moteur au scénario `scenario` de fake-llama
 * (`simple`, `think`, `tools`, `fim`, `slowload`… : voir
 * src-tauri/crates/fake-llama/src/scenario.rs et scenarios/) :
 * `chatonlm-e2e/chatonlm-<scénario>-GGUF:<quant>`. Le faux choisit son
 * scénario d'après l'alias que l'app lui donne, c.-à-d. cet identifiant.
 */
export function fakeModel(scenario, quant = "Q4_K_M") {
  return `chatonlm-e2e/chatonlm-${scenario}-GGUF:${quant}`;
}

/** Capacités déduites du nom (comme `FakeBench` des essais Rust). */
function capabilitiesOf(id) {
  const name = id.toLowerCase();
  const has = (words) => words.some((word) => name.includes(word));
  return {
    tools: has(["tools", "parallel", "noid"]),
    think: has(["think", "effort", "levels"]),
    fim: has(["fim"]),
    vision: has(["vision"]),
  };
}

/** Dossier des données de l'app (celui de la session de test par défaut). */
function dataDirOf(dataDir) {
  const dir = dataDir ?? process.env.CHATONLM_DATA_DIR;
  if (!dir) {
    throw new Error(
      "Dossier des données inconnu : CHATONLM_DATA_DIR (session e2e) ou { dataDir }",
    );
  }
  return dir;
}

/** Fichier de poids d'un identifiant `auteur/dépôt:QUANT`, relatif. */
function weightsOf(id) {
  const [path, quant] = [
    id.slice(0, id.lastIndexOf(":")),
    id.slice(id.lastIndexOf(":") + 1),
  ];
  const repo = path.slice(path.indexOf("/") + 1);
  const name = repo.replace(/-GGUF$/i, "");
  return { dir: path, file: `${path}/${name}-${quant}.gguf` };
}

/** Écrit un GGUF d'essai (`fake-llama gguf`). */
function writeGguf(out, flags) {
  const result = spawnSync(FAKE_LLAMA, ["gguf", out, ...flags], {
    encoding: "utf8",
  });
  if (result.status !== 0) {
    throw new Error(
      `fake-llama gguf ${out} : ${result.stderr || result.error}`,
    );
  }
}

/**
 * Prépare le faux moteur : les modèles `models` (identifiants, ou objets
 * `{ id, tools?, think?, fim?, vision?, audio?, base?, arch?, ctx?,
 * contextMax?, paramsLabel?, capabilities?, size? }` ; `arch` : architecture
 * du GGUF, `gpt-oss` pour les niveaux de réflexion) sont écrits sous
 * `<dataDir>/models/<auteur>/<dépôt>/<Nom>-<QUANT>.gguf` (en-tête lisible ;
 * projecteur `mmproj-F16.gguf` pour la vision ou l'audio) et inscrits au
 * démarrage de l'app (`CHATONLM_E2E_MODELS`). Capacités par défaut d'après le
 * nom (`tools`, `think`, `fim`, `vision`). Donner `env` à `withApp` (avec
 * `options.env` : `FAKE_LLAMA_MODE`, `FAKE_LLAMA_LOAD_MS`…). `dataDir` : celui
 * de la session (`CHATONLM_DATA_DIR`) par défaut ; un test qui donne le sien à
 * l'app le donne aussi ici. Les instances lancées par l'app s'inscrivent dans
 * un registre : `server(id)` rend l'adresse de celle d'un modèle, `requests(id)`
 * ses requêtes (`/fake/requests`).
 */
export function startEngine(models, { dataDir, env = {} } = {}) {
  const data = dataDirOf(dataDir);
  const modelsDir = join(data, "models");
  const registry = join(dirname(data), `fake-llama-registry-${process.pid}`);
  const seeds = models.map((model) => {
    const spec = typeof model === "string" ? { id: model } : model;
    const guessed = capabilitiesOf(spec.id);
    const want = { ...guessed, ...spec };
    const { file, dir } = weightsOf(spec.id);
    const flags = ["--ctx", String(spec.ctx ?? 40960)];
    if (spec.arch) flags.push("--arch", spec.arch);
    for (const flag of ["tools", "think", "fim", "base"]) {
      if (want[flag]) flags.push(`--${flag}`);
    }
    if (spec.size) flags.push("--size", String(spec.size));
    writeGguf(join(modelsDir, file), flags);
    const seed = { id: spec.id, files: [file] };
    if (want.vision || want.audio) {
      seed.mmproj = `${dir}/mmproj-F16.gguf`;
      writeGguf(join(modelsDir, seed.mmproj), [
        want.audio ? "--audio" : "--vision",
      ]);
    }
    if (spec.capabilities) seed.capabilities = spec.capabilities;
    if (spec.contextMax) seed.contextMax = spec.contextMax;
    if (spec.paramsLabel) seed.paramsLabel = spec.paramsLabel;
    return seed;
  });
  return engineHandle({
    seeds,
    modelsDir,
    registry,
    env: {
      CHATONLM_LLAMA_SERVER_BIN: FAKE_LLAMA,
      CHATONLM_E2E_MODELS: JSON.stringify(seeds),
      FAKE_LLAMA_REGISTRY: registry,
      ...env,
    },
  });
}

/**
 * Vrai moteur sur les vrais modèles d'essai (`roles` : `text`, `vision`,
 * `tiny`, `fim`, `plain`, lus dans `models.json` des modèles d'essai, ceux
 * de l'arbre principal) : les fichiers sont liés (lien physique, sinon
 * copiés) dans le dossier des modèles de l'app, jamais modifiés à leur place
 * d'origine. `null` (et le test le dit) si le binaire ou un modèle manque.
 * `CHATONLM_LLAMA_TEST_CPU=1` : processeur seulement, par la variable des
 * builds d'essai `CHATONLM_LLAMA_CPU_ONLY=1` (l'app retire toute variable
 * `GGML_*` héritée avant de lancer llama-server).
 */
export function startRealEngine(roles, { dataDir, env = {} } = {}) {
  const binary = realLlama();
  const models = realModels();
  const listing = join(models, "models.json");
  if (!existsSync(binary) || !existsSync(listing)) {
    console.error(
      `Vrai moteur absent (${binary}, ${listing}) : scripts/build-llama-server.sh, scripts/test-llama.sh fetch`,
    );
    return null;
  }
  const known = JSON.parse(readFileSync(listing, "utf8")).models;
  const modelsDir = join(dataDirOf(dataDir), "models");
  const seeds = [];
  for (const role of roles) {
    const entry = known.find((model) => model.role === role);
    if (!entry) {
      console.error(`Aucun modèle « ${role} » dans ${listing}`);
      return null;
    }
    for (const file of [
      ...entry.files,
      ...(entry.mmproj ? [entry.mmproj] : []),
    ]) {
      const from = join(models, file);
      const to = join(modelsDir, file);
      if (existsSync(to)) continue;
      mkdirSync(dirname(to), { recursive: true });
      try {
        linkSync(from, to);
      } catch {
        copyFileSync(from, to);
      }
    }
    const seed = { id: entry.id, files: entry.files };
    if (entry.mmproj) seed.mmproj = entry.mmproj;
    seeds.push(seed);
  }
  const cpu =
    process.env.CHATONLM_LLAMA_TEST_CPU === "1"
      ? { CHATONLM_LLAMA_CPU_ONLY: "1" }
      : {};
  return engineHandle({
    seeds,
    modelsDir,
    registry: null,
    env: {
      CHATONLM_LLAMA_SERVER_BIN: binary,
      CHATONLM_E2E_MODELS: JSON.stringify(seeds),
      ...cpu,
      ...env,
    },
  });
}

/**
 * Prend le verrou des vrais serveurs (`.tools/locks/llama-real.lock` de
 * l'arbre principal, celui des contrats et de scripts/test-llama.sh ; un
 * essai unitaire donne le sien, `lock`) et le garde jusqu'à la fin du test :
 * un processus gardien `flock` le tient tant que son entrée reste ouverte,
 * c.-à-d. tant que ce processus vit. Attend s'il est pris.
 */
export async function holdRealLock(lock = realLock()) {
  mkdirSync(dirname(lock), { recursive: true });
  const holder = spawn("flock", [lock, "-c", "echo locked; exec cat"], {
    stdio: ["pipe", "pipe", "inherit"],
  });
  await new Promise((resolve, reject) => {
    createInterface({ input: holder.stdout }).once("line", resolve);
    holder.once("error", reject);
    holder.once("exit", (code) =>
      reject(new Error(`flock arrêté (code ${code})`)),
    );
  });
  holder.unref();
  holder.stdout.destroy();
  return () => holder.stdin.end();
}

/** Poignée commune : variables, modèles, et registre du faux. */
function engineHandle({ seeds, modelsDir, registry, env }) {
  const servers = () => {
    if (!registry || !existsSync(registry)) return [];
    return readdirSync(registry)
      .filter((name) => name.endsWith(".json"))
      .map((name) => {
        try {
          return JSON.parse(readFileSync(join(registry, name), "utf8"));
        } catch {
          return null;
        }
      })
      .filter((entry) => {
        if (!entry) return false;
        try {
          process.kill(entry.pid, 0);
          return true;
        } catch {
          return false;
        }
      });
  };
  const server = (id) => servers().find((entry) => entry.alias === id) ?? null;
  return {
    env,
    models: seeds,
    ids: seeds.map((seed) => seed.id),
    modelsDir,
    /** Instances du faux en marche : `{ url, alias, model, pid }`. */
    servers,
    /**
     * Fiches du registre telles quelles (`<pid>.json`, triées), vivantes ou
     * non : une instance arrêtée proprement (SIGTERM) retire la sienne, une
     * instance tuée (SIGKILL) la laisse.
     */
    registryFiles: () =>
      registry && existsSync(registry)
        ? readdirSync(registry)
            .filter((name) => name.endsWith(".json"))
            .sort()
        : [],
    /** Instance du faux qui sert le modèle `id`, ou null. */
    server,
    /**
     * Requêtes reçues par l'instance du modèle `id`, `{ path, body }` dans
     * l'ordre (vide si elle est arrêtée).
     */
    requests: (id) => pilot(id, "requests", []),
    /** Corps des demandes de chat reçues par l'instance du modèle `id`. */
    async chatRequests(id) {
      const requests = await pilot(id, "requests", []);
      return requests
        .filter((request) => request.path.endsWith("chat/completions"))
        .map((request) => request.body);
    },
    /** Flux abandonnés par l'app avant leur fin (instance du modèle `id`). */
    disconnects: (id) => pilot(id, "disconnects", 0),
    /** Arguments de lancement de l'instance du modèle `id` (`-c`, `--mmproj`…). */
    args: (id) => pilot(id, "args", null),
    /**
     * Fige l'instance du modèle `id` (SIGSTOP) : ses connexions restent
     * ouvertes mais rien ne répond, comme un moteur bloqué ; `resume` la
     * relance. Faux si elle n'est pas en marche.
     */
    pause(id) {
      return signal(id, "SIGSTOP");
    },
    resume(id) {
      return signal(id, "SIGCONT");
    },
  };
  function signal(id, name) {
    const entry = server(id);
    if (!entry) return false;
    try {
      process.kill(entry.pid, name);
      return true;
    } catch {
      return false;
    }
  }
  async function pilot(id, route, fallback) {
    const entry = server(id);
    if (!entry) return fallback;
    try {
      const response = await fetch(new URL(`/fake/${route}`, entry.url));
      return response.ok ? response.json() : fallback;
    } catch {
      // Instance arrêtée entre-temps.
      return fallback;
    }
  }
}

/** Lance le binaire avec ce mode ; attend l'adresse qu'il affiche. */
async function spawnFake(mode, port) {
  const child = spawn(FAKE_SERVICES, [mode, String(port)], {
    stdio: ["ignore", "pipe", "inherit"],
  });
  const exited = new Promise((resolve) => {
    child.once("exit", resolve);
    child.once("error", () => resolve(null));
  });
  const url = await new Promise((resolve, reject) => {
    const lines = createInterface({ input: child.stdout });
    lines.once("line", resolve);
    child.once("error", reject);
    child.once("exit", (code) =>
      reject(new Error(`fake-services s'est arrêté (code ${code})`)),
    );
  });
  return { child, exited, url };
}

/**
 * Démarre le faux Hugging Face (binaire fake-services ; dépôts de
 * `src-tauri/crates/fake-services/fixtures/hf/repos.json`). `mode` : `hf`
 * (normal), `hf-status:<code>` (toutes les routes répondent ce code ; page
 * d'état « incident » pour un 5xx), `hf-ratelimit` (429, nouvel essai dans
 * 42 s), `hf-html` (page HTML). Donner `url` à l'app par `CHATONLM_HF_URL` et
 * `CHATONLM_HF_STATUS_URL` (env de `withApp`). Renvoie { url, port, requests,
 * pause, resume, stop } ; `requests()` : requêtes reçues, dans l'ordre
 * ({ method, path, ifNoneMatch, range }).
 */
export async function startFakeHf(mode = "hf", port = 0) {
  const { child, exited, url } = await spawnFake(mode, port);
  return {
    url,
    port: Number(new URL(url).port),
    async requests() {
      const response = await fetch(new URL("/fake/requests", url));
      return response.json();
    },
    /**
     * Fige le serveur (SIGSTOP) : les connexions restent ouvertes mais rien
     * ne répond (un téléchargement reste en cours) ; `resume` le relance.
     * `requests()` ne répond pas tant qu'il est figé.
     */
    pause() {
      child.kill("SIGSTOP");
    },
    resume() {
      child.kill("SIGCONT");
    },
    async stop() {
      child.kill("SIGTERM");
      // Un serveur figé ne traiterait SIGTERM qu'une fois relancé.
      child.kill("SIGCONT");
      await exited;
    },
  };
}
