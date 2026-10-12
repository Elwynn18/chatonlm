// Sources d'un cours (onglet Study) : documents et pages web (liens ouverts
// dans le navigateur), montrées avec le plan et à la fin du cours.

import { DocumentTextIcon, GlobeAltIcon } from "@heroicons/react/20/solid";
import type { StudySource } from "@/bindings/StudySource";
import { ExternalLink } from "@/components/ExternalLink";
import { useT } from "@/i18n";

/** Sources du cours : documents, pages web (liens). */
export function Sources({ sources }: { sources: StudySource[] }) {
  const t = useT();
  if (sources.length === 0) return null;
  return (
    <div className="mt-5">
      <h3 className="text-sm font-semibold text-neutral-900 dark:text-white">
        {t("study.sources.title")}
      </h3>
      <ul className="mt-2 space-y-1">
        {sources.map((source, index) => (
          <li
            key={`${source.url}-${index}`}
            className="flex items-center gap-2 text-sm text-neutral-700 dark:text-neutral-300"
          >
            {source.kind === "document" ? (
              <DocumentTextIcon
                aria-hidden="true"
                className="h-4 w-4 flex-none text-neutral-500"
              />
            ) : (
              <GlobeAltIcon
                aria-hidden="true"
                className="h-4 w-4 flex-none text-neutral-500"
              />
            )}
            {source.url ? (
              <ExternalLink
                href={source.url}
                className="min-w-0 truncate underline underline-offset-2"
              >
                {source.title || source.url}
              </ExternalLink>
            ) : (
              <span className="min-w-0 truncate">{source.title}</span>
            )}
          </li>
        ))}
      </ul>
    </div>
  );
}
