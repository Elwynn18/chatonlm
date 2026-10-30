// Langue de l'interface (propre à ChatonLM : l'app officielle n'existe qu'en
// anglais). Catalogues dans src/locales/<langue>/<espace>.json, communs avec
// le moteur (backend.json, lu par src-tauri/src/i18n.rs). Une clé est le
// chemin pointé « espace.groupe.nom » ; « {nom} » reçoit une valeur ; un
// objet { one, other... } est un pluriel, choisi par la valeur `count`.
//
// L'anglais est la langue source (textes de l'officiel à l'identique) ; le
// type des catalogues espagnol et français est vérifié contre lui (clé
// manquante = erreur de compilation), scripts/check-i18n.mjs vérifie le reste
// (clés en trop, valeurs, balises, textes en dur).

import { useSyncExternalStore } from "react";
import type { Lang } from "@/bindings/Lang";
import enChat from "@/locales/en/chat.json";
import enCommon from "@/locales/en/common.json";
import enDictation from "@/locales/en/dictation.json";
import enHub from "@/locales/en/hub.json";
import enOnboarding from "@/locales/en/onboarding.json";
import enProofread from "@/locales/en/proofread.json";
import enSettings from "@/locales/en/settings.json";
import enStudy from "@/locales/en/study.json";
import enVoiceMode from "@/locales/en/voiceMode.json";
import enVscode from "@/locales/en/vscode.json";
import esChat from "@/locales/es/chat.json";
import esCommon from "@/locales/es/common.json";
import esDictation from "@/locales/es/dictation.json";
import esHub from "@/locales/es/hub.json";
import esOnboarding from "@/locales/es/onboarding.json";
import esProofread from "@/locales/es/proofread.json";
import esSettings from "@/locales/es/settings.json";
import esStudy from "@/locales/es/study.json";
import esVoiceMode from "@/locales/es/voiceMode.json";
import esVscode from "@/locales/es/vscode.json";
import frChat from "@/locales/fr/chat.json";
import frCommon from "@/locales/fr/common.json";
import frDictation from "@/locales/fr/dictation.json";
import frHub from "@/locales/fr/hub.json";
import frOnboarding from "@/locales/fr/onboarding.json";
import frProofread from "@/locales/fr/proofread.json";
import frSettings from "@/locales/fr/settings.json";
import frStudy from "@/locales/fr/study.json";
import frVoiceMode from "@/locales/fr/voiceMode.json";
import frVscode from "@/locales/fr/vscode.json";

export type { Lang };

/** Langues proposées, dans l'ordre du réglage. */
export const LANGUAGES: readonly Lang[] = ["en", "es", "fr"];

const en = {
  common: enCommon,
  chat: enChat,
  settings: enSettings,
  dictation: enDictation,
  hub: enHub,
  onboarding: enOnboarding,
  proofread: enProofread,
  study: enStudy,
  voiceMode: enVoiceMode,
  vscode: enVscode,
};

type Catalog = typeof en;

const catalogs: Record<Lang, Catalog> = {
  en,
  es: {
    common: esCommon,
    chat: esChat,
    settings: esSettings,
    dictation: esDictation,
    hub: esHub,
    onboarding: esOnboarding,
    proofread: esProofread,
    study: esStudy,
    voiceMode: esVoiceMode,
    vscode: esVscode,
  },
  fr: {
    common: frCommon,
    chat: frChat,
    settings: frSettings,
    dictation: frDictation,
    hub: frHub,
    onboarding: frOnboarding,
    proofread: frProofread,
    study: frStudy,
    voiceMode: frVoiceMode,
    vscode: frVscode,
  },
};

/** Formes d'un pluriel (catégories CLDR ; `other` toujours présente). */
type PluralForms = { other: string } & Partial<
  Record<"zero" | "one" | "two" | "few" | "many", string>
>;

/** Chemins pointés des textes et pluriels d'un catalogue. */
type Leaves<T, P extends string = ""> = {
  [K in keyof T & string]: T[K] extends string
    ? `${P}${K}`
    : T[K] extends { other: string }
      ? `${P}${K}`
      : Leaves<T[K], `${P}${K}.`>;
}[keyof T & string];

/** Clé d'un texte de l'interface. */
export type TKey = Leaves<Catalog>;

/** Valeurs des « {nom} » ; un nombre est écrit à la façon de la langue. */
export type TParams = Record<string, string | number>;

/**
 * Fonction de traduction liée à une langue (voir `useT`) ; `lang` sert aux
 * formats (src/i18n/format.ts) des fonctions qui la reçoivent.
 */
export type TFunction = ((key: TKey, params?: TParams) => string) & {
  readonly lang: Lang;
};

let current: Lang = "en";
const listeners = new Set<() => void>();
/** Essais : textes traduits marqués (voir `setPseudoForTests`). */
let pseudo = false;

/** Langue affichée. */
export function getLanguage(): Lang {
  return current;
}

/**
 * Change la langue affichée : tout composant qui lit ses textes par `useT`
 * ou `useLanguage` est rendu de nouveau, sans rechargement (le brouillon du
 * composer, la position de lecture et les menus ouverts restent).
 */
export function setLanguage(lang: Lang) {
  if (!catalogs[lang] || lang === current) return;
  current = lang;
  document.documentElement.lang = lang;
  for (const listener of listeners) listener();
}

/**
 * Essais seulement : chaque texte traduit (et chaque texte produit par
 * `Intl` dans src/i18n/format.ts) est entouré de « ⟦ ⟧ », pour trouver dans
 * un écran rendu tout texte écrit en dur (src/test/i18n.ts).
 */
export function setPseudoForTests(on: boolean) {
  if (pseudo === on) return;
  pseudo = on;
  bound.clear();
  for (const listener of listeners) listener();
}

/** Marque un texte localisé hors des catalogues (essais, voir ci-dessus). */
export function localized(text: string): string {
  return pseudo ? `⟦${text}⟧` : text;
}

function subscribe(listener: () => void) {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

/** Langue affichée ; le composant est rendu de nouveau quand elle change. */
export function useLanguage(): Lang {
  return useSyncExternalStore(subscribe, getLanguage, getLanguage);
}

function lookup(catalog: Catalog, key: string): string | PluralForms | null {
  let node: unknown = catalog;
  for (const part of key.split(".")) {
    if (typeof node !== "object" || node === null) return null;
    node = (node as Record<string, unknown>)[part];
  }
  if (typeof node === "string") return node;
  if (typeof node === "object" && node !== null && "other" in node) {
    return node as PluralForms;
  }
  return null;
}

const pluralRules = new Map<Lang, Intl.PluralRules>();
const numberFormats = new Map<Lang, Intl.NumberFormat>();

function pluralRule(lang: Lang) {
  let rule = pluralRules.get(lang);
  if (!rule) {
    rule = new Intl.PluralRules(lang);
    pluralRules.set(lang, rule);
  }
  return rule;
}

function numberFormat(lang: Lang) {
  let format = numberFormats.get(lang);
  if (!format) {
    // Milliers groupés à partir de 10 000 seulement : « 1010 » reste écrit
    // comme dans les textes de l'officiel (`${n}`).
    format = new Intl.NumberFormat(lang, { useGrouping: "min2" });
    numberFormats.set(lang, format);
  }
  return format;
}

/** Texte de `key` dans `lang` (anglais s'il y manque), valeurs remplies. */
export function translate(lang: Lang, key: TKey, params?: TParams): string {
  const entry = lookup(catalogs[lang], key) ?? lookup(catalogs.en, key);
  if (entry === null) {
    if (import.meta.env.MODE === "test") {
      throw new Error(`Texte inconnu : ${key}`);
    }
    return key;
  }
  let template: string;
  if (typeof entry === "string") {
    template = entry;
  } else {
    const count = params?.count;
    if (typeof count !== "number") {
      if (import.meta.env.MODE === "test") {
        throw new Error(`Pluriel sans « count » : ${key}`);
      }
    }
    const form = pluralRule(lang).select(typeof count === "number" ? count : 0);
    template = entry[form] ?? entry.other;
  }
  const text = template.replace(/\{(\w+)\}/g, (match, name: string) => {
    const value = params?.[name];
    if (value === undefined) {
      if (import.meta.env.MODE === "test") {
        throw new Error(`Valeur « ${name} » manquante : ${key}`);
      }
      return match;
    }
    return typeof value === "number" ? numberFormat(lang).format(value) : value;
  });
  return localized(text);
}

/**
 * La clé existe-t-elle (catalogue anglais) ? Pour une clé construite à partir
 * d'une donnée (identifiant d'un modèle du moteur), avant de la traduire.
 */
export function isKey(key: string): key is TKey {
  return lookup(catalogs.en, key) !== null;
}

const bound = new Map<Lang, TFunction>();

/** Fonction de traduction d'une langue, de même identité à chaque appel. */
export function translator(lang: Lang): TFunction {
  let t = bound.get(lang);
  if (!t) {
    t = Object.assign(
      (key: TKey, params?: TParams) => translate(lang, key, params),
      { lang },
    );
    bound.set(lang, t);
  }
  return t;
}

/**
 * Traduction dans la langue affichée, pour un composant : il est rendu de
 * nouveau quand la langue change, et `t` change alors d'identité (à mettre
 * dans les dépendances d'un `useMemo` qui produit du texte).
 */
export function useT(): TFunction {
  // L'instantané est la fonction elle-même : elle change avec la langue, et
  // aussi quand les essais passent en pseudo-langue (même langue).
  return useSyncExternalStore(subscribe, currentTranslator, currentTranslator);
}

function currentTranslator(): TFunction {
  return translator(current);
}

/**
 * Traduction dans la langue affichée, hors d'un composant (notification,
 * message d'une promesse...). Dans un rendu, préférer `useT` : `tNow` ne fait
 * pas rendre de nouveau le composant quand la langue change.
 */
export function tNow(key: TKey, params?: TParams): string {
  return translate(current, key, params);
}
