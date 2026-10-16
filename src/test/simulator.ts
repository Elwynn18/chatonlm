// Simulateur des échanges avec le moteur Rust, pour les tests vitest ; jamais
// importé par le code de l'app (vérifié par simulator.test.ts).
//
// Il installe un faux IPC Tauri (mockIPC) où send_message rejoue une séquence
// enregistrée par les tests Rust (src/test/fixtures/chat, écrites par
// sequences_for_the_simulator dans src-tauri/src/chat/mod.rs) : événements
// livrés par le vrai mécanisme des canaux de Tauri,
// runCallback(id, {message, index}) avec des index croissants, puis
// runCallback(id, {end: true, index}) ; ensuite la commande se résout, ou
// échoue avec l'erreur de la séquence. Les autres commandes passent par des
// gestionnaires fournis par le test ; une commande sans gestionnaire, ou un
// envoi qui ne correspond pas à la séquence, fait échouer le test.
//
// Un autre gestionnaire qui reçoit un canal (change_models_location) le
// confie à simulator.channel(), qui livre ses messages de la même façon.
//
// Il tient aussi les inscriptions aux événements Tauri (listen, unlisten,
// glisser-déposer de getCurrentWebview(), fenêtre « main » simulée), que le
// test déclenche par emitEvent, et passe aux gestionnaires le corps brut d'une
// commande (stage_bytes) avec ses en-têtes, que mockIPC perdrait.

import { act } from "@testing-library/react";
import type { InvokeArgs, InvokeOptions } from "@tauri-apps/api/core";
import { clearMocks, mockIPC, mockWindows } from "@tauri-apps/api/mocks";
import { onTestFinished } from "vitest";
import type { AppError } from "@/bindings/AppError";
import type { ChatEvent } from "@/bindings/ChatEvent";
import type { SendRequest } from "@/bindings/SendRequest";

/**
 * Préfixe des erreurs du simulateur ; le contrôle du bundle le cherche dans
 * dist/, où il ne doit jamais apparaître.
 */
export const SIMULATOR_MARKER = "chatonlm-ipc-simulator";

/**
 * Horodatage fixé par la normalisation des tests Rust ; remplacé à la
 * livraison par l'heure courante, comme le moteur qui date au moment de
 * l'enregistrement.
 */
export const SEQUENCE_TIME = 1_700_000_000_000;

/** Séquence enregistrée : conversation « chat-1 », flux « stream-1 ». */
export interface Sequence {
  description: string;
  request: SendRequest;
  /** Événements tels qu'envoyés au canal. */
  events: ChatEvent[];
  /** Erreur de la commande refusée, ou null. */
  error: AppError | null;
}

const FILES = import.meta.glob<Sequence>("./fixtures/chat/*.json", {
  eager: true,
  import: "default",
});

const SEQUENCES = new Map(
  Object.entries(FILES).map(([path, sequence]) => [
    path.replace(/^.*\/|\.json$/g, ""),
    sequence,
  ]),
);

/** Noms des séquences disponibles. */
export const sequenceNames = [...SEQUENCES.keys()].sort();

function clone<T>(value: T): T {
  return JSON.parse(JSON.stringify(value ?? null)) as T;
}

/** Séquence enregistrée, copiée : le test peut la modifier. */
export function sequence(name: string): Sequence {
  const found = SEQUENCES.get(name);
  if (!found) {
    throw new Error(
      `[${SIMULATOR_MARKER}] unknown sequence "${name}" (available: ${sequenceNames.join(", ")})`,
    );
  }
  return clone(found);
}

/** Remplace les horodatages normalisés par `now`. */
function restamp(value: unknown, now: number): unknown {
  if (Array.isArray(value)) return value.map((item) => restamp(item, now));
  if (typeof value !== "object" || value === null) return value;
  return Object.fromEntries(
    Object.entries(value).map(([key, field]) => [
      key,
      key.endsWith("At") && field === SEQUENCE_TIME ? now : restamp(field, now),
    ]),
  );
}

export type Args = Record<string, unknown>;
/**
 * Gestionnaire d'une commande : ce qu'il renvoie (ou sa promesse) est la
 * réponse, ce qu'il lance l'erreur.
 */
export type Handler = (args: Args) => unknown;
type ReplayListener = (event: ChatEvent, replay: Replay) => void;
type SendListener = (request: SendRequest, replay: Replay) => void;

/**
 * Arguments d'une commande appelée avec un corps brut (stage_bytes : octets
 * du fichier, nom dans l'en-tête x-filename).
 */
export interface RawBody {
  /** Octets reçus (Uint8Array, ArrayBuffer ou tableau d'octets envoyé). */
  bytes: number[];
  /** En-têtes d'invoke, noms en minuscules (insensibles à la casse en Rust). */
  headers: Record<string, string>;
}

/** Champs de la requête comparés à ceux de la séquence. */
const REQUEST_FIELDS = [
  "chatId",
  "content",
  "model",
  "think",
  "attachmentIds",
  "editMessageId",
  "keptAttachmentIds",
  "webSearch",
  "image",
] as const;
type RequestField = (typeof REQUEST_FIELDS)[number];

export interface ReplayOptions {
  /**
   * Pas à pas : rien n'est livré avant step(), until() ou finish() ; un
   * « cancelled » final attend en plus cancel_message (bouton Stop).
   */
  stepwise?: boolean;
  /** Mode automatique : attente avant chaque événement (0 = tâche suivante). */
  delayMs?: number;
  /**
   * Réponse de la commande avant l'événement final : Tauri ne garantit pas
   * l'ordre entre les messages d'un canal et la réponse de la commande.
   */
  lateFinalEvent?: boolean;
  /** Champs de la requête non comparés à ceux de la séquence. */
  ignore?: RequestField[];
}

interface TauriInternals {
  runCallback: (id: number, data: unknown) => void;
  unregisterCallback: (id: number) => void;
  callbacks: Map<number, unknown>;
  invoke: (
    command: string,
    args?: InvokeArgs,
    options?: InvokeOptions,
  ) => Promise<unknown>;
}

interface EventPluginInternals {
  unregisterListener: (event: string, eventId: number) => void;
}

function internals(): TauriInternals {
  return (window as unknown as { __TAURI_INTERNALS__: TauriInternals })
    .__TAURI_INTERNALS__;
}

function eventInternals(): EventPluginInternals {
  return (
    window as unknown as {
      __TAURI_EVENT_PLUGIN_INTERNALS__: EventPluginInternals;
    }
  ).__TAURI_EVENT_PLUGIN_INTERNALS__;
}

/**
 * Corps brut (Uint8Array, ArrayBuffer, tableau d'octets) : le moteur le
 * reçoit tel quel (InvokeBody::Raw), sans JSON, avec les en-têtes ; null pour
 * des arguments ordinaires.
 */
function rawBody(args: unknown, options?: InvokeOptions): RawBody | null {
  let bytes: Uint8Array;
  if (ArrayBuffer.isView(args)) {
    bytes = new Uint8Array(args.buffer, args.byteOffset, args.byteLength);
  } else if (args instanceof ArrayBuffer) {
    bytes = new Uint8Array(args);
  } else if (Array.isArray(args)) {
    bytes = Uint8Array.from(args as number[]);
  } else {
    return null;
  }
  // Headers, comme fetch : noms en minuscules, valeur non ASCII refusée.
  const headers = new Headers(options?.headers);
  return { bytes: Array.from(bytes), headers: Object.fromEntries(headers) };
}

const CHANNEL_PREFIX = "__CHANNEL__:";

/** Une séquence attendue, puis rejouée pour un send_message. */
export class Replay {
  /** Requête reçue, telle que le moteur la lit (passée par JSON). */
  request: SendRequest | null = null;
  /** Conversation : celle de la requête, ou celle créée par chatCreated. */
  chatId: string | null = null;
  /** Événements déjà livrés. */
  readonly delivered: ChatEvent[] = [];
  cancelRequested = false;
  /** Canal fermé ({end: true} envoyé). */
  closed = false;
  /** Commande terminée (résolue ou rejetée). */
  settled = false;
  /** Résolue à l'arrivée du send_message. */
  readonly started: Promise<void>;
  /** Résolue quand la commande est terminée. */
  readonly done: Promise<void>;

  private channelId = -1;
  private next = 0;
  private markStarted = () => {};
  private markDone = () => {};
  private markCancelled = () => {};
  private readonly cancelSignal: Promise<void>;
  private answer: { resolve: (v: null) => void; reject: (e: unknown) => void } =
    { resolve: () => {}, reject: () => {} };

  constructor(
    readonly sequence: Sequence,
    readonly name: string,
    readonly options: ReplayOptions,
    private readonly simulator: Simulator,
  ) {
    this.started = new Promise((resolve) => (this.markStarted = resolve));
    this.done = new Promise((resolve) => (this.markDone = resolve));
    this.cancelSignal = new Promise(
      (resolve) => (this.markCancelled = resolve),
    );
  }

  get streamId(): string | null {
    return this.request?.streamId ?? null;
  }

  /** Nombre d'événements pas encore livrés. */
  get remaining(): number {
    return this.sequence.events.length - this.next;
  }

  /** Livre les `count` prochains événements (mode pas à pas). */
  async step(count = 1) {
    this.assertStepwise("step");
    await act(async () => {
      for (let i = 0; i < count; i++) {
        if (this.remaining === 0) {
          throw this.simulator.fail(
            `step(): "${this.name}" has no event left (${this.delivered.length} delivered)`,
          );
        }
        await this.answerIfLate();
        this.deliverNext();
      }
      this.endIfComplete();
    });
  }

  /** Livre les événements jusqu'au prochain du type donné, inclus. */
  async until(type: ChatEvent["type"]) {
    this.assertStepwise("until");
    const events = this.sequence.events;
    const index = events.findIndex((e, i) => i >= this.next && e.type === type);
    if (index === -1) {
      throw this.simulator.fail(
        `until("${type}"): no such event left in "${this.name}"`,
      );
    }
    await this.step(index - this.next + 1);
  }

  /** Livre le reste, ferme le canal et termine la commande. */
  async finish() {
    this.assertStepwise("finish");
    await act(async () => {
      while (this.remaining > 0) {
        await this.answerIfLate();
        this.deliverNext();
      }
      this.endIfComplete();
    });
  }

  /** Appelé par send_message : vérifie la requête, puis rejoue. */
  begin(request: SendRequest, channelId: number): Promise<null> {
    this.request = request;
    this.chatId = request.chatId ?? null;
    this.channelId = channelId;
    const answer = new Promise<null>((resolve, reject) => {
      this.answer = { resolve, reject };
    });
    this.markStarted();
    // Un échec y est déjà noté dans les problèmes du test.
    if (!this.options.stepwise) this.autoplay().catch(() => {});
    return answer;
  }

  /** Appelé par cancel_message. */
  cancel() {
    const last = this.sequence.events.at(-1);
    if (last?.type !== "cancelled") {
      this.simulator.fail(
        `cancel_message for "${this.name}", which does not end with "cancelled": replay the "cancelled" sequence to test a cancellation`,
      );
      return;
    }
    this.cancelRequested = true;
    this.markCancelled();
  }

  private assertStepwise(method: string) {
    if (!this.options.stepwise) {
      throw this.simulator.fail(
        `${method}() needs a stepwise replay ("${this.name}")`,
      );
    }
    if (!this.request) {
      throw this.simulator.fail(
        `${method}() before send_message ("${this.name}"): await replay.started first`,
      );
    }
  }

  private async autoplay() {
    const events = this.sequence.events;
    const delay = this.options.delayMs ?? 0;
    while (this.remaining > 0) {
      // Comme le moteur : « cancelled » ne vient qu'après cancel_message.
      if (this.remaining === 1 && events[this.next].type === "cancelled") {
        await this.cancelSignal;
      }
      if (this.remaining === 1 && this.options.lateFinalEvent) this.settle();
      await new Promise((resolve) => setTimeout(resolve, delay));
      if (this.simulator.disposed) return;
      this.deliverNext();
    }
    this.endIfComplete();
  }

  private deliverNext() {
    const event = this.sequence.events[this.next];
    if (event.type === "cancelled" && !this.cancelRequested) {
      throw this.simulator.fail(
        `"${this.name}" ends with "cancelled", which only comes after cancel_message (click Stop first)`,
      );
    }
    const message = restamp(event, Date.now()) as ChatEvent;
    if (message.type === "chatCreated") this.chatId = message.chatId;
    this.simulator.emit(clone(message), this);
    this.send({ message, index: this.next });
    this.next++;
    this.delivered.push(message);
  }

  /**
   * Option lateFinalEvent : avant l'événement final, la commande répond, et
   * le frontend le voit avant cet événement (tâche suivante).
   */
  private async answerIfLate() {
    if (this.remaining !== 1 || !this.options.lateFinalEvent || this.settled)
      return;
    this.settle();
    await new Promise((resolve) => setTimeout(resolve, 0));
  }

  /** Toute la séquence livrée : fermeture du canal, puis réponse. */
  private endIfComplete() {
    if (this.remaining > 0 || this.closed) return;
    this.closed = true;
    this.send({ end: true, index: this.sequence.events.length });
    this.settle();
  }

  private send(data: object) {
    if (this.simulator.disposed) return;
    const tauri = internals();
    if (!tauri.callbacks.has(this.channelId)) {
      throw this.simulator.fail(
        `channel ${this.channelId} of "${this.name}" is no longer registered`,
      );
    }
    tauri.runCallback(this.channelId, data);
  }

  private settle() {
    if (this.settled) return;
    this.settled = true;
    const { error } = this.sequence;
    if (error) this.answer.reject(clone(error));
    else this.answer.resolve(null);
    this.markDone();
  }
}

/** Canal reçu par un gestionnaire, alimenté par le test. */
export interface FakeChannel {
  /** Livre un message, comme le moteur (index croissants), dans act. */
  send(message: unknown): Promise<void>;
}

export interface Simulator {
  /**
   * Canal passé en argument d'une commande (« __CHANNEL__:<id> ») ; un
   * argument qui n'en est pas un fait échouer le test.
   */
  channel(argument: unknown): FakeChannel;
  /** Le prochain send_message rejouera cette séquence (file d'attente). */
  queue(sequence: Sequence | string, options?: ReplayOptions): Replay;
  /** Gestionnaire d'une commande (remplace le précédent). */
  handle(command: string, handler: Handler): void;
  /** Prévenu de chaque événement livré (base de données simulée...). */
  onEvent(listener: ReplayListener): void;
  /**
   * Prévenu de chaque send_message conforme à sa séquence, avant la
   * relecture ; ce qu'il lance refuse la commande et la séquence n'est pas
   * rejouée (lancer simulator.fail(...) pour faire aussi échouer le test).
   */
  onSend(listener: SendListener): void;
  /**
   * Événement Tauri émis par le moteur (glisser-déposer...) : chaque écouteur
   * encore inscrit reçoit `{ event, id, payload }` (id : celui de son
   * inscription), dans act.
   */
  emitEvent(event: string, payload: unknown): Promise<void>;
  /** Nombre d'écouteurs inscrits pour un événement. */
  listening(event: string): number;
  /**
   * Commandes reçues, dans l'ordre, avec leurs arguments passés par JSON (un
   * corps brut : RawBody) ; sans les inscriptions aux événements (listening).
   */
  readonly calls: { command: string; args: Args; at: number }[];
  /** Arguments des appels d'une commande. */
  callsTo(command: string): Args[];
  /** Problèmes constatés ; le test échoue à la fin s'il y en a. */
  readonly problems: string[];
  /** Fin du test : plus aucune livraison. */
  readonly disposed: boolean;
  /** Note un problème et renvoie l'erreur correspondante. */
  fail(problem: string): Error;
  /** Pour Replay : prévient les abonnés d'un événement livré. */
  emit(event: ChatEvent, replay: Replay): void;
}

/**
 * Installe le simulateur pour le test en cours (à appeler dans le test) ; il
 * est retiré à la fin du test, qui échoue si un problème a été constaté.
 */
export function installSimulator({
  handlers = {},
  latencyMs = 0,
}: {
  /** Gestionnaires des commandes autres que send_message et cancel_message. */
  handlers?: Record<string, Handler>;
  /** Délai de leurs réponses (IPC, base de données) ; 0 = tâche suivante. */
  latencyMs?: number;
} = {}): Simulator {
  const table = new Map(Object.entries(handlers));
  const listeners: ReplayListener[] = [];
  const sendListeners: SendListener[] = [];
  const pending: Replay[] = [];
  const replays: Replay[] = [];
  const problems: string[] = [];
  const calls: Simulator["calls"] = [];
  const streamIds = new Set<string>();
  /** Écouteurs d'événements Tauri, par identifiant d'inscription. */
  const subscriptions = new Map<number, { event: string; handler: number }>();
  let lastEventId = 0;

  const simulator: Simulator = {
    channel(argument) {
      if (
        typeof argument !== "string" ||
        !argument.startsWith(CHANNEL_PREFIX)
      ) {
        throw simulator.fail(
          `expected a Channel, got ${JSON.stringify(argument)}`,
        );
      }
      const id = Number(argument.slice(CHANNEL_PREFIX.length));
      let index = 0;
      return {
        async send(message) {
          await act(async () => {
            if (simulator.disposed) return;
            if (!internals().callbacks.has(id)) {
              throw simulator.fail(`channel ${id} is no longer registered`);
            }
            internals().runCallback(id, { message: clone(message), index });
            index += 1;
          });
        },
      };
    },
    queue(source, options = {}) {
      const name = typeof source === "string" ? source : "custom sequence";
      const data = typeof source === "string" ? sequence(source) : source;
      const replay = new Replay(data, name, options, simulator);
      pending.push(replay);
      return replay;
    },
    handle(command, handler) {
      table.set(command, handler);
    },
    onEvent(listener) {
      listeners.push(listener);
    },
    onSend(listener) {
      sendListeners.push(listener);
    },
    async emitEvent(event, payload) {
      await act(async () => {
        for (const [id, subscription] of [...subscriptions]) {
          // Un écouteur précédent a pu en désinscrire un autre.
          if (subscription.event !== event || !subscriptions.has(id)) continue;
          internals().runCallback(subscription.handler, {
            event,
            id,
            payload: clone(payload),
          });
        }
      });
    },
    listening(event) {
      return [...subscriptions.values()].filter((s) => s.event === event)
        .length;
    },
    calls,
    callsTo(command) {
      return calls.filter((c) => c.command === command).map((c) => c.args);
    },
    problems,
    disposed: false,
    fail(problem) {
      const message = `[${SIMULATOR_MARKER}] ${problem}`;
      problems.push(message);
      return new Error(message);
    },
    emit(event, replay) {
      for (const listener of listeners) listener(event, replay);
    },
  };

  function sendMessage(args: Args): Promise<null> {
    const replay = pending.shift();
    if (!replay) {
      throw simulator.fail(
        `send_message without a queued sequence: ${JSON.stringify(args.request)}`,
      );
    }
    const request = args.request as SendRequest;
    const channel = args.onEvent;
    if (typeof channel !== "string" || !channel.startsWith(CHANNEL_PREFIX)) {
      throw simulator.fail(`send_message: onEvent is not a Channel`);
    }
    if (!request.streamId || streamIds.has(request.streamId)) {
      throw simulator.fail(
        `send_message: stream id "${request.streamId}" missing or reused`,
      );
    }
    streamIds.add(request.streamId);
    const ignore = replay.options.ignore ?? [];
    const expected = replay.sequence.request;
    // Champ absent ≠ valeur vide : le frontend omet ce qu'il n'a pas.
    for (const field of REQUEST_FIELDS) {
      if (ignore.includes(field)) continue;
      const got = JSON.stringify(request[field]);
      const want = JSON.stringify(expected[field]);
      if (got !== want) {
        throw simulator.fail(
          `send_message for "${replay.name}": ${field} is ${got ?? "absent"}, the sequence has ${want ?? "absent"}`,
        );
      }
    }
    for (const listener of sendListeners) listener(request, replay);
    replays.push(replay);
    return replay.begin(request, Number(channel.slice(CHANNEL_PREFIX.length)));
  }

  function cancelMessage(args: Args): null {
    const replay = replays.find(
      (r) => r.streamId === args.streamId && !r.closed,
    );
    // Réponse déjà finie, ou inconnue : sans effet, comme le moteur.
    replay?.cancel();
    return null;
  }

  /** plugin:event|listen : identifiant d'inscription, comme le moteur. */
  function listen(args: Args): number {
    const { event, handler } = args;
    if (typeof event !== "string" || typeof handler !== "number") {
      throw simulator.fail(
        `plugin:event|listen: unexpected arguments ${JSON.stringify(args)}`,
      );
    }
    lastEventId += 1;
    subscriptions.set(lastEventId, { event, handler });
    return lastEventId;
  }

  /** plugin:event|unlisten : sans effet pour une inscription inconnue. */
  function unlisten(args: Args): null {
    const id = args.eventId as number;
    if (subscriptions.get(id)?.event === args.event) subscriptions.delete(id);
    return null;
  }

  mockIPC(async (command, raw) => {
    // Le moteur reçoit du JSON : un Channel y devient « __CHANNEL__:<id> ».
    const args = clone((raw ?? {}) as Args);
    // Avant toute attente : unlisten suit aussitôt unregisterListener.
    if (command === "plugin:event|listen") return listen(args);
    if (command === "plugin:event|unlisten") return unlisten(args);
    calls.push({ command, args, at: Date.now() });
    if (command === "send_message") return sendMessage(args);
    if (command === "cancel_message") return cancelMessage(args);
    const handler = table.get(command);
    if (!handler) {
      throw simulator.fail(
        `unexpected command "${command}" ${JSON.stringify(args)}: give it a handler`,
      );
    }
    const answered = new Promise((resolve) => setTimeout(resolve, latencyMs));
    try {
      const result = clone(await handler(args));
      await answered;
      return result;
    } catch (error) {
      await answered;
      throw error instanceof Error ? error : clone(error);
    }
  });
  // mockIPC ignore les options d'invoke : corps brut et en-têtes sont
  // regroupés avant de lui être passés.
  const tauri = internals();
  const mocked = tauri.invoke;
  tauri.invoke = (command, args, options) =>
    mocked(command, (rawBody(args, options) ?? args) as InvokeArgs, options);
  // Celui de mockIPC prend l'identifiant d'inscription pour celui du rappel :
  // comme le moteur, il retrouve le rappel de l'inscription et le retire.
  eventInternals().unregisterListener = (event, eventId) => {
    const subscription = subscriptions.get(eventId);
    if (subscription?.event === event) {
      internals().unregisterCallback(subscription.handler);
    }
  };
  // getCurrentWindow() et getCurrentWebview() : fenêtre « main ».
  mockWindows("main");

  onTestFinished(() => {
    (simulator as { disposed: boolean }).disposed = true;
    clearMocks();
    for (const replay of pending) {
      problems.push(
        `[${SIMULATOR_MARKER}] "${replay.name}" was queued, but no send_message came`,
      );
    }
    if (problems.length > 0) throw new Error(problems.join("\n"));
  });

  return simulator;
}
