// Aides des essais du mode conversation vocale (e2e/voice-mode.test.mjs avec
// le faux serveur, e2e/real-voice-mode.test.mjs avec l'Ollama de test) :
// modèles liés dans le dossier des données, phrases du faux micro
// (CHATONLM_E2E_VOICE_MIC), évènements de la séance notés par la page, mesures
// du moteur (CHATONLM_VOICE_TIMINGS) et du faux micro lues dans le journal.
import {
  existsSync,
  mkdirSync,
  readFileSync,
  readdirSync,
  symlinkSync,
  writeFileSync,
} from "node:fs";
import { ROOT } from "./app.mjs";

const delay = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

/** Modèle de transcription des essais (catalogue de la dictée). */
export const PARAKEET = "parakeet-tdt-0.6b-v3";
const PARAKEET_FILES = `${ROOT}.tools/speech-models/parakeet-tdt-0.6b-v3-int8`;
const VOICES = `${ROOT}.tools/voice-models`;
const SAMPLES = `${ROOT}.tools/speech-samples`;

/** Lie `source` en `target` s'il n'y est pas déjà. */
function link(source, target) {
  if (!existsSync(source)) {
    throw new Error(`Fichier d'essai absent : ${source}`);
  }
  if (!existsSync(target)) symlinkSync(source, target);
}

/** Parakeet « installé » : liens vers la copie de l'essai (670 Mo). */
export function linkParakeet(data) {
  const dir = `${data}/speech-models/${PARAKEET}`;
  mkdirSync(dir, { recursive: true });
  for (const file of [
    "encoder-model.int8.onnx",
    "decoder_joint-model.int8.onnx",
    "nemo128.onnx",
    "vocab.txt",
  ]) {
    link(`${PARAKEET_FILES}/${file}`, `${dir}/${file}`);
  }
}

/** Voix `id` (pocket-tts-fr...) de .tools/voice-models liée, fichier par fichier. */
export function linkVoice(data, id) {
  const source = `${VOICES}/${id}`;
  if (!existsSync(source)) {
    throw new Error(
      `Voix absente : ${source} (scripts/tts/README.md, assemble.py)`,
    );
  }
  const dir = `${data}/voice-models/${id}`;
  mkdirSync(dir, { recursive: true });
  for (const file of readdirSync(source)) {
    link(`${source}/${file}`, `${dir}/${file}`);
  }
}

/**
 * Phrases du faux micro : début des lectures de .tools/speech-samples, coupé
 * dans la pause qui suit la première phrase. Entières, fr0.wav et fr1.wav ont
 * des pauses de 0,76 s, 0,86 s et 1,36 s : près du silence de fin du tour
 * (1 s par défaut), ou au-delà, elles couperaient la phrase en deux tours.
 * - `seconds` : durée gardée ; `speechEnd` : fin de la parole (énergie à
 *   -35 dB du maximum), d'où la fin de parole réelle pendant la lecture ;
 * - `text` : transcription de ces extraits par Parakeet (9 oct. 2026,
 *   .tools/spike-stt), et `keywords`, ses mots comparés avec tolérance.
 */
export const PHRASES = {
  first: {
    source: "fr0.wav",
    seconds: 7.6,
    speechEnd: 7.31,
    text: "Pendant le second siècle, je fis serment d'ouvrir tous les trésors de la terre, à quiconque me mettrait en liberté.",
    keywords: ["siècle", "serment", "trésors", "terre", "liberté"],
  },
  second: {
    source: "fr1.wav",
    seconds: 6.5,
    speechEnd: 6.19,
    text: "Non, ta mort est certaine, dit le génie, choisis seulement de quelle sorte tu veux que je te fasse mourir.",
    keywords: ["mort", "certaine", "génie", "choisis", "mourir"],
  },
  short: {
    source: "fr1.wav",
    seconds: 2.9,
    speechEnd: 2.57,
    text: "Non, ta mort est certaine, dit le génie.",
    keywords: ["mort", "certaine", "génie"],
  },
};

/**
 * Bruit très faible (environ -80 dBFS) ajouté après chaque phrase : la fin du
 * tour (silence de 1 s après la parole) est trouvée avant la fin du WAV. La
 * phrase suivante du faux micro (« on: listening ») attend donc la prochaine
 * écoute, et non la fin de ce WAV pendant que la même écoute dure encore.
 */
export const TAIL_SECONDS = 2;

/** Échantillons PCM 16 bits mono 16 kHz d'un WAV (blocs RIFF lus un à un). */
function readPcm16(file) {
  const bytes = readFileSync(file);
  if (
    bytes.toString("ascii", 0, 4) !== "RIFF" ||
    bytes.toString("ascii", 8, 12) !== "WAVE"
  ) {
    throw new Error(`Pas un WAV : ${file}`);
  }
  let format = null;
  for (let at = 12; at + 8 <= bytes.length;) {
    const id = bytes.toString("ascii", at, at + 4);
    const size = bytes.readUInt32LE(at + 4);
    const body = at + 8;
    if (id === "fmt ") {
      format = {
        code: bytes.readUInt16LE(body),
        channels: bytes.readUInt16LE(body + 2),
        rate: bytes.readUInt32LE(body + 4),
        bits: bytes.readUInt16LE(body + 14),
      };
    } else if (id === "data") {
      if (
        !format ||
        format.code !== 1 ||
        format.channels !== 1 ||
        format.rate !== 16000 ||
        format.bits !== 16
      ) {
        throw new Error(
          `WAV inattendu (PCM 16 bits mono 16 kHz) : ${file} ${JSON.stringify(format)}`,
        );
      }
      return bytes.subarray(body, body + size);
    }
    at = body + size + (size % 2);
  }
  throw new Error(`WAV sans données : ${file}`);
}

/** WAV PCM 16 bits mono 16 kHz de ces données. */
function wavOf(data) {
  const header = Buffer.alloc(44);
  header.write("RIFF", 0, "ascii");
  header.writeUInt32LE(36 + data.length, 4);
  header.write("WAVEfmt ", 8, "ascii");
  header.writeUInt32LE(16, 16);
  header.writeUInt16LE(1, 20);
  header.writeUInt16LE(1, 22);
  header.writeUInt32LE(16000, 24);
  header.writeUInt32LE(32000, 28);
  header.writeUInt16LE(2, 32);
  header.writeUInt16LE(16, 34);
  header.write("data", 36, "ascii");
  header.writeUInt32LE(data.length, 40);
  return Buffer.concat([header, data]);
}

/**
 * Écrit la phrase `phrase` (de PHRASES) dans `dir` : extrait suivi de `tail`
 * secondes (TAIL_SECONDS par défaut) de bruit très faible, toujours le même.
 * Renvoie { wav, afterSpeech } : `afterSpeech`, secondes entre la fin de la
 * parole et la fin du WAV (la ligne `wav <n> ended` du faux micro vient
 * alors).
 */
export function writePhrase(
  phrase,
  dir,
  { tail: seconds = TAIL_SECONDS } = {},
) {
  mkdirSync(dir, { recursive: true });
  const pcm = readPcm16(`${SAMPLES}/${phrase.source}`);
  const kept = pcm.subarray(0, Math.round(phrase.seconds * 16000) * 2);
  const tail = Buffer.alloc(Math.round(seconds * 16000) * 2);
  let seed = 12345;
  for (let i = 0; i < tail.length; i += 2) {
    seed = (Math.imul(seed, 1103515245) + 12345) >>> 0;
    tail.writeInt16LE(((seed >>> 16) % 11) - 5, i);
  }
  const base = phrase.source.replace(/\.wav$/, "");
  const name = `${base}-${phrase.seconds}s-${seconds}t.wav`;
  const wav = `${dir}/${name}`;
  writeFileSync(wav, wavOf(Buffer.concat([kept, tail])));
  return { wav, afterSpeech: phrase.seconds - phrase.speechEnd + seconds };
}

/** Texte sans accents ni majuscules, pour comparer des mots transcrits. */
const fold = (text) =>
  text.normalize("NFD").replace(/\p{M}/gu, "").toLowerCase();

/** Mots de `phrase.keywords` présents dans `text` (accents ignorés). */
export function keywordsIn(text, phrase) {
  const folded = fold(text);
  return phrase.keywords.filter((word) => folded.includes(fold(word)));
}

/**
 * Vrai si `text` est la transcription de `phrase` : tous ses mots clés sauf
 * un au plus (une erreur de reconnaissance tolérée).
 */
export const heard = (text, phrase) =>
  keywordsIn(text, phrase).length >= phrase.keywords.length - 1;

/**
 * Note dans la page (window.__voiceEvents) les évènements de la séance, datés
 * (Date.now()) : `states` (« voice-mode »), `utterances`
 * (« voice-mode-utterance »), `levels` (« voice-mode-level ») et `readAloud`
 * (« read-aloud »). À rappeler après un rechargement de la page.
 */
export async function recordVoiceEvents(driver) {
  const result = await driver.executeAsync(`
    const done = arguments[arguments.length - 1];
    const internals = window.__TAURI_INTERNALS__;
    const record = { states: [], utterances: [], levels: [], readAloud: [] };
    window.__voiceEvents = record;
    const listen = (event, list) =>
      internals.invoke("plugin:event|listen", {
        event,
        target: { kind: "Any" },
        handler: internals.transformCallback((e) =>
          list.push({ at: Date.now(), ...e.payload })),
      });
    Promise.all([
      listen("voice-mode", record.states),
      listen("voice-mode-utterance", record.utterances),
      listen("voice-mode-level", record.levels),
      listen("read-aloud", record.readAloud),
    ]).then(() => done(true), (error) => done(String(error)));`);
  if (result !== true) {
    throw new Error(`Écoute des évènements du mode vocal : ${result}`);
  }
}

/**
 * Évènements notés jusqu'ici, sans les niveaux (nombreux) : { states,
 * utterances, readAloud, levelCount }.
 */
export const voiceEvents = (driver) =>
  driver.execute(`const r = window.__voiceEvents;
    return { states: r.states, utterances: r.utterances,
             readAloud: r.readAloud, levelCount: r.levels.length };`);

/**
 * Niveaux maximaux (entrée, sortie) et nombre d'évènements de niveau reçus
 * entre `from` et `to` (Date.now()).
 */
export const levelPeaks = (driver, from, to = Number.MAX_SAFE_INTEGER) =>
  driver.execute(
    `const levels = window.__voiceEvents.levels.filter(
       (l) => l.at >= arguments[0] && l.at <= arguments[1]);
     return {
       count: levels.length,
       input: Math.max(0, ...levels.map((l) => l.input)),
       output: Math.max(0, ...levels.map((l) => l.output)),
     };`,
    [from, to],
  );

/** Phases notées, « phase(tour) » en une ligne (diagnostic). */
export const describeStates = (states) =>
  states
    .map(
      (s) =>
        `${s.phase}(${s.turn}${s.muted ? ",muted" : ""}${s.notice ? `,${s.notice.kind}` : ""})`,
    )
    .join(" ");

/**
 * Attend que `predicate(events)` (évènements de voiceEvents) renvoie une
 * valeur vraie, et la renvoie ; échoue après `timeoutMs` avec les phases
 * notées et les phrases transcrites.
 */
export async function waitVoice(driver, predicate, what, timeoutMs = 30000) {
  const deadline = Date.now() + timeoutMs;
  for (;;) {
    const events = await voiceEvents(driver);
    const value = predicate(events);
    if (value) return value;
    if (Date.now() > deadline) {
      throw new Error(
        `Délai dépassé (${timeoutMs} ms) : ${what}\n  phases : ${describeStates(events.states)}\n  phrases : ${JSON.stringify(events.utterances)}`,
      );
    }
    await delay(100);
  }
}

/**
 * Premier état d'indice ≥ `from` qui satisfait `test` : { state, index }, ou
 * null.
 */
export function stateAfter(states, test, from = 0) {
  for (let index = from; index < states.length; index++) {
    if (test(states[index])) return { state: states[index], index };
  }
  return null;
}

/**
 * Vrai si les phases de `states` contiennent `expected` dans cet ordre (pas
 * forcément d'affilée).
 */
export function inOrder(states, expected) {
  let next = 0;
  for (const state of states) {
    if (state.phase === expected[next]) next++;
    if (next === expected.length) return true;
  }
  return false;
}

/**
 * Lignes `chatonlm-voice-timing turn=N clé=valeur…` (CHATONLM_VOICE_TIMINGS=1)
 * du journal suivi par watchLog : { at, turn, speech_end_ms, … } (nombres
 * quand la valeur en est un).
 */
export function voiceTimings(log) {
  return log.matching(/chatonlm-voice-timing /).map(({ at, text }) => {
    const fields = { at };
    const tail = text.slice(text.indexOf("chatonlm-voice-timing "));
    for (const [, key, value] of tail.matchAll(/(\w+)=(\S+)/g)) {
      fields[key] = Number.isNaN(Number(value)) ? value : Number(value);
    }
    return fields;
  });
}

/**
 * Fins des WAV du faux micro (`chatonlm-voice-e2e: wav <n> ended`), dans
 * l'ordre où le journal les donne : { at, wav } (`wav` : numéro écrit par
 * l'app).
 */
export const wavEnds = (log) =>
  log.matching(/chatonlm-voice-e2e: wav \d+ ended/).map(({ at, text }) => ({
    at,
    wav: Number(/wav (\d+) ended/.exec(text)[1]),
  }));

/**
 * Délai entre la fin d'un WAV du faux micro (`wavEnd`, de wavEnds) et le
 * premier son de la réponse (`speaking`, état Speaking noté par la page) :
 * { wav, speech } en millisecondes ; `speech` part de la fin de la parole,
 * `afterSpeech` secondes avant la fin du WAV (writePhrase).
 */
export function firstSoundDelay(wavEnd, speaking, afterSpeech) {
  const wav = speaking.at - wavEnd.at;
  return { wav, speech: Math.round(wav + afterSpeech * 1000) };
}

/** Phase et micro coupé de la barre vocale de la page (null sans barre). */
export const barState = (driver) =>
  driver.execute(`const bar = document.querySelector("[data-voice-mode-bar]");
    if (!bar) return null;
    const muted = bar.getAttribute("data-voice-mode-muted");
    return { phase: bar.getAttribute("data-voice-mode-phase"),
             muted: muted === "" || muted === "true" };`);
