/*
 * SPDX-License-Identifier: MIT
 * SPDX-FileCopyrightText: Ollama
 * SPDX-FileCopyrightText: 2026 Elwynn18 and ChatonLM contributors
 *
 * Adapted from Ollama (https://github.com/ollama/ollama),
 * file app/ui/app/src/components/ChatSidebar.tsx, commit 42e911bc3d05
 * (sorting and grouping of the chat history).
 * Modified for ChatonLM: pure functions, dates as milliseconds; a "Favorites"
 * group first (ChatonLM's own favorite chats), where a favorite chat is the
 * only place it appears; groups named by an id, their titles and the date of
 * an untitled chat translated into English, Spanish and French (src/i18n).
 */

import type { ChatInfo } from "@/bindings/ChatInfo";
import type { TFunction } from "@/i18n";
import { localized } from "@/i18n";

/** Rubrique de l'historique (titre traduit par ChatSidebar). */
export type ChatGroupId = "favorites" | "today" | "thisWeek" | "older";

export interface ChatGroup {
  id: ChatGroupId;
  chats: ChatInfo[];
}

const WEEK_MS = 7 * 24 * 60 * 60 * 1000;

function isToday(date: Date, now: Date) {
  return (
    date.getDate() === now.getDate() &&
    date.getMonth() === now.getMonth() &&
    date.getFullYear() === now.getFullYear()
  );
}

function isThisWeek(date: Date, now: Date) {
  const weekAgo = new Date(now.getTime() - WEEK_MS);
  return date > weekAgo && !isToday(date, now);
}

/** Plus récemment active d'abord ; à égalité, identifiant décroissant. */
export function sortChats(chats: ChatInfo[]): ChatInfo[] {
  return [...chats].sort((a, b) => {
    const comparison = b.updatedAt - a.updatedAt;
    if (comparison === 0) {
      return b.id.localeCompare(a.id);
    }
    return comparison;
  });
}

/**
 * Groupes de l'historique ; les groupes vides sont retirés. Les favorites
 * vont toutes dans « Favorites », en tête (même tri), et seulement là.
 */
export function groupChats(chats: ChatInfo[], now = new Date()): ChatGroup[] {
  const groups: ChatGroup[] = [
    { id: "favorites", chats: [] },
    { id: "today", chats: [] },
    { id: "thisWeek", chats: [] },
    { id: "older", chats: [] },
  ];
  for (const chat of sortChats(chats)) {
    const date = new Date(chat.updatedAt);
    if (chat.favorite) {
      groups[0].chats.push(chat);
    } else if (isToday(date, now)) {
      groups[1].chats.push(chat);
    } else if (isThisWeek(date, now)) {
      groups[2].chats.push(chat);
    } else {
      groups[3].chats.push(chat);
    }
  }
  return groups.filter((group) => group.chats.length > 0);
}

/**
 * Texte affiché pour une conversation : titre, sinon début du premier
 * message, sinon sa date de création dans la langue de `t`.
 */
export function chatLabel(t: TFunction, chat: ChatInfo): string {
  return (
    chat.title ||
    chat.userExcerpt ||
    // Comme l'officiel (toLocaleString), dans la langue affichée.
    localized(new Date(chat.createdAt).toLocaleString(t.lang))
  );
}
