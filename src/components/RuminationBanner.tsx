import {
  BANNER_BOX as BOX,
  BANNER_PRIMARY as PRIMARY,
} from "@/components/bannerStyles";
import { useT } from "@/i18n";

/**
 * Le modèle semble tourner en rond pendant son raisonnement (propre à
 * ChatonLM, voir `src/lib/rumination.ts`) : bandeau au-dessus de la zone de
 * saisie, avec Stop. Rien n'est arrêté sans clic.
 */
export function RuminationBanner({ onStop }: { onStop: () => void }) {
  const t = useT();
  return (
    <div role="status" className={BOX}>
      <div className="flex items-center justify-between gap-3">
        <span>{t("chat.thinking.ruminating")}</span>
        <button type="button" onClick={onStop} className={PRIMARY}>
          {t("chat.composer.stop")}
        </button>
      </div>
    </div>
  );
}
