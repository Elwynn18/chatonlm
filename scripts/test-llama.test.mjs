// @vitest-environment node
// scripts/test-llama.sh (instance du vrai moteur d'essai), sans vrai
// llama-server ni vrai modèle : dossier d'essai à part
// (CHATONLM_LLAMA_TEST_HOME sous .tools/tmp), port libre
// (CHATONLM_LLAMA_TEST_PORT), faux binaire (CHATONLM_LLAMA_TEST_BIN) et
// modèles factices avec leurs témoins de vérification. Aussi : racine
// commune des arbres de travail (scripts/lib/roots.sh), instance orpheline
// vue par scripts/leak-check.sh, aucune clé sur une ligne de commande.
import { execFileSync, spawn, spawnSync } from "node:child_process";
import {
  chmodSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  readdirSync,
  rmSync,
  statSync,
  writeFileSync,
} from "node:fs";
import { createServer } from "node:http";
import { createServer as createNetServer } from "node:net";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { afterEach, describe, expect, it, vi } from "vitest";

vi.setConfig({ testTimeout: 30_000 });

const ROOT = fileURLToPath(new URL("../", import.meta.url));
const SCRIPT = join(ROOT, "scripts/test-llama.sh");
const TMP = join(ROOT, ".tools/tmp");

const cleanups = [];
afterEach(async () => {
  for (const cleanup of cleanups.splice(0).reverse()) await cleanup();
});

function tempDir(prefix) {
  mkdirSync(TMP, { recursive: true });
  const dir = mkdtempSync(join(TMP, prefix));
  cleanups.push(() => rmSync(dir, { recursive: true, force: true }));
  return dir;
}

/** Port libre de la boucle locale. */
async function freePort() {
  const server = createNetServer();
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  const { port } = server.address();
  await new Promise((resolve) => server.close(resolve));
  return port;
}

/** Modèles épinglés du script : rôle, dépôt, fichier, empreinte, sorte. */
function pinned() {
  const text = readFileSync(SCRIPT, "utf8");
  const pattern =
    /^\s*"(\w+)\|[^|]+\|([^|]+)\|[^|]+\|([^|]+)\|([0-9a-f]{64})\|(\w+)"$/gm;
  return [...text.matchAll(pattern)].map(([, role, repo, file, sha, kind]) => ({
    role,
    repo,
    file,
    sha,
    kind,
  }));
}

/**
 * Modèles factices des rôles `roles` (tous par défaut) sous `home/models`,
 * avec le témoin que `fetch` écrit après vérification (empreinte attendue,
 * taille du fichier) ; `models.json` si `listing`.
 */
function seedModels(home, { roles = null, listing = true } = {}) {
  const models = join(home, "models");
  for (const model of pinned()) {
    if (roles && !roles.includes(model.role)) continue;
    const file = join(models, model.repo, model.file);
    mkdirSync(dirname(file), { recursive: true });
    writeFileSync(file, `poids de ${model.file}`);
    const stamp = join(home, "verified", model.repo, `${model.file}.sha256`);
    mkdirSync(dirname(stamp), { recursive: true });
    writeFileSync(stamp, `${model.sha} ${statSync(file).size}\n`);
  }
  if (listing) writeFileSync(join(models, "models.json"), "{}\n");
  return models;
}

/** Faux binaire : script bash `body` (exécutable). */
function fakeBinary(dir, name, body) {
  const path = join(dir, name);
  writeFileSync(path, `#!/usr/bin/env bash\n${body}\n`);
  chmodSync(path, 0o755);
  return path;
}

/**
 * Faux llama-server qui répond 200 à /health sur son `--port` (python3 :
 * même PID que le script, arguments gardés dans sa ligne de commande).
 */
function healthyBinary(dir) {
  return fakeBinary(
    dir,
    "fake-server",
    `exec python3 -I -c '
import http.server, sys
port = int(sys.argv[sys.argv.index("--port") + 1])
class Handler(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        self.send_response(200 if self.path == "/health" else 404)
        self.end_headers()
    def log_message(self, *args):
        pass
http.server.HTTPServer(("127.0.0.1", port), Handler).serve_forever()
' "$@"`,
  );
}

/** Environnement du script : dossier, binaire et port d'essai. */
function envFor(home, bin, port) {
  const env = {
    ...process.env,
    CHATONLM_LLAMA_TEST_HOME: home,
    CHATONLM_LLAMA_TEST_BIN: bin,
    CHATONLM_LLAMA_TEST_PORT: String(port),
  };
  delete env.CHATONLM_LLAMA_TEST_MODELS;
  delete env.CHATONLM_LLAMA_TEST_CPU;
  return env;
}

function run(args, env) {
  return spawnSync("bash", [SCRIPT, ...args], { env, encoding: "utf8" });
}

const pidOf = (home) => Number(readFileSync(join(home, "serve.pid"), "utf8"));
const alive = (pid) => {
  try {
    process.kill(pid, 0);
    return true;
  } catch {
    return false;
  }
};

/**
 * Lance l'instance ; l'arrête à la fin de l'essai (et tue le processus
 * lancé si « stop » ne l'a pas reconnu : jamais de reste).
 */
function startInstance(env, prefix = []) {
  const result = prefix.length
    ? spawnSync("bash", [...prefix, SCRIPT, "start"], { env, encoding: "utf8" })
    : run(["start"], env);
  const home = env.CHATONLM_LLAMA_TEST_HOME;
  const pidFile = join(home, "serve.pid");
  const pid = existsSync(pidFile) ? Number(readFileSync(pidFile, "utf8")) : 0;
  // Échec : journal de l'instance joint au message (le dossier est effacé
  // à la fin de l'essai).
  const log = join(home, "serve.log");
  if (result.status !== 0 && existsSync(log)) {
    result.stderr += `\n--- serve.log\n${readFileSync(log, "utf8")}`;
  }
  cleanups.push(() => {
    run(["stop"], env);
    if (pid && alive(pid)) process.kill(pid, "SIGKILL");
  });
  return result;
}

describe("scripts/test-llama.sh start", () => {
  it("starts its own server, then stops it", async () => {
    const home = tempDir("llama-home-");
    seedModels(home, { roles: ["text"] });
    const env = envFor(home, healthyBinary(home), await freePort());
    const started = startInstance(env);
    expect(started.status, started.stderr).toBe(0);
    expect(started.stdout).toContain("Lancée");
    const pid = pidOf(home);
    expect(alive(pid)).toBe(true);
    expect(run(["status"], env).status).toBe(0);
    // Déjà lancée : rien de plus.
    expect(run(["start"], env).stdout).toContain("Déjà lancée");
    const stopped = run(["stop"], env);
    expect(stopped.stdout).toContain("Arrêtée");
    expect(existsSync(join(home, "serve.pid"))).toBe(false);
    for (let i = 0; i < 50 && alive(pid); i++) {
      await new Promise((resolve) => setTimeout(resolve, 20));
    }
    expect(alive(pid)).toBe(false);
  });

  it("refuses a port that another server already answers on", async () => {
    const home = tempDir("llama-home-");
    seedModels(home, { roles: ["text"] });
    const marker = join(home, "launched");
    const bin = fakeBinary(home, "fake-server", `touch '${marker}'; exit 1`);
    const other = createServer((request, response) => {
      response.writeHead(request.url === "/health" ? 200 : 404).end();
    });
    await new Promise((resolve) => other.listen(0, "127.0.0.1", resolve));
    cleanups.push(() => new Promise((resolve) => other.close(resolve)));
    const env = envFor(home, bin, other.address().port);
    const started = startInstance(env);
    expect(started.status).not.toBe(0);
    expect(started.stdout).not.toContain("Lancée");
    expect(started.stderr).toContain("déjà pris");
    // Rien n'a été lancé.
    expect(existsSync(marker)).toBe(false);
  });

  it("reports a server that stops at once", async () => {
    const home = tempDir("llama-home-");
    seedModels(home, { roles: ["text"] });
    const env = envFor(
      home,
      fakeBinary(home, "fake-server", "exit 1"),
      await freePort(),
    );
    const started = startInstance(env);
    expect(started.status).not.toBe(0);
    expect(started.stderr).toContain("s'est arrêté");
    expect(existsSync(join(home, "serve.pid"))).toBe(false);
  });

  it("does not hand an inherited lock to the server", async () => {
    // Appelant qui tient un verrou sur le descripteur 9 (record-fixtures.sh) :
    // une fois le script fini, le serveur (toujours en marche) ne le tient pas.
    const home = tempDir("llama-home-");
    seedModels(home, { roles: ["text"] });
    const env = envFor(home, healthyBinary(home), await freePort());
    const lock = join(home, "real.lock");
    const started = startInstance(env, [
      "-c",
      'exec 9>"$0"; flock 9; exec bash "$1" start',
      lock,
    ]);
    expect(started.status, started.stderr).toBe(0);
    expect(alive(pidOf(home))).toBe(true);
    expect(spawnSync("flock", ["-n", lock, "true"]).status).toBe(0);
  });
});

describe("scripts/test-llama.sh gone", () => {
  // Fonction du script seule : un processus vivant n'est jamais pris pour
  // fini (même ligne de commande vide pendant un `exec`), un zombie ou un
  // processus absent l'est.
  const gone = (pid) =>
    spawnSync("bash", [
      "-c",
      `source <(sed -n '/^gone() {/,/^}/p' "$0"); gone "$1"`,
      SCRIPT,
      String(pid),
    ]).status === 0;

  it("tells a live process from a finished one", async () => {
    const child = spawn("sleep", ["30"], { stdio: "ignore" });
    cleanups.push(() => child.kill("SIGKILL"));
    await new Promise((resolve) => child.once("spawn", resolve));
    expect(gone(child.pid)).toBe(false);
    // Zombie : fini, pas encore attendu par son parent.
    const zombie = spawnSync(
      "bash",
      [
        "-c",
        `source <(sed -n '/^gone() {/,/^}/p' "$0"); sleep 0.01 & p=$!; sleep 0.3; gone $p && echo zombie; wait $p; gone $p && echo absent`,
        SCRIPT,
      ],
      { encoding: "utf8" },
    );
    expect(zombie.stdout.split("\n").filter(Boolean)).toEqual([
      "zombie",
      "absent",
    ]);
  });
});

describe("scripts/test-llama.sh check", () => {
  it("fails loudly when a test model is missing", async () => {
    const home = tempDir("llama-home-");
    const bin = fakeBinary(home, "fake-server", "exit 0");
    const env = envFor(home, bin, await freePort());
    const empty = run(["check"], env);
    expect(empty.status).not.toBe(0);
    expect(empty.stderr).toContain("fetch");
    // Un seul rôle : toujours incomplet.
    seedModels(home, { roles: ["text"] });
    expect(run(["check"], env).status).not.toBe(0);
  });

  it("passes with every model, the listing and the binary", async () => {
    const home = tempDir("llama-home-");
    const models = seedModels(home);
    const bin = fakeBinary(home, "fake-server", "exit 0");
    const env = envFor(home, bin, await freePort());
    const result = run(["check"], env);
    expect(result.status, result.stderr).toBe(0);
    // Les contrats liraient un autre dossier que celui de fetch : refusé.
    expect(
      run(["check"], {
        ...env,
        CHATONLM_LLAMA_TEST_MODELS: join(home, "elsewhere"),
      }).status,
    ).not.toBe(0);
    expect(
      run(["check"], { ...env, CHATONLM_LLAMA_TEST_MODELS: models }).status,
    ).toBe(0);
    // Binaire absent.
    expect(
      run(["check"], { ...env, CHATONLM_LLAMA_TEST_BIN: join(home, "none") })
        .status,
    ).not.toBe(0);
    // models.json absent.
    rmSync(join(models, "models.json"));
    expect(run(["check"], env).status).not.toBe(0);
  });
});

describe("scripts/lib/roots.sh", () => {
  const git = (dir, ...args) =>
    execFileSync(
      "git",
      [
        "-C",
        dir,
        "-c",
        "user.name=e2e",
        "-c",
        "user.email=e2e@invalid",
        ...args,
      ],
      { stdio: "ignore" },
    );
  const shell = (code, ...args) =>
    execFileSync(
      "bash",
      [
        "-c",
        `source "$0"; ${code}`,
        join(ROOT, "scripts/lib/roots.sh"),
        ...args,
      ],
      { encoding: "utf8" },
    ).trim();

  it("gives every working tree the main tree's test home and lock", () => {
    const repo = join(tempDir("repo-"), "main");
    mkdirSync(repo);
    git(repo, "init", "-q");
    git(repo, "commit", "-q", "--allow-empty", "-m", "x");
    const tree = join(repo, ".tools/wt/x");
    git(repo, "worktree", "add", "-q", tree);
    const env = { ...process.env };
    delete env.CHATONLM_LLAMA_TEST_HOME;
    const ask = (fn, dir) =>
      execFileSync(
        "bash",
        [
          "-c",
          `source "$0"; ${fn} "$1"`,
          join(ROOT, "scripts/lib/roots.sh"),
          dir,
        ],
        { encoding: "utf8", env },
      ).trim();
    expect(ask("chatonlm_main_root", tree)).toBe(repo);
    expect(ask("chatonlm_main_root", repo)).toBe(repo);
    expect(ask("chatonlm_llama_test_home", tree)).toBe(
      join(repo, ".tools/llama-test"),
    );
    expect(ask("chatonlm_real_lock", tree)).toBe(
      join(repo, ".tools/locks/llama-real.lock"),
    );
    expect(
      shell('CHATONLM_LLAMA_TEST_HOME=/x chatonlm_llama_test_home "$1"', tree),
    ).toBe("/x");
  });
});

describe("scripts/leak-check.sh processes", () => {
  it("reports a test instance left running", async () => {
    const home = tempDir("llama-home-");
    mkdirSync(join(home, "home"));
    // Instance orpheline : HOME du vrai moteur d'essai.
    const orphan = spawn("sleep", ["30"], {
      env: { PATH: process.env.PATH, HOME: join(home, "home") },
      stdio: "ignore",
    });
    cleanups.push(() => orphan.kill("SIGKILL"));
    await new Promise((resolve) => orphan.once("spawn", resolve));
    const result = spawnSync(
      "bash",
      [join(ROOT, "scripts/leak-check.sh"), "processes"],
      {
        env: { ...process.env, CHATONLM_LLAMA_TEST_HOME: home },
        encoding: "utf8",
      },
    );
    expect(result.status).toBe(1);
    expect(result.stdout).toMatch(new RegExp(`PROCESSUS ${orphan.pid} sleep`));
  });
});

describe("scripts", () => {
  it("never put a key on a command line", () => {
    // La clé d'une instance passe par un fichier (--api-key-file, curl -H
    // @fichier) : jamais « Bearer $(…) » ni « --api-key <valeur> » (visibles
    // par ps).
    const scripts = readdirSync(join(ROOT, "scripts")).filter((name) =>
      name.endsWith(".sh"),
    );
    for (const name of scripts) {
      const text = readFileSync(join(ROOT, "scripts", name), "utf8");
      for (const line of text.split("\n")) {
        if (/^\s*#/.test(line) || /^\s*printf /.test(line)) continue;
        expect(line, `${name} : ${line}`).not.toMatch(/Bearer \$/);
        expect(line, `${name} : ${line}`).not.toMatch(/--api-key[ =]/);
      }
    }
  });
});
