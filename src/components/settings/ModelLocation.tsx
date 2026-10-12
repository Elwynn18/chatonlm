/*
 * SPDX-License-Identifier: MIT
 * SPDX-FileCopyrightText: Ollama
 * SPDX-FileCopyrightText: 2026 Elwynn18 and ChatonLM contributors
 *
 * Adapted from Ollama (https://github.com/ollama/ollama),
 * file app/ui/app/src/components/Settings.tsx (Model Directory), commit
 * 42e911bc3d05.
 * Modified for ChatonLM: the field shows the folder of the Ollama server the
 * app talks to (systemd service, user service, manual launch), "On {host}"
 * for a remote server and "Unknown" in a container; the folder of the system
 * service is changed through polkit after a confirmation (move the models,
 * leave them, or cancel), with progress, "Saved" once done and a message with
 * "Details" on failure (everything put back by then); elsewhere (manual
 * launch, user service, no pkexec, a service that runs as root), the commands
 * to run are given with a Copy button, and Browse is disabled when nothing
 * can be done from here; an existing folder of another account is refused;
 * only the step of a change is announced to screen readers, not the bytes;
 * controls rewritten (no copy of the original ui/ components); texts in
 * English, Spanish and French (src/i18n), sizes written the way of the
 * language, the commands to copy left as they are. When ChatonLM's built-in
 * Ollama serves, the field shows its folder and Browse changes it directly
 * (no polkit): same confirmation (move the models, leave them, or cancel;
 * only "Change the model location?" when its folder has no model), same
 * progress, then the built-in Ollama restarts; another version of a model
 * already in the new folder, or a link in its folder, stops the move before
 * anything is moved, and is said. On Windows, the folder of Ollama's
 * desktop app is shown with guidance only (its own setting, or the
 * OLLAMA_MODELS variable of the account), without Browse; the texts that
 * name Linux permissions or path characters have a Windows wording.
 */

import { ArrowPathIcon, FolderIcon } from "@heroicons/react/20/solid";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Channel } from "@tauri-apps/api/core";
import { useEffect, useRef, useState } from "react";
import type { FolderCheck } from "@/bindings/FolderCheck";
import type { FolderProblem } from "@/bindings/FolderProblem";
import type { ModelsLocation } from "@/bindings/ModelsLocation";
import type { MoveEvent } from "@/bindings/MoveEvent";
import type { MoveFailure } from "@/bindings/MoveFailure";
import type { MoveOutcome } from "@/bindings/MoveOutcome";
import type { MoveStep } from "@/bindings/MoveStep";
import { CommandBlock } from "@/components/CommandBlock";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { modelsLocationKey, refreshTarget } from "@/hooks/useOllamaRuntime";
import { serverDependentKeys } from "@/hooks/useSettings";
import { type TFunction, type TKey, useT } from "@/i18n";
import { formatSizeValue, SIZE_UNITS } from "@/i18n/format";
import { isWindows } from "@/lib/platform";
import {
  changeBuiltinModelsDir,
  changeModelsLocation,
  checkModelsFolder,
  errorMessage,
  getModelsLocation,
  pickModelsFolder,
} from "@/lib/ipc";

type SystemLocation = Extract<ModelsLocation, { kind: "systemService" }>;

/**
 * Dossier changé par l'app : celui du service système (polkit), ou celui de
 * l'Ollama intégré (directement, puis redémarrage).
 */
type Target =
  | { kind: "system"; location: SystemLocation }
  | { kind: "builtin"; dir: string | null; running: boolean };

/** Dossier actuel de la cible. */
const targetDir = (target: Target) =>
  target.kind === "system" ? target.location.dir : target.dir;

/** Ollama tourne : il redémarrera. */
const targetRunning = (target: Target) =>
  target.kind === "system" ? target.location.running : target.running;

/** Exemple pour un conteneur (commande à copier : jamais traduite). */
const CONTAINER_EXAMPLE =
  "docker run -d -v /data/Models:/root/.ollama -p 11434:11434 --name ollama ollama/ollama";

/** Refus avant l'invite (`same` : rien à faire). */
const PROBLEMS: Record<Exclude<FolderProblem, "same">, TKey> = {
  system: "settings.modelLocation.problems.system",
  badCharacters: "settings.modelLocation.problems.badCharacters",
  notEmpty: "settings.modelLocation.problems.notEmpty",
  inside: "settings.modelLocation.problems.inside",
  missing: "settings.modelLocation.problems.missing",
  notYours: "settings.modelLocation.problems.notYours",
};

const K = 1024;

function unitIndex(bytes: number): number {
  if (bytes <= 0) return 0;
  return Math.min(
    Math.floor(Math.log(bytes) / Math.log(K)),
    SIZE_UNITS.length - 1,
  );
}

/**
 * Taille lisible, comme la progression d'un téléchargement (Downloading) :
 * base 1024, une décimale en GB et TB ; `unit` impose l'unité.
 */
function formatSize(
  t: TFunction,
  bytes: number,
  unit = unitIndex(bytes),
): string {
  const symbol = SIZE_UNITS[unit];
  const decimals = symbol === "GB" || symbol === "TB" ? 1 : 0;
  return formatSizeValue(t.lang, bytes / K ** unit, decimals, symbol);
}

/** Texte écrit au rendu, dans la langue affichée à ce moment. */
type Text = (t: TFunction) => string;

/** Message affiché sous le champ. */
interface Notice {
  /** Erreur : annoncée tout de suite (role alert). */
  error?: boolean;
  lines: Text[];
  /** Commandes à copier. */
  commands?: string | null;
  /** Sortie d'erreur du script, repliée sous « Details ». */
  details?: string;
}

type Phase =
  | { kind: "idle" }
  | { kind: "checking" }
  | {
      kind: "confirm";
      picked: string;
      check: FolderCheck;
      target: Target;
    }
  | {
      kind: "running";
      move: boolean;
      restarts: boolean;
      step: MoveStep | null;
      total: number | null;
      done: number;
    };

function fieldValue(
  t: TFunction,
  location: ModelsLocation | undefined,
): string {
  switch (location?.kind) {
    case "systemService":
    case "userService":
    case "manual":
    case "builtin":
    case "desktopApp":
      return location.dir ?? "";
    case "remote":
      return t("settings.modelLocation.remoteField", { host: location.host });
    case "container":
    case "unknown":
      return t("settings.modelLocation.unknownField");
    default:
      return "";
  }
}

function canBrowse(location: ModelsLocation | undefined): boolean {
  switch (location?.kind) {
    case "systemService":
      return location.envFile === null;
    case "userService":
    case "manual":
    case "builtin":
      return true;
    case "unknown":
      return location.running;
    default:
      return false;
  }
}

/** Ce que l'app ne peut pas faire d'ici, dit sans attendre Browse. */
function permanentNotice(location: ModelsLocation | undefined): Notice | null {
  switch (location?.kind) {
    case "container":
      return {
        lines: [(t) => t("settings.modelLocation.container")],
        commands: CONTAINER_EXAMPLE,
      };
    case "remote":
      return {
        lines: [
          (t) => t("settings.modelLocation.remote", { host: location.host }),
        ],
      };
    case "unknown":
      return location.running
        ? null
        : { lines: [(t) => t("settings.modelLocation.notRunning")] };
    case "desktopApp":
      // Windows : l'app de bureau d'Ollama choisit son dossier (son réglage
      // n'est pas lisible d'ici) ; guidage seul, pas de Browse.
      return { lines: [(t) => t("settings.modelLocation.desktopApp")] };
    case "systemService": {
      const file = location.envFile;
      return file === null
        ? null
        : { lines: [(t) => t("settings.modelLocation.envFile", { file })] };
    }
    default:
      return null;
  }
}

function failureText(
  t: TFunction,
  reason: MoveFailure,
  target: Target,
): string {
  if (target.kind === "builtin") return builtinFailureText(t, reason);
  const { location } = target;
  switch (reason.kind) {
    case "noAgent":
      return t("settings.service.noAgent");
    case "notAuthorized":
      return t("settings.modelLocation.failures.notAuthorized");
    case "refused":
      return t(PROBLEMS.system);
    case "notEmpty":
      return t(PROBLEMS.notEmpty);
    case "noSpace":
      return reason.need !== null && reason.free !== null
        ? t("settings.modelLocation.failures.noSpace", {
            need: formatSize(t, reason.need),
            free: formatSize(t, reason.free),
          })
        : t("settings.modelLocation.failures.noSpaceUnknown");
    case "cannotWrite":
      return t("settings.modelLocation.failures.cannotWrite", {
        user: location.user,
      });
    case "environmentFile":
      return reason.file === null
        ? t("settings.modelLocation.envFileUnknown")
        : t("settings.modelLocation.envFile", { file: reason.file });
    case "unitChanged":
      return t("settings.service.unitChanged");
    case "stopFailed":
      return t("settings.modelLocation.failures.stopFailed");
    case "moveFailed":
    case "configFailed":
      return t("settings.modelLocation.failures.putBack");
    case "overridden":
      return t("settings.modelLocation.failures.overridden");
    case "startFailed":
      return t("settings.modelLocation.failures.startFailed");
    case "restoreFailed":
      return t("settings.service.restoreFailed");
    case "unexpected":
      return t("settings.modelLocation.failures.unexpected");
    // Propres au déplacement de l'intégré.
    case "modelConflict":
    case "symlink":
      return builtinFailureText(t, reason);
  }
}

/** Échec d'un changement du dossier de l'intégré (ni service ni polkit). */
function builtinFailureText(t: TFunction, reason: MoveFailure): string {
  switch (reason.kind) {
    case "refused":
      return t(PROBLEMS.system);
    case "notEmpty":
      return t(PROBLEMS.notEmpty);
    case "noSpace":
      return reason.need !== null && reason.free !== null
        ? t("settings.modelLocation.failures.noSpace", {
            need: formatSize(t, reason.need),
            free: formatSize(t, reason.free),
          })
        : t("settings.modelLocation.failures.noSpaceUnknown");
    case "cannotWrite":
      // Les permissions de Linux n'ont pas de sens sous Windows.
      return t(
        isWindows()
          ? "settings.modelLocation.builtin.cannotWriteWindows"
          : "settings.modelLocation.builtin.cannotWrite",
      );
    case "stopFailed":
      return t("settings.modelLocation.failures.stopFailed");
    case "moveFailed":
    case "configFailed":
      return t("settings.modelLocation.failures.putBack");
    case "startFailed":
      return t("settings.modelLocation.builtin.startFailed");
    case "restoreFailed":
      return t("settings.modelLocation.builtin.restoreFailed");
    case "modelConflict":
      return t("settings.modelLocation.builtin.modelConflict", {
        model: reason.model,
      });
    case "symlink":
      return t("settings.modelLocation.builtin.symlink", { path: reason.path });
    default:
      return t("settings.modelLocation.failures.unexpected");
  }
}

/** Étape en cours, seule annoncée aux lecteurs d'écran. */
function stepKey(phase: Extract<Phase, { kind: "running" }>): TKey {
  if (phase.step === "start" && phase.restarts) {
    return "settings.modelLocation.restarting";
  }
  return phase.move
    ? "settings.modelLocation.moving"
    : "settings.modelLocation.changing";
}

/** Octets copiés, pendant le déplacement seulement. */
function bytesText(
  t: TFunction,
  phase: Extract<Phase, { kind: "running" }>,
): string | null {
  if (stepKey(phase) !== "settings.modelLocation.moving" || !phase.total) {
    return null;
  }
  const unit = unitIndex(phase.total);
  return t("settings.modelLocation.progress", {
    done: formatSize(t, phase.done, unit),
    total: formatSize(t, phase.total, unit),
  });
}

function progressText(
  t: TFunction,
  phase: Extract<Phase, { kind: "running" }>,
): string {
  const bytes = bytesText(t, phase);
  return bytes ? `${t(stepKey(phase))} ${bytes}` : t(stepKey(phase));
}

function apply(
  phase: Extract<Phase, { kind: "running" }>,
  event: MoveEvent,
): Phase {
  switch (event.type) {
    case "step":
      return { ...phase, step: event.step };
    case "total":
      return { ...phase, total: event.bytes, done: 0 };
    case "progress":
      return { ...phase, done: event.done };
  }
}

/**
 * Champ « Model location » et bouton « Browse » (le libellé et la
 * description sont ceux de la ligne de Settings). `serverUrl` : adresse
 * effective, dont dépend le dossier ; `onSaved` : badge « Saved » de la page.
 * Quand l'Ollama intégré sert, son dossier, changé sans polkit.
 */
export function ModelLocation({
  inputId,
  describedBy,
  serverUrl,
  onSaved,
}: {
  inputId: string;
  describedBy: string;
  serverUrl: string;
  onSaved: () => void;
}) {
  const t = useT();
  const queryClient = useQueryClient();
  const { data: location } = useQuery({
    queryKey: [...modelsLocationKey, serverUrl],
    queryFn: getModelsLocation,
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
  const failed = (cause: unknown) => {
    const message = errorMessage(cause);
    setNotice({
      error: true,
      lines: [(t) => t("settings.saveError", { message })],
    });
  };
  /** Consigne, puis les commandes à copier. */
  const commandsNotice = (key: TKey, commands: string | null) =>
    setNotice({ lines: [(t) => t(key)], commands });

  const browse = async () => {
    if (!location || busy) return;
    setNotice(null);
    let picked: string | null;
    try {
      picked = await pickModelsFolder();
    } catch (cause) {
      if (mounted.current) failed(cause);
      return;
    }
    if (!picked || !mounted.current) return;
    setPhase({ kind: "checking" });
    let check: FolderCheck;
    try {
      check = await checkModelsFolder(picked);
    } catch (cause) {
      if (!mounted.current) return;
      setPhase({ kind: "idle" });
      failed(cause);
      return;
    }
    if (!mounted.current) return;
    setPhase({ kind: "idle" });
    if (check.problem === "same") return;
    if (check.problem) {
      // Windows : « \ » et « : » font partie de tout chemin ; le moteur
      // refuse les caractères de contrôle et les chemins non Unicode.
      const problem =
        check.problem === "badCharacters" && isWindows()
          ? "settings.modelLocation.problems.badCharactersWindows"
          : PROBLEMS[check.problem];
      setNotice({ error: true, lines: [(t) => t(problem)] });
      return;
    }
    switch (location.kind) {
      case "builtin":
        if (check.downloadsRunning) {
          setNotice({
            error: true,
            lines: [(t) => t("settings.modelLocation.downloads")],
          });
        } else {
          setPhase({
            kind: "confirm",
            picked,
            check,
            // Il sert : le moteur le redémarre sur le nouveau dossier.
            target: { kind: "builtin", dir: location.dir, running: true },
          });
        }
        return;
      case "manual":
        commandsNotice("settings.modelLocation.manual", check.commands);
        return;
      case "userService":
        commandsNotice("settings.modelLocation.userService", check.commands);
        return;
      case "unknown":
        commandsNotice("settings.modelLocation.unknownRunning", check.commands);
        return;
      case "systemService":
        if (location.user === "root") {
          commandsNotice("settings.modelLocation.rootService", check.commands);
        } else if (!location.canElevate) {
          commandsNotice("settings.modelLocation.noPkexec", check.commands);
        } else if (check.downloadsRunning) {
          setNotice({
            error: true,
            lines: [(t) => t("settings.modelLocation.downloads")],
          });
        } else {
          setPhase({
            kind: "confirm",
            picked,
            check,
            target: { kind: "system", location },
          });
        }
        return;
      default:
        return;
    }
  };

  const change = async (
    picked: string,
    check: FolderCheck,
    target: Target,
    move: boolean,
  ) => {
    setPhase({
      kind: "running",
      move,
      restarts: targetRunning(target),
      step: null,
      total: null,
      done: 0,
    });
    const channel = new Channel<MoveEvent>((event) => {
      if (!mounted.current) return;
      setPhase((current) =>
        current.kind === "running" ? apply(current, event) : current,
      );
    });
    let outcome: MoveOutcome;
    try {
      outcome =
        target.kind === "builtin"
          ? await changeBuiltinModelsDir(picked, move, channel)
          : await changeModelsLocation(picked, move, channel);
    } catch (cause) {
      if (!mounted.current) return;
      setPhase({ kind: "idle" });
      failed(cause);
      return;
    }
    // Le service a pu redémarrer, ou être remis : dossier, modèles et santé
    // relus ; l'intégré a redémarré (adresse et version comprises).
    if (outcome.kind !== "cancelled") {
      if (target.kind === "builtin") {
        void refreshTarget(queryClient);
      } else {
        for (const queryKey of [modelsLocationKey, ...serverDependentKeys]) {
          void queryClient.invalidateQueries({ queryKey });
        }
      }
    }
    if (!mounted.current) return;
    setPhase({ kind: "idle" });
    switch (outcome.kind) {
      case "cancelled":
        return;
      case "done": {
        onSaved();
        const lines: Text[] = [];
        if (
          target.kind === "system" &&
          !outcome.restarted &&
          !target.location.running
        ) {
          lines.push((t) => t("settings.modelLocation.stoppedSaved"));
        }
        if (outcome.cleanupFailed) {
          const dir = check.old ?? targetDir(target) ?? "";
          lines.push((t) => t("settings.modelLocation.cleanupFailed", { dir }));
        }
        if (lines.length > 0) setNotice({ lines });
        return;
      }
      case "failed": {
        const { reason } = outcome;
        setNotice({
          error: true,
          lines: [(t) => failureText(t, reason, target)],
          commands: reason.kind === "noAgent" ? check.commands : undefined,
          details: outcome.detail || undefined,
        });
      }
    }
  };

  const permanent = permanentNotice(location);

  return (
    <div className="mt-3">
      <div className="flex gap-2">
        <input
          id={inputId}
          value={fieldValue(t, location)}
          readOnly
          aria-describedby={describedBy}
          className="h-9 min-w-0 flex-1 rounded-lg border border-neutral-300 bg-white px-3 text-sm text-neutral-900 outline-none focus-visible:border-accent-fg focus-visible:ring-2 focus-visible:ring-accent-fg/40 dark:border-neutral-700 dark:bg-neutral-800 dark:text-neutral-100"
        />
        <button
          type="button"
          onClick={() => void browse()}
          disabled={!canBrowse(location) || busy}
          className="flex h-9 items-center rounded-lg border border-neutral-300 px-2 text-sm font-medium text-neutral-800 outline-none hover:bg-neutral-100 focus-visible:ring-2 focus-visible:ring-accent-fg disabled:opacity-50 dark:border-neutral-700 dark:text-neutral-200 dark:hover:bg-neutral-800"
        >
          <FolderIcon aria-hidden="true" className="mr-1 h-4 w-4" />
          {t("settings.modelLocation.browse")}
        </button>
      </div>

      {phase.kind === "running" && (
        <div className="mt-2 space-y-2">
          <p className="flex items-center gap-2 text-xs text-neutral-600 dark:text-neutral-300">
            <ArrowPathIcon
              aria-hidden="true"
              className="h-4 w-4 animate-spin text-neutral-500"
            />
            {/* L'étape seule est annoncée : les octets changent deux fois par
                seconde. */}
            <span>
              <span aria-live="polite">{t(stepKey(phase))}</span>
              {bytesText(t, phase) && ` ${bytesText(t, phase)}`}
            </span>
          </p>
          {phase.move && phase.total ? (
            <div
              role="progressbar"
              aria-label={t("settings.modelLocation.movingLabel")}
              aria-valuemin={0}
              aria-valuemax={100}
              aria-valuenow={Math.floor((phase.done / phase.total) * 100)}
              aria-valuetext={progressText(t, phase)}
              className="relative h-1.5 overflow-hidden rounded-full bg-neutral-200 dark:bg-neutral-700"
            >
              <div
                className="absolute top-0 left-0 h-full rounded-full bg-accent"
                style={{
                  width: `${Math.min(100, (phase.done / phase.total) * 100)}%`,
                }}
              />
            </div>
          ) : null}
        </div>
      )}

      {permanent && <NoticeView notice={permanent} />}
      {notice && <NoticeView notice={notice} />}

      {phase.kind === "confirm" && (
        <Confirmation
          check={phase.check}
          target={phase.target}
          onChoose={(move) =>
            void change(phase.picked, phase.check, phase.target, move)
          }
          onCancel={() => setPhase({ kind: "idle" })}
        />
      )}
    </div>
  );
}

function NoticeView({ notice }: { notice: Notice }) {
  const t = useT();
  return (
    <div
      role={notice.error ? "alert" : undefined}
      aria-live={notice.error ? undefined : "polite"}
      className="mt-2 space-y-2"
    >
      {notice.lines
        .map((line) => line(t))
        .map((line) => (
          <p
            key={line}
            className={`text-xs break-words ${notice.error ? "text-red-600 dark:text-red-400" : "text-neutral-600 dark:text-neutral-300"}`}
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
  );
}

/** Confirmation avant l'invite : déplacer les modèles, les laisser, annuler. */
function Confirmation({
  check,
  target,
  onChoose,
  onCancel,
}: {
  check: FolderCheck;
  target: Target;
  onChoose: (move: boolean) => void;
  onCancel: () => void;
}) {
  const t = useT();
  const running = targetRunning(target);
  const old = targetDir(target) ?? check.old ?? "";
  const count = check.models?.count;
  // Des modèles à déplacer (ou peut-être : service arrêté, nombre inconnu).
  // L'intégré : comptés sur le disque, jamais inconnus ; sans nombre, rien à
  // déplacer.
  const movable =
    check.old !== null &&
    (target.kind === "builtin" ? (count ?? 0) > 0 : count !== 0);
  const path = check.path;
  const lines = [
    running
      ? t("settings.modelLocation.confirm.restartAndStore", { path })
      : t("settings.modelLocation.confirm.store", { path }),
  ];
  if (movable) {
    lines.push(
      check.models && count
        ? t("settings.modelLocation.confirm.models", {
            count,
            size: formatSize(t, check.models.bytes),
            dir: old,
          })
        : t("settings.modelLocation.confirm.modelsUnknown", { dir: old }),
    );
    if (!check.sameDrive) {
      lines.push(t("settings.modelLocation.confirm.otherDrive"));
    }
  }
  if (!running) {
    lines.push(t("settings.modelLocation.confirm.nextStart"));
  }
  if (running && check.repliesRunning) {
    lines.push(t("settings.modelLocation.confirm.replies"));
  }
  return (
    <ConfirmDialog
      title={
        movable
          ? t("settings.modelLocation.confirm.moveTitle")
          : t("settings.modelLocation.confirm.changeTitle")
      }
      message={lines.map((line) => (
        <p key={line}>{line}</p>
      ))}
      onCancel={onCancel}
      actions={
        movable
          ? [
              {
                label: t("settings.modelLocation.confirm.leaveThem"),
                onSelect: () => onChoose(false),
              },
              {
                label: t("settings.modelLocation.confirm.moveModels"),
                onSelect: () => onChoose(true),
              },
            ]
          : [
              {
                label: t("settings.modelLocation.confirm.changeLocation"),
                onSelect: () => onChoose(false),
              },
            ]
      }
    />
  );
}
