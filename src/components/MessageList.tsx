/*
 * SPDX-License-Identifier: MIT
 * SPDX-FileCopyrightText: Ollama
 * SPDX-FileCopyrightText: 2026 Elwynn18 and ChatonLM contributors
 *
 * Adapted from Ollama (https://github.com/ollama/ollama),
 * files app/ui/app/src/components/MessageList.tsx and
 * app/ui/app/src/components/Message.tsx, commit 42e911bc3d05.
 * Modified for ChatonLM: tool calls and their results (ToolCalls.tsx) are
 * shown with the message of the assistant that made them, calls first, then
 * each finished result as its own block (as the official "tool" messages),
 * never with an empty text area nor a copy button; waiting dots also between
 * the last result of a turn and the next reply; actions under the error
 * (web search without an account); the message being edited
 * is passed by its id (saved messages only: not the one being sent); the
 * "edit" button also appears on keyboard focus and is disabled while faded; attachment images come from the backend (chatonlm://), the file kind
 * from the backend instead of the extension, and a message still being saved
 * shows empty squares in their place; the image row only when there are
 * images (no empty 8 px row for text files) and without justify-end (it made
 * the first images of an overflowing row unreachable); messages keyed by their id; no duplicated element id on replies;
 * user messages keep their indentation (pre-wrap); blank thinking not shown;
 * thinking without an end time shown as in progress only while the reply is
 * being written (a reply stopped while thinking is shown as finished); the
 * download block needs a total above 1 KiB, never renders a stray "0" when the
 * total is 0, and drops the unreachable branch for a missing progress; while
 * the model is being loaded into memory, the place of the waiting dots (same
 * 750 ms delay) shows "Loading model..." instead (LoadingModel.tsx), then the
 * dots again once it is loaded, inside a status region present for the whole
 * reply (announced by screen readers); texts translated into English, Spanish
 * and French (src/i18n); ChatonLM's own "Read aloud" button next to Copy, in
 * the conversations of the app only (chatId given; ReadAloudButton.tsx), also
 * shown as "Stop reading" under a reply being written or a tool turn while it
 * is read aloud, or with the error of its reading; when that button goes
 * away with the focus, the focus goes to the text of the reply; a status
 * region under each saved reply announces its reading; ChatonLM's own
 * context compaction: a separator after the last message of each compaction
 * unfolds the summary sent to the model (Compaction.tsx), and an automatic
 * compaction shows "Compacting conversation..." in place of the waiting
 * dots, like "Loading model...".
 */

import {
  memo,
  type ReactNode,
  type RefObject,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import type { AttachmentInfo } from "@/bindings/AttachmentInfo";
import type { Compaction } from "@/bindings/Compaction";
import type { Message } from "@/bindings/Message";
import {
  CompactingConversation,
  CompactionSeparator,
} from "@/components/Compaction";
import { CopyButton } from "@/components/CopyButton";
import { Downloading } from "@/components/Downloading";
import { ErrorMessage } from "@/components/ErrorMessage";
import { ImageThumbnail } from "@/components/ImageThumbnail";
import { LoadingModel } from "@/components/LoadingModel";
import { ReadAloudButton, ReadAloudStatus } from "@/components/ReadAloudButton";
import { StreamingMarkdownContent } from "@/components/StreamingMarkdownContent";
import { Thinking } from "@/components/Thinking";
import { ToolCallDisplay, ToolResult } from "@/components/ToolCalls";
import { useReadAloud } from "@/hooks/useReadAloud";
import { useT } from "@/i18n";
import { attachmentImageUrl } from "@/lib/attachments";
import type { DownloadProgress } from "@/lib/streams";
import { errorFor, phaseFor } from "@/lib/readAloud";
import { isFinished } from "@/lib/toolCalls";

const NO_COMPACTIONS: readonly Compaction[] = [];

export function MessageList({
  chatId,
  messages,
  spacerHeight,
  isStreaming,
  download = null,
  loading = false,
  compacting = false,
  compactions = NO_COMPACTIONS,
  error,
  errorActions = null,
  editingMessageId = null,
  onEditMessage,
}: {
  /**
   * Conversation affichée : ses réponses ont le bouton « Read aloud » (sans
   * elle, Copier seul).
   */
  chatId?: string;
  messages: Message[];
  spacerHeight: number;
  isStreaming: boolean;
  /** Téléchargement du modèle par la réponse en cours. */
  download?: DownloadProgress | null;
  /** Modèle en cours de chargement en mémoire par la réponse en cours. */
  loading?: boolean;
  /** Compactage automatique en cours par la réponse en cours. */
  compacting?: boolean;
  /** Compactages de la conversation, par id croissant. */
  compactions?: readonly Compaction[];
  error: string | null;
  /** Actions sous l'erreur (recherche web sans compte). */
  errorActions?: ReactNode;
  /** Message en cours de modification : lui et les suivants sont estompés. */
  editingMessageId?: number | null;
  onEditMessage?: (message: Message) => void;
}) {
  // Trois points tant que la réponse n'a pas commencé, après 750 ms, mais pas
  // pendant le téléchargement du modèle. Chaque attente est repérée par le
  // message envoyé et la phase (avant ou après un téléchargement), pour
  // repartir de zéro : les points reviennent 750 ms après la fin du
  // téléchargement, comme l'officiel. De même entre les résultats des outils
  // et la réponse suivante (l'officiel n'en montre pas). Pendant le
  // chargement du modèle, la même place (même attente, même repère) montre
  // « Loading model... » : un chargement plus court que 750 ms ne montre
  // rien, et les points le remplacent sans nouvelle attente une fois fini.
  const last = messages.at(-1);
  const downloading = download !== null && !download.done;
  const toolsDone =
    last?.role === "assistant" &&
    last.toolCalls.length > 0 &&
    last.toolCalls.every(isFinished);
  const waitingFor = !isStreaming
    ? null
    : last?.role === "user" && !downloading
      ? `${last.id}:${download === null ? "before" : "after"}`
      : toolsDone
        ? `${last.id}:tools`
        : null;
  const [dotsFor, setDotsFor] = useState<string | null>(null);

  useEffect(() => {
    if (waitingFor === null) return;
    // Wait 750ms before showing dots
    const timer = window.setTimeout(() => setDotsFor(waitingFor), 750);
    return () => window.clearTimeout(timer);
  }, [waitingFor]);
  const waiting = waitingFor !== null;
  const showDots = waiting && dotsFor === waitingFor;
  // Séparateurs, chacun après le dernier message qu'il résume.
  const compactionsAfter = useMemo(() => {
    const after = new Map<number, Compaction[]>();
    for (const compaction of compactions) {
      const list = after.get(compaction.throughMessageId) ?? [];
      list.push(compaction);
      after.set(compaction.throughMessageId, list);
    }
    return after;
  }, [compactions]);
  const editingIndex =
    editingMessageId === null
      ? -1
      : messages.findIndex((m) => m.id === editingMessageId);

  return (
    <div
      className="mx-auto flex max-w-[768px] flex-1 flex-col px-6 pb-12 select-text"
      data-role="message-list"
    >
      {messages.map((message, idx) => (
        <div key={message.id} data-message-index={idx}>
          <MessageView
            chatId={chatId}
            message={message}
            isStreaming={isStreaming && idx === messages.length - 1}
            isFaded={editingIndex !== -1 && idx >= editingIndex}
            onEditMessage={onEditMessage}
          />
          {compactionsAfter.get(message.id)?.map((compaction) => (
            <CompactionSeparator key={compaction.id} compaction={compaction} />
          ))}
        </div>
      ))}

      {/* Inline error message */}
      {error && <ErrorMessage message={error}>{errorActions}</ErrorMessage>}

      {/* Région toujours présente pendant la réponse : seul son contenu
          change, pour que l'annonce soit fiable. */}
      {isStreaming && (
        <div role="status" data-testid="loading-status">
          {waiting &&
            showDots &&
            (compacting ? (
              <CompactingConversation />
            ) : (
              loading && <LoadingModel />
            ))}
        </div>
      )}

      {waiting && showDots && !loading && !compacting && (
        <div
          data-testid="waiting"
          className="mt-2 flex min-w-0 items-center space-x-1.5 self-start rounded-full bg-neutral-100 px-4 py-3 dark:bg-neutral-800"
        >
          {[0, 0.15, 0.3].map((delay) => (
            <div
              key={delay}
              className="h-1.5 w-1.5 animate-[typing_1.4s_infinite] rounded-full bg-neutral-400 opacity-0 dark:bg-neutral-500"
              style={{ animationDelay: `${delay}s` }}
            />
          ))}
        </div>
      )}

      {/* Downloading model */}
      {/* Only show for models larger than 1KiB */}
      {download !== null && download.total > 1024 && (
        <section
          data-testid="downloading"
          className={`transition-all ease-out ${
            messages.length > 0 ? "duration-300" : "duration-0"
          } ${
            download.done
              ? "opacity-0 -translate-y-8"
              : "opacity-100 translate-y-0"
          }`}
        >
          <Downloading completed={download.completed} total={download.total} />
        </section>
      )}

      <div
        style={{ height: `${spacerHeight}px` }}
        aria-hidden="true"
        data-spacer=""
      />
    </div>
  );
}

/**
 * Un message, mémoïsé comme l'officiel : les messages relus en base gardent
 * leur identité (react-query, streams.mergeMessages), seul celui qui reçoit
 * la réponse se réaffiche à chaque mise à jour du flux.
 */
const MessageView = memo(function MessageView({
  chatId,
  message,
  isStreaming,
  isFaded,
  onEditMessage,
}: {
  chatId?: string;
  message: Message;
  isStreaming: boolean;
  isFaded: boolean;
  onEditMessage?: (message: Message) => void;
}) {
  return message.role === "user" ? (
    <UserMessage
      message={message}
      isFaded={isFaded}
      onEditMessage={onEditMessage}
    />
  ) : (
    <AssistantMessage
      chatId={chatId}
      message={message}
      isStreaming={isStreaming}
      isFaded={isFaded}
    />
  );
});

/**
 * Actions d'une réponse d'une conversation : Copier et « Read aloud » une
 * fois la réponse finie ; avant (réponse en cours d'écriture, lue par la
 * lecture automatique) ou pour un tour d'outils, seulement « Stop reading »
 * tant qu'elle est lue, ou le bouton et l'erreur de sa lecture. Même place
 * dans l'arbre d'un cas à l'autre : le bouton garde le focus quand la
 * réponse finit pendant sa lecture ; la barre retirée avec le focus
 * (Arrêter sous une réponse en cours), il va au texte de la réponse.
 */
function MessageActions({
  chatId,
  messageId,
  finished,
  copy,
  content,
}: {
  chatId: string;
  messageId: number;
  finished: boolean;
  copy: ReactNode;
  /** Texte de la réponse : reçoit le focus de la barre retirée. */
  content: RefObject<HTMLElement | null>;
}) {
  const state = useReadAloud();
  const source = { kind: "message", messageId } as const;
  const visible =
    finished ||
    phaseFor(state, source) !== "idle" ||
    errorFor(state, source) !== null;
  // Le focus est dans la barre (pas parti ailleurs dans la page).
  const focused = useRef(false);

  useEffect(() => {
    if (visible || !focused.current) return;
    focused.current = false;
    const active = document.activeElement;
    const target = content.current;
    if ((active !== null && active !== document.body) || !target) return;
    // Focalisable le temps d'y être (pas de place dans l'ordre de Tab).
    if (!target.hasAttribute("tabindex")) {
      target.setAttribute("tabindex", "-1");
      target.addEventListener(
        "blur",
        () => target.removeAttribute("tabindex"),
        { once: true },
      );
    }
    target.focus();
  }, [visible, content]);

  if (!visible) return null;
  return (
    <div
      className="-ml-1 flex flex-wrap items-center"
      data-role="actions"
      onFocus={() => {
        focused.current = true;
      }}
      onBlur={(event) => {
        // Vers un autre élément de la page : plus rien à rendre. Sans
        // destination (élément retiré), on garde.
        const next = event.relatedTarget;
        if (next instanceof Node && !event.currentTarget.contains(next)) {
          focused.current = false;
        }
      }}
    >
      {finished && copy}
      <ReadAloudButton
        target={{ kind: "message", chatId, messageId }}
        className="z-10 text-neutral-500 dark:text-neutral-400"
      />
    </div>
  );
}

/** Image d'une pièce jointe ; pas encore enregistrée (id négatif) : aucune. */
function imageUrl(attachment: AttachmentInfo): string | null {
  return attachment.id > 0 ? attachmentImageUrl(attachment.id) : null;
}

function UserMessage({
  message,
  isFaded,
  onEditMessage,
}: {
  message: Message;
  isFaded: boolean;
  onEditMessage?: (message: Message) => void;
}) {
  const t = useT();
  const images = message.attachments.filter((a) => a.kind === "image");
  const files = message.attachments.filter((a) => a.kind !== "image");
  // Seulement un message enregistré (pas celui en cours d'envoi).
  const editable = onEditMessage !== undefined && message.id > 0;
  return (
    <div
      className={`flex flex-col transition-opacity duration-300 ${isFaded ? "opacity-50" : "opacity-100"}`}
    >
      {/* Show image attachments above the message background */}
      {images.length > 0 && (
        <div className="mb-2 flex max-w-md gap-2 self-end overflow-x-auto">
          {images.map((attachment, index) => (
            <div key={`image-attachment-${index}`} className="flex-shrink-0">
              <ImageThumbnail
                src={imageUrl(attachment)}
                alt={attachment.filename}
                className="h-16 w-16 rounded-md object-cover"
              />
            </div>
          ))}
        </div>
      )}

      <div className="mb-8 max-w-md self-end">
        <div
          className="group/message relative rounded-3xl bg-neutral-200 px-4 py-2 leading-normal dark:bg-neutral-700 dark:text-white"
          data-role="user"
        >
          {/* Show non-image attachments inside the message */}
          {files.length > 0 && (
            <div className="mb-2 flex gap-2 overflow-x-auto">
              {files.map((attachment, index) => (
                <div
                  key={`file-attachment-${index}`}
                  className="flex flex-shrink-0 items-center gap-2 rounded-lg bg-neutral-50 px-2 py-1 transition-colors dark:bg-neutral-600/50"
                >
                  <svg
                    className="h-3 w-3 flex-shrink-0 text-neutral-500"
                    fill="none"
                    viewBox="0 0 24 24"
                    stroke="currentColor"
                    aria-hidden="true"
                  >
                    <path
                      strokeLinecap="round"
                      strokeLinejoin="round"
                      strokeWidth={1.5}
                      d="M9 12h6m-6 4h6m2 5H7a2 2 0 01-2-2V5a2 2 0 012-2h5.586a1 1 0 01.707.293l5.414 5.414a1 1 0 01.293.707V19a2 2 0 01-2 2z"
                    />
                  </svg>
                  <span className="max-w-[120px] truncate text-xs text-neutral-600 dark:text-neutral-300">
                    {attachment.filename}
                  </span>
                </div>
              ))}
            </div>
          )}

          <div className="break-words whitespace-pre-wrap">
            {message.content}
          </div>

          {/* Edit button */}
          {editable && (
            <button
              type="button"
              className={`edit-button absolute right-1 -bottom-5 text-xs ${
                isFaded
                  ? "opacity-30"
                  : "cursor-pointer text-neutral-500 opacity-0 group-hover/message:opacity-100 hover:text-neutral-700 focus-visible:opacity-100 dark:text-neutral-400 dark:hover:text-neutral-200"
              }`}
              onClick={isFaded ? undefined : () => onEditMessage(message)}
              // Estompé : ni focalisable, ni annoncé comme actif.
              disabled={isFaded}
            >
              {t("chat.messages.edit")}
            </button>
          )}
        </div>
      </div>
    </div>
  );
}

/**
 * Horodatages du raisonnement (ms → Date). Sans fin, il n'est en cours que si
 * la réponse s'écrit encore : une réponse finie (arrêtée pendant le
 * raisonnement, pas encore relue en base) est montrée finie, sans durée.
 */
function thinkingTimes(
  message: Message,
  isStreaming: boolean,
): { startTime?: Date; endTime?: Date } {
  const { thinkingStartedAt: start, thinkingEndedAt: end } = message;
  if (start === null) return {};
  const startTime = new Date(start);
  if (end !== null) return { startTime, endTime: new Date(end) };
  return isStreaming ? { startTime } : {};
}

function AssistantMessage({
  chatId,
  message,
  isStreaming,
  isFaded,
}: {
  chatId?: string;
  message: Message;
  isStreaming: boolean;
  isFaded: boolean;
}) {
  const t = useT();
  const messageRef = useRef<HTMLDivElement>(null);
  const hasContent = message.content.trim() !== "";
  // Un raisonnement vide (blancs seuls) n'a rien à montrer.
  const hasThinking = message.thinking.trim() !== "";
  const { toolCalls } = message;
  const fade = isFaded ? "opacity-50" : "opacity-100";
  // Réponse finie, avec un texte, pas un tour d'outils : ses actions.
  const finished = !isStreaming && hasContent && toolCalls.length === 0;
  const copy = (
    <CopyButton
      content={message.content}
      copyRef={messageRef}
      size="md"
      showLabels={false}
      className="copy-button z-10 text-neutral-500 dark:text-neutral-400"
      title={t("common.actions.copy")}
    />
  );

  return (
    <>
      <div
        className={`mb-8 flex flex-col space-y-4 transition-opacity duration-300 ${fade}`}
      >
        <div className="group relative flex max-w-none flex-1 flex-col justify-start text-wrap break-words">
          {/* Thinking area */}
          {hasThinking && (
            <Thinking
              thinking={message.thinking}
              {...thinkingTimes(message, isStreaming)}
            />
          )}

          {/* Only render content div if there's actual content to show */}
          {hasContent && (
            <div
              className="assistant-message-content prose dark:prose-invert max-w-full break-words"
              data-role="assistant"
              ref={messageRef}
            >
              <StreamingMarkdownContent
                content={message.content}
                isStreaming={isStreaming}
              />
            </div>
          )}

          {/* Annonces de la lecture à voix haute : région là avant elle
              (hors du texte copié). */}
          {chatId !== undefined && message.id > 0 && (
            <ReadAloudStatus
              source={{ kind: "message", messageId: message.id }}
            />
          )}
        </div>

        {toolCalls.length > 0 && (
          <div data-role="tool-calls">
            {toolCalls.map((call, index) => (
              <ToolCallDisplay key={index} call={call} />
            ))}
          </div>
        )}

        {/* Comme l'officiel : pas de bouton copier pour un tour d'outils. */}
        {chatId !== undefined && message.id > 0 && hasContent ? (
          <MessageActions
            chatId={chatId}
            messageId={message.id}
            finished={finished}
            copy={copy}
            content={messageRef}
          />
        ) : (
          finished && <div className="-ml-1">{copy}</div>
        )}
      </div>

      {/* Résultats terminés, chacun à la place d'un message « tool » de
          l'officiel. */}
      {toolCalls.map(
        (call, index) =>
          isFinished(call) && (
            <div
              key={index}
              className={`mb-8 flex flex-col space-y-4 transition-opacity duration-300 ${fade}`}
            >
              <div className="group relative flex max-w-none flex-1 flex-col justify-start text-wrap break-words">
                <div
                  className="prose dark:prose-invert max-w-full break-words"
                  data-role="tool"
                >
                  <ToolResult call={call} />
                </div>
              </div>
            </div>
          ),
      )}
    </>
  );
}
