// Mention « Powered by Brave Search » sous les résultats d'une recherche web
// faite par Brave (propre à ChatonLM : la recherche passe par son relais, qui
// dit quel moteur a répondu ; le résultat enregistré n'a que les titres, les
// liens et ce moteur). Discrète : une ligne sous le bloc des résultats.

import type { ToolCall } from "@/bindings/ToolCall";
import { useT } from "@/i18n";
import { searchProvider } from "@/lib/webSearch";

export function WebSearchAttribution({ call }: { call: ToolCall }) {
  const t = useT();
  if (searchProvider(call) !== "brave") return null;
  return (
    <p
      data-testid="web-search-attribution"
      className="mt-1 ml-6 text-xs text-neutral-500 dark:text-neutral-400"
    >
      {t("chat.webSearch.poweredByBrave")}
    </p>
  );
}
