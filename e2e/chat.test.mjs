// Chat dans l'app réelle, avec le faux moteur (fake-llama lancé par l'app,
// scénario choisi par le nom du modèle) : nouvelle conversation sans écran
// de chargement, réponse (après un raisonnement : voir thinking.test.mjs),
// message envoyé remonté en haut de la zone, après la réponse comme pendant ;
// arrêt par le bouton, par Échap, par le bouton pendant que le processus du
// modèle est figé (SIGSTOP) et par le rechargement de la page (réponse
// partielle gardée, client parti vu par le faux, conversation qui accepte
// un nouveau message) ; erreur du modèle effacée au message suivant ;
// processus du modèle tué entre deux messages : relancé au suivant, qui
// reçoit sa réponse.
import assert from "node:assert/strict";
import { withApp } from "./lib/app.mjs";
import { fakeModel, startEngine } from "./lib/fake.mjs";
import { shownModel } from "./lib/page.mjs";

const binary = process.argv[2];
/** Le premier : modèle choisi par défaut (le plus récent). Raisonne. */
const DEFAULT = fakeModel("think");
/** Réponse rapide et continue (un morceau toutes les 10 ms). */
const BRISK = fakeModel("brisk");
/** Un morceau (« Partial answer »), puis plus rien tant que le client reste. */
const STALL = fakeModel("editor-stall");
/** Trois morceaux, puis une erreur 500 dans le flux. */
const ERROR = fakeModel("error");
const engine = startEngine([DEFAULT, BRISK, STALL, ERROR]);
/** Réponse du faux à un message (scénarios `think` et `simple`). */
const hello = (text) => `Hello! You said: ${text}`;
const PARTIAL = "Partial answer";

/** `what` : texte, ou fonction appelée à l'échec (dernier état observé). */
async function waitUntil(check, what, timeoutMs = 8000) {
  const deadline = Date.now() + timeoutMs;
  for (;;) {
    const value = await check();
    if (value) return value;
    if (Date.now() > deadline) {
      throw new Error(
        `Délai dépassé : ${typeof what === "function" ? what() : what}`,
      );
    }
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
}

const texts = (driver, css) =>
  driver.execute(
    "return [...document.querySelectorAll(arguments[0])].map((e) => e.textContent)",
    [css],
  );

const composerDisabled = (driver) =>
  driver.execute(
    "return document.querySelector(\"textarea[aria-label='Message']\").disabled",
  );

const exists = (driver, css) =>
  driver.execute("return document.querySelector(arguments[0]) !== null", [css]);

/** Tape un message et l'envoie avec Entrée. */
async function send(driver, text) {
  const textarea = await driver.waitFor("textarea[aria-label='Message']");
  await driver.type(textarea, text);
  await driver.execute(
    `document.querySelector("textarea[aria-label='Message']").dispatchEvent(
       new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true }));`,
  );
}

/** Recharge la page et attend la nouvelle. */
async function reload(driver) {
  await driver.execute("window.__oldPage = true; location.reload()");
  await waitUntil(
    async () => !(await driver.execute("return window.__oldPage === true")),
    "rechargement",
  );
}

/** Choisit un modèle par le réglage, puis recharge la page. */
async function useModel(driver, model) {
  await driver.invoke("update_settings", { patch: { selectedModel: model } });
  await reload(driver);
  await waitUntil(
    async () => (await shownModel(driver)) === model,
    `modèle ${model}`,
  );
}

/** Choisit un modèle dans le sélecteur, une fois sa liste lue. */
async function pickModel(driver, model) {
  await driver.click(await driver.find("[title='Select model']"));
  await waitUntil(
    () =>
      driver.execute(
        `const model = arguments[0];
         const option = [...document.querySelectorAll("[role=option]")]
           .find((o) => o.dataset.modelId === model);
         option?.click();
         return option !== undefined;`,
        [model],
      ),
    `option ${model} du sélecteur`,
  );
  await waitUntil(
    async () => (await shownModel(driver)) === model,
    `modèle ${model} choisi`,
  );
}

const replies = (driver) => texts(driver, "[data-role=assistant]");

/** Contenu des messages enregistrés de la conversation la plus récente. */
async function savedContents(driver) {
  const id = (await driver.invoke("list_chats"))[0].id;
  const chat = await driver.invoke("get_chat", { id });
  return chat.messages.map((message) => message.content);
}

/** Nouvelle conversation, par le bouton « New chat » (barre latérale fermée). */
async function newChat(driver) {
  await driver.click(await driver.find("a[title='New chat']"));
  await driver.waitFor("[data-testid=new-chat]");
}

/** Arrête la réponse avec Échap (ailleurs que dans un champ). */
const pressEscape = (driver) =>
  driver.execute(
    "window.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', cancelable: true }))",
  );

/** Écart (px) entre le dernier message envoyé et le haut de la zone. */
const sentMessageOffset = (driver) =>
  driver.execute(`
    const section = document.querySelector("[data-testid=chat] section");
    const items = [...section.querySelectorAll("[data-message-index]")];
    const last = items.filter((i) => i.querySelector("[data-role=user]")).at(-1);
    const padding = parseFloat(getComputedStyle(section).paddingTop);
    return last.getBoundingClientRect().top - section.getBoundingClientRect().top - padding;`);

/** Attend que le message envoyé soit en haut (défilement doux terminé). */
async function waitSentMessageOnTop(driver, what) {
  let offset;
  await waitUntil(
    async () => {
      offset = await sentMessageOffset(driver);
      return Math.abs(offset) < 4;
    },
    () => `${what} (message envoyé à ${offset} px du haut)`,
  );
}

/** Espace réservé sous les messages, et hauteur visible de la zone (px). */
const spacerAndView = (driver) =>
  driver.execute(`
    const section = document.querySelector("[data-testid=chat] section");
    const spacer = section.querySelector(
      "[data-role=message-list] > [aria-hidden=true]:last-child");
    return { spacer: spacer.getBoundingClientRect().height, view: section.clientHeight };`);

/** Flux abandonnés par l'app, vus par l'instance du modèle `model`. */
const disconnects = (model = STALL) => engine.disconnects(model);

/** Attend que le faux ait vu le client lâcher un flux de plus. */
async function waitAbandoned(before, what) {
  let count;
  await waitUntil(
    async () => {
      count = await disconnects();
      return count > before;
    },
    () => `${what} (flux abandonnés : ${count}, avant : ${before})`,
  );
  assert.equal(count, before + 1, `${what} : un seul flux abandonné`);
}

await withApp(binary, { name: "chat", env: engine.env }, async (driver) => {
  // Nouvelle conversation : la page de la conversation s'ouvre sans
  // écran de chargement, la réponse enregistrée s'affiche (le modèle
  // raisonne d'abord : bouton Think actif par défaut ; le bloc de
  // raisonnement est hors de [data-role=assistant]), l'historique la
  // liste.
  await driver.waitFor("[data-testid=new-chat]");
  await waitUntil(
    async () => (await shownModel(driver)) === DEFAULT,
    "modèle par défaut",
  );
  await driver.execute(`
        window.__sawLoading = false;
        window.__loadingWatch = new MutationObserver(() => {
          if (document.body.textContent.includes("Loading chat...")) {
            window.__sawLoading = true;
          }
        });
        window.__loadingWatch.observe(document.body,
          { childList: true, subtree: true, characterData: true });`);
  await send(driver, "Say hello in one word.");
  await driver.waitFor("[data-testid=chat]");
  await waitUntil(
    async () =>
      (await replies(driver)).join() === hello("Say hello in one word."),
    "première réponse",
  );
  assert.equal(
    await driver.execute(
      "window.__loadingWatch.disconnect(); return window.__sawLoading",
    ),
    false,
    "« Loading chat... » affiché à la création",
  );
  // Texte de la bulle, sans le bouton « edit ».
  assert.deepEqual(
    await texts(driver, "[data-role=user] .whitespace-pre-wrap"),
    ["Say hello in one word."],
  );
  const chats = await driver.invoke("list_chats");
  assert.equal(chats.length, 1);
  assert.equal(chats[0].userExcerpt, "Say hello in one word.");
  const first = await driver.invoke("get_chat", { id: chats[0].id });
  assert.match(first.messages[1].thinking, /^Let me think/);
  assert.equal(first.messages[1].content, hello("Say hello in one word."));
  await driver.screenshot(".tools/shots/chat-first.png");

  // Deuxième message : il remonte en haut de la zone de messages.
  await send(driver, "And again, please.");
  await waitUntil(
    async () => (await replies(driver)).length === 2,
    "deuxième réponse",
  );
  await waitSentMessageOnTop(driver, "deuxième message");
  await driver.screenshot(".tools/shots/chat-scrolled.png");

  // Réponse rapide et continue (un morceau toutes les 10 ms, donc un rendu
  // par image) : le message envoyé depuis la vue remonte en haut, au-dessus
  // de l'espace réservé, pendant la réponse. (L'ancien défaut, défilement
  // annulé à chaque rendu, ne se voit ici qu'en retard : il est vérifié de
  // façon déterministe par useMessageAutoscroll.test.tsx.)
  await newChat(driver);
  await useModel(driver, BRISK);
  await send(driver, "Fast.");
  await driver.waitFor("[data-testid=chat]");
  await waitUntil(
    async () => (await replies(driver)).length === 1,
    "première réponse rapide",
  );
  await driver.waitFor("button[aria-label='Send message']");
  await send(driver, "Fast again.");
  await driver.waitFor("button[aria-label='Stop']");
  await waitSentMessageOnTop(
    driver,
    "message envoyé pendant une réponse rapide",
  );
  const fast = await spacerAndView(driver);
  assert.ok(fast.spacer > 0, "aucun espace réservé pendant la réponse rapide");
  assert.ok(
    await exists(driver, "button[aria-label='Stop']"),
    "réponse rapide terminée avant que le message remonte",
  );
  await waitUntil(
    async () => (await replies(driver)).length === 2,
    "seconde réponse rapide",
  );

  // Réponse qui cale après un morceau, dans une nouvelle conversation
  // (une conversation ouverte remettrait son propre modèle) : le bouton
  // devient Stop ; l'arrêt garde le début de la réponse, et le faux
  // voit le client partir. Sans l'arrêt, la réponse ne finirait jamais.
  await newChat(driver);
  await useModel(driver, STALL);
  let abandoned = await disconnects();
  await send(driver, "Stall.");
  await driver.waitFor("[data-testid=chat]");
  await driver.waitFor("button[aria-label='Stop']");
  await waitUntil(
    async () => (await replies(driver)).join() === PARTIAL,
    "début de la réponse",
  );
  await driver.click(await driver.find("button[aria-label='Stop']"));
  await driver.waitFor("button[aria-label='Send message']");
  await driver.screenshot(".tools/shots/chat-stopped.png");
  await waitAbandoned(abandoned, "arrêt par le bouton");
  assert.deepEqual(await savedContents(driver), ["Stall.", PARTIAL]);
  assert.deepEqual(await replies(driver), [PARTIAL]);
  assert.equal(await exists(driver, "[role=alert]"), false);

  // Message suivant, envoyé depuis la conversation : pendant la réponse,
  // il est remonté en haut de la zone, au-dessus d'un espace réservé qui
  // compte 40 % de la hauteur visible en plus.
  abandoned = await disconnects();
  await send(driver, "Stall again.");
  await driver.waitFor("button[aria-label='Stop']");
  await waitUntil(
    async () => (await replies(driver)).length === 2,
    "début de la seconde réponse",
  );
  await waitSentMessageOnTop(driver, "message envoyé pendant la réponse");
  const { spacer, view } = await spacerAndView(driver);
  assert.ok(
    spacer >= Math.floor(view * 0.4),
    `espace réservé de ${spacer} px pour une zone de ${view} px`,
  );
  assert.ok(
    await exists(driver, "button[aria-label='Stop']"),
    "réponse terminée avant la mesure",
  );
  await driver.screenshot(".tools/shots/chat-streaming.png");

  // Échap arrête aussi la réponse.
  await pressEscape(driver);
  await driver.waitFor("button[aria-label='Send message']");
  await waitAbandoned(abandoned, "arrêt par Échap");
  assert.deepEqual(await savedContents(driver), [
    "Stall.",
    PARTIAL,
    "Stall again.",
    PARTIAL,
  ]);

  // Processus du modèle figé (SIGSTOP) pendant une réponse : rien ne
  // répond plus, mais Stop arrête quand même (la zone de saisie revient) ;
  // relancé, le faux voit le client parti. Plus de bandeau « serveur qui
  // ne répond pas » : une panne du moteur se dit dans la conversation
  // (EngineErrorActions).
  abandoned = await disconnects();
  await send(driver, "Stall during an outage.");
  await waitUntil(
    async () => (await replies(driver)).length === 3,
    "début de la troisième réponse",
  );
  assert.ok(engine.pause(STALL), "processus du modèle introuvable");
  try {
    await driver.waitFor("button[aria-label='Stop']");
    assert.equal(await composerDisabled(driver), false);
    await driver.screenshot(".tools/shots/chat-stalled-server.png");
    await driver.click(await driver.find("button[aria-label='Stop']"));
    await driver.waitFor("button[aria-label='Send message']");
  } finally {
    engine.resume(STALL);
  }
  await waitAbandoned(abandoned, "arrêt pendant la panne");
  assert.equal(await exists(driver, "[role=alert]"), false);
  assert.deepEqual((await savedContents(driver)).slice(4), [
    "Stall during an outage.",
    PARTIAL,
  ]);

  // Page rechargée pendant une réponse : le flux s'arrête, la réponse
  // partielle est réaffichée, et la conversation accepte un nouveau
  // message (ici à un modèle choisi dans le sélecteur, qui répond).
  abandoned = await disconnects();
  await send(driver, "Stall once more.");
  await waitUntil(
    async () => (await replies(driver)).length === 4,
    "début de la quatrième réponse",
  );
  await reload(driver);
  await waitAbandoned(abandoned, "rechargement pendant la réponse");
  await driver.waitFor("[data-testid=chat]");
  await waitUntil(
    async () => (await replies(driver)).length === 4,
    "réponses réaffichées",
  );
  assert.deepEqual(await replies(driver), [PARTIAL, PARTIAL, PARTIAL, PARTIAL]);
  assert.ok(await exists(driver, "button[aria-label='Send message']"));
  assert.deepEqual((await savedContents(driver)).slice(6), [
    "Stall once more.",
    PARTIAL,
  ]);
  await pickModel(driver, DEFAULT);
  await send(driver, "Now finish.");
  await waitUntil(
    async () => (await replies(driver)).at(-1) === hello("Now finish."),
    "réponse après le rechargement",
  );
  await driver.waitFor("button[aria-label='Send message']");
  assert.deepEqual((await savedContents(driver)).slice(8), [
    "Now finish.",
    hello("Now finish."),
  ]);

  // Erreur du modèle en cours de réponse : affichée, puis effacée au
  // message suivant (envoyé à un modèle choisi dans le sélecteur).
  await newChat(driver);
  await useModel(driver, ERROR);
  await send(driver, "Fail, please.");
  const alert = await driver.waitFor("[data-role=message-list] [role=alert]");
  // Bloc d'erreur (500) au milieu du flux : texte du moteur.
  assert.match(await driver.text(alert), /answered with an error \(HTTP 500\)/);
  await driver.screenshot(".tools/shots/chat-error.png");
  await pickModel(driver, DEFAULT);
  await send(driver, "Now answer.");
  await waitUntil(
    async () =>
      !(await exists(driver, "[data-role=message-list] [role=alert]")),
    "erreur effacée",
  );
  await waitUntil(
    async () => (await savedContents(driver)).at(-1) === hello("Now answer."),
    "réponse après l'erreur",
  );

  // Processus du modèle tué entre deux messages (plantage, OOM) : le
  // message suivant le relance (nouveau processus) et reçoit sa réponse,
  // sans erreur dans la conversation.
  const lost = engine.server(DEFAULT);
  assert.ok(lost, "processus du modèle par défaut introuvable");
  process.kill(lost.pid, "SIGKILL");
  await waitUntil(
    async () => !engine.server(DEFAULT),
    "processus du modèle arrêté",
  );
  await send(driver, "After the crash.");
  await waitUntil(
    async () =>
      (await savedContents(driver)).at(-1) === hello("After the crash."),
    "réponse après la perte du processus",
    20000,
  );
  const relaunched = engine.server(DEFAULT);
  assert.ok(relaunched && relaunched.pid !== lost.pid, "pas relancé");
  assert.equal(
    await exists(driver, "[data-role=message-list] [role=alert]"),
    false,
  );
  await driver.screenshot(".tools/shots/chat-server-lost.png");
});
