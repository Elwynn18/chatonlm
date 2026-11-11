// Mention « Powered by Brave Search » sous les résultats d'une recherche web
// faite par Brave (propre à ChatonLM : la recherche passe par son relais, qui
// dit quel moteur a répondu ; le résultat enregistré n'a que les titres, les
// liens et ce moteur). Discrète : une ligne sous le bloc des résultats, avec
// le lien vers la page « Recherche web » du site (conditions du service et
// données gardées), pour tout résultat du relais.

import type { ToolCall } from "@/bindings/ToolCall";
import { useLanguage, useT } from "@/i18n";
import { openLink } from "@/lib/openLink";
import { searchProvider } from "@/lib/webSearch";

/** Page « Recherche web » du site, dans la langue de l'app. */
export function webSearchPageUrl(lang: string): string {
  return `https://chatonlm.com/${lang}/web-search/`;
}

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
