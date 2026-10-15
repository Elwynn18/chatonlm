// Onglet « Voice mode » des réglages (propre à ChatonLM : l'app officielle n'a
// pas de mode conversation vocale), en trois cartes. La première règle la
// prise de parole (mains libres ou push-to-talk), la pause qui termine une
// phrase en mains libres, l'interruption en parlant (annulation d'écho) et
// la réflexion des modèles pour les messages vocaux. La deuxième, la
// consigne vocale ajoutée aux seules requêtes vocales (vide : celle par
// défaut, montrée en indication dans le champ ; enregistrée en quittant le
// champ, la page ou avant Ctrl+Q ; bouton de remise à zéro : PromptField.tsx). La troisième
// mène aux réglages de la transcription (onglet Dictation) et aux voix
// (ModelHub › Voices). Chaque changement est enregistré aussitôt (« Saved »
// de la page, erreur sous la ligne) ; table à part : « Reset to defaults »
// de General n'y touche pas.

import {
  ArrowPathIcon,
  ChatBubbleLeftRightIcon,
  ClockIcon,
  HandRaisedIcon,
  LightBulbIcon,
  MicrophoneIcon,
  SpeakerWaveIcon,
} from "@heroicons/react/20/solid";
import { Link } from "@tanstack/react-router";
import { type ReactNode, useId, useState } from "react";
import type { VoiceModeSettingsPatch } from "@/bindings/VoiceModeSettingsPatch";
import type { VoicePause } from "@/bindings/VoicePause";
import type { VoiceTurnMode } from "@/bindings/VoiceTurnMode";
import { PromptField } from "@/components/settings/PromptField";
import { Choice, Row, RowError, Switch } from "@/components/settings/Row";
import { useVoiceModeInfo, useVoiceModeSettings } from "@/hooks/useVoiceMode";
import { useT } from "@/i18n";
import { MAX_PROMPT_CHARS } from "@/lib/dictation";
import { errorMessage } from "@/lib/ipc";

const LINK =
  "rounded-md text-sm/6 text-zinc-700 underline underline-offset-2 outline-none hover:text-zinc-950 focus-visible:ring-2 focus-visible:ring-accent-fg dark:text-zinc-300 dark:hover:text-white";
const SMALL_BUTTON =
  "inline-flex flex-none items-center gap-1.5 rounded-lg border border-zinc-950/10 bg-white px-3 py-1 text-sm/6 font-medium text-zinc-950 shadow-sm outline-none hover:bg-zinc-50 focus-visible:ring-2 focus-visible:ring-accent-fg disabled:cursor-default disabled:opacity-50 dark:border-white/15 dark:bg-zinc-900 dark:text-white dark:hover:bg-zinc-800";
const CARD = "overflow-hidden rounded-xl bg-white dark:bg-neutral-800";

type Field = keyof VoiceModeSettingsPatch;

export function VoiceMode({
  onSaved,
}: {
  /** Un changement est enregistré (« Saved » de la page). */
  onSaved: () => void;
}) {
  const t = useT();
  const { settings, error, update, reload } = useVoiceModeSettings({
    refresh: true,
  });
  const info = useVoiceModeInfo();
  const [errors, setErrors] = useState<Partial<Record<Field, string>>>({});
  const ids = {
    turns: useId(),
    turnsDescription: useId(),
    pause: useId(),
    pauseDescription: useId(),
    bargeIn: useId(),
    bargeInDescription: useId(),
    keepThink: useId(),
    keepThinkDescription: useId(),
    instructions: useId(),
    instructionsDescription: useId(),
    instructionsTitle: useId(),
    speechTitle: useId(),
    transcriptionDescription: useId(),
    voicesDescription: useId(),
  };

  /** Enregistre `patch` ; l'erreur s'affiche sous la ligne `field`. */
  const save = async (field: Field, patch: VoiceModeSettingsPatch) => {
    setErrors((current) => ({ ...current, [field]: undefined }));
    try {
      await update(patch);
    } catch (cause) {
      setErrors((current) => ({ ...current, [field]: errorMessage(cause) }));
      return false;
    }
    onSaved();
    return true;
  };

  let body: ReactNode;
  if (error && !settings) {
    body = (
      <div className="flex items-start justify-between gap-4">
        <p
          role="alert"
          className="min-w-0 text-sm/6 break-words text-red-600 dark:text-red-400"
        >
          {t("settings.voiceMode.loadFailed", {
            message: errorMessage(error),
          })}
        </p>
        <button
          type="button"
          className={SMALL_BUTTON}
          onClick={() => void reload()}
        >
          <ArrowPathIcon aria-hidden="true" className="h-4 w-4" />
          {t("settings.tryAgain")}
        </button>
      </div>
    );
  } else if (!settings) {
    body = (
      <div
        data-testid="voice-mode-loading"
        aria-hidden="true"
        className="h-24 animate-pulse rounded-lg bg-neutral-200 dark:bg-neutral-700"
      />
    );
  } else {
    const turnModes: { value: VoiceTurnMode; label: string }[] = [
      { value: "handsFree", label: t("settings.voiceMode.turns.handsFree") },
      { value: "pushToTalk", label: t("settings.voiceMode.turns.pushToTalk") },
    ];
    const pauses: { value: VoicePause; label: string }[] = [
      { value: "short", label: t("settings.voiceMode.pause.short") },
      { value: "normal", label: t("settings.voiceMode.pause.normal") },
      { value: "long", label: t("settings.voiceMode.pause.long") },
    ];
    body = (
      <>
        <div className="flex flex-wrap items-start justify-between gap-4">
          <Row
            icon={HandRaisedIcon}
            label={
              <span id={ids.turns}>{t("settings.voiceMode.turns.label")}</span>
            }
            description={t("settings.voiceMode.turns.description")}
            descriptionId={ids.turnsDescription}
          >
            <RowError message={errors.turnMode} />
          </Row>
          <div className="flex-shrink-0">
            <Choice
              options={turnModes}
              value={settings.turnMode}
              onChange={(turnMode) => void save("turnMode", { turnMode })}
              labelledBy={ids.turns}
              describedBy={ids.turnsDescription}
            />
          </div>
        </div>

        <div className="flex flex-wrap items-start justify-between gap-4">
          <Row
            icon={ClockIcon}
            label={
              <span id={ids.pause}>{t("settings.voiceMode.pause.label")}</span>
            }
            description={t("settings.voiceMode.pause.description")}
            descriptionId={ids.pauseDescription}
          >
            <RowError message={errors.pause} />
          </Row>
          <div className="flex-shrink-0">
            <Choice
              options={pauses}
              value={settings.pause}
              onChange={(pause) => void save("pause", { pause })}
              labelledBy={ids.pause}
              describedBy={ids.pauseDescription}
            />
          </div>
        </div>

        <div className="flex items-start justify-between gap-4">
          <Row
            icon={ChatBubbleLeftRightIcon}
            label={
              <span id={ids.bargeIn}>
                {t("settings.voiceMode.bargeIn.label")}
              </span>
            }
            description={t("settings.voiceMode.bargeIn.description")}
            descriptionId={ids.bargeInDescription}
          >
            <RowError message={errors.bargeIn} />
          </Row>
          <Switch
            label={t("settings.voiceMode.bargeIn.label")}
            checked={settings.bargeIn}
            describedBy={ids.bargeInDescription}
            onChange={(bargeIn) => void save("bargeIn", { bargeIn })}
          />
        </div>

        <div className="flex items-start justify-between gap-4">
          <Row
            icon={LightBulbIcon}
            label={
              <span id={ids.keepThink}>
                {t("settings.voiceMode.keepThink.label")}
              </span>
            }
            description={t("settings.voiceMode.keepThink.description")}
            descriptionId={ids.keepThinkDescription}
          >
            <RowError message={errors.keepThink} />
          </Row>
          <Switch
            label={t("settings.voiceMode.keepThink.label")}
            checked={settings.keepThink}
            describedBy={ids.keepThinkDescription}
            onChange={(keepThink) => void save("keepThink", { keepThink })}
          />
        </div>
      </>
    );
  }

  return (
    <div data-testid="voice-mode-settings" className="space-y-4">
      <div className={CARD}>
        <div className="space-y-4 p-4">{body}</div>
      </div>

      {settings && (
        <section aria-labelledby={ids.instructionsTitle} className={CARD}>
          <div className="space-y-4 p-4">
            <h2
              id={ids.instructionsTitle}
              className="text-sm/6 font-medium text-zinc-950 dark:text-white"
            >
              {t("settings.voiceMode.instructions.title")}
            </h2>
            <PromptField
              id={ids.instructions}
              descriptionId={ids.instructionsDescription}
              data-voice-mode-instructions=""
              label={t("settings.voiceMode.instructions.label")}
              description={t("settings.voiceMode.instructions.description")}
              saved={settings.instructions}
              defaultText={info.data?.defaultInstructions ?? ""}
              placeholder={info.data?.defaultInstructions ?? ""}
              maxLength={MAX_PROMPT_CHARS}
              resetLabel={t("settings.voiceMode.instructions.reset")}
              onSave={(instructions) => save("instructions", { instructions })}
              error={errors.instructions}
            />
          </div>
        </section>
      )}

      <section aria-labelledby={ids.speechTitle} className={CARD}>
        <div className="space-y-4 p-4">
          <h2
            id={ids.speechTitle}
            className="text-sm/6 font-medium text-zinc-950 dark:text-white"
          >
            {t("settings.voiceMode.speech.title")}
          </h2>
          <Row
            icon={MicrophoneIcon}
            label={t("settings.voiceMode.speech.transcription.label")}
            description={t(
              "settings.voiceMode.speech.transcription.description",
            )}
            descriptionId={ids.transcriptionDescription}
          >
            <p className="mt-1">
              <Link
                to="/settings"
                search={{ section: "dictation" }}
                className={LINK}
              >
                {t("settings.voiceMode.speech.transcription.link")}
              </Link>
            </p>
          </Row>
          <Row
            icon={SpeakerWaveIcon}
            label={t("settings.voiceMode.speech.voices.label")}
            description={t("settings.voiceMode.speech.voices.description")}
            descriptionId={ids.voicesDescription}
          >
            <p className="mt-1">
              <Link to="/hub/voices" className={LINK}>
                {t("settings.voiceMode.speech.voices.link")}
              </Link>
            </p>
          </Row>
        </div>
      </section>
    </div>
  );
}
