// État du panneau, sans React : messages de l'hôte (conversation relue,
// événements d'un envoi, reçus, état de la liaison) appliqués par un réducteur
// pur. Les événements d'un envoi suivent la logique de la page de l'app
// (src/lib/streams.ts : réponse par fragments, raisonnement clos au premier
// texte, fin quelle qu'elle soit), sans ce qui n'existe pas ici (modification
// d'un message, images, compactage manuel). Visite guidée et séance vocale
// (lot 2) : leur état vient de l'hôte tel quel ; les tours de parole arrivent
// comme des envois, sous des numéros négatifs.
import type { Chat } from "@/bindings/Chat";
import type { ChatErrorKind } from "@/bindings/ChatErrorKind";
import type { ChatEvent } from "@/bindings/ChatEvent";
import type { Message } from "@/bindings/Message";
import type {
  HostMessage,
  Locale,
  PanelState,
  Receipt,
  Theme,
  TourView,
  VoiceView,
} from "../shared/protocol";

export interface TurnError {
  kind: ChatErrorKind;
  message: string;
}

/** Un envoi de cette session, tant que sa conversation n'est pas relue. */
export interface Turn {
  /** Clé stable pour React. */
  key: number;
  /** null : envoi du panneau que l'hôte n'a pas encore numéroté. */
  requestId: number | null;
  /** Texte tapé dans le panneau, montré avant l'enregistrement du message. */
  text: string | null;
  chatId: string | null;
  user: Message | null;
  replies: Message[];
  loading: boolean;
  compacting: boolean;
  /** Téléchargement du modèle, en pour cent ; null : aucun. */
  download: number | null;
  error: TurnError | null;
  finished: boolean;
}

export interface PanelModel {
  /** `init` reçu : la langue et le thème sont connus. */
  initialized: boolean;
  locale: Locale;
  theme: Theme;
  state: PanelState;
  /** Dernière conversation relue par l'hôte (chat.get). */
  chat: Chat | null;
  turns: Turn[];
  /** Reçus par numéro d'envoi. */
  receipts: Record<number, Receipt>;
  /** Reçus renvoyés par l'hôte, par numéro de message de l'utilisateur
   * (envois faits panneau fermé ou avant un rechargement). */
  savedReceipts: Record<number, Receipt>;
  /** Numéro d'envoi de chaque message de l'utilisateur envoyé ici. */
  requestOf: Record<number, number>;
  /** Erreur du dernier envoi, gardée jusqu'au suivant (comme la page). */
  lastError: { chatId: string | null; error: TurnError } | null;
  nextKey: number;
  /** Visite guidée en cours ou dernière (null : aucune). */
  tour: TourView | null;
  /** Séance vocale tenue par VS Code (null : aucune). */
  voice: VoiceView | null;
  /** Niveau du micro, 0 à 1. */
  voiceLevel: number;
}

export type PanelAction =
  /** Message de l'hôte ; `now` : heure de réception (Date.now()). */
  | { type: "host"; message: HostMessage; now: number }
  /** Texte envoyé depuis la zone de saisie. */
  | { type: "sent"; text: string }
  /** Bouton Stop : les envois que l'hôte n'a pas encore pris s'arrêtent (et
   * tous, si l'hôte ne dit plus rien écrire). */
  | { type: "stopped"; now: number };

export const INITIAL_STATE: PanelState = {
  connection: "connecting",
  models: [],
  cloudModels: [],
  model: null,
  ephemeral: false,
  chatId: null,
  busy: false,
  generation: 0,
  notice: null,
};

export function initialModel(): PanelModel {
  return {
    initialized: false,
    locale: "en",
    theme: "light",
    state: INITIAL_STATE,
    chat: null,
    turns: [],
    receipts: {},
    savedReceipts: {},
    requestOf: {},
    lastError: null,
    nextKey: 1,
    tour: null,
    voice: null,
    voiceLevel: 0,
  };
}

function emptyReply(id: number, at: number, model: string | null): Message {
  return {
    id,
    role: "assistant",
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

function newTurn(model: PanelModel, fields: Partial<Turn>): Turn {
  return {
    key: model.nextKey,
    requestId: null,
    text: null,
    chatId: model.state.chatId,
    user: null,
    replies: [],
    loading: false,
    compacting: false,
    download: null,
    error: null,
    finished: false,
    ...fields,
  };
}

/** Raisonnement clos (s'il a commencé et n'est pas déjà fini). */
function closeThinking(reply: Message, now: number): Message {
  return reply.thinkingStartedAt !== null && reply.thinkingEndedAt === null
    ? { ...reply, thinkingEndedAt: now }
    : reply;
}

function withLastReply(turn: Turn, change: (reply: Message) => Message) {
  const last = turn.replies.at(-1);
  if (!last) return turn;
  return { ...turn, replies: [...turn.replies.slice(0, -1), change(last)] };
}

function withReply(
  turn: Turn,
  id: number,
  change: (reply: Message) => Message,
): Turn {
  const index = turn.replies.findIndex((reply) => reply.id === id);
  if (index === -1) return turn;
  const replies = [...turn.replies];
  replies[index] = change(replies[index]);
  return { ...turn, replies };
}

/** Fin d'un envoi, quelle qu'elle soit : rien ne reste « en cours ». */
function finish(turn: Turn, now: number): Turn {
  return {
    ...withLastReply(turn, (reply) => closeThinking(reply, now)),
    loading: false,
    compacting: false,
    download: null,
    finished: true,
  };
}

/**
 * Nouvel envoi : les erreurs des précédents s'effacent (comme la page), et un
 * envoi fini sans message enregistré (refusé) disparaît.
 */
function forNewTurn(turns: Turn[]): Turn[] {
  return turns.flatMap((turn) =>
    !turn.finished
      ? [turn]
      : turn.user === null
        ? []
        : [turn.error ? { ...turn, error: null } : turn],
  );
}

/**
 * Envoi `requestId` : déjà connu, sinon le plus ancien envoi du panneau pas
 * encore numéroté, sinon un nouvel envoi (action lancée depuis l'éditeur).
 */
function bindTurn(
  model: PanelModel,
  requestId: number,
): { model: PanelModel; index: number } {
  const known = model.turns.findIndex((turn) => turn.requestId === requestId);
  if (known !== -1) return { model, index: known };
  // Tour de parole (numéro négatif) : jamais un message tapé en attente.
  const pending =
    requestId < 0
      ? -1
      : model.turns.findIndex(
          (turn) => turn.requestId === null && !turn.finished,
        );
  if (pending !== -1) {
    const turns = [...model.turns];
    turns[pending] = { ...turns[pending], requestId };
    return { model: { ...model, turns }, index: pending };
  }
  const turn = newTurn(model, { requestId });
  const kept = forNewTurn(model.turns);
  return {
    model: {
      ...model,
      turns: [...kept, turn],
      nextKey: model.nextKey + 1,
      lastError: null,
    },
    index: kept.length,
  };
}

/** Applique un événement d'envoi à son tour (voir src/lib/streams.ts). */
export function applyEvent(turn: Turn, event: ChatEvent, now: number): Turn {
  let next = turn;
  // Chargement et compactage finis au premier autre événement.
  if (event.type !== "loading" && next.loading) {
    next = { ...next, loading: false };
  }
  if (event.type !== "compacting" && next.compacting) {
    next = { ...next, compacting: false };
  }
  switch (event.type) {
    case "loading":
      return { ...next, loading: true };
    case "compacting":
      return { ...next, compacting: true };
    case "chatCreated":
      return { ...next, chatId: event.chatId };
    case "userMessage":
      return { ...next, user: event.message };
    case "download":
      return {
        ...next,
        download: event.done
          ? null
          : event.total > 0
            ? Math.min(100, Math.floor((event.completed / event.total) * 100))
            : 0,
      };
    case "replyStarted":
      return {
        ...next,
        replies: [
          ...next.replies,
          emptyReply(
            event.messageId,
            event.createdAt,
            next.user?.model ?? null,
          ),
        ],
      };
    case "thinking":
      return withLastReply(next, (reply) => ({
        ...reply,
        thinking: reply.thinking + event.text,
        thinkingStartedAt: reply.thinkingStartedAt ?? now,
      }));
    case "content":
      return withLastReply(next, (reply) => ({
        ...closeThinking(reply, now),
        content: reply.content + event.text,
      }));
    case "toolCalls":
      return withReply(next, event.messageId, (reply) => ({
        ...closeThinking(reply, now),
        toolCalls: event.toolCalls,
      }));
    case "toolResult":
      return withReply(next, event.messageId, (reply) => ({
        ...reply,
        toolCalls: reply.toolCalls.map((call, index) =>
          index === event.index
            ? { ...call, result: event.result, error: event.error }
            : call,
        ),
      }));
    case "error":
      return {
        ...finish(next, now),
        error: { kind: event.kind, message: event.message },
      };
    case "done":
    case "cancelled":
      return finish(next, now);
    default:
      // loaded, context, compacted, images : rien à montrer ici.
      return next;
  }
}

/** Identifiants des messages d'un envoi (envoyé et réponses). */
function turnIds(turn: Turn): number[] {
  return [
    ...(turn.user ? [turn.user.id] : []),
    ...turn.replies.map((r) => r.id),
  ];
}

/**
 * Conversation relue : les envois finis qu'elle contient se retirent (leur
 * erreur reste affichée à la fin, jusqu'au prochain envoi). Filet : un envoi
 * jamais fini (fin perdue) se retire aussi quand l'hôte ne dit plus rien
 * écrire et que la conversation a tous ses messages, sans réponse en cours.
 */
function withChat(model: PanelModel, chat: Chat): PanelModel {
  const saved = new Set(chat.messages.map((m) => m.id));
  let lastError = model.lastError;
  const turns = model.turns.filter((turn) => {
    const ours = turn.chatId === chat.id || turn.chatId === null;
    const ids = turnIds(turn);
    const over =
      turn.finished ||
      (!model.state.busy &&
        turn.requestId !== null &&
        (chat.replyRunning === null || !ids.includes(chat.replyRunning)));
    const read =
      over && ours && turn.user !== null && ids.every((id) => saved.has(id));
    if (read && turn.error) lastError = { chatId: chat.id, error: turn.error };
    return !read;
  });
  return { ...model, chat, turns, lastError };
}

/** État de la liaison : une autre conversation laisse les envois des autres. */
function withState(model: PanelModel, state: PanelState): PanelModel {
  let { turns, lastError, chat } = model;
  if (state.generation !== model.state.generation) {
    // Nouvelle conversation : rien de ce qui était montré ne continue ici
    // (l'hôte ne relaie plus les envois d'avant).
    turns = [];
    lastError = null;
    chat = null;
  } else if (state.chatId !== model.state.chatId) {
    turns = turns.filter(
      (turn) =>
        turn.chatId === null ||
        (state.chatId !== null && turn.chatId === state.chatId),
    );
    if (lastError && lastError.chatId !== state.chatId) lastError = null;
    if (state.chatId === null) chat = null;
  }
  // Liaison perdue : un envoi que l'hôte n'a jamais pris ne partira plus.
  if (state.connection !== "connected") {
    turns = turns.filter((turn) => turn.requestId !== null);
  }
  return { ...model, state, turns, lastError, chat };
}

export function reduce(model: PanelModel, action: PanelAction): PanelModel {
  switch (action.type) {
    case "sent":
      return {
        ...model,
        turns: [
          ...forNewTurn(model.turns),
          newTurn(model, { text: action.text }),
        ],
        nextKey: model.nextKey + 1,
        lastError: null,
      };
    case "stopped": {
      // Un envoi numéroté s'arrête par l'hôte ; s'il ne dit plus rien écrire,
      // sa fin a été perdue : l'envoi s'arrête ici.
      const { now } = action;
      return {
        ...model,
        turns: model.turns.map((turn) =>
          !turn.finished && (turn.requestId === null || !model.state.busy)
            ? finish(turn, now)
            : turn,
        ),
      };
    }
    case "host":
      return applyHost(model, action.message, action.now);
  }
}

function applyHost(
  model: PanelModel,
  message: HostMessage,
  now: number,
): PanelModel {
  switch (message.type) {
    case "init":
      return withState(
        {
          ...model,
          initialized: true,
          locale: message.locale,
          theme: message.theme,
        },
        message.state,
      );
    case "state":
      return withState(model, message.state);
    case "theme":
      return { ...model, theme: message.theme };
    case "chat":
      return withChat(model, message.chat);
    case "receipts":
      return { ...model, savedReceipts: message.receipts };
    case "tour":
      return { ...model, tour: message.tour };
    case "voice":
      return {
        ...model,
        voice: message.voice,
        voiceLevel: message.voice ? model.voiceLevel : 0,
      };
    case "voiceLevel":
      return model.voice ? { ...model, voiceLevel: message.level } : model;
    case "receipt": {
      const bound = bindTurn(model, message.requestId);
      return {
        ...bound.model,
        receipts: {
          ...bound.model.receipts,
          [message.requestId]: message.receipt,
        },
      };
    }
    case "event": {
      const bound = bindTurn(model, message.requestId);
      const turns = [...bound.model.turns];
      const turn = applyEvent(turns[bound.index], message.event, now);
      turns[bound.index] = turn;
      const requestOf =
        message.event.type === "userMessage"
          ? {
              ...bound.model.requestOf,
              [message.event.message.id]: message.requestId,
            }
          : bound.model.requestOf;
      return { ...bound.model, turns, requestOf };
    }
  }
}

/** Envois montrés avec la conversation ouverte. */
export function visibleTurns(model: PanelModel): Turn[] {
  const current = model.state.chatId;
  return model.turns.filter(
    (turn) =>
      turn.chatId === null || current === null || turn.chatId === current,
  );
}

/**
 * Messages enregistrés à montrer avant les envois en cours : ceux de la
 * conversation ouverte, sauf ceux qu'un envoi montre déjà (plus frais).
 */
export function savedMessages(model: PanelModel, turns: Turn[]): Message[] {
  const { chat, state } = model;
  if (!chat || state.chatId === null || chat.id !== state.chatId) return [];
  const live = new Set(turns.flatMap(turnIds));
  return chat.messages.filter((message) => !live.has(message.id));
}

/** Reçu d'un message de l'utilisateur envoyé depuis ce panneau. */
export function receiptOf(
  model: PanelModel,
  turn: Turn | null,
  messageId: number | null,
): Receipt | null {
  const requestId =
    turn?.requestId ??
    (messageId !== null ? model.requestOf[messageId] : undefined);
  const live =
    requestId === undefined || requestId === null
      ? null
      : (model.receipts[requestId] ?? null);
  return (
    live ??
    (messageId !== null ? (model.savedReceipts[messageId] ?? null) : null)
  );
}

/** Une réponse s'écrit (pour l'hôte, ou un envoi du panneau en attente). */
export function isBusy(model: PanelModel): boolean {
  return model.state.busy || model.turns.some((turn) => !turn.finished);
}
