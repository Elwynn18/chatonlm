// Carte « Support ChatonLM » de Settings › General, la dernière de la page
// (propre à ChatonLM) : à quoi servent les dons (continuer le développement,
// louer des serveurs pour que la recherche web ne dépende plus d'Ollama) et
// le bouton qui ouvre la page des dons dans le navigateur. Cette adresse du
// site est redirigée vers la plateforme de dons : on peut en changer sans
// nouvelle version de l'app. Échec de l'ouverture : l'avis commun des liens
// (LinkErrorNotice) donne la raison et l'adresse. « Reset to defaults » n'y
// touche pas (aucun réglage).

import { HeartIcon } from "@heroicons/react/20/solid";
import { useId } from "react";
import { Row } from "@/components/settings/Row";
import { useT } from "@/i18n";
import { openLink } from "@/lib/openLink";

/** Page des dons du site, redirigée vers la plateforme choisie. */
export const DONATE_URL = "https://chatonlm.com/donate";

// Aplat roux, toujours avec un texte brun foncé ; focus : le contour décalé de
// la feuille de style (un anneau collé serait invisible en sombre).
const DONATE_BUTTON =
  "inline-flex flex-none cursor-pointer items-center rounded-lg bg-accent px-3 py-1 text-sm/6 font-medium text-neutral-900 shadow-sm hover:bg-accent/85";

export function Donate() {
  const t = useT();
  const titleId = useId();
  const descriptionId = useId();
  return (
    <section
      aria-labelledby={titleId}
      data-testid="donate"
      className="overflow-hidden rounded-xl bg-white dark:bg-neutral-800"
    >
      <div className="flex flex-wrap items-start justify-between gap-4 p-4">
        <Row
          icon={HeartIcon}
          label={
            <h2 id={titleId} className="font-medium">
              {t("settings.donate.title")}
            </h2>
          }
          description={t("settings.donate.description")}
          descriptionId={descriptionId}
        />
        <button
          type="button"
          aria-describedby={descriptionId}
          className={DONATE_BUTTON}
          onClick={() => void openLink(DONATE_URL)}
        >
          {t("settings.donate.button")}
        </button>
      </div>
    </section>
  );
}
