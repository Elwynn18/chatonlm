import { useQueryClient } from "@tanstack/react-query";
import { useNavigate } from "@tanstack/react-router";
import { Channel } from "@tauri-apps/api/core";
import { useCallback, useSyncExternalStore } from "react";
import type { AttachmentInfo } from "@/bindings/AttachmentInfo";
import type { Chat } from "@/bindings/Chat";
import type { ChatEvent } from "@/bindings/ChatEvent";
import type { StagedFile } from "@/bindings/StagedFile";
import type { Think } from "@/bindings/Think";
import type { VoiceSend } from "@/bindings/VoiceSend";
import { accountKey } from "@/hooks/useAccount";
import { chatKey, chatsKey } from "@/hooks/useChats";
import { release } from "@/hooks/useComposerFiles";
import { modelsKey } from "@/hooks/useModels";
import { healthKey } from "@/hooks/useServerHealth";
import { modelDetailsKey } from "@/hooks/useThink";
import { cancelMessage, errorMessage, sendMessage } from "@/lib/ipc";
import * as streams from "@/lib/streams";

/** Délai de grâce si la commande finit avant l'arrivée de l'événement final. */
const FINAL_EVENT_GRACE_MS = 2000;

/**
 * Flux d'une conversation, dans l'ordre d'envoi : ceux qui sont terminés
 * fournissent encore leur réponse tant qu'ils ne sont pas retirés, celui qui
 * répond (streams.activeStream) décide de l'arrêt.
 */
export function useChatStreams(chatId: string): readonly streams.LiveStream[] {
  return useSyncExternalStore(streams.subscribe, () =>
    streams.streamsFor(chatId),
  );
}

/** Flux d'une nouvelle conversation pas encore créée. */
export function useNewChatStream(): streams.LiveStream | null {
  return useSyncExternalStore(streams.subscribe, streams.pendingNewStream);
}

/** Dernière erreur d'envoi d'une conversation (« new » : nouvelle conversation). */
export function useChatError(chatId: string): streams.StreamError | null {
  return useSyncExternalStore(streams.subscribe, () =>
    streams.errorFor(chatId),
  );
}

/**
 * Envoi d'un message : le flux vit dans `streams` pendant la réponse ; à la
 * fin, la conversation et l'historique sont relus en base, puis le flux est
 * retiré. Une nouvelle conversation ouvre sa page dès sa création. `think`
 * (bouton Think) n'est transmis que s'il est donné : sinon, défaut d'Ollama.
 * Un modèle absent est d'abord téléchargé par le moteur (événements
 * download) ; à la fin, la liste des modèles et ses détails sont relus. Les
 * fichiers préparés partent par leurs identifiants ; un envoi refusé
 * d'emblée les libère (le composer, déjà vidé, ne les renverra pas). `edit` :
 * message modifié (lui et les suivants remplacés), avec les pièces jointes
 * gardées. `webSearch` : outils de recherche web proposés au modèle (absent
 * de la requête s'il est faux). `voice` : phrase du mode vocal (propre à
 * ChatonLM) ; `onRefused` reçoit le message d'un envoi refusé d'emblée,
 * `onChatCreated` l'identifiant de la nouvelle conversation, avant d'y aller.
 */
export function useSendMessage() {
  const queryClient = useQueryClient();
  const navigate = useNavigate();

  const send = useCallback(
    async ({
      chatId,
      content,
      model,
      think,
      attachments = [],
      edit,
      webSearch = false,
      voice,
      onRefused,
      onChatCreated,
    }: {
      chatId: string;
      content: string;
      model: string;
      think?: Think;
      attachments?: StagedFile[];
      edit?: { messageId: number; kept: AttachmentInfo[] };
      webSearch?: boolean;
      voice?: VoiceSend;
      onRefused?: (message: string) => void;
      onChatCreated?: (chatId: string) => void;
    }) => {
      const streamId = crypto.randomUUID();
      const existing = chatId === "new" ? null : chatId;
      const attachmentIds = attachments.map((file) => file.id);
      const keptAttachmentIds = edit?.kept.map((a) => a.id) ?? [];
      streams.start({
        streamId,
        chatId: existing,
        content,
        model,
        attachments,
        kept: edit?.kept,
        editOf: edit?.messageId,
      });

      // Raisonnement reçu malgré `think: false` : le moteur a retenu que le
      // modèle raisonne toujours ; ses détails sont relus à la fin (le bouton
      // Think disparaît).
      let thoughtDespiteOff = false;
      let finalEvent: () => void = () => {};
      const ended = new Promise<void>((resolve) => (finalEvent = resolve));
      const channel = new Channel<ChatEvent>();
      channel.onmessage = (event) => {
        streams.apply(streamId, event);
        switch (event.type) {
          case "chatCreated": {
            // Page remplie d'avance, comme l'officiel : pas d'écran de
            // chargement le temps de get_chat ; le flux y superpose le
            // message envoyé, puis la réponse.
            const now = Date.now();
            queryClient.setQueryData<Chat>(chatKey(event.chatId), {
              id: event.chatId,
              title: "",
              createdAt: now,
              updatedAt: now,
              messages: [],
              context: null,
              compactions: [],
            });
            void queryClient.invalidateQueries({ queryKey: chatsKey });
            onChatCreated?.(event.chatId);
            void navigate({
              to: "/c/$chatId",
              params: { chatId: event.chatId },
            });
            break;
          }
          case "userMessage":
            void queryClient.invalidateQueries({ queryKey: chatsKey });
            break;
          case "thinking":
            if (think === false) thoughtDespiteOff = true;
            break;
          case "download":
            // Modèle téléchargé, comme l'officiel (useChats.ts) : il rejoint
            // la liste des modèles, ses détails (bouton Think) sont relus.
            if (event.done) {
              void queryClient.invalidateQueries({ queryKey: modelsKey });
              void queryClient.invalidateQueries({
                queryKey: modelDetailsKey(model),
              });
            }
            break;
          case "error":
            if (event.kind === "unreachable" || event.kind === "connectionLost")
              void queryClient.invalidateQueries({ queryKey: healthKey });
            // Le serveur n'a pas (ou plus) de compte : la carte des réglages,
            // où mène l'erreur, ne doit pas montrer un compte périmé.
            if (event.kind === "webSearchSignedOut")
              void queryClient.invalidateQueries({ queryKey: accountKey });
            finalEvent();
            break;
          case "done":
          case "cancelled":
            finalEvent();
            break;
        }
      };

      try {
        await sendMessage(
          {
            chatId: existing ?? undefined,
            streamId,
            content,
            model,
            ...(think !== undefined && { think }),
            ...(attachmentIds.length > 0 && { attachmentIds }),
            ...(edit && { editMessageId: edit.messageId }),
            ...(keptAttachmentIds.length > 0 && { keptAttachmentIds }),
            ...(webSearch && { webSearch }),
            ...(voice && { voice }),
          },
          channel,
        );
        await Promise.race([
          ended,
          new Promise((resolve) => setTimeout(resolve, FINAL_EVENT_GRACE_MS)),
        ]);
      } catch (error) {
        // Refusé d'emblée : aucun événement final ne viendra. Avant la
        // création de la conversation, l'erreur va à la page de nouvelle
        // conversation.
        release(attachmentIds);
        const message = errorMessage(error);
        streams.fail(streamId, { kind: "internal", message });
        onRefused?.(message);
      }

      if (thoughtDespiteOff) {
        void queryClient.invalidateQueries({
          queryKey: modelDetailsKey(model),
        });
      }
      const id = streams.getStream(streamId)?.chatId;
      if (id) await queryClient.invalidateQueries({ queryKey: chatKey(id) });
      await queryClient.invalidateQueries({ queryKey: chatsKey });
      streams.remove(streamId);
    },
    [queryClient, navigate],
  );

  const cancel = useCallback((streamId: string) => {
    void cancelMessage(streamId);
  }, []);

  return { send, cancel };
}
