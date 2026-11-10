// @vitest-environment node
// Aides des e2e pour le moteur (e2e/lib/fake.mjs) : identifiants des modèles
// du faux, variables données à l'app, registre des instances de fake-llama
// (adresse, requêtes, arguments, fiches brutes), vrai moteur (dossiers de
// l'arbre principal, processeur seulement), verrou des vrais serveurs
// (toujours un verrou temporaire : jamais le vrai depuis un essai unitaire).
// Sans binaire : aucun GGUF n'est écrit ici (liste de modèles vide), le
// registre est rempli à la main. Dossiers temporaires sous .tools/tmp.
import { execFileSync, spawn } from "node:child_process";
import {
  existsSync,
  lstatSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { createServer } from "node:http";
import { dirname, join } from "node:path";
import { createInterface } from "node:readline";
import { afterEach, describe, expect, it, vi } from "vitest";
import {
  FAKE_LLAMA,
  fakeModel,
  holdRealLock,
  mainRootOf,
  realLock,
  startEngine,
  startRealEngine,
} from "../e2e/lib/fake.mjs";
import { ROOT } from "../e2e/lib/app.mjs";

vi.setConfig({ testTimeout: 30_000 });

const TMP = join(ROOT, ".tools/tmp");
const dirs = [];
const children = [];
afterEach(() => {
  for (const child of children.splice(0)) child.kill("SIGKILL");
  for (const dir of dirs.splice(0))
    rmSync(dir, { recursive: true, force: true });
});

function tempDir(prefix) {
  mkdirSync(TMP, { recursive: true });
  const dir = mkdtempSync(join(TMP, prefix));
  dirs.push(dir);
  return dir;
}

function dataDir() {
  const data = join(tempDir("e2e-fake-"), "data");
  mkdirSync(data);
  return data;
}

/** Vrai si personne ne tient `lock`. */
function free(lock) {
  try {
    execFileSync("flock", ["-n", lock, "true"]);
    return true;
  } catch {
    return false;
  }
}

/** Un autre processus prend `lock` et le garde jusqu'à sa fin. */
async function holdElsewhere(lock) {
  const child = spawn("flock", [lock, "-c", "echo locked; exec cat"], {
    stdio: ["pipe", "pipe", "inherit"],
  });
  children.push(child);
  await new Promise((resolve) =>
    createInterface({ input: child.stdout }).once("line", resolve),
  );
  return () => child.stdin.end();
}

/** Variables d'environnement posées le temps de `run`. */
async function withEnv(vars, run) {
  const saved = Object.fromEntries(
    Object.keys(vars).map((name) => [name, process.env[name]]),
  );
  for (const [name, value] of Object.entries(vars)) {
    if (value === undefined) delete process.env[name];
    else process.env[name] = value;
  }
  try {
    return await run();
  } finally {
    for (const [name, value] of Object.entries(saved)) {
      if (value === undefined) delete process.env[name];
      else process.env[name] = value;
    }
  }
}

const git = (dir, ...args) =>
  execFileSync(
    "git",
    ["-C", dir, "-c", "user.name=e2e", "-c", "user.email=e2e@invalid", ...args],
    { stdio: "ignore" },
  );

/** Faux llama-server minimal : `/fake/<route>` rend `answers[route]`. */
async function pilotServer(answers) {
  const server = createServer((request, response) => {
    const route = request.url.replace("/fake/", "");
    if (!(route in answers)) {
      response.writeHead(404).end();
      return;
    }
    response.writeHead(200, { "Content-Type": "application/json" });
    response.end(JSON.stringify(answers[route]));
  });
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  return server;
}

describe("e2e/lib/fake.mjs", () => {
  it("names a fake model after its scenario", () => {
    expect(fakeModel("think")).toBe("chatonlm-e2e/chatonlm-think-GGUF:Q4_K_M");
    expect(fakeModel("simple", "Q8_0")).toBe(
      "chatonlm-e2e/chatonlm-simple-GGUF:Q8_0",
    );
  });

  it("gives the app the fake binary, the models and the registry", () => {
    const data = dataDir();
    const engine = startEngine([], {
      dataDir: data,
      env: { FAKE_LLAMA_MODE: "silent" },
    });
    expect(engine.env.CHATONLM_LLAMA_SERVER_BIN).toBe(FAKE_LLAMA);
    expect(JSON.parse(engine.env.CHATONLM_E2E_MODELS)).toEqual([]);
    expect(dirname(engine.env.FAKE_LLAMA_REGISTRY)).toBe(dirname(data));
    expect(engine.env.FAKE_LLAMA_MODE).toBe("silent");
    expect(engine.modelsDir).toBe(join(data, "models"));
    expect(engine.servers()).toEqual([]);
    expect(engine.server("x")).toBeNull();
  });

  it("finds a live instance in the registry and reads its requests", async () => {
    const data = dataDir();
    const engine = startEngine([], { dataDir: data });
    const id = fakeModel("simple");
    const server = await pilotServer({
      requests: [
        { path: "/v1/chat/completions", body: { messages: [] } },
        { path: "/infill", body: { input_prefix: "a" } },
      ],
      disconnects: 2,
      args: ["--alias", id, "-c", "4096"],
    });
    try {
      const url = `http://127.0.0.1:${server.address().port}`;
      const registry = engine.env.FAKE_LLAMA_REGISTRY;
      mkdirSync(registry, { recursive: true });
      // Une instance vivante (ce processus), une morte, un fichier illisible.
      writeFileSync(
        join(registry, `${process.pid}.json`),
        JSON.stringify({
          url,
          alias: id,
          model: "/m/x.gguf",
          pid: process.pid,
        }),
      );
      writeFileSync(
        join(registry, "999999999.json"),
        JSON.stringify({
          url,
          alias: "dead",
          model: "/m/y.gguf",
          pid: 999999999,
        }),
      );
      writeFileSync(join(registry, "1.json.tmp"), "{");
      writeFileSync(join(registry, "2.json"), "{");
      expect(engine.servers().map((entry) => entry.alias)).toEqual([id]);
      // Fiches brutes : la morte et l'illisible comprises, pas le `.tmp`.
      expect(engine.registryFiles()).toEqual(
        [`${process.pid}.json`, "2.json", "999999999.json"].sort(),
      );
      expect(engine.server(id).url).toBe(url);
      expect(engine.server("dead")).toBeNull();
      expect(await engine.chatRequests(id)).toEqual([{ messages: [] }]);
      expect((await engine.requests(id)).length).toBe(2);
      expect(await engine.disconnects(id)).toBe(2);
      expect(await engine.args(id)).toEqual(["--alias", id, "-c", "4096"]);
      // Modèle sans instance : valeurs vides.
      expect(await engine.requests("none")).toEqual([]);
      expect(await engine.disconnects("none")).toBe(0);
      expect(await engine.args("none")).toBeNull();
    } finally {
      server.close();
    }
  });

  it("lists no registry file without an instance", () => {
    const engine = startEngine([], { dataDir: dataDir() });
    expect(engine.registryFiles()).toEqual([]);
  });

  it("holds the given lock until released", async () => {
    const lock = join(tempDir("lock-"), "real.lock");
    const release = await holdRealLock(lock);
    expect(free(lock)).toBe(false);
    release();
    let freed = false;
    for (let i = 0; i < 50 && !freed; i++) {
      freed = free(lock);
      if (!freed) await new Promise((resolve) => setTimeout(resolve, 20));
    }
    expect(freed).toBe(true);
  });

  it("waits for the given lock, not for the real one", async () => {
    // Le verrou donné est pris ailleurs : l'aide attend (elle ne prend pas
    // le vrai verrou à sa place), puis le prend dès qu'il se libère.
    const lock = join(tempDir("lock-"), "real.lock");
    const releaseElsewhere = await holdElsewhere(lock);
    let taken = false;
    const holding = holdRealLock(lock).then((release) => {
      taken = true;
      return release;
    });
    await new Promise((resolve) => setTimeout(resolve, 400));
    expect(taken).toBe(false);
    releaseElsewhere();
    const release = await holding;
    expect(free(lock)).toBe(false);
    release();
  });

  it("puts the real lock and models in the main working tree", async () => {
    const main = mainRootOf(ROOT);
    expect(realLock()).toBe(join(main, ".tools/locks/llama-real.lock"));
    // Un arbre de travail git rend la racine de l'arbre principal ; hors de
    // git, le dossier lui-même.
    const repo = join(tempDir("repo-"), "main");
    mkdirSync(repo);
    git(repo, "init", "-q");
    git(repo, "commit", "-q", "--allow-empty", "-m", "x");
    const tree = join(repo, ".tools/wt/x");
    git(repo, "worktree", "add", "-q", tree);
    expect(mainRootOf(tree)).toBe(repo);
    expect(mainRootOf(repo)).toBe(repo);
    // Hors de git (recherche du dépôt arrêtée au-dessus du dossier).
    const outside = tempDir("outside-");
    await withEnv({ GIT_CEILING_DIRECTORIES: dirname(outside) }, () => {
      expect(mainRootOf(outside)).toBe(outside);
    });
  });

  it("gives the app the real engine, on the processor when asked", async () => {
    const home = tempDir("llama-test-");
    const models = join(home, "models");
    const repo = "unsloth/Qwen3-0.6B-GGUF";
    mkdirSync(join(models, repo), { recursive: true });
    writeFileSync(join(models, repo, "Qwen3-0.6B-Q4_K_M.gguf"), "weights");
    writeFileSync(
      join(models, "models.json"),
      JSON.stringify({
        version: 1,
        models: [
          {
            role: "text",
            id: `${repo}:Q4_K_M`,
            files: [`${repo}/Qwen3-0.6B-Q4_K_M.gguf`],
            mmproj: null,
          },
        ],
      }),
    );
    const binary = join(home, "llama-server");
    writeFileSync(binary, "");
    const vars = {
      CHATONLM_LLAMA_TEST_HOME: home,
      CHATONLM_LLAMA_TEST_MODELS: undefined,
      CHATONLM_LLAMA_TEST_BIN: binary,
    };
    await withEnv({ ...vars, CHATONLM_LLAMA_TEST_CPU: "1" }, () => {
      const data = dataDir();
      const engine = startRealEngine(["text"], { dataDir: data });
      expect(engine.env.CHATONLM_LLAMA_SERVER_BIN).toBe(binary);
      expect(engine.env.CHATONLM_LLAMA_CPU_ONLY).toBe("1");
      expect(engine.ids).toEqual([`${repo}:Q4_K_M`]);
      const linked = join(data, "models", repo, "Qwen3-0.6B-Q4_K_M.gguf");
      expect(readFileSync(linked, "utf8")).toBe("weights");
      // Le lien retiré, l'original reste.
      rmSync(linked);
      expect(existsSync(join(models, repo, "Qwen3-0.6B-Q4_K_M.gguf"))).toBe(
        true,
      );
    });
    await withEnv({ ...vars, CHATONLM_LLAMA_TEST_CPU: undefined }, () => {
      const engine = startRealEngine(["text"], { dataDir: dataDir() });
      expect(engine.env.CHATONLM_LLAMA_CPU_ONLY).toBeUndefined();
    });
    // Modèle d'un rôle absent : null.
    await withEnv(vars, () => {
      expect(startRealEngine(["fim"], { dataDir: dataDir() })).toBeNull();
    });
    expect(lstatSync(join(models, repo)).isDirectory()).toBe(true);
  });
});
