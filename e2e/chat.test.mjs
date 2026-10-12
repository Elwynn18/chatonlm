// Chat dans l'app réelle, avec le faux serveur (réponses enregistrées et
// scénarios choisis par le nom du modèle) : nouvelle conversation sans écran
// de chargement, réponse (après un raisonnement : voir thinking.test.mjs),
// message envoyé remonté en haut de la zone, après la réponse comme pendant ;
// arrêt par le bouton, par Échap, par le bouton pendant que le serveur est
// figé (Stop au-dessus du voile) et par le rechargement de la page (réponse
// partielle gardée, client parti vu par le serveur, conversation qui accepte
// un nouveau message) ; erreur du modèle effacée au message suivant ; serveur
// perdu puis retrouvé (bandeau, zone de saisie inactive).
import assert from "node:assert/strict";
import { freePort, withApp } from "./lib/app.mjs";
import { startFake } from "./lib/fake.mjs";

const binary = process.argv[2];
const port = await freePort();
let fake = await startFake("ollama", port);

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

/** Choisit un modèle (même absent de la liste), puis recharge la page. */
async function useModel(driver, model) {
  await driver.invoke("update_settings", { patch: { selectedModel: model } });
  await reload(driver);
  await waitUntil(
    async () =>
      (await driver.execute(
        "return document.querySelector('[title=\"Select model\"]')?.textContent",
      )) === model,
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
           .find((o) => o.textContent === model);
         option?.click();
         return option !== undefined;`,
        [model],
      ),
    `option ${model} du sélecteur`,
  );
  await waitUntil(
    async () =>
      (await driver.execute(
        "return document.querySelector('[title=\"Select model\"]')?.textContent",
      )) === model,
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

/** Attend que le serveur ait vu le client lâcher un flux de plus. */
async function waitAbandoned(before, what) {
  let count;
  await waitUntil(
    async () => {
      count = await fake.disconnects();
      return count > before;
    },
    () => `${what} (flux abandonnés : ${count}, avant : ${before})`,
  );
  assert.equal(count, before + 1, `${what} : un seul flux abandonné`);
}

try {
  await withApp(
    binary,
    { name: "chat", env: { OLLAMA_HOST: fake.url } },
    async (driver) => {
      // Nouvelle conversation : la page de la conversation s'ouvre sans
      // écran de chargement, la réponse enregistrée s'affiche (le modèle
      // raisonne d'abord : bouton Think actif par défaut ; le bloc de
      // raisonnement est hors de [data-role=assistant]), l'historique la
      // liste.
      await driver.waitFor("[data-testid=new-chat]");
      await waitUntil(
        async () =>
          (await driver.execute(
            "return document.querySelector('[title=\"Select model\"]').textContent",
          )) === "qwen3:0.6b",
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
        async () => (await replies(driver)).join() === '"Hello"',
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
      assert.match(first.messages[1].thinking, /^Okay, the user wants me/);
      assert.equal(first.messages[1].content, '"Hello"');
      await driver.screenshot(".tools/shots/chat-first.png");

      // Deuxième message : il remonte en haut de la zone de messages.
      await send(driver, "And again, please.");
      await waitUntil(
        async () => (await replies(driver)).length === 2,
        "deuxième réponse",
      );
      await waitSentMessageOnTop(driver, "deuxième message");
      await driver.screenshot(".tools/shots/chat-scrolled.png");

      // Réponse rapide et continue (une ligne toutes les 10 ms, donc un rendu
      // par image) : le message envoyé depuis la vue remonte en haut, au-dessus
      // de l'espace réservé, pendant la réponse. (L'ancien défaut, défilement
      // annulé à chaque rendu, ne se voit ici qu'en retard : il est vérifié de
      // façon déterministe par useMessageAutoscroll.test.tsx.)
      await newChat(driver);
      await useModel(driver, "chatonlm-fast");
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
      assert.ok(
        fast.spacer > 0,
        "aucun espace réservé pendant la réponse rapide",
      );
      assert.ok(
        await exists(driver, "button[aria-label='Stop']"),
        "réponse rapide terminée avant que le message remonte",
      );
      await waitUntil(
        async () => (await replies(driver)).length === 2,
        "seconde réponse rapide",
      );

      // Réponse qui cale après deux fragments, dans une nouvelle conversation
      // (une conversation ouverte remettrait son propre modèle) : le bouton
      // devient Stop ; l'arrêt garde le début de la réponse, et le serveur
      // voit le client partir. Sans l'arrêt, la réponse ne finirait jamais.
      await newChat(driver);
      await useModel(driver, "chatonlm-stall");
      let abandoned = await fake.disconnects();
      await send(driver, "Stall.");
      await driver.waitFor("[data-testid=chat]");
      await driver.waitFor("button[aria-label='Stop']");
      await waitUntil(
        async () => (await replies(driver)).join() === '"Hi',
        "début de la réponse",
      );
      await driver.click(await driver.find("button[aria-label='Stop']"));
      await driver.waitFor("button[aria-label='Send message']");
      await driver.screenshot(".tools/shots/chat-stopped.png");
      await waitAbandoned(abandoned, "arrêt par le bouton");
      assert.deepEqual(await savedContents(driver), ["Stall.", '"Hi']);
      assert.deepEqual(await replies(driver), ['"Hi']);
      assert.equal(await exists(driver, "[role=alert]"), false);

      // Message suivant, envoyé depuis la conversation : pendant la réponse,
      // il est remonté en haut de la zone, au-dessus d'un espace réservé qui
      // compte 40 % de la hauteur visible en plus.
      abandoned = await fake.disconnects();
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
        '"Hi',
        "Stall again.",
        '"Hi',
      ]);

      // Ollama figé pendant une réponse (la réponse reste ouverte, les
      // vérifications échouent) : bandeau et zone de saisie inactive sous un
      // voile, mais le bouton Stop reste au-dessus du voile, donc sous le
      // pointeur ; le serveur relancé voit le client parti.
      abandoned = await fake.disconnects();
      await send(driver, "Stall during an outage.");
      await waitUntil(
        async () => (await replies(driver)).length === 3,
        "début de la troisième réponse",
      );
      fake.pause();
      await driver.waitFor("[role=alert]", { timeoutMs: 10000 });
      assert.match(
        await driver.text(await driver.find("[role=alert]")),
        /Ollama isn't responding/,
      );
      assert.equal(await composerDisabled(driver), true);
      assert.equal(
        await driver.execute(`
          const stop = document.querySelector("button[aria-label='Stop']");
          const box = stop.getBoundingClientRect();
          const hit = document.elementFromPoint(
            box.left + box.width / 2, box.top + box.height / 2);
          return stop.contains(hit);`),
        true,
        "bouton Stop masqué par le voile",
      );
      await driver.screenshot(".tools/shots/chat-stalled-server.png");
      await driver.click(await driver.find("button[aria-label='Stop']"));
      await driver.waitFor("button[aria-label='Send message']");
      fake.resume();
      await waitAbandoned(abandoned, "arrêt pendant la panne");
      await waitUntil(
        async () => !(await composerDisabled(driver)),
        "serveur relancé",
      );
      assert.deepEqual((await savedContents(driver)).slice(4), [
        "Stall during an outage.",
        '"Hi',
      ]);

      // Page rechargée pendant une réponse : le flux s'arrête, la réponse
      // partielle est réaffichée, et la conversation accepte un nouveau
      // message (ici à un modèle choisi dans le sélecteur, qui répond).
      abandoned = await fake.disconnects();
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
      assert.deepEqual(await replies(driver), ['"Hi', '"Hi', '"Hi', '"Hi']);
      assert.ok(await exists(driver, "button[aria-label='Send message']"));
      assert.deepEqual((await savedContents(driver)).slice(6), [
        "Stall once more.",
        '"Hi',
      ]);
      await pickModel(driver, "qwen3:0.6b");
      await send(driver, "Now finish.");
      await waitUntil(
        async () => (await replies(driver)).at(-1) === '"Hello"',
        "réponse après le rechargement",
      );
      await driver.waitFor("button[aria-label='Send message']");
      assert.deepEqual((await savedContents(driver)).slice(8), [
        "Now finish.",
        '"Hello"',
      ]);

      // Erreur du modèle en cours de réponse : affichée, puis effacée au
      // message suivant (envoyé à un modèle choisi dans le sélecteur).
      await newChat(driver);
      await useModel(driver, "chatonlm-error");
      await send(driver, "Fail, please.");
      const alert = await driver.waitFor(
        "[data-role=message-list] [role=alert]",
      );
      assert.match(
        await driver.text(alert),
        /model runner has unexpectedly stopped/,
      );
      await driver.screenshot(".tools/shots/chat-error.png");
      await pickModel(driver, "qwen3:0.6b");
      await send(driver, "Now answer.");
      await waitUntil(
        async () =>
          !(await exists(driver, "[data-role=message-list] [role=alert]")),
        "erreur effacée",
      );
      await waitUntil(
        async () => (await savedContents(driver)).at(-1) === '"Hello"',
        "réponse après l'erreur",
      );

      // Serveur perdu : bandeau et zone de saisie inactive ; retrouvé : tout
      // revient.
      await fake.stop();
      await driver.waitFor("[role=alert]", { timeoutMs: 8000 });
      assert.match(
        await driver.text(await driver.find("[role=alert]")),
        /Ollama isn't responding/,
      );
      assert.equal(await composerDisabled(driver), true);
      await driver.screenshot(".tools/shots/chat-server-lost.png");
      fake = await startFake("ollama", port);
      await waitUntil(
        async () => !(await composerDisabled(driver)),
        "serveur retrouvé",
      );
    },
  );
} finally {
  await fake.stop();
}
