// Textes et repères des modèles d'images du ModelHub (propre à ChatonLM ;
// contrat .tools/research/85-image-hub-clarity-contract.md § 3) : nom
// affiché d'un modèle et sa variante, phrase de sa famille, vitesse en clair,
// usage permis. Fonctions pures (essayées par les pages Images).

import type { ImageMachine } from "@/bindings/ImageMachine";
import type { ImageModelInfo } from "@/bindings/ImageModelInfo";
import type { ImageUsage } from "@/bindings/ImageUsage";
import { isKey, type TFunction, type TKey } from "@/i18n";

/** Nom affiché : celui de la recommandation, sinon celui de la famille. */
export function imageModelTitle(model: ImageModelInfo): string {
  return model.recommended ? model.name : model.family.name;
}

/**
 * Nom complet (boîte de suppression, progression, onglet Downloads) : le nom
 * affiché suivi de la variante, qui distingue deux fichiers d'une famille.
 */
export function imageModelLabel(model: ImageModelInfo): string {
  const title = imageModelTitle(model);
  return model.quant ? `${title} ${model.quant}` : title;
}

/** Phrase « à quoi il sert » d'une famille connue ; rien sinon. */
export function familyBlurb(t: TFunction, familyId: string): string | null {
  const key = `hub.images.families.${familyId}`;
  return isKey(key) ? t(key) : null;
}

/** Durée d'une image : secondes, ou minutes au-delà d'une minute et demie. */
function speedText(t: TFunction, seconds: number, measured: boolean): string {
  if (seconds < 90) {
    return t(
      measured
        ? "hub.images.speed.measuredSeconds"
        : "hub.images.speed.seconds",
      { seconds },
    );
  }
  const minutes = Math.round(seconds / 60);
  return t(
    measured ? "hub.images.speed.measuredMinutes" : "hub.images.speed.minutes",
    { minutes },
  );
}

/**
 * Vitesse : mesurée sur cette machine, sinon estimée ; sans carte
 * graphique, « plusieurs minutes » ; inconnue (machine pas lue, famille sans
 * mesure) : null.
 */
export function speedFact(
  t: TFunction,
  {
    measured,
    estimated,
    machine,
  }: {
    measured: number | null;
    estimated: number | null;
    machine: ImageMachine | undefined;
  },
): { text: string; hint: string } | null {
  if (measured !== null) {
    return {
      text: speedText(t, measured, true),
      hint: t("hub.images.speed.measuredHint"),
    };
  }
  if (estimated !== null) {
    return {
      text: speedText(t, estimated, false),
      hint: t("hub.images.speed.hint"),
    };
  }
  if (machine && !machine.gpu) {
    return {
      text: t("hub.images.speed.processor"),
      hint: t("hub.images.speed.hint"),
    };
  }
  return null;
}

const USAGE: Record<ImageUsage, TKey> = {
  free: "hub.images.usage.free",
  nonCommercial: "hub.images.usage.nonCommercial",
  checkLicense: "hub.images.usage.checkLicense",
};

/** Usage permis en clair (texte seul : dans une carte qui est un lien). */
export function usageLabel(t: TFunction, usage: ImageUsage): string {
  return t(USAGE[usage]);
}

/** Couleur du texte de l'usage : ambre pour un usage non commercial. */
export function usageClass(usage: ImageUsage): string {
  return usage === "nonCommercial"
    ? "text-amber-700 dark:text-amber-400"
    : "text-neutral-700 dark:text-neutral-200";
}
