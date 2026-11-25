// Pastille de la carte Atelier (page Tools, propre à ChatonLM) : un point
// roux quand un projet calcule, « Needs you » en aplat roux quand un projet
// attend une réponse ; rien sinon. Tenue à jour par « workshop-event ». Le
// sens est dit en texte (nom lu de la carte), jamais par la seule couleur.

import {
  useWorkshopList,
  useWorkshopNotices,
} from "@/components/workshop/queries";
import { useT } from "@/i18n";
import { badgeOf } from "@/lib/workshop";

export function WorkshopBadge() {
  const t = useT();
  useWorkshopNotices();
  const list = useWorkshopList();
  const badge = badgeOf(list.data ?? []);
  if (badge === null) return null;
  if (badge === "needsYou") {
    return (
      <span
        data-testid="workshop-badge"
        data-badge={badge}
        className="rounded-full bg-accent px-2 py-0.5 text-xs font-medium text-neutral-900"
      >
        <span aria-hidden="true">{t("workshop.states.waiting")}</span>
        <span className="sr-only">{t("workshop.badge.needsYou")}</span>
      </span>
    );
  }
  return (
    <span
      data-testid="workshop-badge"
      data-badge={badge}
      className="inline-flex items-center"
    >
      <span aria-hidden="true" className="h-2 w-2 rounded-full bg-accent-fg" />
      <span className="sr-only">{t("workshop.badge.working")}</span>
    </span>
  );
}
