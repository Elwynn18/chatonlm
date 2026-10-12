// Mode conversation vocale (propre à ChatonLM) dans l'app réelle : faux
// serveur Ollama, vrai Parakeet (lien vers .tools/speech-models), vraie voix
// française (.tools/voice-models), sortie audio factice
// (CHATONLM_E2E_AUDIO_OUT=null, au rythme réel) et faux micro
// (CHATONLM_E2E_VOICE_MIC : débuts de fr0.wav et fr1.wav, voir
// e2e/lib/voice.mjs), mesures du moteur (CHATONLM_VOICE_TIMINGS=1) :
// 1. prérequis : voix française absente (langue de la dictée) → prérequis
//    lus, voice_mode_start refusé, boîte d'accord (taille de la voix, pas de
//    Parakeet, installé) ; Cancel : rien de téléchargé, pas de séance ;
// 2. deux tours (chatonlm-voice) depuis une nouvelle conversation : phrase
//    transcrite (mots de fr0) = message de l'utilisateur, conversation créée
//    par l'envoi et gardée par la séance, réponse écrite puis lue (Speaking),
//    retour à l'écoute, 2e tour, dont la lecture est coupée par Espace (vraie
//    touche) ; requêtes reçues par le faux serveur : consigne vocale en
//    premier message `system`, `think: false` ; historique sans consigne ;
//    niveaux du micro et de la voix ; latence relevée (fin de la parole et du
//    WAV → premier son, lignes chatonlm-voice-timing) ; micro coupé puis
//    rouvert, Échap ; changement de conversation : séance arrêtée ; brouillon
//    rendu à la fin (séance lancée par Ctrl+Maj+Espace, finie par Échap) ;
// 3. interruption en parlant (chatonlm-voice-long, phrase du faux micro jouée
//    dès que la voix parle) : lecture coupée (plus de son), génération
//    arrêtée (flux abandonné, début de réponse gardé), la phrase devient le
//    2e message ; End pendant la lecture de la 2e réponse : séance finie,
//    lecture arrêtée, la génération continue jusqu'au bout.
import assert from "node:assert/strict";
import { existsSync, rmSync } from "node:fs";
import { setTimeout as delay } from "node:timers/promises";
import { ROOT, withApp } from "./lib/app.mjs";
import { startFake } from "./lib/fake.mjs";
import { pressKeys } from "./lib/keyboard.mjs";
import { watchLog } from "./lib/log.mjs";
import {
  PARAKEET,
  PHRASES,
  barState,
  describeStates,
  firstSoundDelay,
  heard,
  inOrder,
  levelPeaks,
  linkParakeet,
  linkVoice,
  recordVoiceEvents,
  stateAfter,
  voiceEvents,
  voiceTimings,
  waitVoice,
  wavEnds,
  writePhrase,
} from "./lib/voice.mjs";

const binary = process.argv[2];
const DATA = process.env.CHATONLM_DATA_DIR;
assert.ok(
  DATA,
  "CHATONLM_DATA_DIR absent : lancer dans scripts/e2e-session.sh",
);
assert.ok(
  process.env.ORT_DYLIB_PATH && existsSync(process.env.ORT_DYLIB_PATH),
  "ORT_DYLIB_PATH non transmis ou absent : scripts/fetch-onnxruntime.sh",
);

const TMP = `${ROOT}.tools/tmp/voice-mode-${process.pid}`;
const FRENCH = "pocket-tts-fr";
const TEXTAREA = "textarea[aria-label='Message']";
const DIALOG = ":is([role='alertdialog'], [role='dialog'])";
/** Réponses du faux serveur (VOICE_REPLIES de fake-ollama). */
const REPLIES = [
  "Je vous entends très bien. Que puis-je faire pour vous ?",
  "C'est une belle histoire, tirée des Mille et Une Nuits. Voulez-vous que je vous raconte ce que devient ensuite le pêcheur ?",
];
/** Dernière phrase de la longue réponse (VOICE_LONG_END de fake-ollama). */
const LONG_END = "Fin de l'histoire.";

async function waitUntil(check, what, timeoutMs = 10000) {
  const deadline = Date.now() + timeoutMs;
  for (;;) {
    const value = await check();
    if (value) return value;
    if (Date.now() > deadline) {
      throw new Error(
        `Délai dépassé : ${typeof what === "function" ? await what() : what}`,
      );
    }
    await delay(100);
  }
}

/**
 * Erreur renvoyée par une commande (null si elle réussit) : `driver.invoke`
 * prendrait la réponse `{ error }` pour une erreur du serveur WebDriver.
 */
const failureOf = (driver, command, args = {}) =>
  driver
    .executeAsync(
      `const done = arguments[arguments.length - 1];
       window.__TAURI_INTERNALS__.invoke(arguments[0], arguments[1])
         .then(() => done(null), (failure) => done({ failure }));`,
      [command, args],
    )
    .then((result) => result?.failure ?? null);

const exists = (driver, css) =>
  driver.execute("return document.querySelector(arguments[0]) !== null", [css]);
const path = (driver) => driver.execute("return location.pathname");
const phase = async (driver) => (await driver.invoke("voice_mode_state")).phase;
const texts = (driver, css) =>
  driver.execute(
    "return [...document.querySelectorAll(arguments[0])].map((e) => e.textContent)",
    [css],
  );

/** Choisit un modèle du faux serveur, puis recharge la page. */
async function useModel(driver, model) {
  await driver.invoke("update_settings", { patch: { selectedModel: model } });
  await driver.execute("window.__oldPage = true; location.reload()");
  await waitUntil(
    async () => !(await driver.execute("return window.__oldPage === true")),
    "rechargement",
  );
  await driver.tauriReady();
}

/**
 * Langue et modèle de la dictée, dont le mode vocal reprend la
 * transcription : français (voix attendue : la française), Parakeet.
 */
const frenchDictation = (driver) =>
  driver.invoke("update_dictation_settings", {
    patch: { speechModel: PARAKEET, language: "fr" },
  });

/** Focus rendu à la page (Espace ne doit pas activer un bouton). */
const blur = (driver) =>
  driver.execute("document.activeElement?.blur(); return true;");

/** Tape un message et l'envoie (Entrée), puis attend la fin de la réponse. */
async function send(driver, text) {
  await driver.type(await driver.waitFor(TEXTAREA), text);
  await driver.execute(
    `document.querySelector(arguments[0]).dispatchEvent(
       new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true }));`,
    [TEXTAREA],
  );
  await waitUntil(
    async () =>
      (await texts(driver, "[data-role=assistant]")).length > 0 &&
      !(await exists(driver, "button[aria-label='Stop']")),
    "réponse au message tapé",
  );
}

async function openNewChat(driver) {
  await driver.click(await driver.waitFor("a[title='New chat']"));
  await driver.waitFor("[data-testid=new-chat]");
}

async function openChat(driver, id) {
  if (await exists(driver, "button[aria-label='Show sidebar']")) {
    await driver.click(await driver.find("button[aria-label='Show sidebar']"));
  }
  await driver.click(await driver.waitFor(`nav a[href='/c/${id}']`));
}

/** Barre vocale à l'écoute (phase listening ou hearing). */
const waitBarListening = (driver, what) =>
  waitUntil(
    async () => {
      const bar = await barState(driver);
      return bar && ["listening", "hearing"].includes(bar.phase) && bar;
    },
    what,
    60000,
  );

/** Séance finie : état « off », barre partie, zone de saisie rendue. */
async function waitEnded(driver, what) {
  await waitUntil(
    async () =>
      (await phase(driver)) === "off" &&
      (await barState(driver)) === null &&
      (await exists(driver, TEXTAREA)),
    what,
  );
}

/** Lance `fn`, avec une capture de la page si elle échoue (diagnostic). */
async function shotOnFailure(driver, name, fn) {
  try {
    await fn();
  } catch (error) {
    const file = `${ROOT}.tools/shots/e2e-${name}-failure.png`;
    await driver.screenshot(file).catch(() => {});
    console.error(`Capture de la page : ${file}`);
    throw error;
  }
}

const fake = await startFake("ollama");
const env = {
  OLLAMA_HOST: fake.url,
  CHATONLM_NO_GLOBAL_SHORTCUTS: "1",
};
const requests = async () =>
  (await fetch(new URL("/fake/requests", fake.url))).json();

try {
  linkParakeet(DATA);
  rmSync(`${DATA}/voice-models`, { recursive: true, force: true });

  // 1. Prérequis : voix française absente.
  await withApp(
    binary,
    { name: "voice-mode-requirements", env },
    async (driver) => {
      await driver.waitFor("[data-testid=new-chat]");
      await frenchDictation(driver);
      const requirements = await driver.invoke("voice_mode_requirements");
      assert.equal(requirements.unavailable, null);
      assert.equal(requirements.speechModel, PARAKEET);
      assert.equal(requirements.speechModelInstalled, true);
      assert.equal(requirements.speechModelBytes, 0);
      assert.equal(requirements.voiceLanguage, "fr");
      assert.equal(requirements.voiceInstalled, false);
      assert.ok(requirements.voiceBytes > 0, JSON.stringify(requirements));
      // Démarrage refusé par le moteur, pas de séance.
      assert.ok(
        await failureOf(driver, "voice_mode_start"),
        "voice_mode_start accepté sans la voix",
      );
      assert.equal(await phase(driver), "off");

      await useModel(driver, "chatonlm-voice");
      await driver.waitFor("[data-testid=new-chat]");
      await shotOnFailure(driver, "voice-mode-requirements", async () => {
        await driver.click(await driver.waitFor("[data-voice-mode-start]"));
        await driver.waitFor(DIALOG);
        const text = await driver.execute(
          "return document.querySelector(arguments[0]).textContent",
          [DIALOG],
        );
        // Ce qui manque, avec sa taille : la voix, pas Parakeet (installé).
        assert.match(text, /\d+(?:\.\d+)?\s?(?:KB|MB|GB)\b/);
        assert.doesNotMatch(text, /parakeet/i);
        // Refus : rien de téléchargé, pas de séance.
        await driver.execute(
          `[...document.querySelectorAll(arguments[0] + " button")]
             .find((b) => b.textContent.trim() === "Cancel").click()`,
          [DIALOG],
        );
        await waitUntil(
          async () => !(await exists(driver, DIALOG)),
          "boîte fermée",
        );
        await delay(500);
        assert.deepEqual(await driver.invoke("voice_downloads"), []);
        assert.deepEqual(await driver.invoke("speech_downloads"), []);
        assert.equal(await phase(driver), "off");
        assert.equal(await barState(driver), null);
        assert.ok(await exists(driver, "[data-voice-mode-start]"));
        assert.ok(await exists(driver, TEXTAREA));
      });
    },
  );

  // 2. Deux tours, Espace, micro coupé, Échap, autre conversation, brouillon.
  linkVoice(DATA, FRENCH);
  const first = writePhrase(PHRASES.first, TMP);
  const second = writePhrase(PHRASES.second, TMP);
  await withApp(
    binary,
    {
      name: "voice-mode-turns",
      env: {
        ...env,
        CHATONLM_VOICE_TIMINGS: "1",
        CHATONLM_E2E_VOICE_MIC: JSON.stringify([
          { on: "listening", wav: first.wav, delayMs: 300 },
          { on: "listening", wav: second.wav, delayMs: 300 },
        ]),
      },
    },
    async (driver) => {
      const log = watchLog("voice-mode-turns");
      try {
        await driver.waitFor("[data-testid=new-chat]");
        await frenchDictation(driver);
        await useModel(driver, "chatonlm-voice");
        await driver.waitFor("[data-testid=new-chat]");
        await recordVoiceEvents(driver);
        // Réglages par défaut : mains libres, pause normale, interruption
        // par la parole, réflexion coupée, consigne par défaut.
        assert.deepEqual(await driver.invoke("get_voice_mode_settings"), {
          turnMode: "handsFree",
          pause: "normal",
          bargeIn: true,
          keepThink: false,
          instructions: "",
        });
        const instructions = (await driver.invoke("voice_mode_info"))
          .defaultInstructions;
        assert.match(instructions, /spoken conversation/);
        await shotOnFailure(driver, "voice-mode-turns", async () => {
          await turns(driver, log, { first, second, instructions });
        });
      } finally {
        log.stop();
      }
    },
  );

  // 3. Interruption en parlant, puis End pendant la lecture. Phrase 1 sans
  // long silence final : elle finit bien avant le premier son, que la phrase
  // suivante (« on: speaking ») attend.
  const opening = writePhrase(PHRASES.first, TMP, { tail: 0.5 });
  const short = writePhrase(PHRASES.short, TMP);
  await withApp(
    binary,
    {
      name: "voice-mode-barge-in",
      env: {
        ...env,
        CHATONLM_VOICE_TIMINGS: "1",
        CHATONLM_E2E_VOICE_MIC: JSON.stringify([
          { on: "listening", wav: opening.wav, delayMs: 300 },
          { on: "speaking", wav: short.wav, delayMs: 1000 },
        ]),
      },
    },
    async (driver) => {
      await driver.waitFor("[data-testid=new-chat]");
      await frenchDictation(driver);
      await useModel(driver, "chatonlm-voice-long");
      await driver.waitFor("[data-testid=new-chat]");
      await recordVoiceEvents(driver);
      await shotOnFailure(driver, "voice-mode-barge-in", () => bargeIn(driver));
    },
  );
} finally {
  await fake.stop();
  rmSync(TMP, { recursive: true, force: true });
}
console.log("✓ voice-mode");

/** Partie 2 : deux tours, puis micro coupé, Échap, autre conversation, brouillon. */
async function turns(driver, log, { first, second, instructions }) {
  // Démarrage depuis une nouvelle conversation, par le bouton « onde » :
  // prérequis présents, donc pas de boîte ; la barre remplace la zone de
  // texte.
  await driver.click(await driver.waitFor("[data-voice-mode-start]"));
  await waitBarListening(driver, "barre vocale à l'écoute");
  assert.equal(await exists(driver, DIALOG), false, "boîte inattendue");
  assert.equal(await exists(driver, TEXTAREA), false, "zone de texte restée");
  assert.ok(await exists(driver, "[data-voice-mode-end]"));
  assert.ok(await exists(driver, "[data-voice-mode-mute]"));
  await driver.screenshot(`${ROOT}.tools/shots/voice-mode-bar.png`);

  // Tour 1 : phrase transcrite, envoyée, réponse écrite puis lue jusqu'au
  // bout, retour à l'écoute.
  const u1 = await waitVoice(
    driver,
    (e) => e.utterances.find((u) => u.turn === 1),
    "1re phrase transcrite",
    40000,
  );
  assert.ok(heard(u1.text, PHRASES.first), `1re phrase : ${u1.text}`);
  assert.equal(u1.language, "fr");
  const chat = await waitUntil(async () => {
    const at = await path(driver);
    return /^\/c\/(?!new$)/.test(at) && at.slice(3);
  }, "conversation créée par l'envoi vocal");
  const sp1 = await waitVoice(
    driver,
    (e) => stateAfter(e.states, (s) => s.phase === "speaking" && s.turn === 1),
    "lecture de la 1re réponse",
    30000,
  );
  // Séance gardée par la conversation créée : barre toujours là.
  assert.ok(await barState(driver), "séance perdue avec la nouvelle page");
  const back1 = await waitVoice(
    driver,
    (e) =>
      stateAfter(
        e.states,
        (s) => s.phase === "listening" && s.turn === 1,
        sp1.index,
      ),
    "retour à l'écoute après la 1re réponse",
    30000,
  );
  assert.deepEqual(
    (await texts(driver, "[data-role=assistant]")).map((t) => t.trim()),
    [REPLIES[0]],
  );
  const users = await texts(driver, "[data-role=user] .whitespace-pre-wrap");
  assert.equal(users.length, 1);
  assert.equal(users[0].trim(), u1.text.trim());

  // Tour 2 ; Espace (vraie touche, focus hors d'un champ) coupe la lecture.
  const u2 = await waitVoice(
    driver,
    (e) => e.utterances.find((u) => u.turn === 2),
    "2e phrase transcrite",
    40000,
  );
  assert.ok(heard(u2.text, PHRASES.second), `2e phrase : ${u2.text}`);
  const sp2 = await waitVoice(
    driver,
    (e) => stateAfter(e.states, (s) => s.phase === "speaking" && s.turn === 2),
    "lecture de la 2e réponse",
    30000,
  );
  await blur(driver);
  // Noté avant la touche : pressKeys ne rend la main qu'après le démarrage
  // du client libei et un temps de traitement.
  const pressed = Date.now();
  await pressKeys("space");
  const back2 = await waitVoice(
    driver,
    (e) =>
      stateAfter(
        e.states,
        (s) => s.phase !== "speaking" && s.turn === 2,
        sp2.index,
      ),
    "fin de la 2e lecture par Espace",
    15000,
  );
  assert.equal(back2.state.phase, "listening");
  // L'écoute suit l'appui (pas une fin de lecture d'avant).
  assert.ok(
    back2.state.at >= pressed,
    `retour à l'écoute ${pressed - back2.state.at} ms avant l'appui`,
  );
  // Coupée, pas finie : durée de la 2e lecture entière estimée d'après le
  // débit mesuré sur la 1re (lue jusqu'au bout, même voix, même machine),
  // proportionnellement à la longueur du texte ; la lecture doit s'arrêter
  // bien avant.
  const firstRead = back1.state.at - sp1.state.at;
  const fullSecond = firstRead * (REPLIES[1].length / REPLIES[0].length);
  const secondRead = back2.state.at - sp2.state.at;
  assert.ok(
    secondRead < fullSecond * 0.8,
    `2e lecture finie ${secondRead} ms après son début (entière : ~${Math.round(fullSecond)} ms, 1re : ${firstRead} ms)`,
  );
  await waitUntil(
    async () => (await driver.invoke("read_aloud_state")).phase === "idle",
    "lecture arrêtée",
  );

  const events = await voiceEvents(driver);
  assert.ok(
    inOrder(events.states, [
      "starting",
      "listening",
      "hearing",
      "transcribing",
      "responding",
      "speaking",
      "listening",
      "hearing",
      "transcribing",
      "responding",
      "speaking",
      "listening",
    ]),
    describeStates(events.states),
  );
  assert.equal(events.states.at(-1).notice, null);
  // Niveaux : micro pendant la parole, voix pendant la lecture.
  const hearing1 = stateAfter(events.states, (s) => s.phase === "hearing");
  const heardLevels = await levelPeaks(driver, hearing1.state.at, u1.at);
  assert.ok(heardLevels.count > 10, JSON.stringify(heardLevels));
  assert.ok(heardLevels.input > 0.2, JSON.stringify(heardLevels));
  const spokenLevels = await levelPeaks(driver, sp1.state.at, back1.state.at);
  assert.ok(spokenLevels.output > 0.1, JSON.stringify(spokenLevels));

  // Requêtes reçues : consigne vocale en tête, think coupé, historique.
  const voice = (await requests()).filter((r) => r.model === "chatonlm-voice");
  assert.equal(voice.length, 2, JSON.stringify(voice));
  for (const request of voice) {
    assert.equal(request.think, false, JSON.stringify(request));
    assert.equal(request.messages[0].role, "system");
    assert.equal(request.messages[0].content, instructions);
    assert.equal(request.messages.filter((m) => m.role === "system").length, 1);
  }
  assert.deepEqual(
    voice[1].messages.map((m) => m.role),
    ["system", "user", "assistant", "user"],
  );
  assert.equal(voice[0].messages.at(-1).content.trim(), u1.text.trim());
  assert.equal(voice[1].messages[2].content, REPLIES[0]);
  assert.equal(voice[1].messages.at(-1).content.trim(), u2.text.trim());
  // Historique : conversation normale, sans consigne ni raisonnement.
  const saved = await driver.invoke("get_chat", { id: chat });
  assert.deepEqual(
    saved.messages.map((m) => m.role),
    ["user", "assistant", "user", "assistant"],
  );
  assert.equal(saved.messages[0].content.trim(), u1.text.trim());
  assert.equal(saved.messages[1].content, REPLIES[0]);
  assert.equal(saved.messages[2].content.trim(), u2.text.trim());
  assert.equal(saved.messages[3].content, REPLIES[1]);
  assert.ok(saved.messages.every((m) => m.thinking === ""));
  assert.equal(
    await driver.execute(
      "return document.body.textContent.includes('spoken conversation')",
    ),
    false,
    "consigne vocale affichée",
  );

  // Latence (relevée, pas exigée ici : voir l'essai réel).
  const ends = wavEnds(log);
  assert.ok(ends.length >= 2, "lignes « wav <n> ended » du faux micro");
  const timings = voiceTimings(log);
  for (const turn of [1, 2]) {
    assert.ok(
      timings.some(
        (t) => t.turn === turn && typeof t.first_sound_ms === "number",
      ),
      `ligne chatonlm-voice-timing du tour ${turn} : ${JSON.stringify(timings)}`,
    );
  }
  for (const [turn, end, speaking, phrase] of [
    [1, ends[0], sp1.state, first],
    [2, ends[1], sp2.state, second],
  ]) {
    const { wav, speech } = firstSoundDelay(end, speaking, phrase.afterSpeech);
    console.log(
      `  tour ${turn} : fin de la parole → premier son ${speech} ms (fin du WAV → premier son ${wav} ms) ; moteur : ${JSON.stringify(timings.find((t) => t.turn === turn))}`,
    );
  }

  // Micro coupé puis rouvert : phase gardée.
  await driver.click(await driver.find("[data-voice-mode-mute]"));
  await waitUntil(
    async () => (await barState(driver))?.muted === true,
    "barre : micro coupé",
  );
  let state = await driver.invoke("voice_mode_state");
  assert.equal(state.muted, true);
  assert.equal(state.phase, "listening");
  await driver.click(await driver.find("[data-voice-mode-mute]"));
  await waitUntil(
    async () => (await barState(driver))?.muted === false,
    "barre : micro rouvert",
  );
  state = await driver.invoke("voice_mode_state");
  assert.equal(state.muted, false);
  assert.notEqual(state.phase, "off");

  // Échap : séance finie, zone de saisie rendue, plus de niveaux.
  await blur(driver);
  await pressKeys("escape");
  await waitEnded(driver, "fin de la séance par Échap");
  await delay(300);
  const levelsAtEnd = (await voiceEvents(driver)).levelCount;
  await delay(700);
  assert.equal((await voiceEvents(driver)).levelCount, levelsAtEnd);

  // Autre conversation affichée : séance arrêtée.
  await openNewChat(driver);
  await send(driver, "Hello there.");
  const other = await waitUntil(async () => {
    const at = await path(driver);
    return /^\/c\/(?!new$)/.test(at) && at.slice(3);
  }, "conversation du message tapé");
  assert.notEqual(other, chat);
  await driver.click(await driver.waitFor("[data-voice-mode-start]"));
  await waitBarListening(driver, "séance dans l'autre conversation");
  await openChat(driver, chat);
  await waitUntil(
    async () => (await path(driver)) === `/c/${chat}`,
    "conversation d'origine affichée",
  );
  await waitEnded(driver, "séance arrêtée au changement de conversation");
  assert.ok(await exists(driver, "[data-voice-mode-start]"));
  // Message tapé : sans consigne vocale.
  const typed = (await requests()).filter(
    (r) => r.messages.at(-1)?.content === "Hello there.",
  );
  assert.equal(typed.length, 1);
  assert.ok(typed[0].messages.every((m) => m.role !== "system"));

  // Brouillon gardé pendant une séance (Ctrl+Maj+Espace, puis Échap) ; en
  // dernier : rien n'est plus envoyé ensuite.
  await driver.type(await driver.waitFor(TEXTAREA), "Draft kept");
  await blur(driver);
  await pressKeys("ctrl+shift+space");
  await waitBarListening(driver, "séance lancée par Ctrl+Maj+Espace");
  await blur(driver);
  await pressKeys("escape");
  await waitEnded(driver, "fin de la séance par Échap");
  assert.equal(
    await driver.execute("return document.querySelector(arguments[0]).value", [
      TEXTAREA,
    ]),
    "Draft kept",
  );
  // Aucun envoi vocal de plus dans la conversation des deux tours.
  assert.equal(
    (await driver.invoke("get_chat", { id: chat })).messages.length,
    4,
  );
}

/** Partie 3 : interruption en parlant, puis End pendant la lecture. */
async function bargeIn(driver) {
  const abandoned = await fake.disconnects();
  const before = (await requests()).length;
  await driver.click(await driver.waitFor("[data-voice-mode-start]"));
  await waitBarListening(driver, "barre vocale à l'écoute");

  const u1 = await waitVoice(
    driver,
    (e) => e.utterances.find((u) => u.turn === 1),
    "1re phrase transcrite",
    40000,
  );
  assert.ok(heard(u1.text, PHRASES.first), `1re phrase : ${u1.text}`);
  const sp1 = await waitVoice(
    driver,
    (e) => stateAfter(e.states, (s) => s.phase === "speaking" && s.turn === 1),
    "lecture de la longue réponse",
    30000,
  );
  // La phrase du faux micro (1 s après le premier son) coupe la voix : la
  // séance passe directement à l'écoute de la parole.
  const cut = await waitVoice(
    driver,
    (e) => stateAfter(e.states, (s) => s.phase !== "speaking", sp1.index),
    "interruption par la parole",
    15000,
  );
  assert.equal(cut.state.phase, "hearing", "phase après l'interruption");
  console.log(
    `  interruption : ${cut.state.at - sp1.state.at - 1000} ms après le début de la phrase (estimé)`,
  );
  // Génération arrêtée : le flux de la 1re réponse est abandonné.
  await waitUntil(
    async () => (await fake.disconnects()) === abandoned + 1,
    "flux de la 1re réponse abandonné",
    5000,
  );

  const u2 = await waitVoice(
    driver,
    (e) => e.utterances.find((u) => u.turn === 2),
    "phrase de l'interruption transcrite",
    30000,
  );
  assert.ok(heard(u2.text, PHRASES.short), `2e phrase : ${u2.text}`);
  // Lecture coupée : plus de son entre l'interruption et la 2e phrase, et
  // lecture au repos.
  const silent = await levelPeaks(driver, cut.state.at + 500, u2.at);
  assert.ok(silent.output < 0.05, JSON.stringify(silent));
  const events = await voiceEvents(driver);
  assert.ok(
    events.readAloud.some(
      (r) => r.phase === "idle" && r.at > sp1.state.at && r.at <= u2.at,
    ),
    `lecture pas arrêtée : ${JSON.stringify(events.readAloud)}`,
  );

  // End pendant la lecture de la 2e réponse : séance finie, lecture
  // arrêtée, mais la réponse s'écrit jusqu'au bout.
  await waitVoice(
    driver,
    (e) => stateAfter(e.states, (s) => s.phase === "speaking" && s.turn === 2),
    "lecture de la 2e réponse",
    30000,
  );
  await driver.click(await driver.find("[data-voice-mode-end]"));
  await waitEnded(driver, "fin de la séance par End");
  await waitUntil(
    async () => (await driver.invoke("read_aloud_state")).phase === "idle",
    "lecture arrêtée par End",
  );
  const chat = (await path(driver)).slice(3);
  const saved = await waitUntil(
    async () => {
      const found = await driver.invoke("get_chat", { id: chat });
      return found.messages[3]?.content.endsWith(LONG_END) && found;
    },
    "2e réponse écrite jusqu'au bout",
    20000,
  );
  assert.equal(await fake.disconnects(), abandoned + 1);
  const [m1, r1, m2, r2] = saved.messages;
  assert.equal(m1.content.trim(), u1.text.trim());
  assert.equal(m2.content.trim(), u2.text.trim());
  // Début de la 1re réponse gardé, sans sa fin.
  assert.ok(r1.content.length > 0, "1re réponse vide");
  assert.ok(!r1.content.includes(LONG_END), "1re réponse complète");
  assert.ok(r2.content.startsWith(r1.content.trimEnd()), r1.content);

  const all = await voiceEvents(driver);
  assert.ok(
    inOrder(all.states, [
      "listening",
      "hearing",
      "transcribing",
      "responding",
      "speaking",
      "hearing",
      "transcribing",
      "responding",
      "speaking",
      "off",
    ]),
    describeStates(all.states),
  );
  const sent = (await requests())
    .slice(before)
    .filter((r) => r.model === "chatonlm-voice-long");
  assert.equal(sent.length, 2);
  for (const request of sent) {
    assert.equal(request.think, false);
    assert.equal(request.messages[0].role, "system");
  }
}
