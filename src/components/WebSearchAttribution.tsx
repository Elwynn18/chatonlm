// Mention « Powered by Brave Search » sous les résultats d'une recherche web
// faite par Brave (propre à ChatonLM : la recherche est faite depuis cet
// ordinateur, par DuckDuckGo ou le moteur personnel choisi ; le résultat
// enregistré n'a que les titres, les liens et le moteur qui a répondu).
// Discrète : une ligne sous le bloc des résultats, avec le lien vers la page
// « Recherche web » du site, pour toute recherche qui dit son moteur.

import type { ToolCall } from "@/bindings/ToolCall";
import { useLanguage, useT } from "@/i18n";
import { openLink } from "@/lib/openLink";
import { searchProvider, webSearchPageUrl } from "@/lib/webSearch";

export function WebSearchAttribution({ call }: { call: ToolCall }) {
  const t = useT();
  const lang = useLanguage();
  const provider = searchProvider(call);
  if (provider === null) return null;
  const url = webSearchPageUrl(lang);
  return (
    <p
      data-testid="web-search-attribution"
      className="mt-1 ml-6 text-xs text-neutral-500 dark:text-neutral-400"
    >
      {provider === "brave" && (
        <>
          <span>{t("chat.webSearch.poweredByBrave")}</span>
          <span aria-hidden="true"> · </span>
        </>
      )}
      <a
        href={url}
        className="underline underline-offset-2 hover:text-neutral-700 dark:hover:text-neutral-200"
        onClick={(event) => {
          event.preventDefault();
          void openLink(url);
        }}
      >
        {t("chat.webSearch.about")}
      </a>
    </p>
  );
}
