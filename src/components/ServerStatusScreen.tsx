import { ArrowPathIcon } from "@heroicons/react/20/solid";
import { useRef, useState } from "react";
import { flushSync } from "react-dom";
import type { OllamaRuntime } from "@/bindings/OllamaRuntime";
import type { OllamaSource } from "@/bindings/OllamaSource";
import type { ServerStatus } from "@/bindings/ServerStatus";
import type { ServiceStart } from "@/bindings/ServiceStart";
import { AddressField } from "@/components/AddressField";
import { CommandBlock } from "@/components/CommandBlock";
import { Logo } from "@/components/Logo";
import { useT } from "@/i18n";
import { Trans } from "@/i18n/Trans";
import { leaveBuiltinText } from "@/lib/ollamaRuntime";
import { isWindows } from "@/lib/platform";
import { DEFAULT_ADDRESS, DOWNLOAD_URL, adviceFor } from "@/lib/serverAdvice";

interface Props {
  status: ServerStatus;
  /** Adresse enregistrée dans les réglages ; vide = automatique. */
  savedUrl: string;
  /** Vérification demandée par Retry en cours. */
  isChecking: boolean;
  onRetry: () => void;
  /** Enregistre une adresse (chaîne vide : automatique) ; rejette si invalide. */
  onSaveAddress: (url: string) => Promise<void>;
  /** Ouvre un lien dans le navigateur ; rejette en cas d'échec. */
  onOpen: (url: string) => Promise<void>;
  /**
   * Ollama qui sert, quand c'est l'intégré (réglage `source`) ; absent :
   * celui de la machine.
   */
  runtime?: OllamaRuntime | null;
  /** Échec du dernier Retry (relance de l'intégré). */
  retryError?: string | null;
  /** Démarre le service arrêté (« Start Ollama ») ; rejette en cas d'échec. */
  onStart?: () => Promise<ServiceStart>;
  /** Ouvre le journal de l'intégré ; rejette en cas d'échec. */
  onOpenLog?: () => Promise<void>;
  /** Change le réglage `source` ; rejette en cas d'échec. */
  onSetSource?: (source: OllamaSource) => Promise<void>;
}

const message = (cause: unknown) =>
  cause instanceof Error ? cause.message : String(cause);

const primaryButton =
  "h-10 min-w-28 rounded-full bg-neutral-900 px-6 text-sm font-medium text-white outline-none hover:bg-neutral-700 focus-visible:ring-2 focus-visible:ring-accent-fg focus-visible:ring-offset-2 disabled:opacity-60 dark:bg-white dark:text-neutral-900 dark:hover:bg-neutral-200 dark:focus-visible:ring-offset-neutral-900";

const secondaryButton =
  "h-10 min-w-28 rounded-full border border-neutral-300 px-6 text-sm font-medium text-neutral-900 outline-none hover:bg-neutral-100 focus-visible:ring-2 focus-visible:ring-accent-fg focus-visible:ring-offset-2 disabled:opacity-60 dark:border-neutral-600 dark:text-neutral-100 dark:hover:bg-neutral-800 dark:focus-visible:ring-offset-neutral-900";

const textButton =
  "rounded font-medium text-neutral-900 underline-offset-4 outline-none hover:underline focus-visible:ring-2 focus-visible:ring-accent-fg disabled:opacity-60 dark:text-neutral-100";

/**
 * Écran d'accueil quand Ollama ne répond pas à l'adresse configurée, ou que
 * l'Ollama intégré démarre ou a échoué. « Start Ollama » démarre un service
 * arrêté ; un lien passe à l'Ollama intégré (quand ce paquet en a un), un
 * autre en revient quand il a été choisi. Le bouton cliqué disparaît après
 * un démarrage ou une bascule réussis : le focus passe au titre, qui dit le
 * nouvel état ; après un échec, il revient au bouton.
 */
export function ServerStatusScreen({
  status,
  savedUrl,
  isChecking,
  onRetry,
  onSaveAddress,
  onOpen,
  runtime = null,
  retryError = null,
  onStart,
  onOpenLog,
  onSetSource,
}: Props) {
  const t = useT();
  const advice = adviceFor(t, status);
  const [openError, setOpenError] = useState<string | null>(null);
  const [starting, setStarting] = useState(false);
  // Issue de « Start Ollama » autre qu'un démarrage (écrite au rendu).
  const [startIssue, setStartIssue] = useState<Exclude<
    ServiceStart,
    { kind: "started" }
  > | null>(null);
  const [logError, setLogError] = useState<string | null>(null);
  // L'intégré a été choisi (réglage `source`) : l'adresse ne sert plus.
  const builtinChosen = status.builtin && runtime?.source === "builtin";
  const canStart = !!advice.start && !!onStart;
  const titleRef = useRef<HTMLHeadingElement>(null);
  const startRef = useRef<HTMLButtonElement>(null);
  const focusTitle = () => titleRef.current?.focus();

  const start = async () => {
    if (!onStart || starting) return;
    setStarting(true);
    setStartIssue(null);
    let started = false;
    try {
      const outcome = await onStart();
      if (outcome.kind === "started") started = true;
      else setStartIssue(outcome);
    } catch (cause) {
      setStartIssue({ kind: "failed", detail: message(cause) });
    } finally {
      // Réactivé tout de suite, pour pouvoir lui rendre le focus.
      flushSync(() => setStarting(false));
    }
    // Désactivé pendant le démarrage, le bouton a pu perdre le focus.
    if (started) focusTitle();
    else startRef.current?.focus();
  };

  const openDownload = async () => {
    setOpenError(null);
    try {
      await onOpen(DOWNLOAD_URL);
    } catch (cause) {
      setOpenError(message(cause));
    }
  };

  return (
    <main className="flex h-screen overflow-y-auto bg-white px-6 dark:bg-neutral-900">
      <div className="m-auto w-full max-w-md py-10 text-center">
        <Logo />
        <div role="status" aria-live="polite">
          <h1
            ref={titleRef}
            tabIndex={-1}
            className="text-xl font-medium text-neutral-900 outline-none dark:text-neutral-100"
          >
            {advice.title}
          </h1>
          <p className="mt-2 text-sm text-neutral-500 dark:text-neutral-400">
            {advice.description}
          </p>
        </div>
        {status.detail && (
          <p className="mt-2 text-xs break-words text-neutral-500">
            {status.detail}
          </p>
        )}
        {advice.waiting && (
          <ArrowPathIcon
            aria-hidden="true"
            className="mx-auto mt-5 h-5 w-5 animate-spin text-neutral-500"
          />
        )}
        {advice.log && status.logPath && (
          <div className="mt-3 flex items-center gap-3 rounded-xl bg-neutral-100 py-2 pr-2 pl-4 text-left dark:bg-neutral-800">
            <p className="min-w-0 flex-1 font-mono text-[13px] break-all text-neutral-800 select-text dark:text-neutral-200">
              {status.logPath}
            </p>
            {onOpenLog && (
              <button
                type="button"
                onClick={() => {
                  setLogError(null);
                  onOpenLog().catch((cause: unknown) =>
                    setLogError(message(cause)),
                  );
                }}
                className="shrink-0 rounded-lg px-2.5 py-1 text-xs font-medium text-neutral-600 outline-none hover:bg-neutral-200 focus-visible:ring-2 focus-visible:ring-accent-fg dark:text-neutral-300 dark:hover:bg-neutral-700"
              >
                {t("settings.ollama.openLog")}
              </button>
            )}
          </div>
        )}
        {logError && (
          <p
            role="alert"
            className="mt-2 text-xs break-words text-red-600 dark:text-red-400"
          >
            {t("settings.ollama.openLogError", { message: logError })}
          </p>
        )}

        {advice.commands.length > 0 && (
          <div className="mt-5 space-y-3">
            {advice.commands.map(({ label, command }) => (
              <div key={command} className="space-y-1.5">
                {label && (
                  <p className="text-left text-xs text-neutral-500 dark:text-neutral-400">
                    {label}
                  </p>
                )}
                <CommandBlock command={command} />
              </div>
            ))}
          </div>
        )}

        <div className="mt-6 flex flex-wrap items-center justify-center gap-4">
          {canStart && (
            <button
              ref={startRef}
              type="button"
              onClick={() => void start()}
              disabled={starting}
              className={primaryButton}
            >
              {starting ? t("chat.server.starting") : t("chat.server.start")}
            </button>
          )}
          {!advice.waiting && (
            <button
              type="button"
              onClick={onRetry}
              disabled={isChecking}
              className={canStart ? secondaryButton : primaryButton}
            >
              {isChecking
                ? t("chat.actions.checking")
                : t("common.actions.retry")}
            </button>
          )}
          {advice.download && (
            <a
              href={DOWNLOAD_URL}
              onClick={(event) => {
                event.preventDefault();
                void openDownload();
              }}
              // Le menu de WebKit proposerait « Download Linked File ».
              onContextMenu={(event) => event.preventDefault()}
              className={`text-sm ${textButton}`}
            >
              {t("chat.server.download")}
            </a>
          )}
        </div>
        {retryError && (
          <p
            role="alert"
            className="mt-3 text-xs break-words text-red-600 dark:text-red-400"
          >
            {t("chat.server.startFailed", { message: retryError })}
          </p>
        )}
        {startIssue && (
          <p
            role="alert"
            className="mt-3 text-xs break-words text-red-600 dark:text-red-400"
          >
            {startIssue.kind === "failed"
              ? t("chat.server.startFailed", { message: startIssue.detail })
              : startIssue.kind === "notAuthorized"
                ? t("chat.server.notAuthorized")
                : // Windows : pas de service, l'app de bureau d'Ollama.
                  isWindows()
                  ? t("chat.server.noApp")
                  : t("chat.server.noService")}
          </p>
        )}
        {openError && (
          <p
            role="alert"
            className="mt-3 text-xs break-words text-red-600 dark:text-red-400"
          >
            <Trans
              k="chat.linkError.open"
              params={{ detail: openError }}
              tags={{ url: () => DOWNLOAD_URL }}
            />
          </p>
        )}

        {advice.builtin && onSetSource && (
          <SourceSwitch
            label={
              advice.builtin === "instead"
                ? t("settings.ollama.useBuiltinInstead")
                : t("settings.ollama.useBuiltin")
            }
            onSwitch={() => onSetSource("builtin")}
            onSwitched={focusTitle}
          />
        )}
        {builtinChosen && runtime && onSetSource && (
          <SourceSwitch
            label={leaveBuiltinText(t, runtime)}
            onSwitch={() => onSetSource("auto")}
            onSwitched={focusTitle}
          />
        )}

        {status.alternativeUrl && !status.builtin && (
          <AlternativeAddress
            url={status.alternativeUrl}
            onUse={onSaveAddress}
          />
        )}

        {!builtinChosen && (
          <AddressForm
            key={savedUrl}
            saved={savedUrl}
            // L'adresse de l'intégré est privée : celle qu'un champ vide donne
            // à l'Ollama de la machine.
            effective={status.builtin ? DEFAULT_ADDRESS : status.url}
            onSave={onSaveAddress}
          />
        )}
      </div>
    </main>
  );
}

/**
 * Passage à l'Ollama intégré, ou retour à celui de la machine ;
 * `onSwitched` après une bascule réussie (ce lien va disparaître).
 */
function SourceSwitch({
  label,
  onSwitch,
  onSwitched,
}: {
  label: string;
  onSwitch: () => Promise<void>;
  onSwitched: () => void;
}) {
  const t = useT();
  const [switching, setSwitching] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const buttonRef = useRef<HTMLButtonElement>(null);

  const run = async () => {
    if (switching) return;
    setSwitching(true);
    setError(null);
    let switched = false;
    try {
      await onSwitch();
      switched = true;
    } catch (cause) {
      setError(message(cause));
    } finally {
      // Réactivé tout de suite, pour pouvoir lui rendre le focus.
      flushSync(() => setSwitching(false));
    }
    if (switched) onSwitched();
    else buttonRef.current?.focus();
  };

  return (
    <div className="mt-4 text-sm">
      <button
        ref={buttonRef}
        type="button"
        onClick={() => void run()}
        disabled={switching}
        className={textButton}
      >
        {switching ? t("settings.ollama.switching") : label}
      </button>
      {error && (
        <p
          role="alert"
          className="mt-2 text-xs break-words text-red-600 dark:text-red-400"
        >
          {t("settings.ollama.switchError", { message: error })}
        </p>
      )}
    </div>
  );
}

/** Une autre adresse répond comme Ollama : la proposer. */
function AlternativeAddress({
  url,
  onUse,
}: {
  url: string;
  onUse: (url: string) => Promise<void>;
}) {
  const t = useT();
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const use = async () => {
    setSaving(true);
    setError(null);
    try {
      await onUse(url);
    } catch (cause) {
      setError(message(cause));
    } finally {
      setSaving(false);
    }
  };

  return (
    <div className="mt-6 rounded-xl border border-neutral-200 px-4 py-3 text-sm dark:border-neutral-700">
      <p className="text-neutral-600 dark:text-neutral-300">
        {t("chat.server.runningAt", { url })}
      </p>
      <button
        type="button"
        onClick={() => void use()}
        disabled={saving}
        className={`mt-2 ${textButton}`}
      >
        {saving ? t("chat.actions.saving") : t("chat.server.useAddress")}
      </button>
      {error && (
        <p role="alert" className="mt-2 text-xs text-red-600 dark:text-red-400">
          {error}
        </p>
      )}
    </div>
  );
}

function AddressForm({
  saved,
  effective,
  onSave,
}: {
  saved: string;
  effective: string;
  onSave: (url: string) => Promise<void>;
}) {
  const t = useT();
  return (
    <div className="mt-10 border-t border-neutral-200 pt-6 text-left dark:border-neutral-800">
      <label
        htmlFor="server-url"
        className="text-sm font-medium text-neutral-900 dark:text-neutral-100"
      >
        {t("chat.server.address")}
      </label>
      <div className="mt-2">
        <AddressField
          id="server-url"
          saved={saved}
          effective={effective}
          onSave={onSave}
          hint={t("chat.server.addressHint")}
        />
      </div>
    </div>
  );
}
