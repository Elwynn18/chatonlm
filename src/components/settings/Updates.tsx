// Carte « Updates » de Settings › General (étape 18c). Le réglage
// « Auto-download updates » reprend l'officiel (Settings.tsx, textes
// identiques) ; le reste est propre à ChatonLM, demandé par l'utilisateur :
// versions installées (ChatonLM, et l'Ollama intégré s'il est livré), état
// des mises à jour (à jour avec l'heure de la dernière vérification,
// disponible avec ses nouveautés et « Download » quand Auto-download est
// coupé, téléchargement avec sa progression, prête avec « Restart to
// update » et, pour un .deb, la boîte de mot de passe dite à l'avance (pour
// l'installateur de Windows, sa brève apparition),
// installation en cours sans bouton (services arrêtés, mot de passe demandé
// pour un .deb), erreur avec Retry, qui vérifie de nouveau, non prise en
// charge avec sa raison) et « Check for updates », sans effet pendant une
// vérification, un téléchargement, une installation ou une autre action
// (aria-disabled : le bouton garde le focus). Sans installeur (pas de
// paquet, paquet d'une distribution ; sous Windows, copie posée sans
// l'installateur ou dossier d'installation en lecture seule), « Check for updates » et
// Auto-download sont désactivés, décrits par la raison. Le dernier échec
// gardé par le moteur sous une annonce (`lastError` : vérification de fond,
// discrète ; relance refusée, venue aussi de la zone de notification, en
// alerte) est montré sous l'état. Une commande refusée est montrée sous
// l'état, avec Retry qui la relance, tant que l'état ne change pas et reste
// celui que la commande suppose (sinon l'état du moteur dit déjà pourquoi ;
// jamais d'échec muet). L'état est relu après chaque commande (sa réponse et
// les évènements ne sont pas ordonnés entre eux) ; il n'est annoncé (région
// polie) que pendant la suite d'une action de l'utilisateur. Ouverte depuis
// la zone de notification (`focusRequest`), la carte défile dans la vue et
// son titre prend le focus. Réglage à part (table update_settings) :
// « Reset to defaults » n'y touche pas ; « Saved » de la page après succès,
// sinon l'erreur sous la ligne.

import {
  ArrowDownTrayIcon,
  ArrowPathIcon,
  ArrowUpCircleIcon,
  TagIcon,
} from "@heroicons/react/20/solid";
import { useQueryClient } from "@tanstack/react-query";
import { useEffect, useId, useRef, useState } from "react";
import type { UnsupportedReason } from "@/bindings/UnsupportedReason";
import type { UpdateErrorCode } from "@/bindings/UpdateErrorCode";
import type { UpdateErrorInfo } from "@/bindings/UpdateErrorInfo";
import type { UpdateState } from "@/bindings/UpdateState";
import type { UpdateStatus } from "@/bindings/UpdateStatus";
import { Row, RowError, Switch } from "@/components/settings/Row";
import { useFocusRescue } from "@/hooks/useFocusRescue";
import {
  updateStatusKey,
  useAppVersions,
  useUpdateStatus,
} from "@/hooks/useUpdates";
import { type Lang, localized, type TFunction, type TKey, useT } from "@/i18n";
import { formatBytes, formatDateTime, formatPercent } from "@/i18n/format";
import {
  checkForUpdates,
  downloadUpdate,
  errorMessage,
  restartToUpdate,
  setAutoDownload,
} from "@/lib/ipc";

// aria-disabled comme disabled : le bouton d'une action en cours garde le
// focus (un bouton désactivé le perd).
const BUTTON =
  "inline-flex flex-none items-center gap-1.5 rounded-lg border border-zinc-950/10 bg-white px-3 py-1 text-sm/6 font-medium text-zinc-950 shadow-sm outline-none hover:bg-zinc-50 focus-visible:ring-2 focus-visible:ring-accent-fg disabled:cursor-default disabled:opacity-50 aria-disabled:cursor-default aria-disabled:opacity-50 dark:border-white/15 dark:bg-zinc-900 dark:text-white dark:hover:bg-zinc-800";
const ALERT = "text-xs break-words text-red-600 dark:text-red-400";
const HINT = "text-sm/6 text-zinc-500 dark:text-zinc-400";

/** Action lancée depuis la carte. */
type Action = "check" | "download" | "restart";

type StateName = UpdateState["state"];

const COMMANDS: Record<Action, () => Promise<unknown>> = {
  check: checkForUpdates,
  download: downloadUpdate,
  restart: restartToUpdate,
};

/** Titre d'un échec, par action. */
const FAILURE_TITLES: Record<Action, TKey> = {
  check: "settings.updates.error.check",
  download: "settings.updates.error.download",
  restart: "settings.updates.error.install",
};

/**
 * État que l'action suppose : son refus n'est montré que dans cet état
 * (passé ailleurs, l'état du moteur dit déjà pourquoi, par exemple le
 * dossier non inscriptible).
 */
const EXPECTED: Record<Action, (state: StateName) => boolean> = {
  check: (state) => state !== "error" && state !== "unsupported",
  download: (state) => state === "available",
  restart: (state) => state === "ready",
};

/**
 * États où la suite d'une action est encore attendue : l'état reste annoncé
 * jusqu'à ce qu'elle en sorte (ou que la commande soit refusée).
 */
const FOLLOWED: Record<Action, StateName[]> = {
  check: ["checking"],
  download: ["available", "downloading"],
  restart: ["ready", "installing"],
};

/** Délai avant de cesser d'annoncer, l'état final lu. */
const ANNOUNCE_TAIL_MS = 1000;

/** Titre d'une erreur du moteur, par étape. */
const ERROR_TITLES: Record<UpdateErrorCode, TKey> = {
  check: "settings.updates.error.check",
  download: "settings.updates.error.download",
  signature: "settings.updates.error.signature",
  install: "settings.updates.error.install",
  io: "settings.updates.error.io",
};

/** Pourquoi ChatonLM ne peut pas se mettre à jour lui-même. */
const UNSUPPORTED_REASONS: Record<UnsupportedReason, TKey> = {
  noPackage: "settings.updates.status.reasons.noPackage",
  packageManager: "settings.updates.status.reasons.packageManager",
  readOnlyFolder: "settings.updates.status.reasons.readOnlyFolder",
  // Windows : copie posée sans l'installateur, dossier d'installation
  // non inscriptible.
  notInstalled: "settings.updates.status.reasons.notInstalled",
  installFolderReadOnly:
    "settings.updates.status.reasons.installFolderReadOnly",
};

/**
 * Commande refusée ; montrée tant que l'état est celui lu juste après le
 * refus (`during`, même objet grâce au partage structurel de React Query).
 */
interface Failure {
  action: Action;
  message: string;
  during: UpdateStatus | undefined;
}

/**
 * Refus déjà dit par le moteur dans `lastError` (même texte, ou relance
 * refusée : le moteur garde ces refus, d'où qu'ils viennent).
 */
function reportedByEngine(failure: Failure, lastError: UpdateErrorInfo | null) {
  if (lastError === null) return false;
  return (
    lastError.detail === failure.message ||
    (failure.action === "restart" && lastError.code !== "check")
  );
}

/** Jour d'une date ISO 8601 (« 7 oct. 2026 ») ; null si illisible. */
function formatDay(lang: Lang, date: string | null): string | null {
  if (!date) return null;
  const time = Date.parse(date);
  if (!Number.isFinite(time)) return null;
  return localized(
    new Intl.DateTimeFormat(lang, { dateStyle: "medium" }).format(time),
  );
}

/** Ligne d'état : titre (annoncé) et description. */
function headline(
  t: TFunction,
  status: UpdateStatus,
): { title: string; description: string | null } {
  const state = status.state;
  const schedule = t("settings.updates.status.schedule");
  switch (state.state) {
    case "idle":
      return {
        title: t("settings.updates.status.idle"),
        description: schedule,
      };
    case "checking":
      return {
        title: t("settings.updates.status.checking"),
        description: schedule,
      };
    case "upToDate":
      return {
        title: t("settings.updates.status.upToDate"),
        description:
          status.checkedAt === null
            ? schedule
            : t("settings.updates.status.lastCheck", {
                time: formatDateTime(t.lang, status.checkedAt),
              }),
      };
    case "available": {
      const day = formatDay(t.lang, state.date);
      return {
        title: t("settings.updates.status.available", {
          version: state.version,
        }),
        description: day
          ? t("settings.updates.status.released", { date: day })
          : null,
      };
    }
    case "downloading":
      return {
        title: t("settings.updates.status.downloading", {
          version: state.version,
        }),
        description: null,
      };
    case "ready":
      return {
        title: t("settings.updates.status.ready", { version: state.version }),
        description: t("settings.updates.status.restartHint"),
      };
    case "installing":
      return {
        title: t("settings.updates.status.installing", {
          version: state.version,
        }),
        description: t("settings.updates.status.installingHint"),
      };
    case "error":
      return { title: t(ERROR_TITLES[state.code]), description: null };
    case "unsupported":
      // Version annoncée par une vérification : elle est dite d'abord.
      return {
        title:
          state.version === null
            ? t("settings.updates.status.unsupported")
            : t("settings.updates.status.available", {
                version: state.version,
              }),
        description: t(UNSUPPORTED_REASONS[state.reason]),
      };
  }
}

/** Nouveautés de la version (texte du fichier latest.json). */
function Notes({ notes }: { notes: string | null }) {
  const t = useT();
  const titleId = useId();
  if (!notes?.trim()) return null;
  return (
    <div role="group" aria-labelledby={titleId} className="mt-2">
      <h3
        id={titleId}
        className="text-xs font-medium text-zinc-700 dark:text-zinc-300"
      >
        {t("settings.updates.notes")}
      </h3>
      <p
        data-testid="update-notes"
        className="mt-1 max-h-40 overflow-y-auto rounded-lg bg-neutral-50 p-2 text-sm/6 break-words whitespace-pre-wrap text-zinc-700 dark:bg-neutral-900 dark:text-zinc-300"
      >
        {notes.trim()}
      </p>
    </div>
  );
}

/**
 * Dernier échec gardé par le moteur sous l'état : vérification de fond
 * (discrète, l'annonce reste valable) ou refus d'une autre étape (alerte).
 */
function LastError({ error }: { error: UpdateErrorInfo }) {
  const t = useT();
  if (error.code === "check") {
    return (
      <p
        data-testid="update-last-error"
        className="mt-1 text-xs break-words text-zinc-500 dark:text-zinc-400"
      >
        {t("settings.updates.status.checkFailed", { detail: error.detail })}
      </p>
    );
  }
  return (
    <div role="alert" data-testid="update-last-error" className="mt-2">
      <p className={`font-medium ${ALERT}`}>{t(ERROR_TITLES[error.code])}</p>
      <p className={ALERT}>{error.detail}</p>
    </div>
  );
}

/** Progression du téléchargement (ARIA progressbar, comme SpeechProgress). */
function Progress({
  state,
}: {
  state: Extract<UpdateState, { state: "downloading" }>;
}) {
  const t = useT();
  const known = state.total !== null && state.total > 0;
  const percent = known
    ? Math.min(100, Math.floor((state.received / (state.total ?? 1)) * 100))
    : 0;
  const text = known
    ? t("settings.updates.progress", {
        received: formatBytes(t, state.received),
        total: formatBytes(t, state.total ?? 0),
        percent: formatPercent(t.lang, percent),
      })
    : formatBytes(t, state.received);
  return (
    <div className="mt-2">
      <div
        role="progressbar"
        aria-label={t("settings.updates.status.downloading", {
          version: state.version,
        })}
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={known ? percent : undefined}
        aria-valuetext={text}
        className="relative h-1.5 overflow-hidden rounded-full bg-neutral-200 dark:bg-neutral-700"
      >
        <div
          className={`absolute inset-y-0 left-0 rounded-full bg-accent transition-[width] duration-300 ${
            known ? "" : "w-1/4 animate-pulse opacity-40"
          }`}
          style={known ? { width: `${percent}%` } : undefined}
        />
      </div>
      <p
        data-testid="update-progress"
        className="mt-1.5 text-xs text-neutral-500 tabular-nums dark:text-neutral-400"
      >
        {text}
      </p>
    </div>
  );
}

/**
 * Carte « Updates ». `onSaved` : badge « Saved » de la page ;
 * `focusRequest` : demande de montrer la carte (une valeur par navigation,
 * menu de la zone de notification) ; null : aucune.
 */
export function Updates({
  onSaved,
  focusRequest = null,
}: {
  onSaved: () => void;
  focusRequest?: string | null;
}) {
  const t = useT();
  const queryClient = useQueryClient();
  const status = useUpdateStatus();
  const versions = useAppVersions();
  const [pending, setPending] = useState<Action | null>(null);
  const [failure, setFailure] = useState<Failure | null>(null);
  const [savingAuto, setSavingAuto] = useState(false);
  const [autoError, setAutoError] = useState<string | null>(null);
  // Action de l'utilisateur dont la suite est annoncée (région polie) ;
  // null : les changements de fond ne sont pas annoncés.
  const [announced, setAnnounced] = useState<Action | null>(null);
  const cardRef = useRef<HTMLElement>(null);
  const titleRef = useRef<HTMLHeadingElement>(null);
  const ids = {
    title: useId(),
    versions: useId(),
    status: useId(),
    auto: useId(),
    autoDescription: useId(),
  };
  // Une réponse peut arriver après la sortie de la page.
  const mounted = useRef(false);
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);
  // Bouton focalisé retiré (Download, Restart, Retry) : le titre de la carte.
  useFocusRescue(cardRef, titleRef);

  // Suite de l'action annoncée finie (commande rendue, et état sorti de ceux
  // qu'elle attend, ou commande refusée) : plus d'annonce, après un court
  // délai qui laisse lire l'état final.
  const shownState = status.data?.state.state;
  const refused = failure !== null;
  useEffect(() => {
    if (announced === null || pending !== null || shownState === undefined) {
      return;
    }
    if (!refused && FOLLOWED[announced].includes(shownState)) return;
    const timer = window.setTimeout(() => setAnnounced(null), ANNOUNCE_TAIL_MS);
    return () => window.clearTimeout(timer);
  }, [announced, pending, shownState, refused]);

  // Demande de montrer la carte (zone de notification) : défilement et
  // focus sur le titre, une fois l'état lu (hauteur connue), à chaque
  // nouvelle demande ; pas si l'utilisateur a mis le focus ailleurs entre-temps.
  const shown = status.data !== undefined || status.isError;
  const servedRef = useRef<string | null>(null);
  const waitingRef = useRef<{
    request: string;
    active: Element | null;
  } | null>(null);
  useEffect(() => {
    if (focusRequest === null || servedRef.current === focusRequest) return;
    if (waitingRef.current?.request !== focusRequest) {
      waitingRef.current = {
        request: focusRequest,
        active: document.activeElement,
      };
    }
    if (!shown) return;
    const before = waitingRef.current.active;
    servedRef.current = focusRequest;
    waitingRef.current = null;
    const card = cardRef.current;
    if (!card) return;
    const active = document.activeElement;
    if (
      active !== before &&
      active !== null &&
      active !== document.body &&
      !card.contains(active)
    ) {
      return;
    }
    // jsdom n'a pas scrollIntoView.
    card.scrollIntoView?.({ block: "start" });
    titleRef.current?.focus({ preventScroll: true });
  }, [focusRequest, shown]);

  const refresh = () =>
    queryClient.invalidateQueries({ queryKey: updateStatusKey });

  // Appelée par un bouton actif seulement (une action à la fois).
  const run = async (action: Action) => {
    setPending(action);
    setFailure(null);
    setAnnounced(action);
    let message: string | null = null;
    try {
      await COMMANDS[action]();
    } catch (cause) {
      message = errorMessage(cause);
    }
    await refresh();
    if (!mounted.current) return;
    if (message !== null) {
      setFailure({
        action,
        message,
        during: queryClient.getQueryData<UpdateStatus>(updateStatusKey),
      });
    } else if (action === "restart") {
      // Relance acceptée : ChatonLM se ferme, le bouton reste sur
      // « Restarting… ».
      return;
    }
    setPending(null);
  };

  const changeAutoDownload = async (enabled: boolean) => {
    if (savingAuto) return;
    setSavingAuto(true);
    setAutoError(null);
    let saved = false;
    try {
      await setAutoDownload(enabled);
      saved = true;
    } catch (cause) {
      if (mounted.current) setAutoError(errorMessage(cause));
    }
    await refresh();
    if (!mounted.current) return;
    setSavingAuto(false);
    if (saved) onSaved();
  };

  const title = (
    <h2
      ref={titleRef}
      id={ids.title}
      tabIndex={-1}
      className="rounded-md text-sm/6 font-medium text-zinc-950 outline-none dark:text-white"
    >
      {t("settings.updates.title")}
    </h2>
  );

  const versionsRow = (
    <Row
      icon={TagIcon}
      label={t("settings.updates.versions.label")}
      description={
        versions.data
          ? t("settings.updates.versions.chatonlm", {
              version: versions.data.chatonlm,
            })
          : null
      }
      descriptionId={ids.versions}
    >
      {versions.data?.ollama && (
        <p data-testid="ollama-version" className={HINT}>
          {t("settings.updates.versions.ollama", {
            version: versions.data.ollama,
          })}
        </p>
      )}
    </Row>
  );

  let body;
  if (status.isError) {
    body = (
      <div className="flex items-start justify-between gap-4">
        <p
          role="alert"
          className="min-w-0 text-sm/6 break-words text-red-600 dark:text-red-400"
        >
          {t("settings.updates.loadError", {
            message: errorMessage(status.error),
          })}
        </p>
        <button
          type="button"
          className={BUTTON}
          onClick={() => void status.refetch()}
        >
          <ArrowPathIcon aria-hidden="true" className="h-4 w-4" />
          {t("settings.tryAgain")}
        </button>
      </div>
    );
  } else if (!status.data) {
    body = (
      <div
        data-testid="updates-loading"
        aria-hidden="true"
        className="h-12 animate-pulse rounded-lg bg-neutral-200 dark:bg-neutral-700"
      />
    );
  } else {
    const current = status.data;
    const state = current.state;
    const { title: stateTitle, description } = headline(t, current);
    // Sans installeur (pas de paquet, paquet d'une distribution) :
    // vérifier ou télécharger ne sert à rien ici ; la raison le dit.
    const selfUpdating = current.installer !== null;
    const checking = state.state === "checking" || pending === "check";
    // Ni pendant une vérification, un téléchargement ou une installation,
    // ni pendant une autre action de la carte.
    const checkBlocked =
      !selfUpdating ||
      checking ||
      state.state === "downloading" ||
      state.state === "installing" ||
      pending !== null;
    // Refus d'une commande, tant que l'état n'a pas changé depuis et reste
    // celui qu'elle suppose (une erreur du moteur le dit déjà elle-même).
    const shownFailure =
      failure !== null &&
      failure.during === current &&
      EXPECTED[failure.action](state.state) &&
      !reportedByEngine(failure, current.lastError)
        ? failure
        : null;
    // Échec gardé par le moteur sous une annonce (l'état d'erreur le dit
    // déjà lui-même).
    const lastError = state.state === "error" ? null : current.lastError;
    const deb = current.installer === "deb";
    // Installateur de Windows : il apparaît un instant, sans question ni
    // mot de passe.
    const nsis = current.installer === "nsis";

    body = (
      <>
        <div className="flex items-start justify-between gap-4">
          <Row
            icon={ArrowUpCircleIcon}
            label={
              <span
                data-testid="update-state"
                aria-live={announced !== null ? "polite" : undefined}
              >
                {stateTitle}
              </span>
            }
            description={description}
            descriptionId={ids.status}
          >
            {state.state === "ready" && deb && (
              <p data-testid="update-password" className={HINT}>
                {t("settings.updates.status.passwordHint")}
              </p>
            )}
            {state.state === "ready" && nsis && (
              <p data-testid="update-installer" className={HINT}>
                {t("settings.updates.status.installerHint")}
              </p>
            )}
            {state.state === "installing" && deb && (
              <p data-testid="update-password" className={HINT}>
                {t("settings.updates.status.installingPassword")}
              </p>
            )}
            {lastError && <LastError error={lastError} />}
            {state.state === "available" && <Notes notes={state.notes} />}
            {state.state === "ready" && <Notes notes={state.notes} />}
            {state.state === "downloading" && <Progress state={state} />}
            {state.state === "error" && (
              <p role="alert" className={`mt-1 ${ALERT}`}>
                {state.detail}
              </p>
            )}

            {state.state === "available" && !current.autoDownload && (
              <ActionButton
                action="download"
                pending={pending}
                onRun={run}
                label={t("settings.updates.actions.download")}
              />
            )}
            {state.state === "ready" && (
              <ActionButton
                action="restart"
                pending={pending}
                onRun={run}
                label={
                  pending === "restart"
                    ? t("settings.updates.actions.restarting")
                    : t("settings.updates.actions.restart")
                }
              />
            )}
            {/* Un nouvel essai est une vérification (le moteur reprend
                là où il en était). */}
            {state.state === "error" && (
              <ActionButton
                action="check"
                pending={pending}
                onRun={run}
                label={t("common.actions.retry")}
              />
            )}

            {shownFailure && (
              <div role="alert" className="mt-2">
                <p className={`font-medium ${ALERT}`}>
                  {t(FAILURE_TITLES[shownFailure.action])}
                </p>
                <p className={ALERT}>{shownFailure.message}</p>
              </div>
            )}
            {shownFailure && (
              <ActionButton
                action={shownFailure.action}
                pending={pending}
                onRun={run}
                label={t("common.actions.retry")}
              />
            )}
          </Row>
          <button
            type="button"
            className={BUTTON}
            aria-disabled={checkBlocked}
            aria-describedby={selfUpdating ? undefined : ids.status}
            onClick={() => {
              if (!checkBlocked) void run("check");
            }}
          >
            <ArrowPathIcon
              aria-hidden="true"
              className={`h-4 w-4 ${checking ? "animate-spin" : ""}`}
            />
            {t("settings.updates.actions.check")}
          </button>
        </div>

        <div className="flex items-start justify-between gap-4">
          <Row
            icon={ArrowDownTrayIcon}
            label={
              <span id={ids.auto}>
                {t("settings.updates.autoDownload.label")}
              </span>
            }
            description={
              current.autoDownload
                ? t("settings.updates.autoDownload.on")
                : t("settings.updates.autoDownload.off")
            }
            descriptionId={ids.autoDescription}
          >
            <RowError message={autoError} />
          </Row>
          <Switch
            label={t("settings.updates.autoDownload.label")}
            checked={current.autoDownload}
            describedBy={
              selfUpdating
                ? ids.autoDescription
                : `${ids.autoDescription} ${ids.status}`
            }
            disabled={savingAuto || !selfUpdating}
            onChange={(enabled) => void changeAutoDownload(enabled)}
          />
        </div>
      </>
    );
  }

  return (
    <section
      ref={cardRef}
      aria-labelledby={ids.title}
      data-testid="updates"
      className="overflow-hidden rounded-xl bg-white dark:bg-neutral-800"
    >
      <div className="space-y-4 p-4">
        {title}
        {versionsRow}
        {body}
      </div>
    </section>
  );
}

/** Bouton d'une action de l'état (Download, Restart to update, Retry). */
function ActionButton({
  action,
  pending,
  onRun,
  label,
}: {
  action: Action;
  pending: Action | null;
  onRun: (action: Action) => Promise<void>;
  label: string;
}) {
  const busy = pending !== null;
  return (
    <div className="mt-2">
      <button
        type="button"
        className={BUTTON}
        aria-disabled={busy}
        onClick={() => {
          if (!busy) void onRun(action);
        }}
      >
        {pending === action && (
          <ArrowPathIcon aria-hidden="true" className="h-4 w-4 animate-spin" />
        )}
        {label}
      </button>
    </div>
  );
}
