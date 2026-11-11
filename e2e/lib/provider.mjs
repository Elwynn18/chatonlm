// Faux fournisseur de modèles distants (API d'OpenAI) lancé depuis un test :
// le faux llama-server (binaire `fake-llama`, construit par
// scripts/e2e-build.sh), servi comme fournisseur « custom ».
import { spawn, spawnSync } from "node:child_process";
import { mkdtempSync, rmSync } from "node:fs";
import { join } from "node:path";
import { ROOT } from "./app.mjs";

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
