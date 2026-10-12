// Onglet « Read aloud » des réglages (propre à ChatonLM : l'officiel ne lit
// pas à voix haute), en deux cartes. La première règle la lecture
// automatique de chaque nouvelle réponse pendant qu'elle s'écrit (réglage
// `autoRead`, enregistré aussitôt : « Saved » de la page, erreur sous la
// ligne ; coupé par « Reset to defaults » de General). La seconde montre
// l'état des trois voix (installée, en téléchargement, absente) et mène à
// l'onglet Voices du ModelHub, où elles se téléchargent : sans la voix de sa
// langue, une réponse n'est pas lue.

import { ArrowPathIcon, SpeakerWaveIcon } from "@heroicons/react/20/solid";
import { CheckCircleIcon } from "@heroicons/react/24/outline";
import { Link } from "@tanstack/react-router";
import { useId, useState } from "react";
import type { SpeechDownload } from "@/bindings/SpeechDownload";
import type { VoiceModelInfo } from "@/bindings/VoiceModelInfo";
import { Row, RowError, Switch } from "@/components/settings/Row";
import { useVoiceDownloads, useVoiceModels } from "@/hooks/useReadAloud";
import { useSettings } from "@/hooks/useSettings";
import { useT } from "@/i18n";
import { formatPercent } from "@/i18n/format";
import { isActiveSpeechDownload, speechDownloadPercent } from "@/lib/dictation";
import { errorMessage } from "@/lib/ipc";
import { VOICE_RUNTIME_NOTES, voiceTitle } from "@/lib/readAloud";

const MUTED = "text-sm/6 text-zinc-500 dark:text-zinc-400";
const LINK =
  "rounded-md text-sm/6 text-zinc-700 underline underline-offset-2 outline-none hover:text-zinc-950 focus-visible:ring-2 focus-visible:ring-accent-fg dark:text-zinc-300 dark:hover:text-white";
const SMALL_BUTTON =
  "inline-flex flex-none items-center gap-1.5 rounded-lg border border-zinc-950/10 bg-white px-3 py-1 text-sm/6 font-medium text-zinc-950 shadow-sm outline-none hover:bg-zinc-50 focus-visible:ring-2 focus-visible:ring-accent-fg dark:border-white/15 dark:bg-zinc-900 dark:text-white dark:hover:bg-zinc-800";

export function ReadAloud({
  onSaved,
}: {
  /** Un changement est enregistré (« Saved » de la page). */
  onSaved: () => void;
}) {
  const t = useT();
  const { settings, setSettings } = useSettings();
  const [error, setError] = useState<string | null>(null);
  const ids = { auto: useId(), autoDescription: useId() };

  if (!settings) return null;

  const save = async (autoRead: boolean) => {
    setError(null);
    try {
      await setSettings({ autoRead });
    } catch (cause) {
      setError(errorMessage(cause));
      return;
    }
    onSaved();
  };

  return (
    <div data-testid="read-aloud-settings" className="space-y-4">
      <div className="overflow-hidden rounded-xl bg-white dark:bg-neutral-800">
        <div className="space-y-4 p-4">
          <div className="flex items-start justify-between gap-4">
            <Row
              icon={SpeakerWaveIcon}
              label={
                <span id={ids.auto}>{t("settings.readAloud.auto.label")}</span>
              }
              description={t("settings.readAloud.auto.description")}
              descriptionId={ids.autoDescription}
            >
              <RowError message={error} />
            </Row>
            <Switch
              label={t("settings.readAloud.auto.label")}
              checked={settings.autoRead}
              describedBy={ids.autoDescription}
              onChange={(autoRead) => void save(autoRead)}
            />
          </div>
        </div>
      </div>

      <Voices />
    </div>
  );
}

/** Carte « Voices » : état de chaque voix, lien vers le ModelHub. */
function Voices() {
  const t = useT();
  const voices = useVoiceModels();
  const { downloads } = useVoiceDownloads();
  const titleId = useId();
  const descriptionId = useId();

  let body;
  if (voices.isError) {
    body = (
      <div className="flex items-start justify-between gap-4">
        <p
          role="alert"
          className="min-w-0 text-sm/6 break-words text-red-600 dark:text-red-400"
        >
          {t("settings.readAloud.voices.loadFailed", {
            message: errorMessage(voices.error),
          })}
        </p>
        <button
          type="button"
          className={SMALL_BUTTON}
          onClick={() => void voices.refetch()}
        >
          <ArrowPathIcon aria-hidden="true" className="h-4 w-4" />
          {t("settings.tryAgain")}
        </button>
      </div>
    );
  } else if (voices.isPending) {
    body = (
      <div
        data-testid="voices-loading"
        aria-hidden="true"
        className="h-12 animate-pulse rounded-lg bg-neutral-200 dark:bg-neutral-700"
      />
    );
  } else {
    // ONNX Runtime manque à toutes les voix à la fois : dit une fois.
    const runtime = voices.data.find(
      (voice) => voice.runtimeProblem !== null,
    )?.runtimeProblem;
    body = (
      <>
        {/* Séparateurs par bordure sur chaque ligne, comme Loaded models. */}
        <ul>
          {voices.data.map((voice) => (
            <VoiceRow
              key={voice.id}
              voice={voice}
              download={downloads.find((entry) => entry.id === voice.id)}
            />
          ))}
        </ul>
        {runtime && (
          <p className="text-xs break-words text-amber-700 dark:text-amber-400">
            {t(VOICE_RUNTIME_NOTES[runtime.kind], { detail: runtime.detail })}
          </p>
        )}
      </>
    );
  }

  return (
    <section
      data-testid="read-aloud-voices"
      aria-labelledby={titleId}
      aria-describedby={descriptionId}
      className="overflow-hidden rounded-xl bg-white dark:bg-neutral-800"
    >
      <div className="space-y-4 p-4">
        <div>
          <h2
            id={titleId}
            className="text-sm/6 font-medium text-zinc-950 dark:text-white"
          >
            {t("settings.readAloud.voices.title")}
          </h2>
          <p id={descriptionId} className={MUTED}>
            {t("settings.readAloud.voices.description")}
          </p>
        </div>
        {body}
        <p>
          <Link to="/hub/voices" className={LINK}>
            {t("settings.readAloud.voices.manage")}
          </Link>
        </p>
      </div>
    </section>
  );
}

/** Une voix : titre et état (installée, en téléchargement, absente). */
function VoiceRow({
  voice,
  download,
}: {
  voice: VoiceModelInfo;
  download: SpeechDownload | undefined;
}) {
  const t = useT();
  const active = download !== undefined && isActiveSpeechDownload(download);
  let state;
  if (active) {
    state = (
      <span className={`${MUTED} tabular-nums`}>
        {download.totalBytes > 0
          ? t("settings.readAloud.voices.downloadingProgress", {
              percent: formatPercent(t.lang, speechDownloadPercent(download)),
            })
          : t("settings.readAloud.voices.downloading")}
      </span>
    );
  } else if (voice.installed) {
    state = (
      <span className="flex items-center gap-1.5 text-sm/6 text-green-700 dark:text-green-400">
        <CheckCircleIcon aria-hidden="true" className="h-4 w-4" />
        {t("settings.readAloud.voices.installed")}
      </span>
    );
  } else {
    state = (
      <span className={MUTED}>{t("settings.readAloud.voices.missing")}</span>
    );
  }
  return (
    <li
      data-testid="read-aloud-voice"
      data-voice={voice.id}
      className="flex items-center justify-between gap-4 border-t border-zinc-950/5 py-3 first:border-t-0 first:pt-0 last:pb-0 dark:border-white/10"
    >
      <span className="min-w-0 text-sm/6 text-zinc-950 dark:text-white">
        {voiceTitle(t, voice)}
      </span>
      {state}
    </li>
  );
}
