// Bouton « Read aloud » d'une réponse (propre à ChatonLM, sur le modèle de
// CopyButton.tsx) : lit la réponse à voix haute, une seule lecture dans
// l'app (en lancer une arrête la précédente). Au repos : haut-parleur ;
// pendant la préparation ou le téléchargement de la voix : roue et texte
// (« Preparing to read… », progression) ; pendant la lecture : carré
// « Stop reading ». Le même bouton arrête la préparation et la lecture.
// Voix absente : accord demandé avant le téléchargement (ConfirmDialog),
// puis la lecture démarre à la fin du téléchargement. Réponse dans une
// autre langue que l'anglais, le français ou l'espagnol, ou sans rien à
// lire : bouton inactif (mais focalisable), avec la raison en infobulle et à
// côté du bouton. Erreur de la lecture (code du moteur, avec la réponse
// lue) ou commande refusée : texte à côté du bouton, jusqu'à la lecture
// suivante. Le texte montré décrit le bouton (aria-describedby).
//
// Annonces : les étapes et les erreurs de la lecture par ReadAloudStatus,
// région présente sous chaque réponse avant que la lecture ne commence
// (une région née déjà remplie n'est pas toujours lue, et la lecture
// automatique commence sans bouton) ; les refus d'un clic par la région du
// bouton.

// Icônes au trait, comme celle de Copier.
import {
  ArrowPathIcon,
  SpeakerWaveIcon,
  StopIcon,
} from "@heroicons/react/24/outline";
import { useId, useState } from "react";
import type { ReadAloudOutcome } from "@/bindings/ReadAloudOutcome";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { useReadAloud, useReadAloudActions } from "@/hooks/useReadAloud";
import { type TFunction, useT } from "@/i18n";
import { formatBytes, formatPercent } from "@/i18n/format";
import { errorMessage } from "@/lib/ipc";
import {
  errorFor,
  needsVoiceText,
  phaseFor,
  readAloudErrorText,
} from "@/lib/readAloud";

type NeedsVoice = Extract<ReadAloudOutcome, { kind: "needsVoice" }>;

/** Réponse que le moteur refuse de lire (bouton inactif). */
type Blocked = "unsupportedLanguage" | "nothingToRead";

function blockedText(t: TFunction, blocked: Blocked): string {
  return blocked === "unsupportedLanguage"
    ? t("chat.readAloud.unsupported")
    : t("chat.readAloud.nothingToRead");
}

export function ReadAloudButton({
  chatId,
  messageId,
  className = "",
}: {
  chatId: string;
  messageId: number;
  className?: string;
}) {
  const t = useT();
  const state = useReadAloud();
  const { read, stop } = useReadAloudActions();
  const [asking, setAsking] = useState<NeedsVoice | null>(null);
  const [blocked, setBlocked] = useState<Blocked | null>(null);
  // Commande refusée par le moteur (texte du moteur, déjà traduit).
  const [failure, setFailure] = useState<string | null>(null);
  const problemId = useId();
  const phase = phaseFor(state, messageId);
  const error = errorFor(state, messageId);
  const busy = phase === "preparing" || phase === "downloading";
  const running = phase !== "idle";

  const start = (download: boolean) => {
    setFailure(null);
    read
      .mutateAsync({ chatId, messageId, download })
      .then((outcome) => {
        if (outcome.kind === "needsVoice") setAsking(outcome);
        else if (outcome.kind !== "started") setBlocked(outcome.kind);
      })
      .catch((cause: unknown) => setFailure(errorMessage(cause)));
  };

  const onClick = () => {
    if (blocked || read.isPending) return;
    if (running) {
      setFailure(null);
      stop.mutate(undefined, {
        onError: (cause) => setFailure(errorMessage(cause)),
      });
      return;
    }
    start(false);
  };

  const label = running ? t("chat.readAloud.stop") : t("chat.readAloud.read");
  const progress =
    phase === "downloading"
      ? state.percent === null
        ? t("chat.readAloud.downloadingVoice")
        : t("chat.readAloud.downloadingProgress", {
            percent: formatPercent(t.lang, state.percent),
          })
      : phase === "preparing"
        ? t("chat.readAloud.preparing")
        : null;
  // Refus d'un clic (le plus récent), sinon erreur de la dernière lecture.
  const refusal = failure ?? (blocked ? blockedText(t, blocked) : null);
  const problem = refusal ?? (error ? readAloudErrorText(t, error) : null);
  const shownProblem = progress ? null : problem;

  // Mêmes dimensions que Copier (taille « md »).
  const iconClass = "h-7 w-7";
  return (
    <>
      <button
        type="button"
        data-read-aloud={phase}
        className={`flex h-7 w-7 cursor-pointer items-center justify-center rounded-lg px-1 py-0.5 text-xs hover:bg-neutral-100 aria-disabled:cursor-default aria-disabled:opacity-50 aria-disabled:hover:bg-transparent dark:hover:bg-neutral-800 dark:aria-disabled:hover:bg-transparent ${className}`}
        onClick={onClick}
        title={blocked ? blockedText(t, blocked) : label}
        aria-label={label}
        // La raison montrée à côté (refus, erreur) décrit le bouton.
        aria-describedby={shownProblem ? problemId : undefined}
        // Inactif mais focalisable : la raison reste lisible.
        aria-disabled={blocked !== null || undefined}
        aria-busy={busy || undefined}
      >
        {busy ? (
          <ArrowPathIcon
            aria-hidden="true"
            className={`${iconClass} motion-safe:animate-spin`}
          />
        ) : phase === "playing" ? (
          <StopIcon aria-hidden="true" className={iconClass} />
        ) : (
          <SpeakerWaveIcon aria-hidden="true" className={iconClass} />
        )}
      </button>
      {progress && (
        <span
          data-testid="read-aloud-progress"
          className="ml-1 text-xs text-neutral-500 tabular-nums dark:text-neutral-400"
        >
          {progress}
        </span>
      )}
      {shownProblem && (
        <span
          id={problemId}
          data-testid="read-aloud-problem"
          className={`ml-1 text-xs break-words ${
            blocked && !failure
              ? "text-neutral-500 dark:text-neutral-400"
              : "text-red-600 dark:text-red-400"
          }`}
        >
          {shownProblem}
        </span>
      )}
      {/* Hors du bouton : ses enfants ne sont pas annoncés (rôle button).
          Seulement les refus d'un clic : la lecture a ReadAloudStatus. */}
      <span className="sr-only" role="status" data-read-aloud-reason="">
        {refusal ?? ""}
      </span>
      {asking && (
        <ConfirmDialog
          title={t("chat.readAloud.downloadTitle")}
          message={needsVoiceText(
            t,
            asking.language,
            formatBytes(t, asking.bytes),
          )}
          confirmLabel={t("chat.readAloud.downloadAndRead")}
          onConfirm={() => {
            setAsking(null);
            start(true);
          }}
          onCancel={() => setAsking(null)}
        />
      )}
    </>
  );
}

/**
 * Annonce de la lecture d'une réponse (étape, erreur), pour les lecteurs
 * d'écran : région présente sous chaque réponse enregistrée, vide au repos,
 * avant que la lecture ne commence (lecture automatique comprise).
 */
export function ReadAloudStatus({ messageId }: { messageId: number }) {
  const t = useT();
  const state = useReadAloud();
  const phase = phaseFor(state, messageId);
  const error = errorFor(state, messageId);
  // L'étape (sans chaque pourcentage), sinon l'erreur de la lecture finie.
  const text =
    phase === "downloading"
      ? t("chat.readAloud.downloadingVoice")
      : phase === "preparing"
        ? t("chat.readAloud.preparing")
        : phase === "playing"
          ? t("chat.readAloud.playing")
          : error
            ? readAloudErrorText(t, error)
            : "";
  return (
    <span className="sr-only" role="status" data-read-aloud-status="">
      {text}
    </span>
  );
}
