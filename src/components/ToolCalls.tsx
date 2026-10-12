/*
 * SPDX-License-Identifier: MIT
 * SPDX-FileCopyrightText: Ollama
 * SPDX-FileCopyrightText: 2026 Elwynn18 and ChatonLM contributors
 *
 * Adapted from Ollama (https://github.com/ollama/ollama),
 * file app/ui/app/src/components/Message.tsx, commit 42e911bc3d05.
 * Modified for ChatonLM: tool calls and their results come from the message
 * of the assistant that made them (no separate "tool" messages); each result
 * is titled by the query or address of its own call (the official one takes
 * the last one seen, wrong when a turn makes several calls); no browser tool
 * display (ChatonLM gives every model the same two web tools), any other tool
 * shown as "Calling name: preview..."; Heroicons magnifying glass, globe,
 * code brackets and chevron instead of the SF Symbols and Feather glyphs;
 * the result header is a real button (aria-expanded, aria-controls,
 * keyboard, blue focus ring, chevron shown on keyboard focus too); the folded
 * body is hidden from keyboard and screen readers and marked (data-folded)
 * like the thinking block; no shared element id on results; texts translated
 * into English, Spanish and French (src/i18n).
 */

import {
  ChevronDownIcon,
  CodeBracketIcon,
  GlobeAltIcon,
} from "@heroicons/react/24/outline";
import { MagnifyingGlassIcon } from "@heroicons/react/20/solid";
import { useId, useState } from "react";
import type { ToolCall } from "@/bindings/ToolCall";
import { type TFunction, useT } from "@/i18n";
import { Trans } from "@/i18n/Trans";

/** Arguments d'un appel (objet JSON en texte) ; null s'ils sont illisibles. */
function parseArguments(call: ToolCall): Record<string, unknown> | null {
  try {
    const parsed: unknown = JSON.parse(call.arguments);
    return typeof parsed === "object" && parsed !== null
      ? (parsed as Record<string, unknown>)
      : null;
  } catch {
    return null;
  }
}

/** Valeur texte d'un argument, sinon "". */
function stringArgument(call: ToolCall, key: string): string {
  const value = parseArguments(call)?.[key];
  return typeof value === "string" ? value : "";
}

function InlineSearchTerm({ term }: { term: string }) {
  return <span className="font-medium">{term}</span>;
}

/** Ligne « Searching for… » d'un appel, comme ToolCallDisplay de l'officiel. */
export function ToolCallDisplay({ call }: { call: ToolCall }) {
  // frontend tool call display for web_search and web_fetch
  if (call.name === "web_search" || call.name === "web_fetch") {
    const search = call.name === "web_search";
    const term = stringArgument(call, search ? "query" : "url");
    return (
      <div className="relative text-neutral-600 select-text dark:text-neutral-400">
        {/* Magnifying Glass Icon */}
        <MagnifyingGlassIcon
          aria-hidden="true"
          className="absolute top-1 h-4 w-4"
        />
        <div className="ml-6">
          <Trans
            k={search ? "chat.tools.searching" : "chat.tools.fetching"}
            tags={{ term: () => <InlineSearchTerm term={term} /> }}
          />
        </div>
      </div>
    );
  }

  // preview from the tool's JSON arguments.
  const args = parseArguments(call);
  const preferredKey = ["query", "url", "pattern", "id", "file", "path"].find(
    (key) => args !== null && Object.prototype.hasOwnProperty.call(args, key),
  );
  const value = preferredKey ? args?.[preferredKey] : undefined;
  const preview = typeof value === "string" ? value : "";

  return (
    <div className="relative text-neutral-600 select-text dark:text-neutral-400">
      <CodeBracketIcon
        aria-hidden="true"
        strokeWidth={2}
        className="absolute top-1.5 h-4 w-4"
      />
      <div className="ml-6">
        <Trans
          k={preview ? "chat.tools.callingWith" : "chat.tools.calling"}
          tags={{
            name: () => <span className="font-mono">{call.name}</span>,
            preview: () => <InlineSearchTerm term={preview} />,
          }}
        />
      </div>
    </div>
  );
}

/** Titre d'un résultat, par la requête ou l'adresse de son propre appel. */
function resultTitle(t: TFunction, call: ToolCall, result: unknown) {
  const fromResult = (key: string) =>
    typeof result === "object" &&
    result !== null &&
    typeof (result as Record<string, unknown>)[key] === "string"
      ? ((result as Record<string, unknown>)[key] as string)
      : "";
  if (call.name === "web_search") {
    const q =
      stringArgument(call, "query").trim() || fromResult("query").trim();
    return q ? (
      <Trans
        k="chat.tools.searchResults"
        tags={{ term: () => <InlineSearchTerm term={q} /> }}
      />
    ) : (
      t("chat.tools.webSearchResults")
    );
  }
  if (call.name === "web_fetch") {
    const u = stringArgument(call, "url").trim() || fromResult("url");
    return u ? (
      <Trans
        k="chat.tools.fetchResults"
        tags={{ url: () => <span className="break-all">{u}</span> }}
      />
    ) : (
      t("chat.tools.webFetchResults")
    );
  }
  return t("chat.tools.rawResult");
}

/** Résultat relu : objet JSON, ou le texte tel quel s'il n'en est pas un. */
function parseResult(text: string): unknown {
  try {
    return JSON.parse(text) as unknown;
  } catch {
    return text;
  }
}

/**
 * Résultat d'un appel terminé, comme ToolRoleContent de l'officiel : erreur
 * en clair (« Error: … »), sinon bloc replié par défaut (globe, flèche au
 * survol) dont le corps montre le JSON indenté.
 */
export function ToolResult({ call }: { call: ToolCall }) {
  const t = useT();
  const [isCollapsed, setIsCollapsed] = useState(true);
  const bodyId = useId();

  if (call.error !== null) {
    return (
      <div className="space-y-2">
        <pre className="max-h-40 overflow-x-auto rounded-md bg-neutral-100 p-2 text-xs whitespace-pre-wrap text-neutral-800 dark:bg-neutral-800 dark:text-neutral-200">
          <code>{t("chat.tools.error", { error: call.error })}</code>
        </pre>
      </div>
    );
  }
  if (call.result === null) return null;

  const result = parseResult(call.result);
  return (
    // collapsible tool result with raw json
    <div className="space-y-2">
      <div className="relative flex w-full flex-col text-neutral-600 transition-colors select-text hover:text-neutral-800 dark:text-neutral-400 dark:hover:text-neutral-200">
        <button
          type="button"
          className="group/browser relative flex cursor-pointer self-start rounded-md text-left outline-none select-text focus-visible:ring-2 focus-visible:ring-accent-fg"
          aria-expanded={!isCollapsed}
          aria-controls={bodyId}
          onClick={() => setIsCollapsed(!isCollapsed)}
        >
          {/* Globe icon */}
          <GlobeAltIcon
            aria-hidden="true"
            className="absolute top-1 -left-0.5 h-5 w-5 transition-opacity group-hover/browser:opacity-0 group-focus-visible/browser:opacity-0"
          />
          {/* Arrow */}
          <ChevronDownIcon
            aria-hidden="true"
            strokeWidth={2}
            className={`absolute top-1.5 h-4 w-4 opacity-0 transition-all group-hover/browser:opacity-100 group-focus-visible/browser:opacity-100 ${
              isCollapsed ? "-rotate-90" : "rotate-0"
            } will-change-[opacity,transform]`}
          />
          <span className="ml-6">{resultTitle(t, call, result)}</span>
        </button>

        <div
          id={bodyId}
          className="mt-2 ml-6 overflow-y-auto rounded-md text-xs text-neutral-500 transition-[max-height,opacity,visibility] duration-300 ease-in-out"
          style={{
            maxHeight: isCollapsed ? "0px" : "20rem",
            opacity: isCollapsed ? 0 : 1,
            // Replié : ni tabulation ni lecteur d'écran.
            visibility: isCollapsed ? "hidden" : undefined,
          }}
          // Repli en cours ou fait : compté à sa hauteur finale (nulle) par
          // la liste des messages (useMessageAutoscroll).
          data-folded={isCollapsed ? "" : undefined}
        >
          <pre className="overflow-x-auto rounded-md border border-neutral-200 bg-neutral-50 p-2 text-xs text-neutral-800 dark:border-neutral-700 dark:bg-neutral-900 dark:text-neutral-200">
            <code>
              {typeof result === "string"
                ? result
                : JSON.stringify(result, null, 2)}
            </code>
          </pre>
        </div>
      </div>
    </div>
  );
}
