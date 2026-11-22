// « Erase my data » de Settings › General (propre à ChatonLM) : zone rouge à
// part, sous « Reset to defaults ». La boîte de confirmation dit ce qui est
// effacé et ce qui reste (focus sur « Cancel », Échap annule) ; son bouton
// rouge efface tout d'un coup (commande erase_user_data), sans redémarrer
// l'app : ce que la page a lu est oublié ou relu (état vide), la page
// oublie ce qu'elle gardait en mémoire (src/lib/forgetPage.ts), passe à une
// nouvelle conversation et l'annonce « Your data has been
// erased. » suit (avec les fichiers restés, s'il y en a). Pendant
// l'effacement, le bouton est occupé ; un échec est dit sous la description
// (la base est intacte, mais le travail en cours a pu être arrêté).

import { ArrowPathIcon, TrashIcon } from "@heroicons/react/20/solid";
import { useQueryClient } from "@tanstack/react-query";
import { useNavigate } from "@tanstack/react-router";
import { useId, useState } from "react";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { type TKey, useT } from "@/i18n";
import { forgetPageMemory } from "@/lib/forgetPage";
import { eraseUserData, errorMessage, isAppError } from "@/lib/ipc";
import { showNotice } from "@/lib/notice";

const ERASED: TKey[] = [
  "settings.general.erase.dialog.erased.chats",
  "settings.general.erase.dialog.erased.study",
  "settings.general.erase.dialog.erased.links",
  "settings.general.erase.dialog.erased.settings",
  "settings.general.erase.dialog.erased.logs",
];

const KEPT: TKey[] = [
  "settings.general.erase.dialog.kept.models",
  "settings.general.erase.dialog.kept.tools",
  "settings.general.erase.dialog.kept.language",
];

// Rouge des erreurs en aplat, texte crème dans les deux thèmes (4,5:1 au
// moins : scripts/theme-contrast.test.mjs).
const ERASE_BUTTON =
  "inline-flex flex-none items-center gap-2 rounded-lg bg-red-600 px-3 py-1 text-sm/6 font-medium text-white shadow-sm outline-none hover:bg-red-700 focus-visible:ring-2 focus-visible:ring-accent-fg disabled:cursor-not-allowed disabled:opacity-50";

export function EraseData() {
  const t = useT();
  const queryClient = useQueryClient();
  const navigate = useNavigate();
  const titleId = useId();
  const descriptionId = useId();
  const [confirming, setConfirming] = useState(false);
  const [erasing, setErasing] = useState(false);
  // Échec : message du moteur, et si le travail en cours a été arrêté avant
  // (tout refus autre qu'« invalid », donné avant d'arrêter quoi que ce soit).
  const [error, setError] = useState<{
    message: string;
    stopped: boolean;
  } | null>(null);

  const erase = async () => {
    setConfirming(false);
    setErasing(true);
    setError(null);
    let report;
    try {
      report = await eraseUserData();
    } catch (cause) {
      console.error("Failed to erase the user data:", cause);
      setError({
        message: errorMessage(cause),
        stopped: !(isAppError(cause) && cause.kind === "invalid"),
      });
      setErasing(false);
      return;
    }
    const leftovers = [
      ...report.leftovers,
      ...(report.browsingDataCleared
        ? []
        : [t("settings.general.erase.browsingData")]),
    ];
    // Mémoire de la page (Proofread, Étude, envois, brouillons…).
    forgetPageMemory();
    await navigate({ to: "/c/$chatId", params: { chatId: "new" } });
    // Ce que la page avait lu est oublié (pages quittées) ou relu (pages
    // affichées : listes vides, réglages par défaut). Pas de remise à zéro
    // des requêtes affichées : sans réglages lus, l'app se viderait le temps
    // de les relire (App.tsx).
    queryClient.removeQueries({ type: "inactive" });
    void queryClient.invalidateQueries();
    showNotice([
      { key: "settings.general.erase.done" },
      ...(leftovers.length > 0
        ? [
            {
              key: "settings.general.erase.leftovers" as const,
              params: { files: leftovers.join(", ") },
            },
          ]
        : []),
    ]);
    setErasing(false);
  };

  return (
    <section
      aria-labelledby={titleId}
      data-testid="erase-data"
      className="overflow-hidden rounded-xl border border-red-600/40 bg-white dark:border-red-400/40 dark:bg-neutral-800"
    >
      <div className="flex flex-wrap items-start justify-between gap-4 p-4">
        <div className="flex min-w-0 flex-1 items-start space-x-3">
          <TrashIcon
            aria-hidden
            className="mt-1 h-5 w-5 flex-shrink-0 text-red-600 dark:text-red-400"
          />
          <div className="w-full min-w-0">
            <h2
              id={titleId}
              className="text-sm/6 font-medium text-red-600 dark:text-red-400"
            >
              {t("settings.general.erase.title")}
            </h2>
            <p
              id={descriptionId}
              className="text-sm/6 text-zinc-500 dark:text-zinc-400"
            >
              {t("settings.general.erase.description")}
            </p>
            {error && (
              <p
                role="alert"
                className="mt-2 text-xs break-words text-red-600 dark:text-red-400"
              >
                {t(
                  error.stopped
                    ? "settings.general.erase.errorStopped"
                    : "settings.general.erase.error",
                  { message: error.message },
                )}
              </p>
            )}
          </div>
        </div>
        <button
          type="button"
          aria-describedby={descriptionId}
          aria-busy={erasing}
          disabled={erasing}
          onClick={() => setConfirming(true)}
          className={ERASE_BUTTON}
        >
          {erasing && (
            <ArrowPathIcon aria-hidden className="h-4 w-4 animate-spin" />
          )}
          {erasing
            ? t("settings.general.erase.erasing")
            : t("settings.general.erase.button")}
        </button>
      </div>
      {confirming && (
        <ConfirmDialog
          title={t("settings.general.erase.dialog.title")}
          message={
            <>
              <p className="font-medium">
                {t("settings.general.erase.dialog.erasedHeading")}
              </p>
              <ul className="space-y-1 pl-5 [list-style-type:disc]">
                {ERASED.map((key) => (
                  <li key={key}>{t(key)}</li>
                ))}
              </ul>
              <p className="font-medium">
                {t("settings.general.erase.dialog.keptHeading")}
              </p>
              <ul className="space-y-1 pl-5 [list-style-type:disc]">
                {KEPT.map((key) => (
                  <li key={key}>{t(key)}</li>
                ))}
              </ul>
              <p>{t("settings.general.erase.dialog.warning")}</p>
            </>
          }
          confirmLabel={t("settings.general.erase.dialog.confirm")}
          destructive
          initialFocus="cancel"
          onConfirm={() => void erase()}
          onCancel={() => setConfirming(false)}
        />
      )}
    </section>
  );
}
