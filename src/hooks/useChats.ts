import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect } from "react";
import { getChat, listChats, listenChatsUpdated } from "@/lib/ipc";

export const chatsKey = ["chats"] as const;
export const chatKey = (id: string) => ["chat", id] as const;

/** Historique des conversations. */
export function useChats() {
  return useQuery({ queryKey: chatsKey, queryFn: listChats });
}

/** Une conversation ; identifiant vide = aucune (nouvelle conversation). */
export function useChat(id: string) {
  return useQuery({
    queryKey: chatKey(id),
    queryFn: () => getChat(id),
    enabled: id !== "",
    staleTime: 1500,
  });
}

/**
 * Écoute « chats-updated » tant que l'app est montée (route racine) : le
 * moteur a créé ou modifié une conversation hors de la page (message reçu
 * par un bot d'Easy Link) ; l'historique et cette conversation sont relus.
 */
export function useChatsEvents() {
  const queryClient = useQueryClient();
  useEffect(() => {
    let active = true;
    let unlisten: (() => void) | null = null;
    listenChatsUpdated(({ chatId }) => {
      void queryClient.invalidateQueries({ queryKey: chatsKey });
      void queryClient.invalidateQueries({ queryKey: chatKey(chatId) });
    }).then(
      (stop) => {
        if (active) unlisten = stop;
        else stop();
      },
      (error: unknown) =>
        console.error("Failed to listen for chat updates:", error),
    );
    return () => {
      active = false;
      unlisten?.();
    };
  }, [queryClient]);
}
