// Recherche web par le relais de ChatonLM (propre à ChatonLM) : erreurs de
// la recherche (WebSearchErrorActions.tsx), moteur qui a répondu
// (WebSearchAttribution.tsx), bulle du bouton selon l'état du relais
// (WebSearchButton.tsx).

import type { ChatErrorKind } from "@/bindings/ChatErrorKind";
import type { ToolCall } from "@/bindings/ToolCall";
import type { WebSearchStatus } from "@/bindings/WebSearchStatus";
import type { TFunction } from "@/i18n";

/** Erreurs de la recherche web. */
export type WebSearchErrorKind = Extract<
  ChatErrorKind,
  | "webSearchUnavailable"
  | "webSearchLimit"
  | "webSearchPaused"
  | "webSearchOutdated"
>;

export function isWebSearchError(
  kind: ChatErrorKind | undefined,
): kind is WebSearchErrorKind {
  return (
    kind === "webSearchUnavailable" ||
    kind === "webSearchLimit" ||
    kind === "webSearchPaused" ||
    kind === "webSearchOutdated"
  );
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

export const webSearchStatusKey = ["webSearchStatus"] as const;

/** Bulle du bouton allumé : état du relais et recherches restantes. */
export function statusTitle(
  t: TFunction,
  status: WebSearchStatus | undefined,
): string {
  switch (status?.state) {
    case "limit":
      return t("chat.webSearch.status.limit");
    case "paused":
    case "budgetExhausted":
      return t("chat.webSearch.status.paused");
    case "outdated":
      return t("chat.webSearch.status.outdated");
    case "unavailable":
      return t("chat.webSearch.status.unavailable");
    case "ok":
      if (status.remainingToday !== null) {
        return t("chat.webSearch.status.remaining", {
          count: status.remainingToday,
        });
      }
      return t("chat.webSearch.disable");
    default:
      return t("chat.webSearch.disable");
  }
}
