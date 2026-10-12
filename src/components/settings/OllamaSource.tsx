// Settings › General, propre à ChatonLM : ligne « Ollama » (lequel sert,
// celui de la machine ou l'Ollama intégré au paquet, avec sa version ; l'état
// de l'intégré qui sert : démarrage, échec avec son journal, Retry et Open
// log ; le bouton qui passe à l'autre), avis des modèles restés dans le
// dossier de l'intégré quand celui de la machine sert (nombre, taille,
// ouvrir le dossier, « Use the built-in Ollama » : nom accessible distinct
// de celui de la ligne, bascule commune aux deux boutons), et avis du dossier
// des modèles de l'intégré introuvable. L'adresse de l'intégré en lecture
// seule est dans Settings.tsx.

import {
  ArrowPathIcon,
  CpuChipIcon,
  InformationCircleIcon,
} from "@heroicons/react/20/solid";
import {
  useIsMutating,
  useMutation,
  useQueryClient,
} from "@tanstack/react-query";
import { useEffect, useRef, useState } from "react";
import type { OllamaRuntime } from "@/bindings/OllamaRuntime";
import type { OllamaSource } from "@/bindings/OllamaSource";
import { Row } from "@/components/settings/Row";
import { ollamaRuntimeKey, switchOllamaSource } from "@/hooks/useOllamaRuntime";
import { healthKey } from "@/hooks/useServerHealth";
import { useT } from "@/i18n";
import { formatBytes } from "@/i18n/format";
import {
  errorMessage,
  openBuiltinFolder,
  openBuiltinLog,
  restartBuiltinOllama,
} from "@/lib/ipc";
import { leaveBuiltinText, servingText } from "@/lib/ollamaRuntime";

const BUTTON =
  "flex h-8 flex-none items-center gap-1.5 rounded-lg border border-neutral-300 bg-white px-3 text-sm font-medium text-neutral-800 outline-none hover:bg-neutral-100 focus-visible:ring-2 focus-visible:ring-accent-fg disabled:opacity-50 dark:border-neutral-600 dark:bg-neutral-800 dark:text-neutral-200 dark:hover:bg-neutral-700";

const ERROR = "text-xs break-words text-red-600 dark:text-red-400";

/** Une réponse peut arriver après la sortie de la page. */
function useMounted() {
  const mounted = useRef(false);
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);
  return mounted;
}

/** Bascule en cours, où qu'elle ait été demandée sur la page. */
const switchKey = ["switchOllamaSource"] as const;

/**
 * Passage d'un Ollama à l'autre : « Switching… » sur le bouton cliqué
 * pendant le changement, les autres boutons de bascule désactivés (une
 * seule à la fois), « Saved » après, la raison d'un échec sinon.
 */
function useSwitch(onSaved: () => void) {
  const queryClient = useQueryClient();
  const mounted = useMounted();
  const busy = useIsMutating({ mutationKey: switchKey }) > 0;
  const { mutateAsync, isPending } = useMutation({
    mutationKey: switchKey,
    mutationFn: (source: OllamaSource) =>
      switchOllamaSource(queryClient, source),
  });
  const [error, setError] = useState<string | null>(null);

  const run = async (source: OllamaSource) => {
    if (busy) return;
    setError(null);
    try {
      await mutateAsync(source);
      if (mounted.current) onSaved();
    } catch (cause) {
      if (mounted.current) setError(errorMessage(cause));
    }
  };
  return { switching: isPending, busy, error, run };
}

function SwitchButton({
  label,
  name,
  switching,
  busy,
  onClick,
}: {
  label: string;
  /** Nom accessible, quand le libellé seul se répète sur la page. */
  name?: string;
  switching: boolean;
  /** Une bascule est en cours (ce bouton ou un autre). */
  busy: boolean;
  onClick: () => void;
}) {
  const t = useT();
  return (
    <button
      type="button"
      onClick={onClick}
      disabled={busy}
      aria-label={switching ? undefined : name}
      className={BUTTON}
    >
      {switching && (
        <ArrowPathIcon
          aria-hidden="true"
          className="h-4 w-4 animate-spin text-neutral-500"
        />
      )}
      {switching ? t("settings.ollama.switching") : label}
    </button>
  );
}

/**
 * Ligne « Ollama » : lequel sert, son état s'il s'agit de l'intégré
 * (démarrage, échec avec le journal et Retry qui le relance), et le passage
 * à l'autre. Rien tant que `runtime` n'est pas lu.
 */
export function OllamaRow({
  runtime,
  onSaved,
  descriptionId,
}: {
  runtime: OllamaRuntime | undefined;
  onSaved: () => void;
  descriptionId: string;
}) {
  const t = useT();
  const queryClient = useQueryClient();
  const mounted = useMounted();
  const { switching, busy, error, run } = useSwitch(onSaved);
  const [restarting, setRestarting] = useState(false);
  const [restartError, setRestartError] = useState<string | null>(null);
  const [openError, setOpenError] = useState<string | null>(null);
  if (!runtime) return null;

  const builtin = runtime.kind === "builtin";
  // État de l'intégré qui sert (relu toutes les 2 s pendant son démarrage).
  const state = builtin ? runtime.builtinState : null;

  // L'intégré en échec n'est relancé que sur demande.
  const restart = async () => {
    if (restarting) return;
    setRestarting(true);
    setRestartError(null);
    setOpenError(null);
    try {
      await restartBuiltinOllama();
    } catch (cause) {
      if (mounted.current) setRestartError(errorMessage(cause));
    }
    await Promise.all(
      [healthKey, ollamaRuntimeKey].map((queryKey) =>
        queryClient.invalidateQueries({ queryKey }),
      ),
    );
    if (mounted.current) setRestarting(false);
  };

  const openLog = async () => {
    setOpenError(null);
    try {
      await openBuiltinLog();
    } catch (cause) {
      if (mounted.current) setOpenError(errorMessage(cause));
    }
  };
  let action: { label: string; source: OllamaSource } | null = null;
  if (runtime.source === "builtin") {
    action = { label: leaveBuiltinText(t, runtime), source: "auto" };
  } else if (!builtin && runtime.builtinAvailable) {
    action = { label: t("settings.ollama.useBuiltin"), source: "builtin" };
  }

  return (
    <Row
      icon={CpuChipIcon}
      label={t("settings.general.ollama.label")}
      description={servingText(t, runtime)}
      descriptionId={descriptionId}
    >
      {builtin && runtime.source === "auto" && (
        <p className="text-sm/6 text-zinc-500 dark:text-zinc-400">
          {t("settings.ollama.autoBuiltin")}
        </p>
      )}
      {(state === "starting" || state === "stopped") && (
        <p
          role="status"
          className="mt-1 flex items-center gap-2 text-xs text-neutral-600 dark:text-neutral-300"
        >
          <ArrowPathIcon
            aria-hidden="true"
            className="h-4 w-4 animate-spin text-neutral-500"
          />
          {t("chat.advice.builtinStarting.title")}
        </p>
      )}
      {state === "failed" && (
        <div className="mt-1 space-y-2">
          <div role="alert" className={`${ERROR} space-y-1 select-text`}>
            <p>{t("settings.ollama.failed", { path: runtime.logPath })}</p>
            {/* Raison donnée par le moteur, dans la langue de l'échec. */}
            {runtime.builtinFailure && <p>{runtime.builtinFailure}</p>}
          </div>
          <div className="flex flex-wrap items-center gap-2">
            <button
              type="button"
              onClick={() => void restart()}
              disabled={restarting}
              className={BUTTON}
            >
              {restarting
                ? t("chat.server.starting")
                : t("common.actions.retry")}
            </button>
            <button
              type="button"
              onClick={() => void openLog()}
              className={BUTTON}
            >
              {t("settings.ollama.openLog")}
            </button>
          </div>
        </div>
      )}
      {restartError && (
        <p role="alert" className={`mt-2 ${ERROR}`}>
          {t("chat.server.startFailed", { message: restartError })}
        </p>
      )}
      {openError && (
        <p role="alert" className={`mt-2 ${ERROR}`}>
          {t("settings.ollama.openLogError", { message: openError })}
        </p>
      )}
      {action && (
        <div className="mt-3">
          <SwitchButton
            label={action.label}
            switching={switching}
            busy={busy}
            onClick={() => void run(action.source)}
          />
        </div>
      )}
      {error && (
        <p role="alert" className={`mt-2 ${ERROR}`}>
          {t("settings.ollama.switchError", { message: error })}
        </p>
      )}
    </Row>
  );
}

/**
 * Modèles restés dans le dossier de l'intégré pendant que l'Ollama de la
 * machine sert : leur nombre et leur taille, le dossier, « Open folder » et
 * « Use the built-in Ollama ». Rien sinon.
 */
export function BuiltinLeftoverNotice({
  runtime,
  onSaved,
}: {
  runtime: OllamaRuntime | undefined;
  onSaved: () => void;
}) {
  const t = useT();
  const mounted = useMounted();
  const { switching, busy, error, run } = useSwitch(onSaved);
  const [openError, setOpenError] = useState<string | null>(null);
  const leftover = runtime?.builtinModels;
  if (!runtime || runtime.kind !== "machine" || !leftover?.count) return null;

  const open = async () => {
    setOpenError(null);
    try {
      await openBuiltinFolder();
    } catch (cause) {
      if (mounted.current) setOpenError(errorMessage(cause));
    }
  };

  return (
    <div
      data-testid="builtin-leftover"
      className="mt-3 rounded-lg border border-neutral-200 p-3 dark:border-neutral-700"
    >
      <div className="flex items-start gap-2">
        <InformationCircleIcon
          aria-hidden="true"
          className="mt-0.5 h-4 w-4 flex-none text-neutral-500 dark:text-neutral-400"
        />
        <div className="min-w-0 flex-1 space-y-2">
          <p className="text-sm break-words text-neutral-800 dark:text-neutral-100">
            {t("settings.ollama.leftover", {
              count: leftover.count,
              size: formatBytes(t, leftover.bytes),
            })}{" "}
            {t("settings.ollama.leftoverHint")}
          </p>
          {runtime.builtinModelsDir && (
            <p className="font-mono text-xs break-all text-neutral-600 select-text dark:text-neutral-300">
              {runtime.builtinModelsDir}
            </p>
          )}
          <div className="flex flex-wrap items-center gap-2">
            <button
              type="button"
              onClick={() => void open()}
              className={BUTTON}
            >
              {t("settings.ollama.openFolder")}
            </button>
            {runtime.builtinAvailable && (
              <SwitchButton
                label={t("settings.ollama.useBuiltin")}
                // Même libellé que le bouton de la ligne Ollama : le nom dit
                // ce qu'apporte celui-ci (les modèles de l'avis).
                name={t("settings.ollama.useBuiltinModels", {
                  count: leftover.count,
                })}
                switching={switching}
                busy={busy}
                onClick={() => void run("builtin")}
              />
            )}
          </div>
          {openError && (
            <p role="alert" className={ERROR}>
              {t("settings.ollama.openFolderError", { message: openError })}
            </p>
          )}
          {error && (
            <p role="alert" className={ERROR}>
              {t("settings.ollama.switchError", { message: error })}
            </p>
          )}
        </div>
      </div>
    </div>
  );
}

/**
 * Dossier des modèles choisi pour l'intégré introuvable (disque débranché,
 * dossier supprimé) : l'intégré qui sert range ses modèles dans son dossier
 * par défaut, et le dit. Rien sinon.
 */
export function BuiltinModelsDirNotice({
  runtime,
}: {
  runtime: OllamaRuntime | undefined;
}) {
  const t = useT();
  if (runtime?.kind !== "builtin" || !runtime.builtinModelsDirMissing) {
    return null;
  }
  return (
    <p
      data-testid="builtin-dir-missing"
      className="mt-3 flex items-start gap-2 text-xs break-words text-amber-700 dark:text-amber-400"
    >
      <InformationCircleIcon
        aria-hidden="true"
        className="mt-px h-4 w-4 flex-none"
      />
      <span>{t("settings.ollama.modelsDirMissing")}</span>
    </p>
  );
}
