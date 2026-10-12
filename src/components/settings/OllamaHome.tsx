// Alerte de Settings > General (sous Model location) : le service systemd
// d'Ollama ne voit pas son dossier .ollama (ProtectHome=), d'où « permission
// denied » sur sa clé id_ed25519 à chaque téléchargement d'ollama.com et
// pour le compte Ollama. « Fix » le lui rend (une invite polkit, puis
// redémarrage d'Ollama) : en cours, « Fixing… » ; réussi, l'alerte disparaît
// et « Saved » s'affiche ; échoué, la raison et « Details ». Sans pkexec, ou
// sans agent d'authentification dans la session, les commandes à copier.
// Service qui tourne en root : une explication seulement (ni Fix ni
// commandes : un lien glissé à la place de .ollama lui donnerait n'importe
// quel dossier en écriture).
// Lien vers elle sous les erreurs qu'elle explique : OllamaHomeLink.

import {
  ArrowPathIcon,
  ExclamationTriangleIcon,
} from "@heroicons/react/20/solid";
import { useQueryClient } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { useEffect, useRef, useState } from "react";
import type { HiddenHome } from "@/bindings/HiddenHome";
import type { HomeFixFailure } from "@/bindings/HomeFixFailure";
import type { HomeFixOutcome } from "@/bindings/HomeFixOutcome";
import { CommandBlock } from "@/components/CommandBlock";
import {
  isHomeError,
  ollamaHomeKey,
  useOllamaHome,
} from "@/hooks/useOllamaHome";
import { serverDependentKeys, serverIdentityKeys } from "@/hooks/useSettings";
import { type TFunction, useT } from "@/i18n";
import { errorMessage, fixOllamaHome } from "@/lib/ipc";

function failureText(
  t: TFunction,
  reason: HomeFixFailure,
  hidden: HiddenHome,
): string {
  const { dir, user, hiddenRoot: root } = hidden;
  switch (reason) {
    case "noAgent":
      return t("settings.service.noAgent");
    case "notAuthorized":
      return t("settings.ollamaHome.failures.notAuthorized");
    case "unitChanged":
      return t("settings.service.unitChanged");
    case "notYours":
      return t("settings.ollamaHome.failures.notYours", { dir, user });
    case "cannotCreate":
      return t("settings.ollamaHome.failures.cannotCreate", { dir, user });
    case "configFailed":
      return t("settings.ollamaHome.failures.configFailed");
    case "overridden":
      return t("settings.ollamaHome.failures.overridden", { root });
    case "startFailed":
      return t("settings.ollamaHome.failures.startFailed");
    case "restoreFailed":
      return t("settings.service.restoreFailed");
    case "unexpected":
      return t("settings.ollamaHome.failures.unexpected");
  }
}

/** Message sous l'alerte, après « Fix » (écrit au rendu, dans la langue affichée). */
interface Notice {
  lines: ((t: TFunction) => string)[];
  commands?: string;
  details?: string;
}

const BUTTON =
  "flex h-8 flex-none items-center gap-1.5 rounded-lg border border-neutral-300 bg-white px-3 text-sm font-medium text-neutral-800 outline-none hover:bg-neutral-100 focus-visible:ring-2 focus-visible:ring-accent-fg disabled:opacity-50 dark:border-neutral-600 dark:bg-neutral-800 dark:text-neutral-200 dark:hover:bg-neutral-700";

/**
 * Alerte et « Fix » ; rien quand le service voit son dossier. `onSaved` :
 * badge « Saved » de la page.
 */
export function OllamaHomeAlert({ onSaved }: { onSaved: () => void }) {
  const t = useT();
  const queryClient = useQueryClient();
  const { data: hidden } = useOllamaHome();
  const [fixing, setFixing] = useState(false);
  const [notice, setNotice] = useState<Notice | null>(null);
  // Une réponse peut arriver après la sortie de la page.
  const mounted = useRef(false);

  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);

  if (!hidden) return null;

  const fix = async () => {
    if (fixing) return;
    setNotice(null);
    setFixing(true);
    let outcome: HomeFixOutcome;
    try {
      outcome = await fixOllamaHome();
    } catch (cause) {
      if (!mounted.current) return;
      setFixing(false);
      const message = errorMessage(cause);
      setNotice({
        lines: [(t) => t("settings.ollamaHome.fixError", { message })],
      });
      return;
    }
    if (outcome.kind !== "cancelled") {
      // Ollama a pu redémarrer (ou être remis) : modèles, santé et compte
      // relus ; l'alerte, relue d'abord, disparaît avant la fin de « Fixing… ».
      for (const queryKey of [...serverDependentKeys, ...serverIdentityKeys]) {
        if (queryKey !== ollamaHomeKey) {
          void queryClient.invalidateQueries({ queryKey });
        }
      }
      await queryClient.invalidateQueries({ queryKey: ollamaHomeKey });
    }
    if (!mounted.current) return;
    setFixing(false);
    switch (outcome.kind) {
      case "cancelled":
        return;
      case "done":
        if (outcome.changed) onSaved();
        return;
      case "failed": {
        const { reason } = outcome;
        setNotice({
          lines: [(t) => failureText(t, reason, hidden)],
          commands:
            reason === "noAgent" ? (hidden.commands ?? undefined) : undefined,
          details: outcome.detail || undefined,
        });
      }
    }
  };

  return (
    <div
      data-testid="ollama-home"
      className="mt-3 rounded-lg border border-amber-300 bg-amber-50 p-3 dark:border-amber-700/60 dark:bg-amber-950/30"
    >
      <div className="flex items-start gap-2">
        <ExclamationTriangleIcon
          aria-hidden="true"
          className="mt-0.5 h-4 w-4 flex-none text-amber-600 dark:text-amber-400"
        />
        <div className="min-w-0 flex-1 space-y-2">
          <p
            role="alert"
            className="text-sm break-words text-neutral-800 dark:text-neutral-100"
          >
            {t("settings.ollamaHome.alert", {
              dir: hidden.dir,
              root: hidden.hiddenRoot,
            })}
          </p>
          {hidden.user === "root" || hidden.commands === null ? (
            <p className="text-xs text-neutral-600 dark:text-neutral-300">
              {t("settings.ollamaHome.root")}
            </p>
          ) : hidden.canElevate ? (
            <div className="flex flex-wrap items-center gap-x-3 gap-y-1">
              <button
                type="button"
                onClick={() => void fix()}
                disabled={fixing}
                className={BUTTON}
              >
                {fixing && (
                  <ArrowPathIcon
                    aria-hidden="true"
                    className="h-4 w-4 animate-spin text-neutral-500"
                  />
                )}
                {fixing
                  ? t("settings.ollamaHome.fixing")
                  : t("settings.ollamaHome.fix")}
              </button>
              <span className="text-xs text-neutral-600 dark:text-neutral-300">
                {hidden.running
                  ? t("settings.ollamaHome.asksRestart")
                  : t("settings.ollamaHome.asks")}
              </span>
            </div>
          ) : (
            <>
              <p className="text-xs text-neutral-600 dark:text-neutral-300">
                {t("settings.ollamaHome.noPkexec")}
              </p>
              <CommandBlock command={hidden.commands} />
            </>
          )}
          {notice && (
            <div role="alert" className="space-y-2">
              {notice.lines
                .map((line) => line(t))
                .map((line) => (
                  <p
                    key={line}
                    className="text-xs break-words text-red-600 dark:text-red-400"
                  >
                    {line}
                  </p>
                ))}
              {notice.commands && <CommandBlock command={notice.commands} />}
              {notice.details && (
                <details className="text-xs text-neutral-600 dark:text-neutral-300">
                  <summary className="cursor-pointer select-none">
                    {t("settings.service.details")}
                  </summary>
                  <pre className="mt-1 overflow-x-auto rounded-lg bg-neutral-100 p-2 font-mono text-[12px] whitespace-pre-wrap select-text dark:bg-neutral-800">
                    {notice.details}
                  </pre>
                </details>
              )}
            </div>
          )}
        </div>
      </div>
    </div>
  );
}

/**
 * « Fix in Settings › General » sous une erreur que le dossier caché
 * explique (`error`), seulement si le service le cache vraiment.
 */
export function OllamaHomeLink({
  error,
  className = "text-sm",
}: {
  error: string | null | undefined;
  className?: string;
}) {
  const t = useT();
  const relevant = isHomeError(error);
  const { data: hidden } = useOllamaHome({ enabled: relevant });
  if (!relevant || !hidden) return null;
  return (
    <Link
      to="/settings"
      search={{}}
      data-testid="ollama-home-link"
      className={`font-medium text-neutral-800 underline underline-offset-2 outline-none hover:text-neutral-600 focus-visible:ring-2 focus-visible:ring-accent-fg dark:text-neutral-100 dark:hover:text-neutral-300 ${className}`}
    >
      {t("settings.ollamaHome.link")}
    </Link>
  );
}
