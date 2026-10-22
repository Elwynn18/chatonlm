/*
 * SPDX-License-Identifier: MIT
 * SPDX-FileCopyrightText: Ollama
 * SPDX-FileCopyrightText: 2026 Elwynn18 and ChatonLM contributors
 *
 * Adapted from Ollama (https://github.com/ollama/ollama),
 * file app/ui/app/src/routes/c.$chatId.tsx, commit 42e911bc3d05.
 * Modified for ChatonLM: no "launch" redirect and no last home view (the Apps
 * view is not reproduced); a deleted or unknown chat shows "Chat not found"
 * instead of an error; texts translated into English, Spanish and French
 * (src/i18n); ChatonLM's own ephemeral chats: the ghost kitten button at the
 * top right (EphemeralButton.tsx), and the ephemeral chat left for another
 * chat (or a new one) erased (src/hooks/useEphemeral.ts).
 */

import { createFileRoute } from "@tanstack/react-router";
import { Chat } from "@/components/Chat";
import { ChatSidebar } from "@/components/ChatSidebar";
import { EphemeralButton } from "@/components/EphemeralButton";
import { SidebarLayout } from "@/components/layout/layout";
import { chatKey, useChat } from "@/hooks/useChats";
import { useEphemeralRoute } from "@/hooks/useEphemeral";
import { useT } from "@/i18n";
import { getChat, isAppError } from "@/lib/ipc";

export const Route = createFileRoute("/c/$chatId")({
  component: RouteComponent,
  loader: async ({ context, params }) => {
    if (params.chatId !== "new") {
      // Chargement anticipé ; une erreur s'affiche dans la page.
      context.queryClient
        .ensureQueryData({
          queryKey: chatKey(params.chatId),
          queryFn: () => getChat(params.chatId),
          staleTime: 1500,
        })
        .catch(() => undefined);
    }
  },
});

function RouteComponent() {
  const { chatId } = Route.useParams();
  const t = useT();

  // Always call hooks at the top level - use a flag to skip data when chatId is a special view
  const {
    data: chatData,
    isLoading: chatLoading,
    error: chatError,
  } = useChat(chatId === "new" ? "" : chatId);
  const ephemeral = chatData?.ephemeral ?? false;
  useEphemeralRoute(chatId, ephemeral);
  const actions = <EphemeralButton chatId={chatId} ephemeral={ephemeral} />;

  // Handle "new" chat case - just use Chat component which handles everything
  if (chatId === "new") {
    return (
      <SidebarLayout
        sidebar={<ChatSidebar currentChatId={chatId} />}
        actions={actions}
      >
        <Chat chatId={chatId} />
      </SidebarLayout>
    );
  }

  // Handle existing chat case
  if (chatLoading) {
    return (
      <SidebarLayout sidebar={<ChatSidebar currentChatId={chatId} />}>
        <div className="p-4">{t("chat.route.loading")}</div>
      </SidebarLayout>
    );
  }

  const notFound = isAppError(chatError) && chatError.kind === "notFound";
  if (chatError && !notFound) {
    return (
      <SidebarLayout sidebar={<ChatSidebar currentChatId={chatId} />}>
        <div className="p-4 text-red-600 dark:text-red-400">
          {t("chat.route.loadError")}
        </div>
      </SidebarLayout>
    );
  }

  if (!chatData) {
    return (
      <SidebarLayout sidebar={<ChatSidebar currentChatId={chatId} />}>
        <div className="p-4">{t("chat.route.notFound")}</div>
      </SidebarLayout>
    );
  }

  return (
    <SidebarLayout
      sidebar={<ChatSidebar currentChatId={chatId} />}
      actions={actions}
    >
      <Chat chatId={chatId} />
    </SidebarLayout>
  );
}
