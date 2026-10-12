/*
 * SPDX-License-Identifier: MIT
 * SPDX-FileCopyrightText: Ollama
 * SPDX-FileCopyrightText: 2026 Elwynn18 and ChatonLM contributors
 *
 * Adapted from Ollama (https://github.com/ollama/ollama),
 * file app/ui/app/src/hooks/useRenameChat.ts, commit 42e911bc3d05.
 * Modified for ChatonLM: Tauri commands instead of HTTP calls.
 */

import { useMutation, useQueryClient } from "@tanstack/react-query";
import { chatKey, chatsKey } from "@/hooks/useChats";
import { renameChat } from "@/lib/ipc";

export function useRenameChat() {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: ({ chatId, title }: { chatId: string; title: string }) =>
      renameChat(chatId, title),
    onSuccess: (_, { chatId }) => {
      // Invalidate and refetch chats list
      queryClient.invalidateQueries({ queryKey: chatsKey });
      // Invalidate the specific chat to update its title
      queryClient.invalidateQueries({ queryKey: chatKey(chatId) });
    },
    onError: (error) => {
      console.error("Failed to rename chat:", error);
    },
  });
}
