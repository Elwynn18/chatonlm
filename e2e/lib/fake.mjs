// Faux serveur Ollama (crate fake-ollama) lancé depuis un test.
import { spawn } from "node:child_process";
import { createInterface } from "node:readline";
import { ROOT } from "./app.mjs";

export const FAKE_OLLAMA = `${ROOT}.tools/e2e-target/debug/fake-ollama`;

/** Lance le binaire avec ce mode ; attend l'adresse qu'il affiche. */
async function spawnFake(mode, port) {
  const child = spawn(FAKE_OLLAMA, [mode, String(port)], {
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
      reject(new Error(`fake-ollama s'est arrêté (code ${code})`)),
    );
  });
  return { child, exited, url };
}

/**
 * Démarre le faux serveur ; `port` 0 = port libre. Renvoie
 * { url, port, disconnects, pulls, deletes, pause, resume, stop }.
 */
export async function startFake(mode = "ollama", port = 0) {
  const { child, exited, url } = await spawnFake(mode, port);
  return {
    url,
    port: Number(new URL(url).port),
    /**
     * Nombre de flux (chat, pull) abandonnés par le client avant leur fin.
     */
    async disconnects() {
      const response = await fetch(new URL("/fake/disconnects", url));
      return (await response.json()).disconnects;
    },
    /** Corps des requêtes /api/pull reçues, dans l'ordre. */
    async pulls() {
      const response = await fetch(new URL("/fake/pulls", url));
      const body = await response.json();
      return Array.isArray(body) ? body : body.pulls;
    },
    /** Noms reçus par DELETE /api/delete, dans l'ordre. */
    async deletes() {
      const response = await fetch(new URL("/fake/deletes", url));
      return response.json();
    },
    /**
     * Fige le serveur (SIGSTOP) : les connexions restent ouvertes mais rien
     * ne répond, comme un Ollama bloqué ; `resume` le relance.
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

/**
 * Démarre le faux Hugging Face (même binaire, serveur à part ; dépôts de
 * `src-tauri/crates/fake-ollama/fixtures/hf/repos.json`). `mode` : `hf`
 * (normal), `hf-status:<code>` (toutes les routes répondent ce code ; page
 * d'état « incident » pour un 5xx), `hf-ratelimit` (429, nouvel essai dans
 * 42 s), `hf-html` (page HTML). Donner `url` à l'app par `CHATONLM_HF_URL` et
 * `CHATONLM_HF_STATUS_URL` (env de `withApp`). Renvoie { url, port, requests,
 * stop } ; `requests()` : requêtes reçues, dans l'ordre
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
    async stop() {
      child.kill("SIGTERM");
      await exited;
    },
  };
}
