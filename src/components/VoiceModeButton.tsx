// Bouton « onde » de la zone de saisie (propre à ChatonLM : l'app officielle
// n'a pas de mode conversation vocale), à côté d'Envoyer : lance le mode
// vocal pour la conversation de la page (prérequis, accord pour les
// fichiers manquants, démarrage ; voir useVoiceMode.ts). Même raccourci que
// Ctrl+Maj+Espace, rappelé en infobulle. Séance tenue par l'extension VS
// Code : bouton inactif, qui le dit.

import { VoiceWaveIcon } from "@/components/icons";
import { startVoiceMode } from "@/hooks/useVoiceMode";
import { useT } from "@/i18n";
import { keysText, shortcut } from "@/lib/shortcuts";

export function VoiceModeButton({
  chatId,
  disabled,
  editor = false,
}: {
  chatId: string;
  /** Pas de modèle choisi, ou message en cours de modification. */
  disabled: boolean;
  /** Séance tenue par VS Code. */
  editor?: boolean;
}) {
  const t = useT();
  const toggle = shortcut("voiceModeToggle");
  const busy = t("voiceMode.button.editor");
  return (
    <button
      type="button"
      data-voice-mode-start=""
      onClick={() => void startVoiceMode(chatId)}
      disabled={disabled || editor}
      aria-label={editor ? busy : t("voiceMode.button.start")}
      aria-keyshortcuts={toggle.aria}
      title={
        editor
          ? busy
          : t("voiceMode.button.title", { keys: keysText(t, toggle.keys) })
      }
      className="flex h-9 w-9 cursor-pointer items-center justify-center rounded-full border border-transparent bg-neutral-200 focus:outline-2 focus:outline-offset-2 focus:outline-accent-fg disabled:cursor-default disabled:opacity-40 dark:bg-neutral-700"
    >
      <VoiceWaveIcon className="h-4.5 w-4.5 text-neutral-600 dark:text-neutral-300" />
    </button>
  );
}
