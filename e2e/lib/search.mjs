// Faux moteurs de recherche web (binaire `fake-search` du crate
// fake-services : DuckDuckGo, Brave, Mojeek, SearXNG sur un seul port)
// lancés depuis un test. L'app s'y adresse par CHATONLM_DUCKDUCKGO_URL,
// CHATONLM_BRAVE_URL et CHATONLM_MOJEEK_URL (withApp les ferme par défaut) ;
// une instance SearXNG se donne dans les réglages (`searxng`). Binaire
// construit par scripts/e2e-build.sh (`cargo build -p fake-services`).
import { spawn } from "node:child_process";
import { createInterface } from "node:readline";
import { ROOT } from "./app.mjs";

export const FAKE_SEARCH = `${ROOT}.tools/e2e-target/debug/fake-search`;

/**
 * Démarre les faux moteurs. Renvoie { url, env, searxng, setBehavior,
 * requests, stop } ; `env` se passe à withApp.
 */
export async function startFakeSearch() {
  // Entrée standard tenue ouverte : sa fermeture arrête les faux (même si
  // le test meurt).
  const child = spawn(FAKE_SEARCH, [], { stdio: ["pipe", "pipe", "inherit"] });
  const exited = new Promise((resolve) => {
    child.once("exit", resolve);
    child.once("error", () => resolve(null));
  });
  const url = await new Promise((resolve, reject) => {
    const lines = createInterface({ input: child.stdout });
    lines.on("line", (line) => {
      if (line.startsWith("FAKE_SEARCH_URL=")) {
        resolve(line.slice("FAKE_SEARCH_URL=".length));
      }
    });
    child.once("error", reject);
    child.once("exit", (code) =>
      reject(new Error(`fake-search s'est arrêté (code ${code})`)),
    );
  });
  return {
    url,
    env: {
      CHATONLM_DUCKDUCKGO_URL: `${url}/ddg`,
      CHATONLM_BRAVE_URL: `${url}/brave`,
      CHATONLM_MOJEEK_URL: `${url}/mojeek`,
    },
    /** Adresse de la fausse instance SearXNG. */
    searxng: `${url}/searxng`,
    /**
     * Comportement d'un moteur (`duckduckgo`, `brave`, `mojeek`, `searxng`) :
     * `ok`, `blocked`, `slow`, `html`, `status:<code>`.
     */
    async setBehavior(engine, behavior) {
      const response = await fetch(`${url}/fake/behavior`, {
        method: "POST",
        body: JSON.stringify({ engine, behavior }),
      });
      if (!response.ok) throw new Error(`fake-search : ${response.status}`);
    },
    /** Requêtes reçues : { engine, query, lang, count }. */
    async requests() {
      return (await fetch(`${url}/fake/requests`)).json();
    },
    async stop() {
      child.stdin.end();
      const timer = setTimeout(() => child.kill("SIGKILL"), 3000);
      await exited;
      clearTimeout(timer);
    },
  };
}
