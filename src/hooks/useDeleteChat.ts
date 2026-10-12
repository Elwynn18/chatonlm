/*
 * SPDX-License-Identifier: MIT
 * SPDX-FileCopyrightText: Ollama
 * SPDX-FileCopyrightText: 2026 Elwynn18 and ChatonLM contributors
 *
 * Adapted from Ollama (https://github.com/ollama/ollama),
 * file app/ui/app/src/hooks/useDeleteChat.ts, commit 42e911bc3d05.
 * Modified for ChatonLM: Tauri commands instead of HTTP calls; the open chat is
 * passed in rather than read from the address bar.
 */

import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useNavigate } from "@tanstack/react-router";
import { chatKey, chatsKey } from "@/hooks/useChats";
import { deleteChat } from "@/lib/ipc";

export function useDeleteChat(currentChatId?: string) {
  const queryClient = useQueryClient();
  const navigate = useNavigate();

  return useMutation({
    mutationFn: (chatId: string) => deleteChat(chatId),
    onSuccess: (_, chatId) => {
      // If we're currently viewing the deleted chat, navigate away
      if (chatId === currentChatId) {
        navigate({ to: "/c/$chatId", params: { chatId: "new" } });
      }

      queryClient.removeQueries({ queryKey: chatKey(chatId) });
      queryClient.invalidateQueries({ queryKey: chatsKey });
    },
    onError: (error, chatId) => {
      console.error("Failed to delete chat:", chatId, error);
    },
  });
}
