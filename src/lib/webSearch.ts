// Recherche web faite depuis cet ordinateur (propre à ChatonLM : DuckDuckGo
// par défaut, ou le moteur personnel choisi dans Settings › Web search) :
// erreurs de la recherche (WebSearchErrorActions.tsx), moteur qui a répondu
// (WebSearchAttribution.tsx).

import type { ChatErrorKind } from "@/bindings/ChatErrorKind";
import type { ToolCall } from "@/bindings/ToolCall";

/** Erreurs de la recherche web. */
export type WebSearchErrorKind = Extract<
  ChatErrorKind,
  "webSearchUnavailable" | "webSearchRefused"
>;

export function isWebSearchError(
  kind: ChatErrorKind | undefined,
): kind is WebSearchErrorKind {
  return kind === "webSearchUnavailable" || kind === "webSearchRefused";
}

/** Moteur qui a répondu à une recherche (`provider` du résultat), s'il est dit. */
export function searchProvider(call: ToolCall): string | null {
  if (call.name !== "web_search" || call.result === null) return null;
  try {
    const result = JSON.parse(call.result) as unknown;
    if (typeof result !== "object" || result === null) return null;
    const provider = (result as Record<string, unknown>).provider;
    return typeof provider === "string" ? provider : null;
  } catch {
    return null;
  }
}

/** Page « Recherche web » du site, dans la langue de l'app. */
export function webSearchPageUrl(lang: string): string {
  return `https://chatonlm.com/${lang}/web-search/`;
}
