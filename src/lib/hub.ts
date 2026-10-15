// Formats, couleurs et paramètres d'adresse du ModelHub (propre à ChatonLM,
// sans équivalent dans l'app officielle). Fonctions pures, testées à part.
// Les textes viennent des catalogues (src/locales/*/hub.json) : les
// fonctions qui en produisent prennent la traduction `t` en premier.

import type { CSSProperties } from "react";
import type { HubDownload } from "@/bindings/HubDownload";
import type { HubFilters } from "@/bindings/HubFilters";
import type { HubFit } from "@/bindings/HubFit";
import type { HubHardware } from "@/bindings/HubHardware";
import type { HubSectionId } from "@/bindings/HubSectionId";
import type { HubSize } from "@/bindings/HubSize";
import type { HubUse } from "@/bindings/HubUse";
import { type Lang, localized, type TFunction } from "@/i18n";
import { formatBytes } from "@/i18n/format";

// Tailles et dates relatives : formatBytes et formatTimeAgo (src/i18n/format.ts).

const K = 1024;

const compactFormats = new Map<Lang, Intl.NumberFormat>();

/**
 * Nombre court, à la façon de la langue : « 999 », « 1.2K », « 12K »,
 * « 1.2M » (« 1,2 k », « 1,2 mil »), jamais « 1000K ».
 */
export function formatCount(t: TFunction, count: number): string {
  if (!Number.isFinite(count) || count <= 0) return "0";
  let format = compactFormats.get(t.lang);
  if (!format) {
    format = new Intl.NumberFormat(t.lang, { notation: "compact" });
    compactFormats.set(t.lang, format);
  }
  const text = format.format(count);
  // Suffixe de la langue (« k », « mil ») : marqué comme un texte traduit.
  return /\p{L}/u.test(text) ? localized(text) : text;
}

/**
 * Une décimale sous 10, sans « .0 » ; entier au-delà ; séparateur décimal de
 * la langue (« 8,2 »), sans séparateur des milliers.
 */
function trimmed(lang: Lang, value: number): string {
  return new Intl.NumberFormat(lang, {
    maximumFractionDigits: value >= 9.95 ? 0 : 1,
    useGrouping: false,
  }).format(value);
}

/**
 * Paramètres : « 8.2B », « 27B », « 135M » (« 8,2B » en français : suffixe
 * d'usage dans toutes les langues, comme les noms des modèles).
 */
export function formatParameters(t: TFunction, count: number): string {
  if (count >= 1e9) return `${trimmed(t.lang, count / 1e9)}B`;
  if (count >= 1e6) return `${trimmed(t.lang, count / 1e6)}M`;
  return formatCount(t, count);
}

/** Contexte : « 128K », « 4K », « 1M » (« 1,5K » en français). */
export function formatContext(t: TFunction, tokens: number): string {
  if (tokens >= K * K) return `${trimmed(t.lang, tokens / (K * K))}M`;
  if (tokens >= K) return `${trimmed(t.lang, tokens / K)}K`;
  return String(tokens);
}

/** Débit : « 48 MB/s » (« 48 Mo/s »). */
export function formatSpeed(t: TFunction, bytesPerSecond: number): string {
  return t("hub.progress.speed", { size: formatBytes(t, bytesPerSecond) });
}

/** Temps restant : « 45 s left », « 3 min left », « 1 h 5 min left ». */
export function formatTimeLeft(t: TFunction, seconds: number): string {
  if (seconds < 60) {
    return t("hub.progress.secondsLeft", {
      count: Math.max(1, Math.ceil(seconds)),
    });
  }
  if (seconds < 3600) {
    return t("hub.progress.minutesLeft", { count: Math.round(seconds / 60) });
  }
  const hours = Math.floor(seconds / 3600);
  const minutes = Math.round((seconds % 3600) / 60);
  return minutes === 0
    ? t("hub.progress.hoursLeft", { count: hours })
    : t("hub.progress.hoursMinutesLeft", { hours, minutes });
}

/** Téléchargement en file ou en cours (pas encore fini). */
export function isActiveDownload(download: HubDownload): boolean {
  return (
    download.state === "queued" ||
    download.state === "starting" ||
    download.state === "downloading" ||
    download.state === "finishing"
  );
}

/** Téléchargement pas encore fini : en file, en cours ou en pause. */
export function isPendingDownload(download: HubDownload): boolean {
  return isActiveDownload(download) || download.state === "paused";
}

/** Pourcentage entier d'un téléchargement ; 0 si le total est inconnu. */
export function downloadPercent(completed: number, total: number): number {
  if (total <= 0) return 0;
  return Math.min(100, Math.floor((completed / total) * 100));
}

/**
 * Ligne de progression : « 2.1 GB of 5.0 GB · 48 MB/s · 1 min left » ;
 * total inconnu : la partie reçue seule.
 */
export function progressText(t: TFunction, download: HubDownload): string {
  const { completed, total, bytesPerSecond } = download;
  const parts =
    total > 0
      ? [
          t("hub.progress.ofTotal", {
            completed: formatBytes(t, completed),
            total: formatBytes(t, total),
          }),
        ]
      : [formatBytes(t, completed)];
  if (bytesPerSecond && bytesPerSecond > 0) {
    parts.push(formatSpeed(t, bytesPerSecond));
    if (total > completed) {
      parts.push(formatTimeLeft(t, (total - completed) / bytesPerSecond));
    }
  }
  return parts.join(" · ");
}

/** Ligne d'état d'un téléchargement actif ou en pause. */
export function activeText(t: TFunction, download: HubDownload): string {
  switch (download.state) {
    case "queued":
      return t("hub.progress.waiting");
    case "paused":
      // « Paused · 2.1 GB of 5.0 GB » : ce qui est déjà reçu est gardé.
      return download.total > 0
        ? `${t("hub.downloadState.paused")} · ${t("hub.progress.ofTotal", {
            completed: formatBytes(t, download.completed),
            total: formatBytes(t, download.total),
          })}`
        : t("hub.downloadState.paused");
    case "starting":
      return download.completed > 0
        ? progressText(t, download)
        : t("hub.progress.starting");
    case "finishing":
      return t("hub.progress.finishing");
    default:
      return progressText(t, download);
  }
}

/** Texte d'une pastille de compatibilité. */
export function fitLabel(t: TFunction, fit: HubFit, gpu: boolean): string {
  switch (fit) {
    case "fits":
      return gpu ? t("hub.fit.fitsGpu") : t("hub.fit.fitsMemory");
    case "partial":
      return t("hub.fit.partial");
    case "tooLarge":
      return t("hub.fit.tooLarge");
  }
}

/** La machine a-t-elle un GPU reconnu (calcul hors processeur) ? */
export function hasGpu(hardware: HubHardware | null | undefined): boolean {
  return (hardware?.gpus.length ?? 0) > 0;
}

/** Nom court d'un GPU : « NVIDIA GeForce RTX 3090 » → « RTX 3090 ». */
export function gpuName(name: string): string {
  const short = name
    .replace(
      /\b(NVIDIA|GeForce|AMD|Radeon\(TM\)|Radeon|Graphics|Intel\(R\))\b/gi,
      " ",
    )
    .replace(/\s+/g, " ")
    .trim();
  return short || name.trim();
}

/** Pastille de la machine : « RTX 3090 · 24 GB VRAM · 32 GB RAM ». */
export function machineLabel(t: TFunction, hardware: HubHardware): string {
  const ram = t("hub.machine.ram", {
    size: formatBytes(t, hardware.ramTotal),
  });
  if (hardware.gpus.length === 0) {
    return `${t("hub.machine.cpuOnly")} · ${ram}`;
  }
  const vram = hardware.gpus.reduce((sum, gpu) => sum + gpu.vramTotal, 0);
  const names = [...new Set(hardware.gpus.map((gpu) => gpuName(gpu.name)))];
  const gpus =
    hardware.gpus.length > 1 && names.length === 1
      ? `${hardware.gpus.length} × ${names[0]}`
      : names.join(" + ");
  return `${gpus} · ${t("hub.machine.vram", { size: formatBytes(t, vram) })} · ${ram}`;
}

/** Infobulle de la pastille de la machine : sens des pastilles des modèles. */
export function machineHint(t: TFunction, hardware: HubHardware): string {
  const context = formatContext(t, hardware.context);
  return hasGpu(hardware)
    ? [
        t("hub.machine.hint.estimates", { context }),
        t("hub.machine.hint.fitsGpu"),
        t("hub.machine.hint.partial"),
        t("hub.machine.hint.tooLarge"),
      ].join("\n")
    : [
        t("hub.machine.hint.estimatesCpu", { context }),
        t("hub.machine.hint.fitsMemory"),
        t("hub.machine.hint.tooLarge"),
      ].join("\n");
}

/**
 * Rubriques de l'accueil (titres : `hub.sections.<id>`), dans l'ordre du
 * moteur ; « uncensored » seulement si le réglage la montre.
 */
const SECTION_IDS: readonly HubSectionId[] = [
  "forYou",
  "trending",
  "popular",
  "coding",
  "vision",
  "reasoning",
  "small",
  "uncensored",
];

/** Usages proposés en filtre, dans l'ordre des puces (`hub.uses.<id>`). */
export const USES: readonly HubUse[] = ["code", "vision", "reasoning", "tools"];

/** Tailles proposées en filtre, dans l'ordre des puces (`hub.sizes.<id>`). */
export const SIZES: readonly HubSize[] = ["fits", "small", "medium", "large"];

/**
 * Paramètres d'adresse de la page Découvrir. `uses` : usages séparés par des
 * virgules (« code,vision ») ; `license` : « open » seulement.
 */
export interface HubParams {
  q?: string;
  family?: string;
  section?: HubSectionId;
  uses?: string;
  size?: HubSize;
  license?: "open";
  verified?: boolean;
}

/** Texte d'un paramètre : le routeur lit « 8 » comme un nombre. */
function paramText(value: unknown): string | undefined {
  if (typeof value === "number" && Number.isFinite(value)) return String(value);
  if (typeof value !== "string") return undefined;
  const text = value.trim();
  return text === "" ? undefined : text;
}

/** Usages d'un paramètre `uses`, sans doublon, dans l'ordre des puces. */
export function usesFromParam(value: unknown): HubUse[] {
  const words = Array.isArray(value)
    ? value.map(String)
    : (paramText(value) ?? "").split(",");
  const wanted = new Set(words.map((word) => word.trim().toLowerCase()));
  return USES.filter((use) => wanted.has(use));
}

/** Paramètre `uses` d'une liste d'usages ; aucun : absent. */
export function usesParam(uses: readonly HubUse[]): string | undefined {
  const sorted = USES.filter((use) => uses.includes(use));
  return sorted.length > 0 ? sorted.join(",") : undefined;
}

/**
 * Paramètres d'adresse validés : toute valeur inconnue est retirée (adresse
 * tapée, ancienne version...).
 */
export function validateHubParams(raw: Record<string, unknown>): HubParams {
  const params: HubParams = {};
  const q = paramText(raw.q);
  if (q) params.q = q.slice(0, 200);
  const family = paramText(raw.family);
  if (family && /^[\w.-]{1,64}$/.test(family)) params.family = family;
  const section = paramText(raw.section);
  if (section && (SECTION_IDS as readonly string[]).includes(section)) {
    params.section = section as HubSectionId;
  }
  const uses = usesParam(usesFromParam(raw.uses));
  if (uses) params.uses = uses;
  const size = paramText(raw.size);
  if (size && (SIZES as readonly string[]).includes(size)) {
    params.size = size as HubSize;
  }
  if (paramText(raw.license) === "open") params.license = "open";
  if (raw.verified === true || raw.verified === "true") params.verified = true;
  return params;
}

/**
 * Filtres envoyés au moteur. `verifiedOnly` ne vaut que pour la recherche :
 * les autres vues ne montrent que des publieurs de confiance.
 */
export function filtersFromParams(
  params: HubParams,
  searching: boolean,
): HubFilters {
  return {
    uses: usesFromParam(params.uses),
    size: params.size ?? null,
    openLicense: params.license === "open",
    verifiedOnly: searching && params.verified === true,
  };
}

/** Attente après la dernière frappe avant de chercher. */
export const SEARCH_DEBOUNCE_MS = 400;

/** Texte de recherche retenu : 2 caractères au moins, sinon aucun. */
export function searchText(text: string): string {
  const trimmedText = text.trim();
  return trimmedText.length >= 2 ? trimmedText : "";
}

/**
 * Dégradé doux d'un avatar ou d'une tuile de famille, clair et sombre : les
 * couleurs de la palette de Tailwind (100 → 200, texte 900 ; en sombre 400 et
 * 500 translucides, texte 100), passées en variables CSS (gradientStyle) à
 * quelques classes fixes (GRADIENT_CLASS) plutôt qu'en ~80 classes de
 * couleur, ~14 Ko de CSS de moins.
 */
export interface Gradient {
  from: string;
  to: string;
  text: string;
  darkFrom: string;
  darkTo: string;
  darkText: string;
}

export const GRADIENTS: readonly Gradient[] = [
  // violet → indigo
  {
    from: "oklch(94.3% 0.029 294.588)",
    to: "oklch(87% 0.065 274.039)",
    text: "oklch(35.9% 0.144 278.697)",
    darkFrom: "oklch(70.2% 0.183 293.541 / 0.25)",
    darkTo: "oklch(58.5% 0.233 277.117 / 0.35)",
    darkText: "oklch(93% 0.034 272.788)",
  },
  // sky → blue
  {
    from: "oklch(95.1% 0.026 236.824)",
    to: "oklch(88.2% 0.059 254.128)",
    text: "oklch(37.9% 0.146 265.522)",
    darkFrom: "oklch(74.6% 0.16 232.661 / 0.25)",
    darkTo: "oklch(62.3% 0.214 259.815 / 0.35)",
    darkText: "oklch(93.2% 0.032 255.585)",
  },
  // emerald → teal
  {
    from: "oklch(95% 0.052 163.051)",
    to: "oklch(91% 0.096 180.426)",
    text: "oklch(38.6% 0.063 188.416)",
    darkFrom: "oklch(76.5% 0.177 163.223 / 0.25)",
    darkTo: "oklch(70.4% 0.14 182.503 / 0.35)",
    darkText: "oklch(95.3% 0.051 180.801)",
  },
  // amber → orange
  {
    from: "oklch(96.2% 0.059 95.617)",
    to: "oklch(90.1% 0.076 70.697)",
    text: "oklch(40.8% 0.123 38.172)",
    darkFrom: "oklch(82.8% 0.189 84.429 / 0.25)",
    darkTo: "oklch(70.5% 0.213 47.604 / 0.35)",
    darkText: "oklch(95.4% 0.038 75.164)",
  },
  // rose → pink
  {
    from: "oklch(94.1% 0.03 12.58)",
    to: "oklch(89.9% 0.061 343.231)",
    text: "oklch(41% 0.159 10.272)",
    darkFrom: "oklch(71.2% 0.194 13.428 / 0.25)",
    darkTo: "oklch(65.6% 0.241 354.308 / 0.35)",
    darkText: "oklch(94.1% 0.03 12.58)",
  },
  // cyan → sky
  {
    from: "oklch(95.6% 0.045 203.388)",
    to: "oklch(90.1% 0.058 230.902)",
    text: "oklch(39.8% 0.07 227.392)",
    darkFrom: "oklch(78.9% 0.154 211.53 / 0.25)",
    darkTo: "oklch(68.5% 0.169 237.323 / 0.35)",
    darkText: "oklch(95.6% 0.045 203.388)",
  },
  // lime → green
  {
    from: "oklch(96.7% 0.067 122.328)",
    to: "oklch(92.5% 0.084 155.995)",
    text: "oklch(39.3% 0.095 152.535)",
    darkFrom: "oklch(84.1% 0.238 128.85 / 0.25)",
    darkTo: "oklch(72.3% 0.219 149.579 / 0.35)",
    darkText: "oklch(96.2% 0.044 156.743)",
  },
  // fuchsia → purple
  {
    from: "oklch(95.2% 0.037 318.852)",
    to: "oklch(90.2% 0.063 306.703)",
    text: "oklch(38.1% 0.176 304.987)",
    darkFrom: "oklch(74% 0.238 322.16 / 0.25)",
    darkTo: "oklch(62.7% 0.265 303.9 / 0.35)",
    darkText: "oklch(94.6% 0.033 307.174)",
  },
  // sable (échelle chaude du thème, à la place du gris slate → zinc)
  {
    from: "oklch(96.2% 0.02 74.662)",
    to: "oklch(87.6% 0.041 69.691)",
    text: "oklch(29.3% 0.028 47.744)",
    darkFrom: "oklch(87.6% 0.041 69.691 / 0.25)",
    darkTo: "oklch(73.8% 0.046 65.894 / 0.35)",
    darkText: "oklch(96.2% 0.02 74.662)",
  },
  // yellow → amber
  {
    from: "oklch(97.3% 0.071 103.193)",
    to: "oklch(92.4% 0.12 95.746)",
    text: "oklch(41.4% 0.112 45.904)",
    darkFrom: "oklch(85.2% 0.199 91.936 / 0.25)",
    darkTo: "oklch(76.9% 0.188 70.08 / 0.35)",
    darkText: "oklch(96.2% 0.059 95.617)",
  },
];

/**
 * Classes communes des teintes (couleurs : variables de gradientStyle). Un
 * aplat, celui de la fin du dégradé : la direction visuelle de ChatonLM
 * n'emploie pas de dégradés (les teintes de départ restent dans la palette).
 */
export const GRADIENT_CLASS =
  "bg-(--hub-to) text-(--hub-text) dark:bg-(--hub-to-dark) dark:text-(--hub-text-dark)";

/** Couleurs choisies pour les familles connues ; les autres : hachage. */
const FAMILY_GRADIENTS: Record<string, number> = {
  qwen: 0,
  gemma: 1,
  phi: 2,
  mistral: 3,
  glm: 4,
  deepseek: 5,
  granite: 6,
  lfm: 7,
  "gpt-oss": 8,
  llama: 9,
  smollm: 3,
  minicpm: 4,
  nemotron: 6,
  kimi: 5,
};

/**
 * Familles reconnues dans un nom (modèle, dépôt, `details.family` d'Ollama),
 * dans l'ordre de curation.json : DeepSeek avant Qwen pour les distillations
 * R1 (« DeepSeek-R1-Distill-Qwen »).
 */
const FAMILY_PATTERNS: [string, RegExp][] = [
  ["deepseek", /deepseek/],
  ["gpt-oss", /gpt-oss/],
  ["qwen", /qwen|qwq/],
  ["gemma", /gemma/],
  ["llama", /llama|muse-glimmer/],
  ["mistral", /mistral|ministral|magistral|devstral|codestral|mixtral|pixtral/],
  ["phi", /(^|[^a-z])phi-?[0-9]/],
  ["granite", /granite/],
  ["lfm", /lfm[0-9]/],
  ["glm", /glm/],
  ["kimi", /kimi/],
  ["nemotron", /nemotron/],
  ["minicpm", /minicpm/],
  ["smollm", /smollm/],
];

/** Famille reconnue dans le premier nom qui en contient une, sinon null. */
export function familyOf(
  ...names: (string | null | undefined)[]
): string | null {
  for (const name of names) {
    const lower = name?.toLowerCase() ?? "";
    for (const [id, pattern] of FAMILY_PATTERNS) {
      if (pattern.test(lower)) return id;
    }
  }
  return null;
}

/** Hachage FNV-1a 32 bits : même nom, même couleur, d'un lancement à l'autre. */
function hash(text: string): number {
  let value = 0x811c9dc5;
  for (let i = 0; i < text.length; i++) {
    value ^= text.charCodeAt(i);
    value = Math.imul(value, 0x01000193);
  }
  return value >>> 0;
}

/** Dégradé d'une famille ou d'un auteur. */
export function gradientFor(key: string): Gradient {
  const name = key.trim().toLowerCase();
  const index = FAMILY_GRADIENTS[name] ?? hash(name) % GRADIENTS.length;
  return GRADIENTS[index];
}

/** Variables CSS du dégradé d'une famille ou d'un auteur (avec GRADIENT_CLASS). */
export function gradientStyle(key: string): CSSProperties {
  const gradient = gradientFor(key);
  return {
    "--hub-from": gradient.from,
    "--hub-to": gradient.to,
    "--hub-text": gradient.text,
    "--hub-from-dark": gradient.darkFrom,
    "--hub-to-dark": gradient.darkTo,
    "--hub-text-dark": gradient.darkText,
  } as CSSProperties;
}

/**
 * Initiales d'un nom de modèle : première lettre, puis le premier chiffre
 * collé au premier mot ou formant le second (« Qwen3-8B » → « Q3 »,
 * « gemma-4-12B » → « G4 »), sinon l'initiale du second mot (« gpt-oss » →
 * « GO »), sinon la deuxième lettre (« Mistral » → « Mi »).
 */
export function initials(name: string): string {
  const words = name
    .replace(/[-_]gguf$/i, "")
    .split(/[\s\-_/:.]+/)
    .filter(Boolean);
  const first = words[0] ?? "";
  const letter = first.match(/[A-Za-z0-9]/)?.[0]?.toUpperCase() ?? "?";
  const attached = first.match(/^[A-Za-z]+(\d)/)?.[1];
  if (attached) return letter + attached;
  const second = words[1] ?? "";
  const number = second.match(/^(\d)/)?.[1];
  if (number) return letter + number;
  const initial = second.match(/[A-Za-z]/)?.[0];
  if (initial) return letter + initial.toUpperCase();
  const next = first.slice(1).match(/[A-Za-z0-9]/)?.[0];
  return next ? letter + next.toLowerCase() : letter;
}

/** Identifiant « auteur/dépôt » en paramètres de route. */
export function repoParams(id: string): { author: string; repo: string } {
  const slash = id.indexOf("/");
  return slash === -1
    ? { author: id, repo: "" }
    : { author: id.slice(0, slash), repo: id.slice(slash + 1) };
}

/** Page du dépôt sur Hugging Face. */
export function huggingFaceUrl(id: string): string {
  return `https://huggingface.co/${id}`;
}
