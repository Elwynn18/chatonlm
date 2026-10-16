// Bascule « Image » de la zone de saisie (propre à ChatonLM : l'app
// officielle ne crée pas d'images), à l'image du bouton de la recherche web
// (WebSearchButton.tsx) : active, le message part tel quel au modèle
// d'images choisi dans le ModelHub, sans modèle de langage. Montrée si le
// moteur d'images est livré ; un processeur sans les instructions qu'il lui
// faut (AVX2...) la garde grisée, inerte, avec la raison en infobulle.

import { PhotoIcon } from "@heroicons/react/24/outline";
import { useT } from "@/i18n";

export function ImageButton({
  isVisible,
  missingCpuFeatures = [],
  isActive,
  onToggle,
}: {
  isVisible: boolean;
  /** Instructions du processeur qui manquent au moteur : grisée, sans effet. */
  missingCpuFeatures?: readonly string[];
  isActive: boolean;
  onToggle: () => void;
}) {
  const t = useT();
  if (!isVisible) return null;

  if (missingCpuFeatures.length > 0) {
    const reason = t("chat.image.cpuUnsupported", {
      features: missingCpuFeatures.join(", "),
    });
    // aria-disabled plutôt que disabled : le bouton garde le focus et son
    // infobulle, qui dit pourquoi.
    return (
      <button
        type="button"
        aria-label={t("chat.image.label")}
        aria-disabled="true"
        aria-description={reason}
        title={reason}
        className="flex h-9 w-9 cursor-not-allowed items-center justify-center rounded-full border border-transparent bg-neutral-200 whitespace-nowrap text-neutral-500 opacity-50 select-none focus:outline-2 focus:outline-offset-2 focus:outline-accent-fg dark:bg-neutral-700 dark:text-neutral-400"
      >
        <PhotoIcon aria-hidden="true" className="h-5 w-5" strokeWidth={2} />
      </button>
    );
  }

  return (
    <button
      type="button"
      aria-label={t("chat.image.label")}
      aria-pressed={isActive}
      title={t(isActive ? "chat.image.disable" : "chat.image.enable")}
      onClick={onToggle}
      className={`flex h-9 w-9 cursor-pointer items-center justify-center rounded-full border border-transparent whitespace-nowrap transition-all select-none focus:outline-2 focus:outline-offset-2 focus:outline-accent-fg ${
        isActive
          ? "bg-accent text-neutral-900"
          : "bg-neutral-200 text-neutral-500 dark:bg-neutral-700 dark:text-neutral-400"
      }`}
    >
      <PhotoIcon aria-hidden="true" className="h-5 w-5" strokeWidth={2} />
    </button>
  );
}
