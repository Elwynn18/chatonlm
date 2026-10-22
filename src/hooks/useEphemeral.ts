// Conversations éphémères (propres à ChatonLM) : effacement en ouvrant une
// autre conversation, « Keep ». État partagé : src/lib/ephemeral.ts.

import {
  type QueryClient,
  useMutation,
  useQueryClient,
} from "@tanstack/react-query";
import { useEffect } from "react";
import type { Chat } from "@/bindings/Chat";
import { chatKey, chatsKey } from "@/hooks/useChats";
import { forgetChat, keepChat } from "@/lib/ipc";
import { markEphemeral, openedChat, unmarkEphemeral } from "@/lib/ephemeral";

/**
 * Efface la conversation éphémère quittée (`left`, null : aucune) et oublie
 * ses données en cache. Un échec est seulement journalisé : la fin de l'app
 * et le démarrage suivant l'effacent de toute façon.
 */
export function forgetLeft(queryClient: QueryClient, left: string | null) {
  if (left === null) return;
  queryClient.removeQueries({ queryKey: chatKey(left) });
  forgetChat(left).then(
    () => queryClient.removeQueries({ queryKey: chatKey(left) }),
    (error: unknown) =>
      console.error("Failed to erase the ephemeral chat:", left, error),
  );
}

/**
 * Page d'une conversation (`chatId`, ou "new") : la conversation éphémère
 * ouverte avant, si c'en est une autre, est effacée ; une conversation relue
 * éphémère (`ephemeral`, après un rechargement de la page) est suivie.
 */
export function useEphemeralRoute(chatId: string, ephemeral: boolean) {
  const queryClient = useQueryClient();
  useEffect(() => {
    forgetLeft(queryClient, openedChat(chatId));
  }, [queryClient, chatId]);
  useEffect(() => {
    if (ephemeral) forgetLeft(queryClient, markEphemeral(chatId));
  }, [queryClient, chatId, ephemeral]);
}

/**
 * « Keep » : la conversation éphémère passe dans l'historique (barre
 * latérale), et n'est plus effacée en la quittant.
 */
export function useKeepChat() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (chatId: string) => keepChat(chatId),
    onSuccess: (_, chatId) => {
      unmarkEphemeral(chatId);
      queryClient.setQueryData<Chat>(
        chatKey(chatId),
        (chat) => chat && { ...chat, ephemeral: false },
      );
      void queryClient.invalidateQueries({ queryKey: chatKey(chatId) });
      void queryClient.invalidateQueries({ queryKey: chatsKey });
    },
    onError: (error, chatId) => {
      console.error("Failed to keep the chat:", chatId, error);
    },
  });
}
