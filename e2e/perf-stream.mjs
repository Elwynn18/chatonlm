// Mesure (pas un test de check.sh) : un long flux Markdown (bloc de 238 lignes
// de TypeScript, modèle chatonlm-markdown-long du faux serveur), avec le rendu
// DMABUF de WebKitGTK (défaut) puis sans (WEBKIT_DISABLE_DMABUF_RENDERER=1,
// le repli des pilotes qui affichent une fenêtre blanche). Pour chaque
// configuration et chaque essai : durée de la réponse, pire écart entre deux
// images pendant le flux et à la fin, nombre d'écarts > 50 ms.
//   scripts/e2e-session.sh e2e/run.sh e2e/perf-stream.mjs "$(scripts/e2e-build.sh)"
// Variable PERF_RUNS : essais par configuration (3 par défaut).
import { setTimeout as delay } from "node:timers/promises";
import { withApp } from "./lib/app.mjs";
import { startFake } from "./lib/fake.mjs";
import { chordsOf, pressKeys } from "./lib/keyboard.mjs";
import { PENDING_CALLS } from "./lib/page.mjs";

const binary = process.argv[2];
const runs = Number(process.env.PERF_RUNS ?? 3);
const MODEL = "chatonlm-markdown-long";

async function waitUntil(check, what, timeoutMs = 20000) {
  const deadline = Date.now() + timeoutMs;
  for (;;) {
    const value = await check();
    if (value) return value;
    if (Date.now() > deadline) throw new Error(`Délai dépassé : ${what}`);
    await delay(100);
  }
}

const shownModel = (driver) =>
  driver.execute(
    "return document.querySelector('[title=\"Select model\"]')?.textContent ?? null",
  );

/** Choisit le modèle du flux long (absent de la liste) puis recharge. */
async function useLongModel(driver) {
  await waitUntil(() => shownModel(driver), "modèle affiché");
  // Le modèle de secours est enregistré dès que la liste est lue : l'attendre,
  // sinon il remplacerait celui choisi ici.
  await waitUntil(
    async () => (await driver.invoke("get_settings")).selectedModel !== "",
    "modèle de secours enregistré",
  );
  await driver.invoke("update_settings", { patch: { selectedModel: MODEL } });
  await waitUntil(
    () =>
      driver.execute(
        `${PENDING_CALLS}
         if (pending.length > 0) return false;
         window.__oldPage = true;
         location.reload();
         return true;`,
      ),
    "appels en cours avant le rechargement",
  );
  await waitUntil(
    async () => !(await driver.execute("return window.__oldPage === true")),
    "rechargement",
  );
  await waitUntil(async () => (await shownModel(driver)) === MODEL, MODEL);
}

const WATCH = `
  const frames = (window.__frames = { last: performance.now(), worst: 0,
    worstDuring: 0, slow: 0, count: 0, doneAt: null, running: true,
    sentAt: null });
  const tick = (now) => {
    const gap = now - frames.last;
    frames.last = now;
    frames.count += 1;
    if (gap > 50) frames.slow += 1;
    if (gap > frames.worst) frames.worst = gap;
    if (frames.doneAt === null && gap > frames.worstDuring) frames.worstDuring = gap;
    if (frames.running) requestAnimationFrame(tick);
  };
  requestAnimationFrame(tick);
  document.addEventListener("keydown", (e) => {
    if (e.key === "Enter" && frames.sentAt === null) frames.sentAt = performance.now();
  }, true);
  // Fin : le bouton Stop, apparu à l'envoi, a disparu et reste absent
  // 500 ms (il peut disparaître un instant pendant le flux).
  let stopSeen = false;
  let candidate = null;
  const watch = new MutationObserver(() => {
    const stop = document.querySelector("[aria-label='Stop']") !== null;
    if (stop) {
      stopSeen = true;
      candidate = null;
    } else if (stopSeen && candidate === null) {
      const at = performance.now();
      candidate = at;
      setTimeout(() => {
        if (candidate === at && !document.querySelector("[aria-label='Stop']")) {
          frames.doneAt = at;
          watch.disconnect();
        }
      }, 500);
    }
  });
  watch.observe(document.body, { childList: true, subtree: true });`;

async function measure(label, env) {
  const fake = await startFake("ollama");
  const results = [];
  try {
    await withApp(
      binary,
      { name: `perf-${label}`, env: { OLLAMA_HOST: fake.url, ...env } },
      async (driver) => {
        await driver.waitFor("[data-testid=new-chat]");
        await useLongModel(driver);
        for (let run = 0; run < runs; run++) {
          if (run > 0) {
            await pressKeys("ctrl+n");
            await driver.waitFor("[data-testid=new-chat]");
          }
          await driver.execute(WATCH);
          await driver.click(
            await driver.waitFor("textarea[aria-label='Message']"),
          );
          await pressKeys([...chordsOf("Show me a long module."), "enter"]);
          await waitUntil(
            () => driver.execute("return window.__frames.doneAt !== null"),
            "fin de la réponse",
            60000,
          );
          await delay(1000);
          const f = await driver.execute(
            "window.__frames.running = false; return window.__frames",
          );
          results.push({
            total: Math.round(f.doneAt - f.sentAt),
            worstDuring: Math.round(f.worstDuring),
            worst: Math.round(f.worst),
            slow: f.slow,
            frames: f.count,
          });
        }
      },
    );
  } finally {
    await fake.stop();
  }
  return results;
}

const median = (values) => {
  const sorted = [...values].sort((a, b) => a - b);
  return sorted[Math.floor(sorted.length / 2)];
};

const report = {};
for (const [label, env] of [
  ["dmabuf", {}],
  ["sans-dmabuf", { WEBKIT_DISABLE_DMABUF_RENDERER: "1" }],
]) {
  const results = await measure(label, env);
  report[label] = {
    essais: results,
    médianes: Object.fromEntries(
      ["total", "worstDuring", "worst", "slow"].map((key) => [
        key,
        median(results.map((r) => r[key])),
      ]),
    ),
  };
  console.log(`${label} : ${JSON.stringify(report[label])}`);
}
console.log(`PERF ${JSON.stringify(report)}`);
