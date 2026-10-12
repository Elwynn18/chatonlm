import { useQuery } from "@tanstack/react-query";
import { getChat, listChats } from "@/lib/ipc";

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
