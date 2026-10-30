/*
 * SPDX-License-Identifier: MIT
 * SPDX-FileCopyrightText: Ollama
 * SPDX-FileCopyrightText: 2026 Elwynn18 and ChatonLM contributors
 *
 * Adapted from Ollama (https://github.com/ollama/ollama),
 * file app/ui/app/src/routes/settings.tsx, commit 42e911bc3d05.
 * Modified for ChatonLM: named import of the Settings component; a "section"
 * search parameter, the tab of the page ("general", the default; "models":
 * loading of the models into memory; "account":
 * the Ollama account card, focused; "dictation": the dictation cards,
 * scrolled into view and focused, again on each navigation that is not a
 * mere change of tab; "readAloud": the reading aloud of replies, ChatonLM's
 * own; "voiceMode": the voice mode, ChatonLM's own; "easyLink": the Telegram
 * and Discord bots, ChatonLM's own; "vscode": the bridge of the VS Code
 * extension, ChatonLM's own; "shortcuts": the keyboard shortcuts,
 * ChatonLM's own),
 * any other value read as "general" by the page; a "card" search parameter
 * ("updates": the Updates card of General, scrolled into view and focused
 * the same way, from the system tray); the
 * sidebar shows the chat history too (ChatSidebar, Settings highlighted);
 * the title in English, Spanish and French (src/i18n).
 */

import { createFileRoute, useRouterState } from "@tanstack/react-router";
import { ChatSidebar } from "@/components/ChatSidebar";
import { SidebarLayout } from "@/components/layout/layout";
import {
  Settings,
  type SettingsCard,
  type SettingsSection,
} from "@/components/Settings";
import { useT } from "@/i18n";

/**
 * Onglet des réglages à montrer (lien de l'erreur de recherche web, menu de
 * la zone de notification, onglets).
 */
export interface SettingsSearch {
  section?: SettingsSection;
  /** Carte de General à montrer (« updates » : zone de notification). */
  card?: SettingsCard;
}

/**
 * Type du paramètre pour la navigation. Le routeur garde aussi les
 * paramètres bruts : la page compare elle-même la valeur (« general »,
 * « models », « account », « dictation », « readAloud », « voiceMode »,
 * « easyLink », « vscode », « shortcuts »).
 */
export function validateSettingsSearch(
  raw: Record<string, unknown>,
): SettingsSearch {
  return {
    ...(raw.section === "general" ||
    raw.section === "models" ||
    raw.section === "account" ||
    raw.section === "dictation" ||
    raw.section === "readAloud" ||
    raw.section === "voiceMode" ||
    raw.section === "easyLink" ||
    raw.section === "vscode" ||
    raw.section === "shortcuts"
      ? { section: raw.section }
      : {}),
    ...(raw.card === "updates" ? { card: raw.card } : {}),
  };
}

export const Route = createFileRoute("/settings")({
  validateSearch: validateSettingsSearch,
  component: SettingsRoute,
});

function SettingsRoute() {
  const t = useT();
  const { section, card } = Route.useSearch();
  // Une clé par entrée d'historique : chaque navigation vers la page (lien,
  // menu de la zone de notification, Back) redemande la carte à montrer ;
  // un simple changement d'onglet, non (le focus reste sur l'onglet).
  const request = useRouterState({
    select: (state) =>
      state.location.state.settingsTab
        ? null
        : (state.location.state.__TSR_key ?? state.location.href),
  });
  return (
    <SidebarLayout
      title={t("settings.title")}
      sidebar={<ChatSidebar current="settings" />}
    >
      <Settings section={section} card={card} request={request} />
    </SidebarLayout>
  );
}
