/*
 * SPDX-License-Identifier: MIT
 * SPDX-FileCopyrightText: Ollama
 * SPDX-FileCopyrightText: 2026 Elwynn18 and ChatonLM contributors
 *
 * Adapted from Ollama (https://github.com/ollama/ollama),
 * files app/ui/app/src/components/DisplayLogin.tsx,
 * app/ui/app/src/components/DisplayUpgrade.tsx and
 * app/ui/app/src/components/Chat.tsx (upgrade address by plan),
 * commit 42e911bc3d05.
 * Modified for ChatonLM: actions under the error of the message instead of
 * banners above the composer; Sign In opens the Account settings (the sign-in
 * page is opened there), then Retry once the account is linked; the upgrade
 * page is opened in the browser by the backend; a third case of ChatonLM's
 * own, cloud models turned off in the settings, leads to Settings › Models.
 */

import { useState } from "react";
import { useT } from "@/i18n";
import { type CloudErrorKind, upgradeUrl } from "@/lib/cloud";
import { errorMessage, openExternal } from "@/lib/ipc";

const PRIMARY =
  "rounded-full bg-neutral-900 px-3 py-1 text-sm font-medium text-white outline-none hover:bg-neutral-700 focus-visible:ring-2 focus-visible:ring-accent-fg disabled:cursor-default disabled:opacity-50 dark:bg-white dark:text-neutral-900 dark:hover:bg-neutral-200";

/**
 * Actions sous une erreur d'un modèle cloud : sans compte, « Sign In » (carte
 * du compte dans les réglages), puis « Retry » une fois le compte relié ;
 * limite d'usage atteinte, « Upgrade » (page d'ollama.com, celle de la
 * facturation pour un compte Pro) ; modèles cloud coupés, « Open settings »
 * (Settings › Models).
 */
export function CloudErrorActions({
  kind,
  onSignIn,
  onOpenSettings,
  onRetry,
  signedIn = false,
  plan,
  retryDisabled = false,
}: {
  kind: CloudErrorKind;
  /** Ouvre la carte du compte dans les réglages. */
  onSignIn: () => void;
  /** Ouvre l'onglet Models des réglages. */
  onOpenSettings: () => void;
  /** Relance du dernier message ; absente : rien à relancer. */
  onRetry?: () => void;
  /** Compte relié depuis l'erreur. */
  signedIn?: boolean;
  /** Forfait du compte relié (« free », « pro »...). */
  plan?: string;
  retryDisabled?: boolean;
}) {
  const t = useT();
  const [failure, setFailure] = useState<string | null>(null);
  let action;
  if (kind === "cloudOff") {
    action = (
      <button type="button" onClick={onOpenSettings} className={PRIMARY}>
        {t("chat.cloud.openSettings")}
      </button>
    );
  } else if (kind === "cloudLimit") {
    action = (
      <button
        type="button"
        onClick={() => {
          setFailure(null);
          openExternal(upgradeUrl(plan)).catch((cause: unknown) =>
            setFailure(errorMessage(cause)),
          );
        }}
        className={PRIMARY}
      >
        {t("chat.cloud.upgrade")}
      </button>
    );
  } else if (signedIn && onRetry) {
    action = (
      <button
        type="button"
        onClick={onRetry}
        disabled={retryDisabled}
        className={PRIMARY}
      >
        {t("common.actions.retry")}
      </button>
    );
  } else {
    action = (
      <button type="button" onClick={onSignIn} className={PRIMARY}>
        {t("chat.cloud.signIn")}
      </button>
    );
  }
  return (
    <div className="flex flex-col items-start gap-2">
      {action}
      {failure !== null && (
        <p className="text-xs break-words text-red-600 dark:text-red-400">
          {failure}
        </p>
      )}
    </div>
  );
}
