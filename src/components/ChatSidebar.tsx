/*
 * SPDX-License-Identifier: MIT
 * SPDX-FileCopyrightText: Ollama
 * SPDX-FileCopyrightText: 2026 Elwynn18 and ChatonLM contributors
 *
 * Adapted from Ollama (https://github.com/ollama/ollama),
 * file app/ui/app/src/components/ChatSidebar.tsx, commit 42e911bc3d05.
 * Modified for ChatonLM: context menu and delete confirmation drawn in the page
 * instead of the native ones, sorting and grouping moved to src/lib/chatGroups.ts, router links,
 * Escape in the rename field no longer reaches the rest of the app, no hidden
 * debug copy feature, no-op class scrollbar-gutter dropped, Ctrl+, in the
 * rename field saves the title before opening the settings; shown on the
 * ModelHub and Settings pages too (their entry highlighted, no chat); favorite
 * chats, ChatonLM's own ("Favorites" group, entry of the context menu); on
 * hover or keyboard focus, star and delete buttons at the right of each chat
 * (star always shown for a favorite), the delete button opening the same
 * confirmation as the context menu; texts, group titles and the date of an
 * untitled chat translated into English, Spanish and French (src/i18n); a
 * chat that comes from a Telegram or Discord bot (ChatonLM's own Easy Link)
 * has a small sign before its title, named after its messenger.
 */

import { LinkIcon } from "@heroicons/react/20/solid";
import { StarIcon, TrashIcon } from "@heroicons/react/24/outline";
import { StarIcon as FilledStarIcon } from "@heroicons/react/24/solid";
import { useQueryClient } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import {
  useCallback,
  useEffect,
  useId,
  useMemo,
  useRef,
  useState,
} from "react";
import type { ChatInfo } from "@/bindings/ChatInfo";
import type { EasyLinkPlatform } from "@/bindings/EasyLinkPlatform";
import { AppNavigation, type AppSection } from "@/components/AppSidebar";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { ContextMenu } from "@/components/ContextMenu";
import { useChatFavorite } from "@/hooks/useChatFavorite";
import { chatKey, chatsKey, useChats } from "@/hooks/useChats";
import { useDeleteChat } from "@/hooks/useDeleteChat";
import { useRenameChat } from "@/hooks/useRenameChat";
import { type TKey, useT } from "@/i18n";
import { type ChatGroupId, chatLabel, groupChats } from "@/lib/chatGroups";
import { getChat } from "@/lib/ipc";
import { isSettingsShortcut } from "@/lib/shortcuts";

/**
 * Boutons d'une ligne de l'historique : montrés au survol de la ligne ou quand
 * le focus clavier y est (pas après un simple clic, qui laisse le focus au
 * lien sous WebKitGTK) ; la ligne porte group/chat.
 */
const REVEALED_ON_HOVER =
  "opacity-0 group-hover/chat:opacity-100 group-has-[:focus-visible]/chat:opacity-100";

/** Titres des rubriques de l'historique. */
const GROUP_TITLES: Record<ChatGroupId, TKey> = {
  favorites: "chat.history.groups.favorites",
  today: "chat.history.groups.today",
  thisWeek: "chat.history.groups.thisWeek",
  older: "chat.history.groups.older",
};

const ACTION_CLASS =
  "pointer-events-auto flex size-6 items-center justify-center rounded-md outline-none hover:bg-neutral-300 focus-visible:ring-2 focus-visible:ring-accent-fg dark:hover:bg-neutral-700";

/**
 * Signe d'une conversation venue d'un bot d'Easy Link, nommé d'après sa
 * messagerie (« Telegram chat »).
 */
function ChatSource({ source }: { source: EasyLinkPlatform }) {
  const t = useT();
  const name = t(`chat.history.source.${source}`);
  return (
    <span
      role="img"
      data-testid="chat-source"
      aria-label={name}
      title={name}
      className="mr-1.5 flex-none text-neutral-500"
    >
      <LinkIcon aria-hidden="true" className="size-3.5" />
    </span>
  );
}

interface ChatSidebarProps {
  /** Conversation affichée, surlignée (aucune hors du chat). */
  currentChatId?: string;
  /** Entrée de la navigation surlignée. */
  current?: AppSection;
}

export function ChatSidebar({
  currentChatId,
  current = "chat",
}: ChatSidebarProps) {
  const t = useT();
  const { data, isLoading, error } = useChats();
  const queryClient = useQueryClient();
  const renameMutation = useRenameChat();
  const deleteMutation = useDeleteChat(currentChatId);
  const { mutate: setFavorite } = useChatFavorite();
  const navRef = useRef<HTMLElement>(null);
  /** Préfixe des identifiants des titres (description des boutons). */
  const titleIds = useId();
  /**
   * Partie de la ligne qui avait le focus clavier quand la conversation a
   * changé de rubrique : la ligne est recréée ailleurs, le focus lui est
   * rendu.
   */
  const refocus = useRef<{
    chatId: string;
    favorite: boolean;
    part: string;
  } | null>(null);
  const [editingChatId, setEditingChatId] = useState<string | null>(null);
  const [editValue, setEditValue] = useState("");
  const [menu, setMenu] = useState<{
    x: number;
    y: number;
    chat: ChatInfo;
  } | null>(null);
  const [pendingDeleteId, setPendingDeleteId] = useState<string | null>(null);
  const inputRef = useRef<HTMLInputElement>(null);

  const handleMouseEnter = useCallback(
    (chatId: string) => {
      queryClient.prefetchQuery({
        queryKey: chatKey(chatId),
        queryFn: () => getChat(chatId),
        staleTime: 1500,
      });
    },
    [queryClient],
  );

  const startEditing = useCallback((chatId: string, currentTitle: string) => {
    setEditingChatId(chatId);
    setEditValue(currentTitle);
  }, []);

  const saveRename = useCallback(async () => {
    if (!editingChatId || !editValue.trim()) {
      setEditingChatId(null);
      return;
    }

    const newTitle = editValue.trim();
    const chatId = editingChatId;

    // Exit edit mode immediately to prevent flash
    setEditingChatId(null);
    setEditValue("");

    // Optimistically update the cache
    queryClient.setQueryData(chatsKey, (oldData: ChatInfo[] | undefined) =>
      oldData?.map((chat) =>
        chat.id === chatId ? { ...chat, title: newTitle } : chat,
      ),
    );

    try {
      await renameMutation.mutateAsync({ chatId, title: newTitle });
    } catch {
      // Revert optimistic update on error
      queryClient.invalidateQueries({ queryKey: chatsKey });
    }
  }, [editingChatId, editValue, renameMutation, queryClient]);

  useEffect(() => {
    if (editingChatId && inputRef.current) {
      inputRef.current.focus();
      inputRef.current.select();
    }
  }, [editingChatId]);

  useEffect(() => {
    const handleClickOutside = (event: MouseEvent) => {
      if (
        inputRef.current &&
        !inputRef.current.contains(event.target as Node)
      ) {
        saveRename();
      }
    };

    if (editingChatId) {
      document.addEventListener("mousedown", handleClickOutside);
      return () => {
        document.removeEventListener("mousedown", handleClickOutside);
      };
    }
  }, [editingChatId, editValue, saveRename]);

  const chatGroups = useMemo(() => groupChats(data ?? []), [data]);

  const toggleFavorite = useCallback(
    (chat: ChatInfo) => {
      const favorite = !chat.favorite;
      const active = document.activeElement;
      const row = active?.closest<HTMLElement>("[data-chat-id]");
      // Focus clavier seulement : après un clic (WebKitGTK donne le focus au
      // bouton ou au lien cliqué), le rendre à la ligne déplacée y laisserait
      // un anneau et la poubelle visibles. Vaut aussi pour le menu
      // contextuel, qui a rendu le focus à l'élément qui l'avait.
      if (
        active instanceof HTMLElement &&
        row?.dataset.chatId === chat.id &&
        active.matches(":focus-visible")
      ) {
        refocus.current = {
          chatId: chat.id,
          favorite,
          part: active.dataset.testid ?? "link",
        };
      }
      setFavorite({ chatId: chat.id, favorite });
    },
    [setFavorite],
  );

  // La ligne déplacée (Favorites <-> date) est une autre : si le focus était
  // dans l'ancienne et n'est pas allé ailleurs, il passe à la nouvelle.
  useEffect(() => {
    const target = refocus.current;
    if (!target) return;
    const chat = data?.find((c) => c.id === target.chatId);
    if (chat && chat.favorite !== target.favorite) return;
    refocus.current = null;
    const active = document.activeElement;
    if (!chat || (active !== null && active !== document.body)) return;
    const row = [
      ...(navRef.current?.querySelectorAll<HTMLElement>("[data-chat-id]") ??
        []),
    ].find((element) => element.dataset.chatId === chat.id);
    const element =
      target.part === "link"
        ? row?.querySelector("a")
        : row?.querySelector<HTMLElement>(`[data-testid="${target.part}"]`);
    element?.focus();
  }, [data]);

  // Appelée après confirmation (voir ConfirmDialog plus bas).
  const handleDeleteChat = useCallback(
    async (chatId: string) => {
      try {
        await deleteMutation.mutateAsync(chatId);
      } catch (error) {
        console.error("Failed to delete chat:", error);
      }
    },
    [deleteMutation],
  );

  const closeMenu = useCallback(() => setMenu(null), []);
  const cancelDelete = useCallback(() => setPendingDeleteId(null), []);

  return (
    <nav
      ref={navRef}
      aria-busy={isLoading || undefined}
      className="flex flex-1 flex-col min-h-0 select-none"
    >
      <header className="flex flex-col gap-0.5 px-4 pb-2">
        <AppNavigation current={current} />
      </header>
      <div className="flex flex-1 flex-col px-4 py-1 overflow-y-auto overscroll-auto">
        {error ? (
          <div className="px-2 pt-4 text-sm text-red-600 dark:text-red-400">
            {t("chat.history.loadError")}
          </div>
        ) : (
          <div className="flex flex-col gap-3 pt-4">
            {chatGroups.map((group) => (
              <div key={group.id} className="flex flex-col gap-0.5">
                <h3 className="text-xs font-medium text-neutral-500 px-2 py-1 select-none">
                  {t(GROUP_TITLES[group.id])}
                </h3>
                {group.chats.map((chat) => (
                  <div
                    key={chat.id}
                    data-chat-id={chat.id}
                    className={`group/chat allow-context-menu flex items-center relative text-sm text-neutral-800 dark:text-neutral-400 rounded-lg hover:bg-neutral-200 dark:hover:bg-neutral-800 ${
                      chat.id === currentChatId
                        ? "bg-neutral-200 text-black dark:bg-neutral-800"
                        : ""
                    }`}
                    onMouseEnter={() => handleMouseEnter(chat.id)}
                    onContextMenu={(event) => {
                      event.preventDefault();
                      setMenu({ x: event.clientX, y: event.clientY, chat });
                    }}
                  >
                    {editingChatId === chat.id ? (
                      <div className="flex-1 flex items-center min-w-0 px-2 py-2 bg-white text-black dark:bg-neutral-700 rounded-lg">
                        <span className="truncate font-sans text-sm w-full">
                          <input
                            ref={inputRef}
                            type="text"
                            aria-label={t("chat.history.titleField")}
                            value={editValue}
                            onChange={(e) => setEditValue(e.target.value)}
                            onKeyDown={(e) => {
                              // Ctrl+, quitte la page : le titre tapé est
                              // enregistré, comme par un clic ailleurs.
                              if (isSettingsShortcut(e.nativeEvent)) {
                                saveRename();
                              } else if (e.key === "Enter") {
                                e.preventDefault();
                                saveRename();
                              } else if (e.key === "Escape") {
                                // Consommée ici : Échap n'arrêtera pas une
                                // réponse en cours (écart voulu).
                                e.preventDefault();
                                setEditingChatId(null);
                                setEditValue("");
                              }
                            }}
                            className="bg-transparent border-0 focus:outline-none w-full dark:text-white"
                            style={{
                              font: "inherit",
                              lineHeight: "inherit",
                              padding: 0,
                              margin: 0,
                            }}
                          />
                        </span>
                      </div>
                    ) : (
                      <>
                        <Link
                          to="/c/$chatId"
                          params={{ chatId: chat.id }}
                          // Marge des boutons quand ils sont montrés (toujours
                          // pour une favorite) : le titre s'arrête avant.
                          className={`flex-1 flex items-center min-w-0 py-2 pl-2 select-none ${
                            chat.favorite
                              ? "pr-13"
                              : "pr-2 group-hover/chat:pr-13 group-has-[:focus-visible]/chat:pr-13"
                          }`}
                          draggable={false}
                        >
                          {/* Venue d'un bot (Easy Link) : signe générique,
                              pas le logo d'une marque ; hors du titre, qui
                              garde sa troncature et décrit seul les boutons. */}
                          {chat.source && <ChatSource source={chat.source} />}
                          <span
                            id={`${titleIds}-${chat.id}`}
                            className="truncate font-sans text-sm"
                          >
                            {chatLabel(t, chat)}
                          </span>
                        </Link>
                        {/* Frères du lien (pas de bouton dans un lien) :
                            un clic ne navigue pas. Le conteneur laisse
                            passer les clics à côté des boutons (lien). */}
                        <div className="pointer-events-none absolute inset-y-0 right-1 flex items-center">
                          <button
                            type="button"
                            data-testid="chat-favorite"
                            // Nom fixe, état dans aria-pressed (bouton
                            // bascule) ; conversation en description.
                            aria-label={t("chat.history.favorite")}
                            aria-pressed={chat.favorite}
                            aria-describedby={`${titleIds}-${chat.id}`}
                            title={t(
                              chat.favorite
                                ? "chat.history.removeFavorite"
                                : "chat.history.addFavorite",
                            )}
                            onClick={(event) => {
                              // Second clic d'un double-clic : la ligne a
                              // changé de rubrique, il tomberait sur l'étoile
                              // d'une autre conversation.
                              if (event.detail > 1) return;
                              toggleFavorite(chat);
                            }}
                            className={`${ACTION_CLASS} ${
                              chat.favorite
                                ? "text-accent-fg hover:text-neutral-900 dark:hover:text-white"
                                : `${REVEALED_ON_HOVER} text-neutral-500 hover:text-neutral-800 dark:text-neutral-400 dark:hover:text-neutral-100`
                            }`}
                          >
                            {chat.favorite ? (
                              <FilledStarIcon
                                aria-hidden="true"
                                className="size-4"
                              />
                            ) : (
                              <StarIcon aria-hidden="true" className="size-4" />
                            )}
                          </button>
                          <button
                            type="button"
                            data-testid="chat-delete"
                            aria-label={t("chat.history.deleteChat")}
                            aria-describedby={`${titleIds}-${chat.id}`}
                            title={t("chat.history.deleteChat")}
                            onClick={() => setPendingDeleteId(chat.id)}
                            className={`${ACTION_CLASS} ${REVEALED_ON_HOVER} text-red-600 hover:text-red-700 dark:text-red-400 dark:hover:text-red-300`}
                          >
                            <TrashIcon aria-hidden="true" className="size-4" />
                          </button>
                        </div>
                      </>
                    )}
                  </div>
                ))}
              </div>
            ))}
          </div>
        )}
      </div>
      {menu && (
        <ContextMenu
          x={menu.x}
          y={menu.y}
          onClose={closeMenu}
          items={[
            {
              label: t(
                menu.chat.favorite
                  ? "chat.history.removeFavorite"
                  : "chat.history.addFavorite",
              ),
              onSelect: () => toggleFavorite(menu.chat),
            },
            {
              label: t("chat.history.rename"),
              onSelect: () =>
                startEditing(menu.chat.id, chatLabel(t, menu.chat)),
            },
            {
              label: t("common.actions.delete"),
              onSelect: () => setPendingDeleteId(menu.chat.id),
            },
          ]}
        />
      )}
      {pendingDeleteId && (
        <ConfirmDialog
          message={t("chat.history.confirmDelete")}
          onConfirm={() => {
            setPendingDeleteId(null);
            void handleDeleteChat(pendingDeleteId);
          }}
          onCancel={cancelDelete}
        />
      )}
    </nav>
  );
}
