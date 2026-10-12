// Téléchargement d'un modèle absent et sélecteur de modèles dans l'app réelle
// (build e2e : frontend embarqué, CSP de production), avec le faux serveur
// (scénarios chatonlm-pull*, contrat de l'étape 10, § 3 et § 4), qui sert
// aussi de registre (CHATONLM_REGISTRY_URL) :
// - sélecteur : modèles recommandés par le serveur (gemma4, qwen3.5) en tête,
//   avec l'icône de téléchargement, puis les modèles installés ; nom tapé
//   llama3.2 proposé après vérification au registre, nom inconnu non proposé ;
//   le nom choisi est seulement enregistré, en fin de liste avec l'icône ;
// - chatonlm-pull-slow : bloc « Downloading model » avec « x / 4.7 GB (n%) »
//   qui progresse, sans trois points ; Stop : bouton d'envoi revenu, bloc
//   retiré, téléchargement abandonné vu par le faux serveur, aucune réponse
//   ni message vide en base ;
// - chatonlm-pull : le bloc arrive à 100 %, s'efface en fondu (opacité et
//   translation, 300 ms), puis la réponse arrive ;
// - chatonlm-pull-offline : message d'erreur officiel, sans bloc ;
// - aucune violation de la CSP ni erreur ou avertissement dans la console.
import assert from "node:assert/strict";
import { withApp } from "./lib/app.mjs";
import { startFake } from "./lib/fake.mjs";
import { PENDING_CALLS } from "./lib/page.mjs";

const binary = process.argv[2];
const fake = await startFake("ollama");
const env = { OLLAMA_HOST: fake.url, CHATONLM_REGISTRY_URL: fake.url };

const HELLO = "Say hello in one word.";
const OFFLINE =
  "Unable to download model. Please check your internet connection to download the model for offline use.";
/** Progression de la grosse couche des scénarios de pull (4.7 GB). */
const PROGRESS = /^(\d+\.\d) GB \/ 4\.7 GB \((\d+)%\)$/;

const delay = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

/** `what` : texte, ou fonction appelée à l'échec (dernier état observé). */
async function waitUntil(check, what, timeoutMs = 10000) {
  const deadline = Date.now() + timeoutMs;
  for (;;) {
    const value = await check();
    if (value) return value;
    if (Date.now() > deadline) {
      throw new Error(
        `Délai dépassé : ${typeof what === "function" ? what() : what}`,
      );
    }
    await delay(100);
  }
}

const exists = (driver, css) =>
  driver.execute("return document.querySelector(arguments[0]) !== null", [css]);

// Violations de la CSP, erreurs et avertissements de la console : notés par
// la page (écouteurs reposés après chaque rechargement), relevés avant chaque
// rechargement et à la fin.
const violations = [];
const consoleErrors = [];
const consoleWarnings = [];
const WATCH_PAGE = `
  if (!window.__csp) {
    window.__csp = [];
    window.__errors = [];
    window.__warnings = [];
    document.addEventListener("securitypolicyviolation", (event) =>
      window.__csp.push(event.violatedDirective + " " + event.blockedURI));
    const error = console.error;
    console.error = (...args) => {
      window.__errors.push(args.map(String).join(" "));
      error.apply(console, args);
    };
    const warn = console.warn;
    console.warn = (...args) => {
      window.__warnings.push(args.map(String).join(" "));
      warn.apply(console, args);
    };
  }`;

async function collectViolations(driver) {
  const found = await driver.execute(
    `const found = { csp: window.__csp ?? [], errors: window.__errors ?? [],
       warnings: window.__warnings ?? [] };
     window.__csp = [];
     window.__errors = [];
     window.__warnings = [];
     return found;`,
  );
  violations.push(...found.csp);
  consoleErrors.push(...found.errors);
  consoleWarnings.push(...found.warnings);
}

/**
 * Recharge la page et attend la nouvelle (écouteurs reposés), seulement quand
 * aucun appel au backend n'est en cours (voir thinking.test.mjs).
 */
async function reload(driver) {
  await collectViolations(driver);
  await waitUntil(
    () =>
      driver.execute(
        `${PENDING_CALLS}
         if (pending.length > 0) return false;
         window.__oldPage = true;
         location.reload();
         return true;`,
      ),
    "appels au backend toujours en cours avant le rechargement",
  );
  await waitUntil(
    async () => !(await driver.execute("return window.__oldPage === true")),
    "rechargement",
  );
  await driver.execute(WATCH_PAGE);
}

const shownModel = (driver) =>
  driver.execute(
    "return document.querySelector('[title=\"Select model\"]')?.textContent ?? null",
  );

/** Choisit un modèle (absent du serveur) par le réglage, puis recharge. */
async function useModel(driver, model) {
  assert.ok(await shownModel(driver), "aucun modèle affiché avant useModel");
  await driver.invoke("update_settings", { patch: { selectedModel: model } });
  await reload(driver);
  await waitUntil(
    async () => (await shownModel(driver)) === model,
    `modèle ${model}`,
  );
}

/** Nouvelle conversation, par le bouton « New chat ». */
async function newChat(driver) {
  await driver.click(await driver.waitFor("a[title='New chat']"));
  await driver.waitFor("[data-testid=new-chat]");
}

/** Tape un message et l'envoie avec Entrée. */
async function send(driver, text) {
  await driver.type(
    await driver.waitFor("textarea[aria-label='Message']"),
    text,
  );
  await driver.execute(
    `document.querySelector("textarea[aria-label='Message']").dispatchEvent(
       new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true }));`,
  );
}

/** Identifiant de la conversation affichée. */
const currentChatId = (driver) =>
  driver.execute(
    "return (location.hash + location.pathname).match(/\\/c\\/([^/?#]+)/)?.[1] ?? null",
  );

/** Contenus des messages enregistrés d'une conversation. */
async function savedContents(driver, id) {
  const chat = await driver.invoke("get_chat", { id });
  return chat.messages.map((message) => `${message.role}:${message.content}`);
}

/** Lignes du sélecteur ouvert : nom, et icône « non téléchargé ». */
const pickerRows = (driver) =>
  driver.execute(
    `return [...document.querySelectorAll("[role=option]")].map((o) => ({
       model: o.textContent,
       missing: o.querySelector("[aria-label='Not downloaded']") !== null,
     }))`,
  );

/** Ouvre le sélecteur et attend que sa première ligne soit `first`. */
async function openPicker(driver, first) {
  await driver.click(await driver.find("[title='Select model']"));
  await driver.waitFor("[role=listbox]");
  let rows;
  await waitUntil(
    async () => {
      rows = await pickerRows(driver);
      return rows[0]?.model === first;
    },
    () => `première ligne ${first} : ${JSON.stringify(rows)}`,
  );
  return rows;
}

/** Ferme le sélecteur par son bouton (la recherche est effacée). */
async function closePicker(driver) {
  await driver.click(await driver.find("[title='Select model']"));
  await waitUntil(
    async () => !(await exists(driver, "[role=listbox]")),
    "sélecteur fermé",
  );
}

/** Bloc de téléchargement affiché : titre, progression, classes ; ou null. */
const downloadBlock = (driver) =>
  driver.execute(`
    const section = document.querySelector("[data-testid=downloading]");
    if (!section) return null;
    return {
      title: section.querySelector(".ml-6")?.textContent ?? null,
      progress: section.querySelector(".text-sm")?.textContent ?? null,
      className: section.className,
    };`);

/**
 * Note dans la page, pendant un envoi : chaque progression affichée, les
 * trois points vus pendant le téléchargement, l'état du bloc quand il passe
 * à « fini » (transitions en cours) et l'heure d'arrivée de la réponse.
 */
const WATCH_DOWNLOAD = `
  window.__download?.stop();
  const state = { progress: [], dotsWhileDownloading: false, done: null,
    doneAt: null, replyAt: null };
  const check = () => {
    // Même heure pour tout ce qui change dans un même rendu.
    const now = performance.now();
    const section = document.querySelector("[data-testid=downloading]");
    const reply = document.querySelector("[data-role=assistant]");
    if (reply && state.replyAt === null) state.replyAt = now;
    if (!section) return;
    const progress = section.querySelector(".text-sm")?.textContent;
    if (progress && state.progress.at(-1) !== progress) state.progress.push(progress);
    const finished = section.className.includes("opacity-0");
    if (!finished && document.querySelector("[data-testid=waiting]")) {
      state.dotsWhileDownloading = true;
    }
    if (finished && state.done === null) {
      state.doneAt = now;
      const style = getComputedStyle(section);
      state.done = {
        className: section.className,
        duration: style.transitionDuration,
        timing: style.transitionTimingFunction,
        transitions: section.getAnimations().map((a) => a.transitionProperty),
        replyShown: reply !== null,
      };
    }
  };
  const observer = new MutationObserver(check);
  observer.observe(document.body,
    { childList: true, subtree: true, attributes: true, characterData: true });
  window.__download = { state, stop: () => observer.disconnect() };`;

const downloadState = (driver) =>
  driver.execute("return window.__download.state");

/** fn(driver), avec une capture de la fenêtre en cas d'échec. */
const shotOnFailure = (name, fn) => async (driver) => {
  try {
    await fn(driver);
  } catch (error) {
    await driver.screenshot(`.tools/shots/${name}-failure.png`).catch(() => {});
    throw error;
  }
};

try {
  await withApp(
    binary,
    { name: "download", env },
    shotOnFailure("download", async (driver) => {
      await driver.waitFor("[data-testid=new-chat]");
      await driver.execute(WATCH_PAGE);
      // Modèle par défaut : le plus récemment installé, jamais un recommandé.
      await waitUntil(
        async () => (await shownModel(driver)) === "qwen3:0.6b",
        "modèle par défaut",
      );

      // Sélecteur : recommandés en tête (non installés : icône), puis les
      // modèles installés.
      let rows = await openPicker(driver, "gemma4");
      assert.deepEqual(rows, [
        { model: "gemma4", missing: true },
        { model: "qwen3.5", missing: true },
        { model: "qwen3:0.6b", missing: false },
        { model: "qwen3.5:0.8b", missing: false },
      ]);
      await driver.screenshot(".tools/shots/download-picker.png");

      // Nom inconnu : rien n'est proposé, même après la réponse du registre.
      await driver.type(
        await driver.find("[aria-label='Find model']"),
        "nosuchmodel",
      );
      await delay(1500);
      assert.deepEqual(await pickerRows(driver), []);
      assert.equal(
        await driver.text(await driver.find("[role=listbox]")),
        "No models found",
      );
      await closePicker(driver);

      // Nom présent au registre : proposé avec l'icône, puis choisi (il
      // sera téléchargé au prochain envoi).
      await openPicker(driver, "gemma4");
      await driver.type(
        await driver.find("[aria-label='Find model']"),
        "llama3.2",
      );
      await waitUntil(
        async () => (await pickerRows(driver))[0]?.model === "llama3.2",
        "llama3.2 proposé après le registre",
      );
      assert.deepEqual(await pickerRows(driver), [
        { model: "llama3.2", missing: true },
      ]);
      await driver.screenshot(".tools/shots/download-picker-registry.png");
      await driver.click(await driver.find("[role=option]"));
      await waitUntil(
        async () => (await shownModel(driver)) === "llama3.2",
        "llama3.2 choisi",
      );
      assert.equal(
        (await driver.invoke("get_settings")).selectedModel,
        "llama3.2",
      );
      rows = await openPicker(driver, "gemma4");
      assert.deepEqual(rows.at(-1), { model: "llama3.2", missing: true });
      assert.equal(rows.length, 5);
      await closePicker(driver);

      // chatonlm-pull-slow : progression visible, sans trois points ; Stop
      // arrête le téléchargement.
      await useModel(driver, "chatonlm-pull-slow");
      const abandoned = await fake.disconnects();
      await driver.execute(WATCH_DOWNLOAD);
      await send(driver, HELLO);
      await driver.waitFor("[data-testid=chat]");
      const slowChat = await currentChatId(driver);
      let block;
      await waitUntil(
        async () => {
          block = await downloadBlock(driver);
          return block !== null && PROGRESS.test(block.progress);
        },
        () => `bloc de téléchargement : ${JSON.stringify(block)}`,
      );
      assert.equal(block.title, "Downloading model");
      assert.match(block.className, /opacity-100 translate-y-0/);
      assert.ok(await exists(driver, "button[aria-label='Stop']"));
      const firstPercent = Number(PROGRESS.exec(block.progress)[2]);
      await waitUntil(
        async () => {
          block = await downloadBlock(driver);
          const match = PROGRESS.exec(block?.progress ?? "");
          return match !== null && Number(match[2]) > firstPercent;
        },
        () => `progression après ${firstPercent} % : ${block?.progress}`,
      );
      // Plus de 750 ms de téléchargement : les points seraient là.
      await delay(1000);
      assert.equal(await exists(driver, "[data-testid=waiting]"), false);
      await driver.screenshot(".tools/shots/download-progress.png");
      const percentAtStop = Number(PROGRESS.exec(block.progress)[2]);
      assert.ok(percentAtStop < 100, `déjà fini : ${block.progress}`);

      await driver.click(await driver.find("button[aria-label='Stop']"));
      await driver.waitFor("button[aria-label='Send message']");
      assert.equal(await downloadBlock(driver), null);
      await waitUntil(
        async () => (await fake.disconnects()) === abandoned + 1,
        "téléchargement abandonné vu par le faux serveur",
      );
      const slowPulls = (await fake.pulls()).filter(
        (pull) => pull.model === "chatonlm-pull-slow",
      );
      assert.equal(slowPulls.length, 1);
      let state = await downloadState(driver);
      assert.equal(state.dotsWhileDownloading, false);
      assert.equal(state.done, null, "fini malgré Stop");
      assert.ok(state.progress.length >= 2, JSON.stringify(state.progress));
      assert.ok(
        state.progress.every((text) => PROGRESS.test(text)),
        JSON.stringify(state.progress),
      );
      assert.equal(await exists(driver, "[data-role=assistant]"), false);
      // Ni réponse, ni message vide en base.
      await waitUntil(
        async () =>
          JSON.stringify(await savedContents(driver, slowChat)) ===
          JSON.stringify([`user:${HELLO}`]),
        "seul le message envoyé est enregistré",
      );

      // chatonlm-pull : 100 %, fondu, puis la réponse.
      await newChat(driver);
      await useModel(driver, "chatonlm-pull");
      await driver.execute(WATCH_DOWNLOAD);
      await send(driver, HELLO);
      await driver.waitFor("[data-testid=chat]");
      const pullChat = await currentChatId(driver);
      await waitUntil(async () => {
        const replies = await driver.execute(
          "return [...document.querySelectorAll('[data-role=assistant]')].map((r) => r.textContent)",
        );
        return replies.length === 1 && replies[0].trim() !== "";
      }, "réponse après le téléchargement");
      await driver.waitFor("button[aria-label='Send message']");
      state = await downloadState(driver);
      assert.equal(state.progress.at(-1), "4.7 GB / 4.7 GB (100%)");
      assert.ok(
        state.progress.every((text) => PROGRESS.test(text)),
        JSON.stringify(state.progress),
      );
      assert.ok(state.done, "bloc jamais passé à « fini »");
      assert.match(state.done.className, /opacity-0 -translate-y-8/);
      assert.match(
        state.done.className,
        /transition-all ease-out duration-300/,
      );
      assert.equal(state.done.duration, "0.3s");
      // ease-out de Tailwind 4 (--ease-out), comme l'officiel.
      assert.equal(state.done.timing, "cubic-bezier(0, 0, 0.2, 1)");
      assert.ok(
        state.done.transitions.includes("opacity"),
        `transitions : ${JSON.stringify(state.done.transitions)}`,
      );
      assert.ok(state.doneAt <= state.replyAt, "réponse avant la fin du bloc");
      assert.equal(state.dotsWhileDownloading, false);
      // Réponse finie : plus de bloc.
      assert.equal(await downloadBlock(driver), null);
      const saved = await savedContents(driver, pullChat);
      assert.equal(saved.length, 2);
      assert.equal(saved[0], `user:${HELLO}`);
      assert.match(saved[1], /^assistant:\S/);
      assert.equal(
        (await fake.pulls()).filter((pull) => pull.model === "chatonlm-pull")
          .length,
        1,
      );

      // chatonlm-pull-offline : message officiel, pas de bloc.
      await newChat(driver);
      await useModel(driver, "chatonlm-pull-offline");
      await send(driver, HELLO);
      await driver.waitFor("[data-testid=chat]");
      const alert = await driver.waitFor(
        "[data-role=message-list] [role=alert]",
      );
      assert.ok(
        (await driver.text(alert)).includes(OFFLINE),
        await driver.text(alert),
      );
      await driver.waitFor("button[aria-label='Send message']");
      assert.equal(await downloadBlock(driver), null);
      assert.equal(await exists(driver, "[data-role=assistant]"), false);
      await driver.screenshot(".tools/shots/download-offline.png");
      await collectViolations(driver);
    }),
  );
} finally {
  await fake.stop();
}

assert.deepEqual(violations, [], "violations de la CSP");
assert.deepEqual(consoleErrors, [], "erreurs dans la console");
assert.deepEqual(consoleWarnings, [], "avertissements dans la console");
