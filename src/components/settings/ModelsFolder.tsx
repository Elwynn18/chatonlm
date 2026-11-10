// Settings › General, propre à ChatonLM : contenu de la ligne « Model
// location » (lot L5b, à la place de ModelLocation.tsx, qui changeait le
// dossier d'Ollama). Le libellé et la description officiels restent ceux de
// la ligne dans Settings.tsx. Dossier des modèles de llama.cpp : son chemin
// (« Default folder » s'il n'a pas été changé), ses modèles et leur taille,
// la place libre ; « Change… » ouvre le sélecteur natif (annulé : rien),
// puis une confirmation quand il y a des modèles : les déplacer, utiliser le
// dossier tel quel (seulement s'il les a déjà) ou annuler ; sans modèle,
// aucune question. Le moteur change le dossier, avec la progression des
// copies : c'est lui qui décharge et gèle les modèles, une fois le changement
// accepté (la page ne décharge rien : un refus ne coupe aucune réponse) ; « Saved » ensuite, avec un avis si des copies n'ont pu être
// effacées de l'ancien dossier. Un refus du moteur (téléchargement ou
// reprise en cours, dossier refusé) est dit tel quel sous la ligne.

import { ArrowPathIcon, FolderIcon } from "@heroicons/react/20/solid";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Channel } from "@tauri-apps/api/core";
import { useEffect, useRef, useState } from "react";
import type { EngineModelsDir } from "@/bindings/EngineModelsDir";
import type { FolderEvent } from "@/bindings/FolderEvent";
import type { FolderMoved } from "@/bindings/FolderMoved";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { hubKey } from "@/hooks/useHub";
import { modelsKey } from "@/hooks/useModels";
import { runningModelsKey } from "@/hooks/useRunningModels";
import { type TFunction, useT } from "@/i18n";
import { formatBytes, formatPercent } from "@/i18n/format";
import { engineModelsDirKey, folderSummary } from "@/lib/engine";
import {
  changeEngineModelsDir,
  engineModelsDir,
  errorMessage,
  isAppError,
  pickModelsFolder,
} from "@/lib/ipc";

const MUTED = "text-xs break-words text-neutral-600 dark:text-neutral-300";
const ERROR = "text-xs break-words text-red-600 dark:text-red-400";

type Phase =
  | { kind: "idle" }
  | { kind: "confirm"; path: string; current: EngineModelsDir }
  | {
      kind: "running";
      move: boolean;
      total: number | null;
      done: number;
    };

type Running = Extract<Phase, { kind: "running" }>;

/** Texte écrit au rendu, dans la langue affichée à ce moment. */
type Notice = { error: boolean; text: (t: TFunction) => string };

/** Pourcentage copié (0 à 100). */
function percentOf(phase: Running): number {
  if (!phase.total) return 0;
  return Math.min(100, Math.floor((phase.done / phase.total) * 100));
}

/** Étape en cours, seule annoncée aux lecteurs d'écran. */
function stepText(t: TFunction, phase: Running): string {
  return phase.move && phase.total
    ? t("settings.modelsFolder.moving")
    : t("settings.modelsFolder.changing");
}

/** Octets copiés et pourcentage, pendant le déplacement seulement. */
function bytesText(t: TFunction, phase: Running): string | null {
  if (!phase.move || !phase.total) return null;
  return t("settings.modelsFolder.progress", {
    done: formatBytes(t, phase.done),
    total: formatBytes(t, phase.total),
    percent: formatPercent(t.lang, percentOf(phase)),
  });
}

function apply(phase: Running, event: FolderEvent): Running {
  switch (event.kind) {
    case "total":
      return { ...phase, total: event.bytes, done: 0 };
    case "progress":
      return { ...phase, done: event.done };
  }
}

/** Échec : un refus du moteur tel quel, sinon dans une phrase. */
function failure(cause: unknown): Notice {
  const message = errorMessage(cause);
  if (isAppError(cause) && cause.kind === "invalid") {
    return { error: true, text: () => message };
  }
  return {
    error: true,
    text: (t) => t("settings.modelsFolder.error", { message }),
  };
}

/**
 * Champ « Model location » et bouton « Change… » (le libellé et la
 * description sont ceux de la ligne de Settings). `onSaved` : badge « Saved »
 * de la page.
 */
export function ModelsFolder({
  inputId,
  describedBy,
  onSaved,
}: {
  inputId: string;
  describedBy: string;
  onSaved: () => void;
}) {
  const t = useT();
  const queryClient = useQueryClient();
  const { data: dir, error } = useQuery({
    queryKey: engineModelsDirKey,
    queryFn: engineModelsDir,
  });
  const [phase, setPhase] = useState<Phase>({ kind: "idle" });
  const [notice, setNotice] = useState<Notice | null>(null);
  // Une réponse peut arriver après la sortie de la page.
  const mounted = useRef(false);
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);

  const busy = phase.kind !== "idle";

  const change = async (path: string, move: boolean, old: string) => {
    setPhase({ kind: "running", move, total: null, done: 0 });
    let moved: FolderMoved;
    try {
      // Le moteur décharge lui-même les modèles, seulement s'il accepte.
      const channel = new Channel<FolderEvent>((event) => {
        if (!mounted.current) return;
        setPhase((current) =>
          current.kind === "running" ? apply(current, event) : current,
        );
      });
      moved = await changeEngineModelsDir(path, move, channel);
    } catch (cause) {
      if (!mounted.current) return;
      setPhase({ kind: "idle" });
      setNotice(failure(cause));
      return;
    }
    // Dossier, modèles installés, ModelHub et modèles chargés (déchargés par
    // le moteur) relus.
    for (const queryKey of [
      engineModelsDirKey,
      modelsKey,
      hubKey,
      runningModelsKey,
    ]) {
      void queryClient.invalidateQueries({ queryKey });
    }
    if (!mounted.current) return;
    setPhase({ kind: "idle" });
    onSaved();
    if (moved.cleanupFailed) {
      setNotice({
        error: false,
        text: (t) => t("settings.modelsFolder.cleanupFailed", { dir: old }),
      });
    }
  };

  const browse = async () => {
    if (!dir || busy) return;
    setNotice(null);
    let picked: string | null;
    try {
      picked = await pickModelsFolder();
    } catch (cause) {
      if (mounted.current) setNotice(failure(cause));
      return;
    }
    // Annulé, ou le même dossier : rien à faire.
    if (!picked || picked === dir.path || !mounted.current) return;
    if (dir.models > 0) {
      setPhase({ kind: "confirm", path: picked, current: dir });
    } else {
      void change(picked, true, dir.path);
    }
  };

  return (
    <div className="mt-3">
      <div className="flex gap-2">
        <input
          id={inputId}
          value={dir?.path ?? ""}
          readOnly
          aria-describedby={describedBy}
          className="h-9 min-w-0 flex-1 rounded-lg border border-neutral-300 bg-white px-3 text-sm text-neutral-900 outline-none focus-visible:border-accent-fg focus-visible:ring-2 focus-visible:ring-accent-fg/40 dark:border-neutral-700 dark:bg-neutral-800 dark:text-neutral-100"
        />
        <button
          type="button"
          onClick={() => void browse()}
          disabled={!dir || busy}
          className="flex h-9 items-center rounded-lg border border-neutral-300 px-2 text-sm font-medium text-neutral-800 outline-none hover:bg-neutral-100 focus-visible:ring-2 focus-visible:ring-accent-fg disabled:opacity-50 dark:border-neutral-700 dark:text-neutral-200 dark:hover:bg-neutral-800"
        >
          <FolderIcon aria-hidden="true" className="mr-1 h-4 w-4" />
          {t("settings.modelsFolder.change")}
        </button>
      </div>

      {dir && (
        <p
          data-testid="models-folder-summary"
          className="mt-1 flex flex-wrap gap-x-1.5 text-xs text-neutral-500 dark:text-neutral-400"
        >
          {folderSummary(t, dir).map((part, index) => (
            <span key={part} className="inline-flex gap-1.5">
              {index > 0 && <span aria-hidden="true">·</span>}
              {part}
            </span>
          ))}
        </p>
      )}

      {error && (
        <p role="alert" className={`mt-2 ${ERROR}`}>
          {t("settings.modelsFolder.error", { message: errorMessage(error) })}
        </p>
      )}

      {phase.kind === "running" && (
        <div className="mt-2 space-y-2">
          <p className={`flex items-center gap-2 ${MUTED}`}>
            <ArrowPathIcon
              aria-hidden="true"
              className="h-4 w-4 flex-none animate-spin text-neutral-500"
            />
            {/* L'étape seule est annoncée : les octets changent souvent. */}
            <span>
              <span aria-live="polite">{stepText(t, phase)}</span>
              {bytesText(t, phase) && ` ${bytesText(t, phase)}`}
            </span>
          </p>
          {bytesText(t, phase) && (
            <div
              role="progressbar"
              aria-label={t("settings.modelsFolder.movingLabel")}
              aria-valuemin={0}
              aria-valuemax={100}
              aria-valuenow={percentOf(phase)}
              aria-valuetext={bytesText(t, phase) ?? undefined}
              className="relative h-1.5 overflow-hidden rounded-full bg-neutral-200 dark:bg-neutral-700"
            >
              <div
                className="absolute top-0 left-0 h-full rounded-full bg-accent"
                style={{ width: `${percentOf(phase)}%` }}
              />
            </div>
          )}
        </div>
      )}

      {notice && (
        <p
          role={notice.error ? "alert" : undefined}
          aria-live={notice.error ? undefined : "polite"}
          className={`mt-2 ${notice.error ? ERROR : MUTED}`}
        >
          {notice.text(t)}
        </p>
      )}

      {phase.kind === "confirm" && (
        <ConfirmDialog
          title={t("settings.modelsFolder.confirm.title")}
          message={
            <>
              <p>
                {t("settings.modelsFolder.confirm.store", { path: phase.path })}
              </p>
              <p>
                {t("settings.modelsFolder.confirm.models", {
                  count: phase.current.models,
                  size: formatBytes(t, phase.current.bytes),
                  dir: phase.current.path,
                })}
              </p>
              <p>{t("settings.modelsFolder.confirm.useAsIs")}</p>
              <p>{t("settings.modelsFolder.confirm.unload")}</p>
            </>
          }
          onCancel={() => setPhase({ kind: "idle" })}
          actions={[
            {
              label: t("settings.modelsFolder.confirm.useFolder"),
              onSelect: () =>
                void change(phase.path, false, phase.current.path),
            },
            {
              label: t("settings.modelsFolder.confirm.moveModels"),
              onSelect: () => void change(phase.path, true, phase.current.path),
            },
          ]}
        />
      )}
    </div>
  );
}
