// Faux relais de la recherche web (binaire `fake-relay` du crate
// relay-server, fonctionnalité `fake`) lancé depuis un test : le vrai relais
// avec de faux moteurs (Brave, Mojeek) sur la boucle locale, preuve de
// travail minuscule. L'app s'y adresse par CHATONLM_RELAY_URL (withApp la
// ferme par défaut). Binaire construit par scripts/e2e-build.sh
// (`cargo build -p relay-server --features fake --bin fake-relay`).
import { spawn, spawnSync } from "node:child_process";
import { mkdtempSync, rmSync } from "node:fs";
import { join } from "node:path";
import { createInterface } from "node:readline";
import { ROOT } from "./app.mjs";

export const FAKE_RELAY = `${ROOT}.tools/e2e-target/debug/fake-relay`;

/**
 * Démarre le faux relais. Options : `daily` (recherches par jour et par
 * installation), `paused`, `budgetExhausted`, `minVersion` (version minimale
 * de l'app), `providersDown` (aucun moteur ne répond). Renvoie
 * { url, env, stop } ; `env` se passe à withApp.
 */
export async function startFakeRelay({
  daily,
  paused = false,
  budgetExhausted = false,
  minVersion,
  providersDown = false,
} = {}) {
  const dir = mkdtempSync(join(ROOT, ".tools/tmp/fake-relay-"));
  const args = ["--dir", dir];
  if (daily !== undefined) args.push("--daily", String(daily));
  if (paused) args.push("--paused");
  if (budgetExhausted) args.push("--budget-exhausted");
  if (minVersion) args.push("--min-version", minVersion);
  if (providersDown) args.push("--providers-down");
  // Entrée standard tenue ouverte : sa fermeture arrête le relais (même si
  // le test meurt).
  const child = spawn(FAKE_RELAY, args, {
    stdio: ["pipe", "pipe", "inherit"],
  });
  const exited = new Promise((resolve) => {
    child.once("exit", resolve);
    child.once("error", () => resolve(null));
  });
  const url = await new Promise((resolve, reject) => {
    const lines = createInterface({ input: child.stdout });
    lines.on("line", (line) => {
      if (line.startsWith("FAKE_RELAY_URL=")) {
        resolve(line.slice("FAKE_RELAY_URL=".length));
      }
    });
    child.once("error", reject);
    child.once("exit", (code) =>
      reject(new Error(`fake-relay s'est arrêté (code ${code})`)),
    );
  });
  return {
    url,
    env: { CHATONLM_RELAY_URL: url },
    async stop() {
      child.stdin.end();
      const timer = setTimeout(() => child.kill("SIGKILL"), 3000);
      await exited;
      clearTimeout(timer);
      rmSync(dir, { recursive: true, force: true });
    },
  };
}

export const FAKE_LLAMA = `${ROOT}.tools/e2e-target/debug/fake-llama`;

/** Port libre de la boucle locale. */
async function freePort() {
  const { createServer } = await import("node:net");
  return new Promise((resolve, reject) => {
    const server = createServer();
    server.once("error", reject);
    server.listen(0, "127.0.0.1", () => {
      const { port } = server.address();
      server.close(() => resolve(port));
    });
  });
}

/**
 * Faux fournisseur de modèles distants (API d'OpenAI) : le faux
 * `llama-server` (binaire `fake-llama`), son modèle nommé `alias` (scénario
 * du faux : `chatonlm-remote-openai`, `chatonlm-websearch`…), clé `key`.
 * Renvoie { url, requests, stop } ; `url` se donne comme adresse d'un
 * fournisseur « custom ».
 */
export async function startFakeProvider({ alias, key }) {
  const port = await freePort();
  // Le faux refuse un fichier qui n'est pas un GGUF lisible, comme le vrai :
  // en-tête d'essai écrit par `fake-llama gguf`, dans un dossier à part.
  const dir = mkdtempSync(join(ROOT, ".tools/tmp/fake-provider-"));
  const model = join(dir, `${alias}.gguf`);
  const written = spawnSync(FAKE_LLAMA, ["gguf", model], { encoding: "utf8" });
  if (written.status !== 0) {
    rmSync(dir, { recursive: true, force: true });
    throw new Error(
      `fake-llama gguf ${model} : ${written.stderr || written.error}`,
    );
  }
  const child = spawn(
    FAKE_LLAMA,
    [
      "-m",
      model,
      "--alias",
      alias,
      "--api-key",
      key,
      "--host",
      "127.0.0.1",
      "--port",
      String(port),
    ],
    { stdio: ["ignore", "ignore", "inherit"] },
  );
  const exited = new Promise((resolve) => {
    child.once("exit", resolve);
    child.once("error", () => resolve(null));
  });
  const url = `http://127.0.0.1:${port}`;
  const deadline = Date.now() + 10000;
  for (;;) {
    try {
      if ((await fetch(`${url}/health`)).ok) break;
    } catch {
      // Pas encore à l'écoute.
    }
    if (Date.now() > deadline) {
      child.kill("SIGKILL");
      rmSync(dir, { recursive: true, force: true });
      throw new Error("fake-llama ne répond pas");
    }
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
  return {
    url,
    /** Requêtes reçues (`/fake/requests` du faux). */
    async requests() {
      return (await fetch(`${url}/fake/requests`)).json();
    },
    async stop() {
      child.kill("SIGTERM");
      const timer = setTimeout(() => child.kill("SIGKILL"), 3000);
      await exited;
      clearTimeout(timer);
      rmSync(dir, { recursive: true, force: true });
    },
  };
}
