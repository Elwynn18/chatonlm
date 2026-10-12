/*
 * SPDX-License-Identifier: MIT
 * SPDX-FileCopyrightText: Ollama
 * SPDX-FileCopyrightText: 2026 Elwynn18 and ChatonLM contributors
 *
 * Adapted from Ollama (https://github.com/ollama/ollama),
 * file app/ui/app/src/components/ErrorMessage.tsx, commit 42e911bc3d05.
 * Modified for ChatonLM: links open in the browser through the backend
 * (shared ExternalLink: middle click, own context menu, failure notice,
 * address shown as tooltip); optional actions under the message (web search
 * without an account: Sign In, Generate without web search); title translated
 * into English, Spanish and French (src/i18n), the message shown as given.
 */

import type { ReactNode } from "react";
import { ExternalLink } from "@/components/ExternalLink";
import { useT } from "@/i18n";

const renderWithLinks = (text: string) => {
  const urlRegex =
    /(https?:\/\/(?!127\.|localhost|0\.0\.0\.0|10\.|192\.168\.|172\.(?:1[6-9]|2[0-9]|3[0-1])\.)[^\s]+)/g;

  const urls = new Set(text.match(urlRegex) ?? []);

  // Split text by URLs
  // for example if the text is "connection failed. Try visiting https://ollama.com or check https://ollama.com/doc for help"
  // then the parsed parts will be ["connection failed. Try visiting ", "https://ollama.com", " or check ", "https://ollama.com/doc", " for help"]
  const parts = text.split(urlRegex);

  return parts.map((part, index) => {
    if (urls.has(part)) {
      return (
        <ExternalLink key={index} href={part} className="break-all underline">
          {part}
        </ExternalLink>
      );
    }
    return part;
  });
};

export function ErrorMessage({
  message,
  children,
}: {
  message: string;
  /** Actions proposées sous le message. */
  children?: ReactNode;
}) {
  const t = useT();
  return (
    <div
      role="alert"
      className="mb-8 flex w-full flex-col text-neutral-800 transition-colors dark:text-neutral-200"
    >
      <div className="relative flex items-center self-start">
        {/* Circled X icon */}
        <div className="mr-3 flex h-4 w-4 flex-shrink-0 items-center justify-center rounded-full border border-black dark:border-white">
          <svg
            className="h-2.5 w-2.5 fill-current text-black dark:text-white"
            viewBox="0 0 20 20"
            fill="none"
          >
            <path
              fillRule="evenodd"
              d="M4.293 4.293a1 1 0 011.414 0L10 8.586l4.293-4.293a1 1 0 111.414 1.414L11.414 10l4.293 4.293a1 1 0 01-1.414 1.414L10 11.414l-4.293 4.293a1 1 0 01-1.414-1.414L8.586 10 4.293 5.707a1 1 0 010-1.414z"
              clipRule="evenodd"
            />
          </svg>
        </div>
        <h3>{t("chat.messages.error")}</h3>
      </div>

      <div className="mt-2 ml-[1.8rem] flex items-start">
        <div className="flex-1 text-sm text-neutral-500 dark:text-neutral-400">
          {renderWithLinks(message)}
        </div>
      </div>

      {children && <div className="mt-3 ml-[1.8rem]">{children}</div>}
    </div>
  );
}
