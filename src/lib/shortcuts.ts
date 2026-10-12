// Raccourcis clavier de l'app. Code propre à ChatonLM : l'officiel ouvre les
// réglages par Cmd+, dans le menu de macOS ; ici, Ctrl+, depuis n'importe
// quelle page, aussi depuis un champ de saisie.
//
// Une seule source : chaque raccourci est une entrée du catalogue
// `SHORTCUTS` (touches montrées et prédicat), lue par l'onglet « Keyboard
// shortcuts » des réglages ; les gestionnaires (route racine, zone de saisie
// du chat, ModelHub, page Proofread) appellent les mêmes prédicats.

import type { ShortcutKeys } from "@/bindings/ShortcutKeys";
import type { TFunction } from "@/i18n";

/**
 * Saisie d'une combinaison de raccourci global en cours (Windows, où l'app
 * la fait choisir : ShortcutRecorder.tsx) : toute touche lui revient, les
 * raccourcis de l'app (Ctrl+Q, Ctrl+N, zoom...) ne réagissent pas.
 */
let recording = false;

export function setRecordingShortcut(active: boolean): void {
  recording = active;
}

export function recordingShortcut(): boolean {
  return recording;
}

/** Touches de modification seules : Maj, Ctrl, Alt, Windows (gauche, droite). */
const MODIFIER_KEY_CODES = new Set([16, 17, 18, 91, 92]);

/** Ce qu'une touche pressée pendant la saisie d'une combinaison demande. */
export type RecordedKey =
  /** Modificateur seul ou répétition : rien, la saisie continue. */
  | { kind: "ignore" }
  /** Échap seule : la saisie est annulée. */
  | { kind: "cancel" }
  /** Tab : laissée au navigateur (atteindre « Cancel » au clavier). */
  | { kind: "pass" }
  /** Combinaison à proposer au moteur, qui la contrôle. */
  | { kind: "keys"; keys: ShortcutKeys };

/**
 * Lit une touche pressée pendant la saisie. `keyCode` et non `code` : le
 * moteur attend le code virtuel de Windows, qui suit la disposition du
 * clavier (en AZERTY, `KeyQ` est la touche marquée A).
 */
export function recordedKey(event: KeyboardEvent): RecordedKey {
  if (event.repeat || MODIFIER_KEY_CODES.has(event.keyCode)) {
    return { kind: "ignore" };
  }
  const held = event.ctrlKey || event.altKey || event.metaKey;
  if (event.key === "Escape" && !held && !event.shiftKey) {
    return { kind: "cancel" };
  }
  if (event.key === "Tab" && !held) return { kind: "pass" };
  return {
    kind: "keys",
    keys: {
      ctrl: event.ctrlKey,
      alt: event.altKey,
      shift: event.shiftKey,
      meta: event.metaKey,
      keyCode: event.keyCode,
    },
  };
}

/**
 * Ctrl+, : ouvre les réglages. Ctrl seul (ni Alt, ni Maj, ni Méta), touche
 * non répétée et hors composition (méthode de saisie). `event.key` et non
 * `event.code` : la virgule n'est pas au même endroit en AZERTY.
 */
export function isSettingsShortcut(event: KeyboardEvent): boolean {
  return (
    event.ctrlKey &&
    !event.altKey &&
    !event.shiftKey &&
    !event.metaKey &&
    !event.repeat &&
    !event.isComposing &&
    event.key === ","
  );
}

/**
 * Ctrl+N : nouvelle conversation (l'officiel le fait sous Windows ; le menu
 * natif s'en charge sous macOS). Ctrl seul ; « N » accepté sans Maj
 * (verrouillage des majuscules).
 */
export function isNewChatShortcut(event: KeyboardEvent): boolean {
  return (
    event.ctrlKey &&
    !event.altKey &&
    !event.metaKey &&
    !event.repeat &&
    !event.isComposing &&
    (event.key === "n" || (event.key === "N" && !event.shiftKey))
  );
}

/**
 * Ctrl+Q : quitte l'app (propre à ChatonLM : fermer la fenêtre la réduit dans
 * la zone de notification). Ctrl seul ; « Q » accepté sans Maj
 * (verrouillage des majuscules) ; la touche, pas sa position (AZERTY).
 */
export function isQuitShortcut(event: KeyboardEvent): boolean {
  return (
    event.ctrlKey &&
    !event.altKey &&
    !event.metaKey &&
    !event.repeat &&
    !event.isComposing &&
    (event.key === "q" || (event.key === "Q" && !event.shiftKey))
  );
}

export type ZoomShortcut = "zoomIn" | "zoomOut" | "zoomReset";

/**
 * Zoom, comme l'officiel : Ctrl et « + » ou « = » (avant), « - » (arrière),
 * « 0 » (remise à 100 %). Aussi les touches du pavé numérique et la touche du
 * 0 de la rangée du haut par son code (en AZERTY, Ctrl+0 y donne « à »). Pas
 * avec Alt : AltGr se lit Ctrl+Alt.
 */
export function zoomShortcut(event: KeyboardEvent): ZoomShortcut | null {
  if (!(event.ctrlKey || event.metaKey) || event.altKey || event.isComposing) {
    return null;
  }
  if (event.key === "+" || event.key === "=" || event.code === "NumpadAdd") {
    return "zoomIn";
  }
  if (event.key === "-" || event.code === "NumpadSubtract") return "zoomOut";
  if (
    event.key === "0" ||
    event.code === "Digit0" ||
    event.code === "Numpad0"
  ) {
    return "zoomReset";
  }
  return null;
}

/**
 * Entrée envoie le message (Maj+Entrée : nouvelle ligne). Ctrl+Entrée aussi,
 * comme l'officiel ; jamais pendant une composition (méthode de saisie),
 * que l'appelant vérifie aussi par ses propres événements.
 */
export function isSendShortcut(event: KeyboardEvent): boolean {
  return event.key === "Enter" && !event.shiftKey && !event.isComposing;
}

/** Maj+Entrée : nouvelle ligne (comportement natif de la zone de texte). */
export function isNewLineShortcut(event: KeyboardEvent): boolean {
  return event.key === "Enter" && event.shiftKey && !event.isComposing;
}

/** Échap : arrête la réponse, la correction, ou annule une modification. */
export function isEscapeKey(event: KeyboardEvent): boolean {
  return event.key === "Escape";
}

/** Ctrl+V : coller (la zone de saisie du chat prend d'abord le focus). */
export function isPasteShortcut(event: KeyboardEvent): boolean {
  return (event.ctrlKey || event.metaKey) && event.key === "v";
}

/** « / » : champ de recherche du ModelHub, sans Ctrl, Méta ni Alt. */
export function isSearchShortcut(event: KeyboardEvent): boolean {
  return event.key === "/" && !event.ctrlKey && !event.metaKey && !event.altKey;
}

/**
 * Ctrl+Maj+Espace : démarre ou termine le mode vocal (page d'une
 * conversation, propre à ChatonLM). Ctrl et Maj seuls, touche non répétée,
 * hors composition.
 */
export function isVoiceModeShortcut(event: KeyboardEvent): boolean {
  return (
    event.ctrlKey &&
    event.shiftKey &&
    !event.altKey &&
    !event.metaKey &&
    !event.repeat &&
    !event.isComposing &&
    (event.key === " " || event.code === "Space")
  );
}

/**
 * Espace seule (barre du mode vocal) : « j'ai fini » ou interrompre en mains
 * libres, maintenue pour parler en push-to-talk.
 */
export function isVoiceModeSpace(event: KeyboardEvent): boolean {
  return (
    event.key === " " &&
    !event.ctrlKey &&
    !event.shiftKey &&
    !event.altKey &&
    !event.metaKey &&
    !event.isComposing
  );
}

/** Ctrl seul (ni Alt — AltGr se lit Ctrl+Alt —, ni Méta), hors répétition. */
function ctrlOnly(event: KeyboardEvent): boolean {
  return (
    event.ctrlKey &&
    !event.altKey &&
    !event.metaKey &&
    !event.repeat &&
    !event.isComposing
  );
}

/** Ctrl+Entrée : Correct (page Proofread). */
export function isCorrectShortcut(event: KeyboardEvent): boolean {
  return ctrlOnly(event) && !event.shiftKey && event.key === "Enter";
}

/** Ctrl+Maj+Entrée : Rephrase (page Proofread). */
export function isRephraseShortcut(event: KeyboardEvent): boolean {
  return ctrlOnly(event) && event.shiftKey && event.key === "Enter";
}

/**
 * Ctrl+Z : Undo de la revue (page Proofread) ; « Z » accepté sans Maj
 * (verrouillage des majuscules), la touche et non sa position (AZERTY).
 */
export function isUndoShortcut(event: KeyboardEvent): boolean {
  return (
    ctrlOnly(event) &&
    !event.shiftKey &&
    (event.key === "z" || event.key === "Z")
  );
}

/**
 * Ctrl+Maj+Z ou Ctrl+Y : Redo de la revue (page Proofread, après Undo) ;
 * « Z » et « y » acceptés quelle que soit la casse (verrouillage des
 * majuscules), touche non répétée.
 */
export function isRedoShortcut(event: KeyboardEvent): boolean {
  if (!ctrlOnly(event)) return false;
  return event.shiftKey
    ? event.key === "z" || event.key === "Z"
    : event.key === "y" || event.key === "Y";
}

/** Ctrl (ni Alt, ni Méta), hors composition ; touche maintenue permise. */
function ctrlHeld(event: KeyboardEvent): boolean {
  return event.ctrlKey && !event.altKey && !event.metaKey && !event.isComposing;
}

/** La touche Z (verrouillage des majuscules ou Maj : « Z »). */
function isZ(event: KeyboardEvent): boolean {
  return event.key === "z" || event.key === "Z";
}

/** Ctrl+Z dans un champ de texte : annuler la saisie. */
export function isUndoTypingShortcut(event: KeyboardEvent): boolean {
  return ctrlHeld(event) && !event.shiftKey && isZ(event);
}

/** Ctrl+Maj+Z (ou Ctrl+Y) dans un champ de texte : rétablir. */
export function isRedoTypingShortcut(event: KeyboardEvent): boolean {
  if (!ctrlHeld(event)) return false;
  return event.shiftKey ? isZ(event) : event.key === "y" || event.key === "Y";
}

/** Types de champ `<input>` où l'on tape du texte. */
const TEXT_INPUTS = new Set([
  "text",
  "search",
  "url",
  "email",
  "password",
  "tel",
  "number",
]);

function isTextField(target: EventTarget | null): boolean {
  if (target instanceof HTMLTextAreaElement) return true;
  if (target instanceof HTMLInputElement) return TEXT_INPUTS.has(target.type);
  return target instanceof HTMLElement && target.isContentEditable;
}

/**
 * Commande d'édition native d'une touche dans un champ de texte : WebKitGTK
 * ne lie pas Ctrl+Z ni Ctrl+Maj+Z (Epiphany le fait lui-même), la page passe
 * donc par `document.execCommand` (pile d'annulation du champ). Null hors
 * d'un champ de texte ou pour une autre touche.
 */
export function textEditingCommand(
  event: KeyboardEvent,
): "undo" | "redo" | null {
  if (!isTextField(event.target)) return null;
  if (isUndoTypingShortcut(event)) return "undo";
  if (isRedoTypingShortcut(event)) return "redo";
  return null;
}

/** Groupe de l'onglet « Keyboard shortcuts », dans l'ordre de l'écran. */
export type ShortcutGroup =
  "general" | "chat" | "voiceMode" | "proofread" | "hub";

export const SHORTCUT_GROUPS: readonly ShortcutGroup[] = [
  "general",
  "chat",
  "voiceMode",
  "proofread",
  "hub",
];

/**
 * Touche montrée : modificateurs et touches nommées traduits à l'affichage
 * (« Maj », « Entrée »...), autres caractères tels quels.
 */
export type ShortcutKey =
  "ctrl" | "shift" | "enter" | "esc" | "space" | (string & {});

/** Touches nommées (texte traduit à l'affichage). */
export const NAMED_KEYS = ["ctrl", "shift", "enter", "esc", "space"] as const;

export type ShortcutId =
  | "newChat"
  | "settings"
  | "quit"
  | "zoomIn"
  | "zoomOut"
  | "actualSize"
  | "undoTyping"
  | "redoTyping"
  | "send"
  | "newLine"
  | "stopAnswer"
  | "cancelEdit"
  | "paste"
  | "voiceModeToggle"
  | "voiceModeSpace"
  | "voiceModeEnd"
  | "correct"
  | "rephrase"
  | "stopProofread"
  | "undoCorrection"
  | "redoCorrection"
  | "pasteInReview"
  | "searchModels";

/** Combinaison : touches dans l'ordre où on les presse. */
export type ShortcutCombo = readonly ShortcutKey[];

export interface Shortcut {
  id: ShortcutId;
  group: ShortcutGroup;
  /**
   * Combinaisons qui déclenchent l'action, la principale d'abord (montrées
   * « Ctrl+Maj+Z ou Ctrl+Y »).
   */
  keys: readonly ShortcutCombo[];
  /**
   * Valeur de l'attribut aria-keyshortcuts (noms de touches du DOM ; une
   * espace entre deux combinaisons).
   */
  aria: string;
  /** L'événement déclenche-t-il ce raccourci ? */
  matches: (event: KeyboardEvent) => boolean;
}

/** Tous les raccourcis de l'app (les raccourcis globaux sont à part). */
export const SHORTCUTS: readonly Shortcut[] = [
  {
    id: "newChat",
    group: "general",
    keys: [["ctrl", "N"]],
    aria: "Control+N",
    matches: isNewChatShortcut,
  },
  {
    id: "settings",
    group: "general",
    keys: [["ctrl", ","]],
    aria: "Control+,",
    matches: isSettingsShortcut,
  },
  {
    id: "quit",
    group: "general",
    keys: [["ctrl", "Q"]],
    aria: "Control+Q",
    matches: isQuitShortcut,
  },
  {
    id: "zoomIn",
    group: "general",
    keys: [["ctrl", "+"]],
    aria: "Control+Plus",
    matches: (event) => zoomShortcut(event) === "zoomIn",
  },
  {
    id: "zoomOut",
    group: "general",
    keys: [["ctrl", "-"]],
    aria: "Control+Minus",
    matches: (event) => zoomShortcut(event) === "zoomOut",
  },
  {
    id: "actualSize",
    group: "general",
    keys: [["ctrl", "0"]],
    aria: "Control+0",
    matches: (event) => zoomShortcut(event) === "zoomReset",
  },
  {
    id: "undoTyping",
    group: "general",
    keys: [["ctrl", "Z"]],
    aria: "Control+Z",
    matches: isUndoTypingShortcut,
  },
  {
    id: "redoTyping",
    group: "general",
    keys: [
      ["ctrl", "shift", "Z"],
      ["ctrl", "Y"],
    ],
    aria: "Control+Shift+Z Control+Y",
    matches: isRedoTypingShortcut,
  },
  {
    id: "send",
    group: "chat",
    // Ctrl+Entrée aussi, comme l'officiel (isSendShortcut).
    keys: [["enter"], ["ctrl", "enter"]],
    aria: "Enter Control+Enter",
    matches: isSendShortcut,
  },
  {
    id: "newLine",
    group: "chat",
    keys: [["shift", "enter"]],
    aria: "Shift+Enter",
    matches: isNewLineShortcut,
  },
  {
    id: "stopAnswer",
    group: "chat",
    keys: [["esc"]],
    aria: "Escape",
    matches: isEscapeKey,
  },
  {
    id: "cancelEdit",
    group: "chat",
    keys: [["esc"]],
    aria: "Escape",
    matches: isEscapeKey,
  },
  {
    id: "paste",
    group: "chat",
    keys: [["ctrl", "V"]],
    aria: "Control+V",
    matches: isPasteShortcut,
  },
  {
    id: "voiceModeToggle",
    group: "voiceMode",
    keys: [["ctrl", "shift", "space"]],
    aria: "Control+Shift+Space",
    matches: isVoiceModeShortcut,
  },
  {
    // Mains libres : « j'ai fini » ou interrompre ; push-to-talk : maintenue.
    id: "voiceModeSpace",
    group: "voiceMode",
    keys: [["space"]],
    aria: "Space",
    matches: isVoiceModeSpace,
  },
  {
    id: "voiceModeEnd",
    group: "voiceMode",
    keys: [["esc"]],
    aria: "Escape",
    matches: isEscapeKey,
  },
  {
    id: "correct",
    group: "proofread",
    keys: [["ctrl", "enter"]],
    aria: "Control+Enter",
    matches: isCorrectShortcut,
  },
  {
    id: "rephrase",
    group: "proofread",
    keys: [["ctrl", "shift", "enter"]],
    aria: "Control+Shift+Enter",
    matches: isRephraseShortcut,
  },
  {
    id: "stopProofread",
    group: "proofread",
    keys: [["esc"]],
    aria: "Escape",
    matches: isEscapeKey,
  },
  {
    id: "undoCorrection",
    group: "proofread",
    keys: [["ctrl", "Z"]],
    aria: "Control+Z",
    matches: isUndoShortcut,
  },
  {
    id: "redoCorrection",
    group: "proofread",
    keys: [
      ["ctrl", "shift", "Z"],
      ["ctrl", "Y"],
    ],
    aria: "Control+Shift+Z Control+Y",
    matches: isRedoShortcut,
  },
  {
    // Dans la revue : édition du texte corrigé, collage à la fin.
    id: "pasteInReview",
    group: "proofread",
    keys: [["ctrl", "V"]],
    aria: "Control+V",
    matches: isPasteShortcut,
  },
  {
    id: "searchModels",
    group: "hub",
    keys: [["/"]],
    aria: "/",
    matches: isSearchShortcut,
  },
];

/** Entrée du catalogue. */
export function shortcut(id: ShortcutId): Shortcut {
  const found = SHORTCUTS.find((entry) => entry.id === id);
  if (!found) throw new Error(`unknown shortcut: ${id}`);
  return found;
}

/**
 * Nom montré d'une touche : modificateurs et touches nommées traduits
 * (« Maj », « Entrée », « Échap »), autres caractères tels quels.
 */
export function keyLabel(t: TFunction, key: ShortcutKey): string {
  switch (key) {
    case "ctrl":
      return t("settings.shortcuts.keys.ctrl");
    case "shift":
      return t("settings.shortcuts.keys.shift");
    case "enter":
      return t("settings.shortcuts.keys.enter");
    case "esc":
      return t("settings.shortcuts.keys.esc");
    case "space":
      return t("settings.shortcuts.keys.space");
    default:
      return key;
  }
}

/** Une combinaison en texte : « Ctrl+Entrée ». */
export function comboText(t: TFunction, combo: ShortcutCombo): string {
  return combo.map((key) => keyLabel(t, key)).join("+");
}

/**
 * Deux possibilités réunies par le texte traduit « <first/> or <second/> »
 * (ses balises, rendues par <Trans> dans les pastilles de ShortcutKeys,
 * remplacées ici par du texte).
 */
export function eitherText(t: TFunction, first: string, second: string) {
  return t("settings.shortcuts.either").replace(
    /<(first|second)\/>/g,
    (_, name: string) => (name === "first" ? first : second),
  );
}

/** Combinaisons d'un raccourci en texte : « Ctrl+Maj+Z ou Ctrl+Y ». */
export function keysText(t: TFunction, keys: readonly ShortcutCombo[]): string {
  return keys
    .map((combo) => comboText(t, combo))
    .reduce((first, second) => eitherText(t, first, second));
}
