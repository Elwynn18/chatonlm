/*
 * SPDX-License-Identifier: MIT
 * SPDX-FileCopyrightText: Ollama
 * SPDX-FileCopyrightText: 2026 Elwynn18 and ChatonLM contributors
 *
 * Adapted from Ollama (https://github.com/ollama/ollama),
 * file app/ui/app/src/components/Chat.tsx, commit 42e911bc3d05.
 * Modified for ChatonLM: ChatonLM logo; replies stream from the Rust backend
 * (src/lib/streams.ts); "Ollama isn't responding" banner with Retry above the
 * disabled composer, also on the new chat page ("Starting Ollama..." while
 * the built-in Ollama starts; once it has failed: its log, and Retry
 * restarts it); a new chat refused before its
 * creation shows the error above the composer; one view per chat (key), so
 * that switching chats starts from a clean state (the edit mode included);
 * editing a message keeps its attachments by id; a web search without an
 * Ollama account ends with an error offering Sign In (the account card of
 * the settings) and Generate without web search (the last message sent
 * again, as an edit, without web search); once the account is signed in
 * (read while that error is shown), Retry replaces Sign In and sends the
 * message again with web search; an error due to Ollama's own folder hidden
 * from its systemd service (a model download or the account refused: key
 * id_ed25519 unreadable) links to the fix in Settings > General; the reply
 * being written says when its model is loaded into memory ("Loading
 * model...", see MessageList.tsx); the replies of a conversation have
 * ChatonLM's own "Read aloud" button (the list gets the conversation id);
 * the sentences of ChatonLM's own voice mode are sent like messages, with
 * `voice` (a reply still being written is stopped first, then the sentence
 * is sent only if the voice mode still waits for it; a send refused at once
 * is reported to the voice mode, which listens again; the chat created by
 * the first sentence of a new chat keeps the voice mode); a reply stopped
 * because the model repeated itself word for word offers Retry (the last
 * message sent again, as an edit), and a thinking that seems to go in
 * circles shows a banner with Stop above the composer (both ChatonLM's own).
 */

import { useNavigate } from "@tanstack/react-router";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { Message } from "@/bindings/Message";
import type { Think } from "@/bindings/Think";
import {
  ChatForm,
  type ChatFormHandle,
  type VoiceSubmit,
} from "@/components/ChatForm";
import { ErrorMessage } from "@/components/ErrorMessage";
import { LoopErrorActions } from "@/components/LoopErrorActions";
import { MessageList } from "@/components/MessageList";
import { OllamaHomeLink } from "@/components/settings/OllamaHome";
import { RuminationBanner } from "@/components/RuminationBanner";
import { ServerLostBanner } from "@/components/ServerLostBanner";
import { useAccount } from "@/hooks/useAccount";
import { WebSearchErrorActions } from "@/components/WebSearchErrorActions";
import { useChat } from "@/hooks/useChats";
import { isHomeError } from "@/hooks/useOllamaHome";
import { useMessageAutoscroll } from "@/hooks/useMessageAutoscroll";
import {
  useChatError,
  useChatStreams,
  useNewChatStream,
  useSendMessage,
} from "@/hooks/useSendMessage";
import { useServerHealth } from "@/hooks/useServerHealth";
import { adoptVoiceModeChat, voiceModeSnapshot } from "@/hooks/useVoiceMode";
import { voiceModeReplyFailed } from "@/lib/ipc";
import { isRuminating } from "@/lib/rumination";
import {
  activeStream,
  clearError,
  mergeMessages,
  NEW_CHAT,
  pendingNewStream,
  type StreamError,
  streamsFor,
  whenIdle,
} from "@/lib/streams";

export function Chat({ chatId }: { chatId: string }) {
  if (chatId === "new") return <NewChat />;
  return <ChatView key={chatId} chatId={chatId} />;
}

/** Ouvre la carte du compte dans les réglages (rien ne s'ouvre ailleurs). */
function useOpenAccountSettings() {
  const navigate = useNavigate();
  return useCallback(() => {
    void navigate({ to: "/settings", search: { section: "account" } });
  }, [navigate]);
}

/** Envoi d'une phrase du mode vocal (contenu, modèle et réglages). */
interface VoiceSendRequest {
  content: string;
  model: string;
  think: Think | undefined;
  webSearch: boolean;
}

/**
 * Envoi d'une phrase du mode vocal : la réponse en cours de la conversation
 * (envoi au clavier) est d'abord arrêtée ; après cette attente, la phrase
 * n'est envoyée que si la séance est toujours celle de la conversation et
 * attend toujours ce tour (ni fin, ni interruption, ni phrase plus récente,
 * qui part seule) ; un envoi refusé d'emblée est signalé au moteur (avis,
 * nouvelle écoute) ; la conversation créée par la première phrase d'une
 * nouvelle conversation garde la séance.
 */
function useSendVoice() {
  const { send, cancel } = useSendMessage();
  return useCallback(
    async (chatId: string, request: VoiceSendRequest, voice: VoiceSubmit) => {
      const current =
        chatId === NEW_CHAT
          ? pendingNewStream()
          : activeStream(streamsFor(chatId));
      if (current) {
        cancel(current.streamId);
        await whenIdle(chatId);
        const { owner, state } = voiceModeSnapshot();
        if (
          owner !== chatId ||
          state.phase !== "responding" ||
          state.turn !== voice.turn
        ) {
          return;
        }
      }
      await send({
        chatId,
        ...request,
        voice: voice.language ? { language: voice.language } : {},
        onRefused: (message) => {
          voiceModeReplyFailed(voice.turn, message).catch((error: unknown) =>
            console.error("Failed to report the refused voice send:", error),
          );
        },
        ...(chatId === NEW_CHAT && {
          onChatCreated: (id: string) => adoptVoiceModeChat(NEW_CHAT, id),
        }),
      });
    },
    [send, cancel],
  );
}

/** Recherche web refusée faute de compte relié au serveur. */
function isWebSearchSignedOut(error: StreamError | null) {
  return error?.kind === "webSearchSignedOut";
}

/** Nouvelle conversation : logo et zone de saisie, centrés verticalement. */
function NewChat() {
  const stream = useNewChatStream();
  // Envoi refusé avant la création de la conversation (le texte a quitté la
  // zone de saisie) : erreur gardée jusqu'au prochain envoi ou jusqu'à ce que
  // l'on quitte la page.
  const error = useChatError(NEW_CHAT);
  useEffect(() => () => clearError(NEW_CHAT), []);
  const { send, cancel } = useSendMessage();
  const sendVoice = useSendVoice();
  const health = useServerHealth();
  const openAccountSettings = useOpenAccountSettings();
  // Le moteur crée la conversation avant cette erreur : elle n'arrive ici
  // que par accident, sans message à relancer (Sign In seul).
  const banner = (error || !health.healthy) && (
    <>
      {error && (
        <div className="mx-auto w-full max-w-[768px] px-5">
          <ErrorMessage message={error.message}>
            {isWebSearchSignedOut(error) && (
              <WebSearchErrorActions onSignIn={openAccountSettings} />
            )}
          </ErrorMessage>
        </div>
      )}
      {!health.healthy && (
        <ServerLostBanner
          onRetry={health.retry}
          starting={health.starting}
          failed={health.failed}
        />
      )}
    </>
  );

  return (
    <div
      data-testid="new-chat"
      className="relative flex h-screen flex-col justify-center"
    >
      <div className="px-6">
        <ChatForm
          chatId="new"
          hasMessages={false}
          isStreaming={stream !== null}
          isDisabled={!health.healthy}
          banner={banner}
          onSubmit={(
            content,
            model,
            think,
            attachments,
            _kept,
            webSearch,
            _editOf,
            voice,
          ) =>
            void (voice
              ? sendVoice(NEW_CHAT, { content, model, think, webSearch }, voice)
              : send({
                  chatId: "new",
                  content,
                  model,
                  think,
                  attachments,
                  webSearch,
                }))
          }
          onCancel={() => {
            if (stream) cancel(stream.streamId);
          }}
        />
      </div>
    </div>
  );
}

/** Conversation ouverte : messages défilants, zone de saisie en bas. */
function ChatView({ chatId }: { chatId: string }) {
  const { data: chat } = useChat(chatId);
  // Tous les flux de la conversation : un envoi suivant ne masque pas la
  // réponse d'un flux terminé pas encore retiré, et inversement.
  const live = useChatStreams(chatId);
  const active = activeStream(live);
  const error = useChatError(chatId);
  const { send, cancel } = useSendMessage();
  const sendVoice = useSendVoice();
  const health = useServerHealth();
  const saved = chat?.messages;
  // Même tableau tant que rien ne change (bandeau, espace réservé...).
  const messages = useMemo(
    () => mergeMessages(saved ?? [], live),
    [saved, live],
  );
  const isStreaming = active !== null;
  // Raisonnement en cours qui semble tourner en rond : bandeau avec Stop.
  const lastReply = active?.replies.at(-1);
  const ruminating = useMemo(
    () =>
      lastReply !== undefined &&
      lastReply.thinkingEndedAt === null &&
      isRuminating(lastReply.thinking),
    [lastReply],
  );
  const formRef = useRef<ChatFormHandle>(null);
  // Message en cours de modification : lui et les suivants sont estompés.
  const [editing, setEditing] = useState<Message | null>(null);
  const startEdit = useCallback((message: Message) => {
    setEditing(message);
    formRef.current?.startEdit(message);
  }, []);
  // Comme handleCancelEdit de l'officiel : l'erreur affichée disparaît.
  const cancelEdit = useCallback(() => {
    setEditing(null);
    clearError(chatId);
  }, [chatId]);
  const { containerRef, spacerHeight, onMessageSent } = useMessageAutoscroll({
    messages,
    isStreaming,
  });
  const openAccountSettings = useOpenAccountSettings();
  const signedOutError = isWebSearchSignedOut(error);
  // Compte lu tant que l'erreur est là : relié entre-temps (réglages),
  // Retry relance avec la recherche web.
  const { isAuthenticated } = useAccount({ enabled: signedOutError });
  // Dernier message enregistré de l'utilisateur : celui qu'on relance.
  const lastSent = messages.findLast((m) => m.role === "user" && m.id > 0);
  const errorActions = signedOutError ? (
    <WebSearchErrorActions
      onSignIn={openAccountSettings}
      onRetry={
        lastSent
          ? (webSearch) => formRef.current?.retry(lastSent, { webSearch })
          : undefined
      }
      signedIn={isAuthenticated}
      retryDisabled={isStreaming || !health.healthy}
    />
  ) : error?.kind === "loop" ? (
    <LoopErrorActions
      onRetry={lastSent ? () => formRef.current?.retry(lastSent) : undefined}
      retryDisabled={isStreaming || !health.healthy}
    />
  ) : (
    // Clé d'Ollama cachée à son service (téléchargement, compte) : la
    // correction.
    isHomeError(error?.message) && <OllamaHomeLink error={error?.message} />
  );

  // <div> plutôt que le <main> officiel : la mise en page en fournit déjà un.
  return (
    <div
      data-testid="chat"
      className="allow-context-menu relative flex h-screen w-full flex-col select-none"
    >
      <section
        ref={containerRef}
        className="relative min-h-0 flex-1 overflow-y-auto overscroll-contain select-none xl:pt-4"
      >
        <MessageList
          chatId={chatId}
          messages={messages}
          spacerHeight={spacerHeight}
          isStreaming={isStreaming}
          download={active?.download ?? null}
          loading={active?.loading ?? false}
          error={error?.message ?? null}
          errorActions={errorActions}
          editingMessageId={editing?.id ?? null}
          onEditMessage={startEdit}
        />
      </section>

      <div className="sticky bottom-0 z-20 flex-shrink-0">
        <ChatForm
          chatId={chatId}
          hasMessages={messages.length > 0}
          isStreaming={isStreaming}
          isDisabled={!health.healthy}
          banner={
            !health.healthy ? (
              <ServerLostBanner
                onRetry={health.retry}
                starting={health.starting}
                failed={health.failed}
              />
            ) : (
              ruminating &&
              active && (
                <RuminationBanner onStop={() => cancel(active.streamId)} />
              )
            )
          }
          onSubmit={(
            content,
            model,
            think,
            attachments,
            kept,
            webSearch,
            editOf,
            voice,
          ) => {
            onMessageSent();
            // Phrase du mode vocal : un nouveau message, jamais une
            // modification.
            if (voice) {
              void sendVoice(
                chatId,
                { content, model, think, webSearch },
                voice,
              );
              return;
            }
            // Relance sans recherche web : son propre message, la
            // modification en cours (s'il y en a une) continue.
            const messageId = editOf ?? editing?.id;
            void send({
              chatId,
              content,
              model,
              think,
              attachments,
              ...(messageId !== undefined && { edit: { messageId, kept } }),
              webSearch,
            });
            if (editOf === undefined) setEditing(null);
          }}
          onCancel={() => {
            if (active) cancel(active.streamId);
          }}
          ref={formRef}
          editingMessage={editing}
          onCancelEdit={cancelEdit}
        />
      </div>
    </div>
  );
}
