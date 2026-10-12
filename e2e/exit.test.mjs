// Fin de l'app (étape 18a) : son processus web (WebKitWebProcess) est
// arrêté par l'app elle-même avant qu'elle se termine
// (`webkit_web_view_terminate_web_process`). Sans cela, il finit seul après
// l'app, une fois sa connexion fermée : ~80 ms plus tard dans la session
// virtuelle (mesuré le 7 oct. 2026), parfois 11 à 14 s auparavant (pilote
// NVIDIA : son fil de rendu GPU, SkiaGPUWorker, tardait à finir ; plus
// reproduit). Deux sorties, plusieurs fois chacune (`E2E_EXIT_ROUNDS`, 3 par
// défaut) :
// - Ctrl+Q (commande `quit_app`, comme « Quit ChatonLM ») : ExitRequested ;
// - fermeture de la fenêtre par le bureau, sans zone de notification :
//   CloseRequested (la fenêtre est détruite avant ExitRequested).
// Vérifié : le processus web n'est plus vivant quand la fin de l'app est
// constatée ; tous les processus WebKit finis dans les 2 s (délais affichés).
import assert from "node:assert/strict";
import { readFileSync, readdirSync } from "node:fs";
import { withApp } from "./lib/app.mjs";
import { startFake } from "./lib/fake.mjs";
import { forWindows, runKWinScript } from "./lib/kwin.mjs";

const binary = process.argv[2];
const ROUNDS = Number(process.env.E2E_EXIT_ROUNDS ?? 3);
const LIMIT_MS = 2000;
const fake = await startFake("ollama");
const delay = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

/** Descendants vivants d'un processus (pid, nom), d'après /proc. */
function descendants(root) {
  const children = new Map();
  for (const entry of readdirSync("/proc")) {
    if (!/^\d+$/.test(entry)) continue;
    try {
      const stat = readFileSync(`/proc/${entry}/stat`, "utf8");
      const ppid = Number(stat.slice(stat.lastIndexOf(")") + 2).split(" ")[1]);
      if (!children.has(ppid)) children.set(ppid, []);
      children.get(ppid).push(Number(entry));
    } catch {
      // Terminé entre-temps.
    }
  }
  const found = [];
  const pending = [root];
  while (pending.length > 0) {
    for (const pid of children.get(pending.pop()) ?? []) {
      let name = "?";
      try {
        name = readFileSync(`/proc/${pid}/cmdline`, "utf8")
          .split("\0")[0]
          .split("/")
          .pop();
      } catch {
        // Terminé entre-temps.
      }
      found.push({ pid, name });
      pending.push(pid);
    }
  }
  return found;
}

/** Vivant (ni terminé, ni zombie). */
function alive(pid) {
  try {
    const stat = readFileSync(`/proc/${pid}/stat`, "utf8");
    return stat.slice(stat.lastIndexOf(")") + 2)[0] !== "Z";
  } catch {
    return false;
  }
}

/**
 * Lance l'app, la fait quitter par `quit`, puis mesure combien de temps ses
 * processus WebKit lui survivent (20 s au plus).
 */
async function measure(name, quit) {
  let survived;
  let outlived = [];
  let helpers = [];
  await withApp(
    binary,
    { name, env: { OLLAMA_HOST: fake.url } },
    async (driver, app) => {
      await driver.waitFor("[data-testid=new-chat]");
      helpers = descendants(app.pid).filter((helper) =>
        helper.name.startsWith("WebKit"),
      );
      assert.ok(
        helpers.some((helper) => helper.name === "WebKitWebProcess"),
        `processus web absent : ${JSON.stringify(helpers)}`,
      );
      await quit(driver);
      const code = await Promise.race([app.exited, delay(15000)]);
      // Aussitôt la fin de l'app constatée : le processus web ne lui a pas
      // survécu (sans arrêt explicite, il vit encore ~80 ms).
      outlived = helpers
        .filter((helper) => helper.name === "WebKitWebProcess")
        .filter((helper) => alive(helper.pid))
        .map((helper) => helper.pid);
      assert.equal(code, 0, "l'app n'a pas quitté proprement");
      const end = Date.now();
      while (helpers.some((helper) => alive(helper.pid))) {
        if (Date.now() - end > 20000) break;
        await delay(20);
      }
      survived = Date.now() - end;
    },
  );
  const left = helpers.filter((helper) => alive(helper.pid));
  return {
    survived,
    outlived,
    helpers: helpers.map((helper) => helper.name),
    left,
  };
}

const byCommand = (driver) =>
  // Sans attendre la réponse : l'app se termine pendant l'appel.
  driver
    .execute("window.__TAURI_INTERNALS__.invoke('quit_app'); return true")
    .catch(() => {});

const byDesktop = () =>
  runKWinScript(forWindows("ChatonLM", "w.closeWindow();"), { settleMs: 0 });

const results = [];
try {
  for (let round = 1; round <= ROUNDS; round++) {
    for (const [kind, quit] of [
      ["quit_app", byCommand],
      ["fermeture", byDesktop],
    ]) {
      const result = await measure(`exit-${kind}-${round}`, quit);
      console.log(
        `${kind} ${round} : processus WebKit finis ${result.survived} ms après l'app (${result.helpers.join(", ")})${result.outlived.length > 0 ? " ; processus web vivant à sa fin" : ""}`,
      );
      results.push({ kind, round, ...result });
    }
  }
  for (const { kind, round, survived, outlived, left } of results) {
    assert.deepEqual(
      outlived,
      [],
      `${kind} ${round} : processus web encore vivant à la fin de l'app`,
    );
    assert.deepEqual(left, [], `${kind} ${round} : processus restés`);
    assert.ok(
      survived <= LIMIT_MS,
      `${kind} ${round} : processus WebKit finis ${survived} ms après l'app`,
    );
  }
  console.log("exit : OK");
} finally {
  await fake.stop();
}
