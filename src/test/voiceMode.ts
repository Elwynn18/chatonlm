// Faux moteur du mode conversation vocale pour le simulateur (tests vitest
// seulement) : état de la séance, réglages (table à part, validés comme le
// moteur), consigne par défaut et prérequis, déduits des faux moteurs de la
// dictée (modèle de transcription : réglage de la dictée, sinon le premier
// installé, sinon Parakeet ; langue) et de la lecture à voix haute (voix de
// la langue de la dictée si fr/en/es, sinon de l'interface ; ici l'anglais).
// Comme le moteur, chaque commande change l'état puis émet « voice-mode »
// après sa réponse : voice_mode_start (prérequis vérifiés) passe par
// Starting puis Listening ; stop revient au repos (séance de la page), et
// stop_editor aussi (séance de VS Code) ; mute ferme ou rouvre le
// micro ; interrupt (Responding, Speaking) et resume (Paused) reviennent à
// Listening ; end_turn (Hearing) passe à Transcribing ; hold(true) à
// Hearing, hold(false) à Transcribing ; reply_failed pose l'avis `failed`
// et revient à Listening. Le test fait parler l'utilisateur par utter()
// (phrase transcrite, puis Responding), avance la séance par setState() et
// envoie des niveaux par level(). Une panne d'une commande se force par
// `failures` ; `unavailable` rend la machine incapable.

import type { AppError } from "@/bindings/AppError";
import type { VoiceLanguage } from "@/bindings/VoiceLanguage";
import type { VoiceModeInfo } from "@/bindings/VoiceModeInfo";
import type { VoiceModeRequirements } from "@/bindings/VoiceModeRequirements";
import type { VoiceModeSettings } from "@/bindings/VoiceModeSettings";
import type { VoiceModeSettingsPatch } from "@/bindings/VoiceModeSettingsPatch";
import type { VoiceModeState } from "@/bindings/VoiceModeState";
import { type FakeDictation, PARAKEET } from "@/test/dictation";
import type { Args, Simulator } from "@/test/simulator";
import type { FakeTts } from "@/test/tts";

/** Début de la consigne vocale par défaut (voice_mode::DEFAULT_INSTRUCTIONS). */
export const DEFAULT_VOICE_INSTRUCTIONS =
  "You are having a spoken conversation. The user talks to you through speech recognition, and your reply will be read aloud by a text-to-speech voice.";

/** Refus de voice_mode_start quand un fichier manque (texte du moteur). */
export const MISSING_FILES = "Download the voice mode files first";

export const VOICE_OFF: VoiceModeState = {
  phase: "off",
  muted: false,
  turn: 0,
  notice: null,
  editor: false,
};

/** Refus de voice_mode_start quand VS Code tient la séance (texte du moteur). */
export const HELD_BY_EDITOR = "Voice is in use by VS Code: end it there first";

export function defaultVoiceModeSettings(): VoiceModeSettings {
  return {
    turnMode: "handsFree",
    pause: "normal",
    bargeIn: true,
    keepThink: false,
    instructions: "",
  };
}

export interface FakeVoiceMode {
  state: VoiceModeState;
  settings: VoiceModeSettings;
  info: VoiceModeInfo;
  /** La machine ne peut pas (texte du moteur), null sinon. */
  unavailable: string | null;
  /** Erreur (panne) renvoyée par une commande, par nom. */
  failures: Map<string, AppError>;
  /** Prérequis tels que voice_mode_requirements les donne. */
  requirements(): VoiceModeRequirements;
  /** Change l'état puis émet « voice-mode ». */
  setState(patch: Partial<VoiceModeState>): Promise<void>;
  /**
   * Phrase transcrite : tour suivant, « voice-mode-utterance », puis
   * Responding (avis effacé).
   */
  utter(text: string, language?: VoiceLanguage | null): Promise<void>;
  /** Émet des niveaux (« voice-mode-level »). */
  level(input: number, output?: number): Promise<void>;
}

const MAX_INSTRUCTIONS = 4000;
const TURN_MODES = ["handsFree", "pushToTalk"];
const PAUSES = ["short", "normal", "long"];

function invalid(message: string): AppError {
  return { kind: "invalid", message };
}

export function fakeVoiceMode(
  simulator: Simulator,
  { dictation, tts }: { dictation: FakeDictation; tts: FakeTts },
): FakeVoiceMode {
  const voice: FakeVoiceMode = {
    state: { ...VOICE_OFF },
    settings: defaultVoiceModeSettings(),
    info: { defaultInstructions: DEFAULT_VOICE_INSTRUCTIONS },
    unavailable: null,
    failures: new Map(),
    requirements() {
      const models = dictation.models;
      const chosen = models.find(
        (m) => m.id === dictation.settings.speechModel,
      );
      const speech =
        chosen ??
        models.find((m) => m.installed) ??
        models.find((m) => m.id === PARAKEET);
      if (!speech) throw simulator.fail("no speech model in the catalog");
      const spoken = dictation.settings.language;
      const language: VoiceLanguage =
        spoken === "fr" || spoken === "es" || spoken === "en" ? spoken : "en";
      const found = tts.voices.find((v) => v.language === language);
      if (!found) throw simulator.fail(`no voice for "${language}"`);
      return {
        speechModel: speech.id,
        speechModelInstalled: speech.installed,
        speechModelBytes: speech.installed ? 0 : speech.sizeBytes,
        voiceModel: found.id,
        voiceLanguage: language,
        voiceInstalled: found.installed,
        voiceBytes: found.installed ? 0 : found.sizeBytes,
        unavailable: voice.unavailable,
      };
    },
    async setState(patch) {
      voice.state = { ...voice.state, ...patch };
      await simulator.emitEvent("voice-mode", voice.state);
    },
    async utter(text, language = null) {
      const turn = voice.state.turn + 1;
      voice.state = { ...voice.state, turn };
      await simulator.emitEvent("voice-mode-utterance", {
        turn,
        text,
        language,
      });
      await voice.setState({ phase: "responding", notice: null });
    },
    level: (input, output = 0) =>
      simulator.emitEvent("voice-mode-level", { input, output }),
  };

  function failure(command: string) {
    const error = voice.failures.get(command);
    if (error) throw error;
  }

  /** Changements émis par le moteur après la réponse de la commande. */
  function emitLater(...states: Partial<VoiceModeState>[]) {
    setTimeout(() => {
      void (async () => {
        for (const patch of states) {
          if (simulator.disposed) return;
          await voice.setState(patch);
        }
      })();
    }, 0);
  }

  const active = () => voice.state.phase !== "off";

  simulator.handle("voice_mode_requirements", () => {
    failure("voice_mode_requirements");
    return voice.requirements();
  });
  simulator.handle("voice_mode_start", () => {
    failure("voice_mode_start");
    // Séance tenue par VS Code : refusée ; celle de la page : sans effet.
    if (active() && voice.state.editor) throw invalid(HELD_BY_EDITOR);
    if (active()) return null;
    const needs = voice.requirements();
    if (needs.unavailable) throw invalid(needs.unavailable);
    if (!needs.speechModelInstalled || !needs.voiceInstalled) {
      throw invalid(MISSING_FILES);
    }
    emitLater(
      { phase: "starting", muted: false, turn: 0, notice: null, editor: false },
      { phase: "listening" },
    );
    return null;
  });
  simulator.handle("voice_mode_stop", () => {
    failure("voice_mode_stop");
    // Le moteur n'arrête que la séance de la page.
    if (active() && !voice.state.editor) emitLater({ ...VOICE_OFF });
    return null;
  });
  simulator.handle("voice_mode_stop_editor", () => {
    failure("voice_mode_stop_editor");
    // Seulement la séance tenue par VS Code (bandeau de la page).
    if (active() && voice.state.editor) emitLater({ ...VOICE_OFF });
    return null;
  });
  simulator.handle("voice_mode_mute", (args: Args) => {
    failure("voice_mode_mute");
    if (typeof args.muted !== "boolean") {
      throw simulator.fail(
        `voice_mode_mute: bad arguments ${JSON.stringify(args)}`,
      );
    }
    if (active()) emitLater({ muted: args.muted });
    return null;
  });
  simulator.handle("voice_mode_interrupt", () => {
    failure("voice_mode_interrupt");
    const { phase } = voice.state;
    if (phase === "responding" || phase === "speaking") {
      emitLater({ phase: "listening" });
    }
    return null;
  });
  simulator.handle("voice_mode_end_turn", () => {
    failure("voice_mode_end_turn");
    if (voice.state.phase === "hearing") emitLater({ phase: "transcribing" });
    return null;
  });
  simulator.handle("voice_mode_hold", (args: Args) => {
    failure("voice_mode_hold");
    if (typeof args.pressed !== "boolean") {
      throw simulator.fail(
        `voice_mode_hold: bad arguments ${JSON.stringify(args)}`,
      );
    }
    if (!active()) return null;
    if (args.pressed) emitLater({ phase: "hearing" });
    else if (voice.state.phase === "hearing") {
      emitLater({ phase: "transcribing" });
    }
    return null;
  });
  simulator.handle("voice_mode_resume", () => {
    failure("voice_mode_resume");
    if (voice.state.phase === "paused") emitLater({ phase: "listening" });
    return null;
  });
  simulator.handle("voice_mode_reply_failed", (args: Args) => {
    failure("voice_mode_reply_failed");
    if (typeof args.turn !== "number" || typeof args.message !== "string") {
      throw simulator.fail(
        `voice_mode_reply_failed: bad arguments ${JSON.stringify(args)}`,
      );
    }
    if (active() && args.turn === voice.state.turn) {
      emitLater({
        phase: "listening",
        notice: { kind: "failed", message: args.message },
      });
    }
    return null;
  });
  simulator.handle("voice_mode_state", () => {
    failure("voice_mode_state");
    return voice.state;
  });
  simulator.handle("voice_mode_info", () => {
    failure("voice_mode_info");
    return voice.info;
  });
  simulator.handle("get_voice_mode_settings", () => {
    failure("get_voice_mode_settings");
    return voice.settings;
  });
  // Comme le moteur : tout le correctif est validé avant d'écrire.
  simulator.handle("update_voice_mode_settings", (args: Args) => {
    failure("update_voice_mode_settings");
    const patch = args.patch as VoiceModeSettingsPatch;
    if (patch.turnMode !== undefined && !TURN_MODES.includes(patch.turnMode)) {
      throw invalid(`unknown turn mode: ${String(patch.turnMode)}`);
    }
    if (patch.pause !== undefined && !PAUSES.includes(patch.pause)) {
      throw invalid(`unknown pause: ${String(patch.pause)}`);
    }
    if (
      patch.instructions !== undefined &&
      [...patch.instructions].length > MAX_INSTRUCTIONS
    ) {
      throw invalid("The instructions are too long");
    }
    voice.settings = { ...voice.settings, ...patch };
    return voice.settings;
  });

  return voice;
}
