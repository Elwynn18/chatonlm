// Bandeau « VS Code utilise votre micro » (propre à ChatonLM) : tant que
// l'extension VS Code tient une séance du mode vocal ou une dictée (lot 4b),
// il reste visible en haut de la fenêtre, sur toutes les pages (monté par la
// route racine), avec un bouton qui arrête cette séance ou abandonne cette
// dictée. Le micro n'est ainsi jamais ouvert à l'insu de l'utilisateur, même
// panneau de VS Code replié. Une séance ou une dictée de l'app n'a pas de
// bandeau (sa barre est dans la zone de saisie, ou sur la page des réglages).

import { MicrophoneIcon } from "@heroicons/react/24/outline";
import { useState } from "react";
import { BANNER_PRIMARY } from "@/components/bannerStyles";
import { useDictationStatus } from "@/hooks/useDictation";
import { useVoiceModeSnapshot } from "@/hooks/useVoiceMode";
import { useT } from "@/i18n";
import {
  dictationStopEditor,
  errorMessage,
  voiceModeStopEditor,
} from "@/lib/ipc";
import { heldByEditor } from "@/lib/voiceMode";

export function EditorVoiceBanner() {
  const t = useT();
  const { state } = useVoiceModeSnapshot();
  const { data: dictation } = useDictationStatus();
  const [stopping, setStopping] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const voice = heldByEditor(state);
  // Les deux ne vont jamais ensemble (une seule écoute dans l'app).
  const dictating = !voice && dictation?.editor === true;
  if (!voice && !dictating) return null;

  const stop = async () => {
    setStopping(true);
    setError(null);
    try {
      await (dictating ? dictationStopEditor() : voiceModeStopEditor());
    } catch (cause) {
      setError(errorMessage(cause));
    } finally {
      setStopping(false);
    }
  };

  return (
    <div
      data-testid="editor-voice-banner"
      className="pointer-events-none fixed inset-x-0 top-2 z-30 flex flex-col items-center px-4"
    >
      <div className="pointer-events-auto flex max-w-full items-center gap-3 rounded-full border border-neutral-200 bg-white py-1.5 pr-1.5 pl-4 text-sm text-neutral-800 shadow-sm dark:border-neutral-700 dark:bg-neutral-800 dark:text-neutral-100">
        <MicrophoneIcon
          aria-hidden="true"
          className="h-4 w-4 flex-none text-accent-fg"
        />
        <span role="status">{t("voiceMode.editorBanner.text")}</span>
        <button
          type="button"
          onClick={() => void stop()}
          disabled={stopping}
          aria-label={
            dictating
              ? t("voiceMode.editorBanner.stopDictationLabel")
              : t("voiceMode.editorBanner.stopLabel")
          }
          className={BANNER_PRIMARY}
        >
          {t("voiceMode.editorBanner.stop")}
        </button>
      </div>
      {error && (
        <p
          role="alert"
          className="pointer-events-auto mt-1.5 max-w-full rounded-2xl bg-white px-3 py-1 text-xs break-words text-red-600 dark:bg-neutral-800 dark:text-red-400"
        >
          {dictating
            ? t("voiceMode.editorBanner.stopDictationFailed", { error })
            : t("voiceMode.editorBanner.stopFailed", { error })}
        </p>
      )}
    </div>
  );
}
