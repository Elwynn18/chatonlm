// Réponses en cours d'envoi, hors de React : un magasin externe lu par
// useSyncExternalStore. Les fragments arrivent par centaines ; les abonnés
// sont prévenus tout de suite pour le premier, puis au plus une fois toutes
// les 16 ms.
import type { AttachmentInfo } from "@/bindings/AttachmentInfo";
import type { ChatErrorKind } from "@/bindings/ChatErrorKind";
import type { ChatEvent } from "@/bindings/ChatEvent";
import type { Compaction } from "@/bindings/Compaction";
import type { ContextUsage } from "@/bindings/ContextUsage";
import type { ImageProgress } from "@/bindings/ImageProgress";
import type { Message } from "@/bindings/Message";
import type { StagedFile } from "@/bindings/StagedFile";

export interface StreamError {
  kind: ChatErrorKind;
  message: string;
}

/**
 * Téléchargement du modèle avant la réponse (somme de toutes les couches ;
 * l'officiel ne suit que la plus grosse), en octets ; `done` : terminé, la
 * réponse va suivre.
 */
export interface DownloadProgress {
  total: number;
  completed: number;
  done: boolean;
}

export interface LiveStream {
  streamId: string;
  /** null tant que la nouvelle conversation n'est pas créée. */
  chatId: string | null;
  /** Texte envoyé. */
  content: string;
  model: string;
  /**
   * Message modifié (son id en base) : lui et les suivants laissent la place
   * au message envoyé et à sa réponse ; null pour un nouveau message.
   */
  editOf: number | null;
  /** Départ (Date.now()) : le message enregistré pour ce flux est postérieur. */
  startedAt: number;
  /**
   * Message envoyé tel qu'affiché avant son enregistrement : identifiant
   * négatif propre au flux, même objet tout du long (mémoïsation).
   */
  pending: Message;
  /** Message de l'utilisateur, une fois enregistré. */
  user: Message | null;
  /**
   * Réponses, chacune dès son premier fragment : une par tour (recherche
   * web : le modèle appelle des outils, puis reprend dans une nouvelle
   * réponse). La dernière reçoit raisonnement et texte.
   */
  replies: Message[];
  /**
   * Dernier état du téléchargement du modèle, s'il est absent ; effacé à la
   * fin du flux, comme l'officiel (done, erreur, arrêt).
   */
  download: DownloadProgress | null;
  /**
   * Modèle en cours de chargement en mémoire (entre `loading` et `loaded`,
   * ou le premier événement suivant) : la ligne « Loading model... » prend
   * la place des trois points. Jamais gardé dans l'historique.
   */
  loading: boolean;
  /**
   * Compactage automatique en cours (entre `compacting` et `compacted`, ou le
   * premier événement suivant) : la ligne « Compacting conversation… » prend
   * la place des trois points (propre à ChatonLM).
   */
  compacting: boolean;
  /**
   * Contexte annoncé par le dernier événement `context` ou `compacted` du
   * flux (null : taille inconnue) ; undefined tant qu'aucun n'est venu (la
   * jauge garde la valeur enregistrée de la conversation).
   */
  context: ContextUsage | null | undefined;
  /** Compactages faits par cet envoi, pas encore relus en base. */
  compactions: Compaction[];
  /**
   * Envoi du bouton « Image » (propre à ChatonLM) : le texte part tel quel au
   * modèle d'images.
   */
  image: boolean;
  /**
   * Dernier avancement de chaque image en cours (appel `generate_image`),
   * par `imageKey(messageId, index)` ; jamais enregistré en base.
   */
  images: Readonly<Record<string, ImageProgress>>;
  finished: boolean;
}

/** Clé d'un appel d'outil dans `LiveStream.images`. */
export function imageKey(messageId: number, index: number): string {
  return `${messageId}:${index}`;
}

/** Clé des erreurs d'une nouvelle conversation refusée avant sa création. */
export const NEW_CHAT = "new";

const BATCH_MS = 16;

let streams = new Map<string, LiveStream>();
/** Dernière erreur par conversation, effacée au prochain envoi. */
let errors = new Map<string, StreamError>();
const listeners = new Set<() => void>();
let scheduled: ReturnType<typeof setTimeout> | null = null;
let lastNotify = 0;
let nextPendingId = -1;

function notify() {
  scheduled = null;
  lastNotify = Date.now();
  for (const listener of listeners) listener();
}

function changed() {
  if (scheduled) return;
  const wait = BATCH_MS - (Date.now() - lastNotify);
  if (wait <= 0) notify();
  else scheduled = setTimeout(notify, wait);
}

function update(streamId: string, change: (stream: LiveStream) => LiveStream) {
  const stream = streams.get(streamId);
  if (!stream) return;
  streams = new Map(streams).set(streamId, change(stream));
  changed();
}

export function subscribe(listener: () => void) {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

const NO_STREAMS: readonly LiveStream[] = [];

/** Dernier résultat de streamsFor par conversation. */
let snapshots = new Map<
  string,
  { source: Map<string, LiveStream>; value: readonly LiveStream[] }
>();

/**
 * Flux d'une conversation, dans l'ordre d'envoi : celui qui répond, et ceux
 * qui sont terminés mais pas encore retirés (leur réponse n'est peut-être pas
 * encore relue en base). Même tableau tant qu'ils n'ont pas changé, comme
 * l'exige useSyncExternalStore.
 */
export function streamsFor(chatId: string): readonly LiveStream[] {
  const cached = snapshots.get(chatId);
  if (cached?.source === streams) return cached.value;
  const current = [...streams.values()].filter((s) => s.chatId === chatId);
  if (current.length === 0) {
    snapshots.delete(chatId);
    return NO_STREAMS;
  }
  const previous = cached?.value ?? NO_STREAMS;
  const unchanged =
    current.length === previous.length &&
    current.every((stream, i) => stream === previous[i]);
  const value = unchanged ? previous : current;
  snapshots.set(chatId, { source: streams, value });
  return value;
}

/**
 * Dernier avancement de l'image de l'appel `index` de la réponse
 * `messageId`, quel que soit son flux ; null : aucun (pas commencée, ou plus
 * de flux). Même objet tant qu'il ne change pas (useSyncExternalStore).
 */
export function imageProgressFor(
  messageId: number,
  index: number,
): ImageProgress | null {
  const key = imageKey(messageId, index);
  for (const stream of streams.values()) {
    const progress = stream.images[key];
    if (progress) return progress;
  }
  return null;
}

/** Flux qui répond encore parmi ceux d'une conversation. */
export function activeStream(list: readonly LiveStream[]): LiveStream | null {
  return list.find((stream) => !stream.finished) ?? null;
}

/** Flux d'une nouvelle conversation pas encore créée. */
export function pendingNewStream(): LiveStream | null {
  for (const stream of streams.values()) {
    if (stream.chatId === null && !stream.finished) return stream;
  }
  return null;
}

/**
 * Attend que la conversation ne réponde plus (NEW_CHAT : nouvelle
 * conversation pas encore créée), au plus `timeoutMs` : la phrase du mode
 * vocal qui suit un arrêt part une fois la réponse arrêtée (le moteur
 * refuse un envoi à une conversation occupée).
 */
export function whenIdle(chatId: string, timeoutMs = 10_000): Promise<void> {
  const busy = () =>
    chatId === NEW_CHAT
      ? pendingNewStream() !== null
      : activeStream(streamsFor(chatId)) !== null;
  return new Promise((resolve) => {
    if (!busy()) {
      resolve();
      return;
    }
    const done = () => {
      clearTimeout(timer);
      listeners.delete(check);
      resolve();
    };
    const check = () => {
      if (!busy()) done();
    };
    const timer = setTimeout(done, timeoutMs);
    listeners.add(check);
  });
}

/** Dernière erreur d'une conversation (NEW_CHAT : nouvelle conversation). */
export function errorFor(chatId: string): StreamError | null {
  return errors.get(chatId) ?? null;
}

export function getStream(streamId: string): LiveStream | null {
  return streams.get(streamId) ?? null;
}

function emptyMessage(
  role: Message["role"],
  id: number,
  at: number,
  model: string | null,
): Message {
  return {
    id,
    role,
    content: "",
    thinking: "",
    model,
    createdAt: at,
    updatedAt: at,
    thinkingStartedAt: null,
    thinkingEndedAt: null,
    attachments: [],
    toolCalls: [],
  };
}

export function start({
  attachments = [],
  kept = [],
  editOf = null,
  image = false,
  ...stream
}: {
  streamId: string;
  chatId: string | null;
  content: string;
  model: string;
  /** Fichiers joints, montrés avec le message avant son enregistrement. */
  attachments?: StagedFile[];
  /** Pièces jointes gardées du message modifié (avant les nouvelles). */
  kept?: AttachmentInfo[];
  editOf?: number | null;
  /** Envoi du bouton « Image ». */
  image?: boolean;
}) {
  const startedAt = Date.now();
  streams = new Map(streams).set(stream.streamId, {
    ...stream,
    editOf,
    image,
    startedAt,
    pending: {
      ...emptyMessage("user", nextPendingId--, startedAt, null),
      content: stream.content,
      // Nouvelles : identifiants négatifs, pas encore en base, donc pas
      // d'image à demander (vignette vide, voir MessageList).
      attachments: [
        ...kept,
        ...attachments.map((file, index) => ({
          id: -(index + 1),
          filename: file.filename,
          kind: file.kind,
          mime: file.mime,
          size: file.size,
        })),
      ],
    },
    user: null,
    replies: [],
    download: null,
    loading: false,
    compacting: false,
    context: undefined,
    compactions: [],
    images: {},
    finished: false,
  });
  // L'erreur de la conversation, et celle d'une nouvelle conversation
  // refusée plus tôt, qui n'a plus de sens après un autre envoi.
  const keys = [stream.chatId ?? NEW_CHAT, NEW_CHAT];
  if (keys.some((key) => errors.has(key))) {
    errors = new Map(errors);
    for (const key of keys) errors.delete(key);
  }
  notify();
}

/** Efface la dernière erreur d'une conversation (page quittée). */
export function clearError(chatId: string) {
  if (!errors.has(chatId)) return;
  errors = new Map(errors);
  errors.delete(chatId);
  notify();
}

/** Remplace la dernière réponse (aucune : flux inchangé). */
function withLastReply(
  stream: LiveStream,
  change: (reply: Message) => Message,
): LiveStream {
  const last = stream.replies.at(-1);
  if (!last) return stream;
  return { ...stream, replies: [...stream.replies.slice(0, -1), change(last)] };
}

/** Remplace la réponse `id` (inconnue : flux inchangé). */
function withReply(
  stream: LiveStream,
  id: number,
  change: (reply: Message) => Message,
): LiveStream {
  const index = stream.replies.findIndex((reply) => reply.id === id);
  if (index === -1) return stream;
  const replies = [...stream.replies];
  replies[index] = change(replies[index]);
  return { ...stream, replies };
}

/** Raisonnement clos (s'il a commencé et n'est pas déjà fini). */
function closeThinking(reply: Message, now: number): Message {
  return reply.thinkingStartedAt !== null && reply.thinkingEndedAt === null
    ? { ...reply, thinkingEndedAt: now }
    : reply;
}

/** Applique un événement du backend. */
export function apply(streamId: string, event: ChatEvent) {
  const now = Date.now();
  // Chargement fini à `loaded`, mais aussi à tout autre événement (le moteur
  // n'enverrait rien d'autre entre les deux) : la ligne ne reste jamais
  // affichée sous une réponse, une erreur ou un arrêt.
  if (event.type !== "loading" && streams.get(streamId)?.loading) {
    update(streamId, (s) => ({ ...s, loading: false }));
  }
  // De même pour le compactage automatique.
  if (event.type !== "compacting" && streams.get(streamId)?.compacting) {
    update(streamId, (s) => ({ ...s, compacting: false }));
  }
  switch (event.type) {
    case "loading":
      update(streamId, (s) => ({ ...s, loading: true }));
      break;
    case "loaded":
      // Déjà fait plus haut.
      break;
    case "chatCreated":
      update(streamId, (s) => ({ ...s, chatId: event.chatId }));
      break;
    case "userMessage":
      update(streamId, (s) => ({ ...s, user: event.message }));
      break;
    case "download":
      update(streamId, (s) => ({
        ...s,
        download: {
          total: event.total,
          completed: event.completed,
          done: event.done,
        },
      }));
      break;
    case "replyStarted":
      // Réponse suivante d'un même envoi : la précédente a déjà reçu ses
      // appels d'outils, raisonnement clos.
      update(streamId, (s) => ({
        ...s,
        replies: [
          ...s.replies,
          emptyMessage("assistant", event.messageId, event.createdAt, s.model),
        ],
      }));
      break;
    case "thinking":
      update(streamId, (s) =>
        withLastReply(s, (reply) => ({
          ...reply,
          thinking: reply.thinking + event.text,
          thinkingStartedAt: reply.thinkingStartedAt ?? now,
        })),
      );
      break;
    case "content":
      update(streamId, (s) =>
        withLastReply(s, (reply) => ({
          ...closeThinking(reply, now),
          content: reply.content + event.text,
        })),
      );
      break;
    case "toolCalls":
      // Fin d'un tour qui appelle des outils : le moteur clôt son
      // raisonnement en même temps qu'il enregistre les appels.
      update(streamId, (s) =>
        withReply(s, event.messageId, (reply) => ({
          ...closeThinking(reply, now),
          toolCalls: event.toolCalls,
        })),
      );
      break;
    case "toolResult":
      update(streamId, (s) =>
        withReply(s, event.messageId, (reply) => ({
          ...reply,
          toolCalls: reply.toolCalls.map((call, index) =>
            index === event.index
              ? { ...call, result: event.result, error: event.error }
              : call,
          ),
        })),
      );
      break;
    case "imageProgress":
      update(streamId, (s) => ({
        ...s,
        images: {
          ...s.images,
          [imageKey(event.messageId, event.index)]: event.progress,
        },
      }));
      break;
    case "imageReady":
      // Image enregistrée en pièce jointe de la réponse (avant le résultat
      // de l'appel, qui la désigne).
      update(streamId, (s) =>
        withReply(s, event.messageId, (reply) => ({
          ...reply,
          attachments: [...reply.attachments, event.attachment],
        })),
      );
      break;
    case "context":
      update(streamId, (s) => ({
        ...s,
        context: { used: event.used, size: event.size },
      }));
      break;
    case "compacting":
      update(streamId, (s) => ({ ...s, compacting: true }));
      break;
    case "compacted":
      update(streamId, (s) => ({
        ...s,
        context: event.context,
        compactions: [...s.compactions, event.compaction],
      }));
      break;
    case "error":
      fail(streamId, { kind: event.kind, message: event.message });
      break;
    case "done":
    case "cancelled":
      update(streamId, (s) => finish(s, now));
      break;
  }
}

/**
 * Flux terminé, quelle que soit la fin : le raisonnement resté ouvert (arrêt
 * ou erreur pendant le raisonnement) est clos, comme le moteur le fait en
 * base ; la réponse affichée jusqu'à sa relecture n'est jamais « en cours » ;
 * plus de téléchargement, de chargement ni de compactage en cours.
 */
function finish(stream: LiveStream, now: number): LiveStream {
  return {
    ...withLastReply(stream, (reply) => closeThinking(reply, now)),
    download: null,
    loading: false,
    compacting: false,
    finished: true,
  };
}

/**
 * Fin en erreur : l'erreur reste affichée jusqu'au prochain envoi ; avant la
 * création de la conversation, elle va à la page de nouvelle conversation.
 * `null` : refus déjà montré par la zone de saisie (avis du mode Image),
 * aucune erreur gardée.
 */
export function fail(streamId: string, error: StreamError | null) {
  const stream = streams.get(streamId);
  if (stream && error) {
    errors = new Map(errors).set(stream.chatId ?? NEW_CHAT, error);
  }
  update(streamId, (s) => finish(s, Date.now()));
}

/** Retire le flux, une fois la conversation relue en base. */
export function remove(streamId: string) {
  if (!streams.has(streamId)) return;
  streams = new Map(streams);
  streams.delete(streamId);
  notify();
}

/**
 * Messages à afficher : ceux de la base, complétés par les flux de la
 * conversation, dans l'ordre d'envoi (message envoyé pas encore relu, réponse
 * plus fraîche que la base). Les messages gardent leur identité d'un appel à
 * l'autre tant qu'ils ne changent pas.
 */
export function mergeMessages(
  saved: Message[],
  live: readonly LiveStream[],
): Message[] {
  if (live.length === 0) return saved;
  // Message modifié : lui et les suivants s'effacent dès l'envoi (comme la
  // troncature optimiste de l'officiel), tant que le flux est là ; un envoi
  // fini sans message enregistré (refusé, écriture en échec) n'a rien
  // changé en base : ils reviennent.
  const cut = editCut(live);
  const messages = saved.filter((m) => m.id < cut);
  // Messages de la base déjà attribués à un flux.
  const claimed = new Set(live.flatMap((s) => (s.user ? [s.user.id] : [])));
  for (const stream of live) {
    const { user, replies } = stream;
    // Modification refusée : les messages d'origine suffisent.
    if (stream.editOf !== null && stream.finished && !user) continue;
    // Envoi précédent, fini mais pas encore retiré, dont les messages sont
    // coupés par la modification en cours.
    if (
      stream.editOf === null &&
      ((user !== null && user.id >= cut) ||
        replies.some((reply) => reply.id >= cut))
    ) {
      continue;
    }
    if (user) {
      if (!messages.some((m) => m.id === user.id)) messages.push(user);
    } else {
      // Relu en base avant l'événement userMessage (même horloge système) :
      // le message envoyé y est déjà.
      const stored = saved.find(
        (m) =>
          m.role === "user" &&
          !claimed.has(m.id) &&
          m.content === stream.content &&
          m.createdAt >= stream.startedAt,
      );
      if (stored) claimed.add(stored.id);
      else messages.push(stream.pending);
    }
    for (const reply of replies) {
      const index = messages.findIndex((m) => m.id === reply.id);
      if (index === -1) messages.push(reply);
      else messages[index] = reply;
    }
  }
  return messages;
}

/**
 * Premier message coupé par une modification en cours (comme
 * mergeMessages) ; +Infini : aucune.
 */
function editCut(live: readonly LiveStream[]): number {
  return Math.min(
    ...live.map((s) =>
      s.editOf !== null && (s.user !== null || !s.finished)
        ? s.editOf
        : Number.POSITIVE_INFINITY,
    ),
  );
}

/**
 * Compactages à afficher (propres à ChatonLM) : ceux de la base, complétés
 * par ceux des flux pas encore relus, par id croissant. Ceux qui couvrent un
 * message coupé par une modification en cours disparaissent (le moteur les
 * supprime).
 */
export function mergeCompactions(
  saved: readonly Compaction[],
  live: readonly LiveStream[],
): readonly Compaction[] {
  const cut = editCut(live);
  // Rien à ajouter ni à retirer : même tableau (mémoïsation).
  if (cut === Infinity && live.every((s) => s.compactions.length === 0)) {
    return saved;
  }
  const byId = new Map<number, Compaction>();
  for (const compaction of saved) byId.set(compaction.id, compaction);
  for (const stream of live) {
    for (const compaction of stream.compactions) {
      byId.set(compaction.id, compaction);
    }
  }
  return [...byId.values()]
    .filter((c) => c.throughMessageId < cut)
    .sort((a, b) => a.id - b.id);
}

/**
 * Contexte à montrer par la jauge : celui du dernier événement `context` ou
 * `compacted` des flux de la conversation, sinon `saved` (get_chat). Une
 * modification de message en cours sans événement : inconnu (le moteur
 * oublie le contexte de la conversation modifiée).
 */
export function liveContext(
  saved: ContextUsage | null,
  live: readonly LiveStream[],
): ContextUsage | null {
  for (let i = live.length - 1; i >= 0; i--) {
    const stream = live[i];
    if (stream.context !== undefined) return stream.context;
    // Modification refusée (finie sans message enregistré) : rien n'a changé.
    if (stream.editOf !== null && (stream.user !== null || !stream.finished)) {
      return null;
    }
  }
  return saved;
}

/** Pour les tests : remet le magasin à zéro. */
export function resetStreams() {
  streams = new Map();
  errors = new Map();
  snapshots = new Map();
  if (scheduled) clearTimeout(scheduled);
  scheduled = null;
}
