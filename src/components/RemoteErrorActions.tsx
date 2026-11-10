// Actions sous une erreur d'un modèle distant (propre à ChatonLM ; remplace
// celles des modèles cloud d'Ollama) : modèles distants coupés, « Open
// settings » ; clé refusée, « Check the key » (réglages) puis Retry ; limite
// du fournisseur atteinte ou fournisseur injoignable, Retry (et les réglages
// pour la limite). Le texte de l'erreur, déjà traduit par le moteur, est
// au-dessus (ErrorMessage).

import { useT } from "@/i18n";
import type { RemoteErrorKind } from "@/lib/remoteModels";

const PRIMARY =
  "rounded-full bg-neutral-900 px-3 py-1 text-sm font-medium text-white outline-none hover:bg-neutral-700 focus-visible:ring-2 focus-visible:ring-accent-fg disabled:cursor-default disabled:opacity-50 dark:bg-white dark:text-neutral-900 dark:hover:bg-neutral-200";
const SECONDARY =
  "rounded-full border border-neutral-300 px-3 py-1 text-sm font-medium text-neutral-700 outline-none hover:bg-neutral-100 focus-visible:ring-2 focus-visible:ring-accent-fg disabled:cursor-default disabled:opacity-50 disabled:hover:bg-transparent dark:border-neutral-600 dark:text-neutral-200 dark:hover:bg-neutral-800";

export function RemoteErrorActions({
  kind,
  onOpenSettings,
  onRetry,
  retryDisabled = false,
}: {
  kind: RemoteErrorKind;
  /** Ouvre Settings › Models (modèles distants). */
  onOpenSettings: () => void;
  /** Relance du dernier message ; absente : rien à relancer. */
  onRetry?: () => void;
  retryDisabled?: boolean;
}) {
  const t = useT();
  const settings = (primary: boolean, label: "openSettings" | "checkKey") => (
    <button
      type="button"
      onClick={onOpenSettings}
      className={primary ? PRIMARY : SECONDARY}
    >
      {t(
        label === "checkKey"
          ? "chat.remote.checkKey"
          : "chat.remote.openSettings",
      )}
    </button>
  );
  const retry = (primary: boolean) =>
    onRetry && (
      <button
        type="button"
        onClick={onRetry}
        disabled={retryDisabled}
        className={primary ? PRIMARY : SECONDARY}
      >
        {t("common.actions.retry")}
      </button>
    );
  return (
    <div className="flex flex-wrap items-center gap-2">
      {kind === "remoteOff" && settings(true, "openSettings")}
      {kind === "remoteKey" && (
        <>
          {settings(true, "checkKey")}
          {retry(false)}
        </>
      )}
      {kind === "remoteLimit" && (
        <>
          {retry(true)}
          {settings(false, "openSettings")}
        </>
      )}
      {kind === "remoteUnreachable" && retry(true)}
    </div>
  );
}
