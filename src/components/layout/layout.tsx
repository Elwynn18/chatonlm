/*
 * SPDX-License-Identifier: MIT
 * SPDX-FileCopyrightText: Ollama
 * SPDX-FileCopyrightText: 2026 Elwynn18 and ChatonLM contributors
 *
 * Adapted from Ollama (https://github.com/ollama/ollama),
 * file app/ui/app/src/components/layout/layout.tsx, commit 42e911bc3d05.
 * Modified for ChatonLM: native window decorations only (the Windows branch of
 * the original), no drag handlers, icons from Heroicons or drawn for ChatonLM,
 * notice shown at the top of the content area when a link can't be opened
 * (main is positioned for it); texts translated into English, Spanish and
 * French (src/i18n); the open state of the sidebar is kept in src/lib/sidebar.ts
 * (same behaviour), so that the first-launch tour can open it; buttons
 * of the page at the top right of the content area (`actions`: ChatonLM's own
 * ephemeral chat button).
 */

import { PencilSquareIcon } from "@heroicons/react/24/outline";
import { Link } from "@tanstack/react-router";
import type { PropsWithChildren, ReactNode } from "react";
import { SidebarToggleIcon } from "@/components/icons";
import { LinkErrorNotice } from "@/components/LinkErrorNotice";
import { useT } from "@/i18n";
import { setSidebarOpen, useSidebarOpen } from "@/lib/sidebar";

export function SidebarLayout({
  sidebar,
  title,
  actions,
  children,
}: PropsWithChildren<{
  sidebar: ReactNode;
  title?: string;
  /** Boutons en haut à droite de la zone de contenu. */
  actions?: ReactNode;
}>) {
  const t = useT();
  const sidebarOpen = useSidebarOpen();
  const toggleLabel = t(
    sidebarOpen ? "chat.layout.hideSidebar" : "chat.layout.showSidebar",
  );

  const toggleSidebar = () => setSidebarOpen(!sidebarOpen);

  return (
    <div className="flex h-screen w-full overflow-hidden dark:bg-neutral-900">
      <div className="absolute left-2 z-20 mx-2 flex items-center py-2 text-neutral-500 dark:text-neutral-400">
        <button
          type="button"
          onClick={toggleSidebar}
          className="flex h-9 w-9 cursor-pointer items-center justify-center rounded-full hover:bg-neutral-200 dark:hover:bg-neutral-700/75"
          aria-label={toggleLabel}
          title={toggleLabel}
        >
          <SidebarToggleIcon className="h-5 w-5" />
        </button>
        {!title && (
          <Link
            to="/c/$chatId"
            params={{ chatId: "new" }}
            title={t("chat.layout.newChat")}
            aria-label={t("chat.layout.newChat")}
            className={`ml-1 flex h-9 w-9 items-center justify-center rounded-full transition-opacity duration-375 hover:bg-neutral-200 dark:hover:bg-neutral-700 ${
              sidebarOpen ? "pointer-events-none opacity-0" : "opacity-100"
            }`}
          >
            <PencilSquareIcon className="h-5 w-5" />
          </Link>
        )}
      </div>
      <div
        className={`flex max-h-screen flex-col transition-[width] duration-300 ${
          sidebarOpen
            ? "w-48 border-r border-neutral-200 bg-neutral-100 dark:border-neutral-800 dark:bg-neutral-950/40"
            : "w-0"
        }`}
      >
        <div className="h-13 w-full flex-none"></div>
        {sidebarOpen && sidebar}
      </div>
      {/* relative : repère de l'avis d'échec d'un lien (LinkErrorNotice). */}
      <main className="relative flex min-w-0 flex-1 flex-col transition-all duration-300">
        <div
          className={`z-10 flex h-13 w-full flex-none items-center bg-white dark:bg-neutral-900 ${title ? "" : "xl:hidden"}`}
        >
          {title && (
            <h1
              className={`${sidebarOpen ? "pl-6" : "pl-16"} font-rounded text-base font-medium transition-[padding-left] duration-300 dark:text-white`}
            >
              {title}
            </h1>
          )}
        </div>
        {actions && (
          <div className="absolute top-0 right-2 z-20 mx-2 flex items-center py-2">
            {actions}
          </div>
        )}
        {children}
        {/* Seul affichage des échecs d'ouverture de lien (ExternalLink) : en
            haut de la zone de contenu, centré sur elle comme les messages,
            loin de la zone de saisie. */}
        <LinkErrorNotice />
      </main>
    </div>
  );
}
