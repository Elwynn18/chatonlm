// Messages de la conversation ouverte : ceux que l'app a enregistrés, puis
// les envois de cette session (message envoyé, réponse en flux, raisonnement
// repliable, erreur), avec le reçu de ce qui est parti sous chaque message
// envoyé d'ici. Rendu des réponses : celui de l'app (StreamingMarkdownContent,
// Thinking), liens et copies par l'hôte du panneau (src/lib/host.ts).
import { memo, useLayoutEffect, useRef } from "react";
import { CopyButton } from "@/components/CopyButton";
import { ErrorMessage } from "@/components/ErrorMessage";
import { StreamingMarkdownContent } from "@/components/StreamingMarkdownContent";
import { Thinking } from "@/components/Thinking";
import type { Message } from "@/bindings/Message";
import { useT } from "@/i18n";
import type { Receipt } from "../shared/protocol";
import {
  type PanelModel,
  receiptOf,
  savedMessages,
  type Turn,
  type TurnError,
} from "./model";
import { ReceiptView } from "./ReceiptView";

/** Horodatages du raisonnement (ms → Date), comme MessageList. */
function thinkingTimes(
  message: Message,
  streaming: boolean,
): { startTime?: Date; endTime?: Date } {
  const { thinkingStartedAt: start, thinkingEndedAt: end } = message;
  if (start === null) return {};
  const startTime = new Date(start);
  if (end !== null) return { startTime, endTime: new Date(end) };
  return streaming ? { startTime } : {};
}

const UserMessage = memo(function UserMessage({
  content,
  receipt,
}: {
  content: string;
  receipt: Receipt | null;
}) {
  return (
    <div className="mb-6 flex max-w-[90%] flex-col self-end" data-role="user">
      <div className="max-h-60 overflow-y-auto rounded-3xl bg-neutral-200 px-4 py-2 leading-normal break-words whitespace-pre-wrap text-neutral-900 dark:bg-neutral-700 dark:text-white">
        {content}
      </div>
      {receipt && <ReceiptView receipt={receipt} />}
    </div>
  );
});

const AssistantMessage = memo(function AssistantMessage({
  message,
  streaming,
}: {
  message: Message;
  streaming: boolean;
}) {
  const t = useT();
  const ref = useRef<HTMLDivElement>(null);
  const hasContent = message.content.trim() !== "";
  const hasThinking = message.thinking.trim() !== "";
  if (!hasContent && !hasThinking) return null;
  return (
    <div className="mb-6 flex flex-col" data-role="assistant">
      {hasThinking && (
        <Thinking
          thinking={message.thinking}
          {...thinkingTimes(message, streaming)}
        />
      )}
      {hasContent && (
        <div
          ref={ref}
          className="prose dark:prose-invert max-w-full text-sm break-words"
        >
          <StreamingMarkdownContent
            content={message.content}
            isStreaming={streaming}
          />
        </div>
      )}
      {hasContent && !streaming && (
        <div className="-ml-1">
          <CopyButton
            content={message.content}
            copyRef={ref}
            size="md"
            className="text-neutral-500 dark:text-neutral-400"
            title={t("common.actions.copy")}
          />
        </div>
      )}
    </div>
  );
});

/** Ligne d'attente avant le premier fragment d'une réponse. */
function Waiting({ turn }: { turn: Turn }) {
  const t = useT();
  const text =
    turn.download !== null
      ? t("vscode.reply.download", { percent: turn.download })
      : turn.loading
        ? t("vscode.reply.loading")
        : turn.compacting
          ? t("vscode.reply.compacting")
          : null;
  if (text) {
    return (
      <p
        className="mb-6 text-sm text-neutral-500 dark:text-neutral-400"
        role="status"
      >
        {text}
      </p>
    );
  }
  return (
    <div
      className="mb-6 flex items-center space-x-1.5 self-start rounded-full bg-neutral-100 px-4 py-3 dark:bg-neutral-800"
      role="status"
      aria-label={t("vscode.reply.writing")}
    >
      {[0, 0.2, 0.4].map((delay) => (
        <span
          key={delay}
          className="h-1.5 w-1.5 rounded-full bg-neutral-400 opacity-0 motion-safe:animate-[typing_1.4s_infinite] motion-reduce:opacity-100 dark:bg-neutral-500"
          style={{ animationDelay: `${delay}s` }}
        />
      ))}
    </div>
  );
}

function TurnView({ model, turn }: { model: PanelModel; turn: Turn }) {
  const content = turn.user?.content ?? turn.text;
  const last = turn.replies.at(-1);
  const started =
    last !== undefined &&
    (last.content.trim() !== "" || last.thinking.trim() !== "");
  return (
    <>
      {content !== null && (
        <UserMessage
          content={content}
          receipt={receiptOf(model, turn, turn.user?.id ?? null)}
        />
      )}
      {turn.replies.map((reply) => (
        <AssistantMessage
          key={reply.id}
          message={reply}
          streaming={!turn.finished && reply === last}
        />
      ))}
      {!turn.finished && !started && <Waiting turn={turn} />}
      {turn.error && <ErrorMessage message={turn.error.message} />}
    </>
  );
}

export function Conversation({
  model,
  turns,
  lastError,
}: {
  model: PanelModel;
  turns: Turn[];
  lastError: TurnError | null;
}) {
  const saved = savedMessages(model, turns);
  const scroller = useRef<HTMLDivElement>(null);
  const atBottom = useRef(true);

  // Reste en bas pendant le flux, sauf si l'utilisateur est remonté lire.
  useLayoutEffect(() => {
    const node = scroller.current;
    if (node && atBottom.current) node.scrollTop = node.scrollHeight;
  });

  return (
    <div
      ref={scroller}
      className="min-h-0 flex-1 overflow-y-auto px-3 pt-4 select-text"
      onScroll={(event) => {
        const node = event.currentTarget;
        atBottom.current =
          node.scrollHeight - node.scrollTop - node.clientHeight < 24;
      }}
    >
      <div className="flex flex-col" data-testid="messages">
        {saved.map((message) =>
          message.role === "user" ? (
            <UserMessage
              key={message.id}
              content={message.content}
              receipt={receiptOf(model, null, message.id)}
            />
          ) : (
            <AssistantMessage
              key={message.id}
              message={message}
              streaming={false}
            />
          ),
        )}
        {turns.map((turn) => (
          <TurnView key={turn.key} model={model} turn={turn} />
        ))}
        {lastError && <ErrorMessage message={lastError.message} />}
      </div>
    </div>
  );
}
