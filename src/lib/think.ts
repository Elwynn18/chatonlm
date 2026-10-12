// Règles du bouton Think : contrôle proposé pour un modèle et valeur de
// `think` envoyée avec un message. Code propre à ChatonLM : l'officiel ne
// propose le bouton que d'après le nom du modèle (niveaux pour gpt-oss,
// bascule pour deepseek-v3.1) et n'envoie jamais `think: false` ; ici, tout
// modèle qui a la capacité « thinking » a son contrôle, construit d'après
// l'objet `thinking` de /api/show (valeurs permises et défaut), et un choix
// fait est toujours envoyé explicitement. Libellés traduits (src/i18n).

import type { ModelDetails } from "@/bindings/ModelDetails";
import type { Think } from "@/bindings/Think";
import type { TFunction, TKey } from "@/i18n";

/** Entrée du menu des niveaux. */
export interface ThinkOption {
  value: Think;
  label: string;
}

/**
 * Contrôle du raisonnement pour un modèle :
 * - `none` : aucun bouton, `think` non envoyé (pas de capacité, détails
 *   inconnus, ou modèle qui ne sait pas raisonner) ;
 * - `fixed` : le modèle raisonne toujours (Ollama l'annonce par `[true]`, ou
 *   ChatonLM l'a vu raisonner malgré `think: false`) : aucun bouton (choix de
 *   l'utilisateur, recette de l'étape 15), `think` non envoyé, raisonnement
 *   affiché comme d'habitude ;
 * - `toggle` : bascule marche / arrêt, envoyée explicitement ;
 * - `levels` : menu des valeurs permises, niveaux et booléens (« Off »,
 *   « On »), envoyé explicitement.
 */
export type ThinkControl =
  | { kind: "none" }
  | { kind: "fixed" }
  | { kind: "toggle"; value: boolean }
  | { kind: "levels"; value: Think; options: ThinkOption[] };

export const NO_THINK_CONTROL: ThinkControl = { kind: "none" };

/**
 * Libellés des niveaux : officiels (ThinkButton.tsx), puis ceux d'autres
 * modèles d'Ollama qu'une simple majuscule initiale rendrait mal (« Xhigh »).
 */
const LEVEL_LABELS: Record<string, TKey> = {
  low: "chat.think.levels.low",
  medium: "chat.think.levels.medium",
  high: "chat.think.levels.high",
  xhigh: "chat.think.levels.xhigh",
  max: "chat.think.levels.max",
};

/**
 * Libellé d'une valeur dans la langue de `t` : Off, On, niveau connu ; un
 * autre niveau (nom donné par Ollama) est seulement capitalisé.
 */
export function thinkLabel(t: TFunction, value: Think): string {
  if (value === false) return t("chat.think.off");
  if (value === true) return t("chat.think.on");
  const key = Object.hasOwn(LEVEL_LABELS, value) ? LEVEL_LABELS[value] : null;
  return key ? t(key) : value.charAt(0).toUpperCase() + value.slice(1);
}

function isThink(value: unknown): value is Think {
  return (
    typeof value === "boolean" || (typeof value === "string" && value !== "")
  );
}

/**
 * Contrôle d'un modèle, d'après ses détails (/api/show ; null ou undefined :
 * inconnus) et la préférence mémorisée pour lui (null : aucune). Une
 * préférence qui n'est plus permise par le modèle est ignorée. Libellés du
 * menu dans la langue de `t`.
 */
export function thinkControl(
  t: TFunction,
  details: ModelDetails | null | undefined,
  pref: Think | null | undefined,
): ThinkControl {
  if (!details?.capabilities.includes("thinking")) return NO_THINK_CONTROL;
  // Vu raisonner malgré `think: false` (gabarit qui ouvre toujours le
  // raisonnement, comme LFM2.5 tiré de Hugging Face).
  if (details.alwaysThinks) return { kind: "fixed" };
  const thinking = details.thinking;
  const values = [...new Set((thinking?.values ?? []).filter(isThink))];

  // Pas de métadonnées : Ollama raisonne par défaut, on peut l'arrêter.
  if (!thinking || values.length === 0) {
    return { kind: "toggle", value: typeof pref === "boolean" ? pref : true };
  }

  const levels = values.filter((value) => typeof value === "string");
  if (levels.length > 0) {
    // Toutes les valeurs, dans l'ordre d'Ollama : « Off » pour false, « On »
    // pour true. true garde sa place : c'est le raisonnement complet, défaut
    // de Nemotron 3.5 Nano ([false, true, "medium"], où "medium" réduit
    // l'effort).
    const options = values.map((value) => ({
      value,
      label: thinkLabel(t, value),
    }));
    const allowed = (value: Think | null | undefined): value is Think =>
      value !== null && value !== undefined && values.includes(value);
    // Défaut absent ou non permis : premier niveau, ni Off ni On.
    const fallback = allowed(thinking.default) ? thinking.default : levels[0];
    return { kind: "levels", value: allowed(pref) ? pref : fallback, options };
  }

  const canEnable = values.includes(true);
  const canDisable = values.includes(false);
  if (canEnable && canDisable) {
    const fallback =
      typeof thinking.default === "boolean" ? thinking.default : true;
    return {
      kind: "toggle",
      value: typeof pref === "boolean" ? pref : fallback,
    };
  }
  // [true] : raisonne toujours ; [false] : ne sait pas raisonner.
  return canEnable ? { kind: "fixed" } : NO_THINK_CONTROL;
}

/** Valeur de `think` à envoyer ; undefined : champ absent (défaut d'Ollama). */
export function thinkToSend(control: ThinkControl): Think | undefined {
  switch (control.kind) {
    case "toggle":
    case "levels":
      return control.value;
    default:
      return undefined;
  }
}

/**
 * Valeur de `think` d'une phrase du mode vocal (propre à ChatonLM) : le
 * raisonnement retarde la voix, il est coupé quand le modèle le permet —
 * bascule : `false` ; niveaux : `false` s'il est proposé (« Off »), sinon le
 * niveau « low », sinon le premier niveau proposé ; modèle qui raisonne
 * toujours ou sans contrôle : champ absent. `keepThink` (réglage « Let models
 * think ») : la valeur du bouton Think, comme un envoi au clavier.
 */
export function voiceThinkToSend(
  control: ThinkControl,
  keepThink: boolean,
): Think | undefined {
  if (keepThink) return thinkToSend(control);
  switch (control.kind) {
    case "toggle":
      return false;
    case "levels": {
      const values = control.options.map((option) => option.value);
      if (values.includes(false)) return false;
      if (values.includes("low")) return "low";
      return values.find((value) => typeof value === "string") ?? control.value;
    }
    default:
      return undefined;
  }
}
