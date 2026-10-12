// Favoris de l'historique (propre à ChatonLM) : bascule d'une conversation
// dans la rubrique « Favorites » ou hors d'elle, affichée aussitôt (cache de
// l'historique modifié avant la réponse), puis annulée si la commande échoue.

import { useMutation, useQueryClient } from "@tanstack/react-query";
import type { ChatInfo } from "@/bindings/ChatInfo";
import { chatsKey } from "@/hooks/useChats";
import { setChatFavorite } from "@/lib/ipc";

const favoriteKey = ["setChatFavorite"] as const;

interface FavoriteChange {
  chatId: string;
  favorite: boolean;
}

/** Donne `favorite` à la conversation `chatId` dans le cache de l'historique. */
function markFavorite(
  chats: ChatInfo[] | undefined,
  chatId: string,
  favorite: boolean,
) {
  return chats?.map((chat) =>
    chat.id === chatId ? { ...chat, favorite } : chat,
  );
}

export function useChatFavorite() {
  const queryClient = useQueryClient();

  return useMutation({
    mutationKey: favoriteKey,
    mutationFn: ({ chatId, favorite }: FavoriteChange) =>
      setChatFavorite(chatId, favorite),
    onMutate: async ({ chatId, favorite }) => {
      // Une relecture en cours écraserait le changement affiché.
      await queryClient.cancelQueries({ queryKey: chatsKey });
      queryClient.setQueryData<ChatInfo[]>(chatsKey, (chats) =>
        markFavorite(chats, chatId, favorite),
      );
    },
    onError: (error, { chatId, favorite }) => {
      // Seule cette conversation reprend son état d'avant : une autre
      // bascule ou un renommage a pu suivre entre-temps.
      queryClient.setQueryData<ChatInfo[]>(chatsKey, (chats) =>
        markFavorite(chats, chatId, !favorite),
      );
      console.error("Failed to update favorite:", chatId, error);
    },
    onSettled: () => {
      // Relu une fois la dernière bascule finie, pas entre deux : la
      // réponse d'avant la suivante l'effacerait un instant.
      if (queryClient.isMutating({ mutationKey: favoriteKey }) === 1) {
        void queryClient.invalidateQueries({ queryKey: chatsKey });
      }
    },
  });
}
