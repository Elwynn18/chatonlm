// Ligne « Loading model... » du chat (propre à ChatonLM : l'officiel n'a que
// les trois points) : le modèle est chargé en mémoire avant la réponse. Elle
// prend la place des trois points (MessageList) et a l'allure de l'en-tête
// « Thinking... » en cours (Thinking.tsx) : icône de 20 px à gauche du texte,
// mêmes couleurs. Éphémère : rien n'est gardé dans l'historique. Annoncée
// par la région role="status" de MessageList qui la contient.

import { CpuChipIcon } from "@heroicons/react/24/outline";
import { useT } from "@/i18n";

export function LoadingModel() {
  const t = useT();
  return (
    <div
      data-testid="loading-model"
      className="mt-2 mb-4 flex text-neutral-800 dark:text-neutral-200"
    >
      <span className="relative flex items-center">
        <CpuChipIcon
          aria-hidden="true"
          className="absolute top-1/2 -left-1 h-5 w-5 -translate-y-1/2"
        />
        <span className="ml-6">{t("chat.messages.loadingModel")}</span>
      </span>
    </div>
  );
}
