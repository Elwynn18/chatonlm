// Faux moteur du ModelHub pour le simulateur (tests vitest seulement) :
// catalogue de démonstration réaliste (familles, publieurs vérifiés ou non,
// vision, code, réflexion, versions découpées, dépôt restreint), machine RTX
// 3090 24 GB / 32 GB RAM, téléchargements en mémoire (deux à la fois, file ;
// noms hf.co/... ou de la bibliothèque d'Ollama comme le moteur),
// modèles installés tirés de la liste du faux backend (list_models). Règles
// simplifiées de celles du moteur (contrat 37 § 2-6). Le test force hors
// ligne, panne, limite de requêtes et données anciennes par `problem` et
// `cached`, une panne interne par `failures`, et fait avancer un
// téléchargement par progress(), finish() et fail(), qui émettent
// l'événement « hub-downloads » comme le moteur.

import type { AppError } from "@/bindings/AppError";
import type { HubCapabilities } from "@/bindings/HubCapabilities";
import type { HubDownload } from "@/bindings/HubDownload";
import type { HubDownloadRequest } from "@/bindings/HubDownloadRequest";
import type { HubFamily } from "@/bindings/HubFamily";
import type { HubFilters } from "@/bindings/HubFilters";
import type { HubFit } from "@/bindings/HubFit";
import type { HubHardware } from "@/bindings/HubHardware";
import type { HubHome } from "@/bindings/HubHome";
import type { HubInstalled } from "@/bindings/HubInstalled";
import type { HubList } from "@/bindings/HubList";
import type { HubModel } from "@/bindings/HubModel";
import type { HubProblem } from "@/bindings/HubProblem";
import type { HubQuant } from "@/bindings/HubQuant";
import type { HubQuantTier } from "@/bindings/HubQuantTier";
import type { HubReadme } from "@/bindings/HubReadme";
import type { HubRepo } from "@/bindings/HubRepo";
import type { HubRepoResult } from "@/bindings/HubRepoResult";
import type { HubScope } from "@/bindings/HubScope";
import type { HubSearch } from "@/bindings/HubSearch";
import type { HubSectionId } from "@/bindings/HubSectionId";
import type { HubStatus } from "@/bindings/HubStatus";
import type { HubTrust } from "@/bindings/HubTrust";
import type { Args, Simulator } from "@/test/simulator";

export const GIB = 1024 ** 3;

/** Machine de démonstration : RTX 3090 24 GB, 32 GB de RAM, contexte 32K. */
export const RTX_3090: HubHardware = {
  gpus: [
    {
      name: "NVIDIA GeForce RTX 3090",
      vramTotal: 24 * GIB,
      vramFree: 22 * GIB,
    },
  ],
  ramTotal: 32 * GIB,
  ramAvailable: 20 * GIB,
  context: 32768,
};

/**
 * Date des données « en cache » (données anciennes) : trois jours avant le
 * lancement des essais, pour un bandeau « saved 3 days ago » quel que soit
 * le jour (une date fixe devenait « last week » au bout d'une semaine).
 */
export const SAVED_AT = Date.now() - 3 * 24 * 60 * 60 * 1000;

/** Libellés des familles (curation.json du moteur). */
export const FAMILY_LABELS: Record<string, string> = {
  qwen: "Qwen",
  gemma: "Gemma",
  "gpt-oss": "gpt-oss",
  mistral: "Mistral",
  llama: "Llama",
  phi: "Phi",
  granite: "Granite",
  deepseek: "DeepSeek",
  smollm: "SmolLM",
  lfm: "LFM (Liquid AI)",
};

/** Dépôt de démonstration : résumé, et ce qui ne sert qu'au faux moteur. */
export interface DemoRepo {
  model: HubModel;
  /** Score de tendance (rubrique Trending). */
  trend: number;
  gated?: boolean;
  languages?: string[];
  /** Versions imposées ; sinon une gamme standard. */
  quants?: Omit<HubQuant, "fit" | "memory" | "recommended">[];
  /** Variantes à ajouter à la gamme standard (fichier, étiquette). */
  extra?: { label: string; file: string; bits: number; tier: HubQuantTier }[];
  /** Version rangée dans un sous-dossier (non téléchargeable). */
  subfolder?: boolean;
  otherSources?: string[];
}

function caps(...names: (keyof HubCapabilities)[]): HubCapabilities {
  return {
    tools: names.includes("tools"),
    thinking: names.includes("thinking"),
    vision: names.includes("vision"),
    code: names.includes("code"),
  };
}

function demo(
  id: string,
  fields: {
    family: string | null;
    trust: HubTrust;
    parameters: number;
    architecture: string;
    contextLength: number;
    capabilities: HubCapabilities;
    license: string | null;
    downloads: number;
    likes: number;
    lastModified: string;
    baseModel?: string;
  },
  extra: Omit<DemoRepo, "model"> = { trend: 0 },
): DemoRepo {
  const [author, repo] = id.split("/");
  return {
    ...extra,
    model: {
      id,
      author,
      name: repo.replace(/[-_]gguf$/i, ""),
      family: fields.family,
      baseModel: fields.baseModel ?? null,
      trust: fields.trust,
      parameters: fields.parameters,
      architecture: fields.architecture,
      contextLength: fields.contextLength,
      capabilities: fields.capabilities,
      license: fields.license,
      downloads: fields.downloads,
      likes: fields.likes,
      lastModified: fields.lastModified,
      estimatedSize: null,
      estimatedMemory: null,
      fit: null,
    },
  };
}

/** Catalogue de démonstration (copie neuve à chaque appel). */
export function demoCatalog(): DemoRepo[] {
  return [
    demo(
      "unsloth/Qwen3.5-9B-GGUF",
      {
        family: "qwen",
        trust: "quantizer",
        parameters: 9.65e9,
        architecture: "qwen35",
        contextLength: 262144,
        capabilities: caps("tools", "thinking", "vision"),
        license: "apache-2.0",
        downloads: 1_850_000,
        likes: 920,
        lastModified: "2026-09-28T10:00:00Z",
        baseModel: "Qwen/Qwen3.5-9B",
      },
      {
        trend: 95,
        languages: ["en", "zh", "fr", "de", "es", "ja"],
        extra: [
          {
            label: "UD-Q4_K_XL",
            file: "Qwen3.5-9B-UD-Q4_K_XL.gguf",
            bits: 5.0,
            tier: "balanced",
          },
        ],
        subfolder: true,
        otherSources: ["Qwen/Qwen3-8B-GGUF", "bartowski/Qwen3.8-27B-GGUF"],
      },
    ),
    demo(
      "google/gemma-4-12B-it-qat-q4_0-gguf",
      {
        family: "gemma",
        trust: "lab",
        parameters: 12.2e9,
        architecture: "gemma4",
        contextLength: 131072,
        capabilities: caps("tools", "vision"),
        license: "apache-2.0",
        downloads: 960_000,
        likes: 780,
        lastModified: "2026-09-20T08:00:00Z",
      },
      { trend: 90, languages: ["en"] },
    ),
    demo(
      "bartowski/Qwen3.8-27B-GGUF",
      {
        family: "qwen",
        trust: "quantizer",
        parameters: 27.2e9,
        architecture: "qwen35",
        contextLength: 131072,
        capabilities: caps("tools", "thinking"),
        license: "apache-2.0",
        downloads: 268_000,
        likes: 154,
        lastModified: "2026-09-30T12:00:00Z",
        baseModel: "Qwen/Qwen3.8-27B",
      },
      {
        trend: 88,
        extra: [
          {
            label: "Qwen3.8-27B-Q4_K_L.gguf",
            file: "Qwen3.8-27B-Q4_K_L.gguf",
            bits: 5.1,
            tier: "balanced",
          },
        ],
      },
    ),
    demo(
      "unsloth/gpt-oss-20b-GGUF",
      {
        family: "gpt-oss",
        trust: "quantizer",
        parameters: 20.9e9,
        architecture: "gpt-oss",
        contextLength: 131072,
        capabilities: caps("tools", "thinking"),
        license: "apache-2.0",
        downloads: 1_200_000,
        likes: 650,
        lastModified: "2026-08-14T08:00:00Z",
      },
      { trend: 60 },
    ),
    demo(
      "LiquidAI/LFM2-1.2B-GGUF",
      {
        family: "lfm",
        trust: "lab",
        parameters: 1.17e9,
        architecture: "lfm2",
        contextLength: 32768,
        capabilities: caps("tools"),
        license: "other",
        downloads: 120_000,
        likes: 140,
        lastModified: "2026-07-02T08:00:00Z",
      },
      { trend: 45 },
    ),
    demo(
      "Qwen/Qwen3-8B-GGUF",
      {
        family: "qwen",
        trust: "lab",
        parameters: 8.19e9,
        architecture: "qwen3",
        contextLength: 40960,
        capabilities: caps("tools", "thinking"),
        license: "apache-2.0",
        downloads: 412_000,
        likes: 310,
        lastModified: "2026-05-02T08:00:00Z",
      },
      { trend: 40, languages: ["en", "zh"] },
    ),
    demo(
      "ibm-granite/granite-4.0-h-tiny-GGUF",
      {
        family: "granite",
        trust: "lab",
        parameters: 6.94e9,
        architecture: "granitehybrid",
        contextLength: 131072,
        capabilities: caps("tools"),
        license: "apache-2.0",
        downloads: 54_000,
        likes: 48,
        lastModified: "2026-06-10T08:00:00Z",
      },
      { trend: 35 },
    ),
    demo(
      "lmstudio-community/Devstral-Small-2507-GGUF",
      {
        family: "mistral",
        trust: "quantizer",
        parameters: 23.6e9,
        architecture: "llama",
        contextLength: 131072,
        capabilities: caps("tools", "code"),
        license: "apache-2.0",
        downloads: 98_000,
        likes: 60,
        lastModified: "2026-07-21T08:00:00Z",
      },
      { trend: 30 },
    ),
    demo(
      "unsloth/Phi-4-mini-reasoning-GGUF",
      {
        family: "phi",
        trust: "quantizer",
        parameters: 3.84e9,
        architecture: "phi3",
        contextLength: 131072,
        capabilities: caps("thinking"),
        license: "mit",
        downloads: 76_000,
        likes: 70,
        lastModified: "2026-04-30T08:00:00Z",
      },
      { trend: 25 },
    ),
    demo(
      "ggml-org/SmolLM3-3B-GGUF",
      {
        family: "smollm",
        trust: "quantizer",
        parameters: 3.08e9,
        architecture: "smollm3",
        contextLength: 65536,
        capabilities: caps("tools", "thinking"),
        license: "apache-2.0",
        downloads: 33_000,
        likes: 41,
        lastModified: "2026-03-11T08:00:00Z",
      },
      { trend: 22 },
    ),
    demo(
      "Qwen/Qwen2.5-Coder-7B-Instruct-GGUF",
      {
        family: "qwen",
        trust: "lab",
        parameters: 7.62e9,
        architecture: "qwen2",
        contextLength: 32768,
        capabilities: caps("tools", "code"),
        license: "apache-2.0",
        downloads: 520_000,
        likes: 410,
        lastModified: "2025-11-20T08:00:00Z",
      },
      { trend: 20 },
    ),
    demo(
      "unsloth/DeepSeek-R1-Distill-Qwen-14B-GGUF",
      {
        family: "deepseek",
        trust: "quantizer",
        parameters: 14.8e9,
        architecture: "qwen2",
        contextLength: 131072,
        capabilities: caps("thinking"),
        license: "mit",
        downloads: 640_000,
        likes: 330,
        lastModified: "2025-12-01T08:00:00Z",
      },
      { trend: 18 },
    ),
    demo(
      "ggml-org/gemma-3-270m-it-GGUF",
      {
        family: "gemma",
        trust: "quantizer",
        parameters: 268e6,
        architecture: "gemma3",
        contextLength: 32768,
        capabilities: caps(),
        license: "gemma",
        downloads: 145_000,
        likes: 95,
        lastModified: "2026-02-05T08:00:00Z",
      },
      { trend: 15 },
    ),
    demo(
      "bartowski/Llama-3.3-70B-Instruct-GGUF",
      {
        family: "llama",
        trust: "quantizer",
        parameters: 70.6e9,
        architecture: "llama",
        contextLength: 131072,
        capabilities: caps("tools"),
        license: "llama3.3",
        downloads: 310_000,
        likes: 280,
        lastModified: "2025-10-08T08:00:00Z",
      },
      { trend: 10 },
    ),
    // Recherche seulement : publieur hors de la liste de confiance.
    demo(
      "janedoe/Qwen3-8B-Medical-GGUF",
      {
        family: "qwen",
        trust: "unverified",
        parameters: 8.19e9,
        architecture: "qwen3",
        contextLength: 40960,
        capabilities: caps("tools", "thinking"),
        license: "apache-2.0",
        downloads: 2_300,
        likes: 12,
        lastModified: "2026-09-02T08:00:00Z",
      },
      { trend: 5 },
    ),
    // Restreint : jamais listé, fiche sans bouton de téléchargement.
    demo(
      "google/gemma-3-27b-it-qat-q4_0-gguf",
      {
        family: "gemma",
        trust: "lab",
        parameters: 27e9,
        architecture: "gemma3",
        contextLength: 131072,
        capabilities: caps("vision"),
        license: "gemma",
        downloads: 230_000,
        likes: 400,
        lastModified: "2025-04-18T08:00:00Z",
      },
      { trend: 8, gated: true },
    ),
  ];
}

/** Gamme standard : bits par poids des quantifications courantes. */
const LADDER: { label: string; bits: number; tier: HubQuantTier }[] = [
  { label: "BF16", bits: 16, tier: "full" },
  { label: "Q8_0", bits: 8.5, tier: "high" },
  { label: "Q6_K", bits: 6.56, tier: "high" },
  { label: "Q5_K_M", bits: 5.69, tier: "balanced" },
  { label: "Q4_K_M", bits: 4.85, tier: "balanced" },
  { label: "IQ4_XS", bits: 4.25, tier: "balanced" },
  { label: "Q3_K_M", bits: 3.91, tier: "compact" },
  { label: "Q2_K", bits: 3.0, tier: "tiny" },
];

/** Ordre de préférence de la version conseillée. */
const PREFERRED = ["Q8_0", "Q6_K", "Q5_K_M", "Q4_K_M"];

/** Taille d'un projecteur de vision (mmproj F16). */
const PROJECTOR = 0.9 * GIB;

export interface FakeHub {
  /** Machine du serveur ; null : serveur distant. */
  hardware: HubHardware | null;
  /** Dépôts connus (catalogue et recherche). */
  catalog: DemoRepo[];
  /** README par dépôt ; absent : texte généré, null : pas de README. */
  readmes: Map<string, string | null>;
  /**
   * Problème de Hugging Face pour home, browse, search, repo et readme ;
   * `problems` le précise par commande (null : aucun).
   */
  problem: HubProblem | null;
  problems: Partial<Record<string, HubProblem | null>>;
  /** Données en cache disponibles quand un problème est posé. */
  cached: boolean;
  /** Erreur (panne interne) renvoyée par une commande, par nom. */
  failures: Map<string, AppError>;
  /** Résultats par page de recherche. */
  pageSize: number;
  /** Téléchargements, dans l'ordre de demande. */
  downloads: HubDownload[];
  /** Détails des modèles installés, par nom (sans la casse). */
  installedInfo: Map<string, Partial<HubInstalled>>;
  /** Émet la liste des téléchargements (événement du moteur). */
  emit(): Promise<void>;
  /** Fait avancer un téléchargement (état downloading), puis émet. */
  progress(
    model: string,
    completed: number,
    options?: { total?: number; bytesPerSecond?: number | null },
  ): Promise<void>;
  /** Change l'état d'un téléchargement, puis émet. */
  setState(model: string, state: HubDownload["state"]): Promise<void>;
  /** Termine un téléchargement : modèle installé, puis émet. */
  finish(model: string): Promise<void>;
  /** Fait échouer un téléchargement avec ce message, puis émet. */
  fail(model: string, error: string): Promise<void>;
}

/** Ce que le faux moteur partage avec le reste du faux backend. */
export interface HubHost {
  /** Modèles installés (list_models), du plus récent au plus ancien. */
  models: string[];
  /** Modèles dont le registre a une version plus récente (outdated_models). */
  outdated: Set<string>;
}

const ACTIVE: HubDownload["state"][] = [
  "queued",
  "starting",
  "downloading",
  "finishing",
];

function lower(text: string) {
  return text.trim().toLowerCase();
}

/**
 * Nom comme Ollama le résout : sans la casse, « :latest » s'il n'a pas
 * d'étiquette (cherchée après le dernier « / »).
 */
export function canonicalModel(name: string): string {
  const model = lower(name);
  return model.slice(model.lastIndexOf("/") + 1).includes(":")
    ? model
    : `${model}:latest`;
}

/**
 * Gestionnaires des 12 commandes hub_*, branchés sur le simulateur (par
 * fakeBackend) ; `host` donne et reçoit les modèles installés.
 */
export function fakeHub(simulator: Simulator, host: HubHost): FakeHub {
  const hub: FakeHub = {
    hardware: RTX_3090,
    catalog: demoCatalog(),
    readmes: new Map(),
    problem: null,
    problems: {},
    cached: true,
    failures: new Map(),
    pageSize: 6,
    downloads: [],
    installedInfo: new Map(),
    emit: () => simulator.emitEvent("hub-downloads", hub.downloads),
    async progress(model, completed, options = {}) {
      const download = entry(model);
      download.state = "downloading";
      download.completed = completed;
      if (options.total !== undefined) download.total = options.total;
      if (options.bytesPerSecond !== undefined) {
        download.bytesPerSecond = options.bytesPerSecond;
      }
      await hub.emit();
    },
    async setState(model, state) {
      entry(model).state = state;
      await hub.emit();
    },
    async finish(model) {
      const download = entry(model);
      download.state = "done";
      if (download.total === 0) download.total = download.completed || GIB;
      download.completed = download.total;
      download.bytesPerSecond = null;
      download.finishedAt = Date.now();
      const same = (name: string) =>
        canonicalModel(name) === canonicalModel(model);
      if (!host.models.some(same)) host.models.unshift(download.model);
      // Téléchargé de nouveau : c'est la version du registre.
      for (const name of [...host.outdated]) {
        if (same(name)) host.outdated.delete(name);
      }
      promote();
      await hub.emit();
    },
    async fail(model, error) {
      const download = entry(model);
      download.state = "failed";
      download.error = error;
      download.bytesPerSecond = null;
      download.finishedAt = Date.now();
      promote();
      await hub.emit();
    },
  };

  function entry(model: string): HubDownload {
    const found = hub.downloads.find((d) => lower(d.model) === lower(model));
    if (!found) throw simulator.fail(`no download of "${model}"`);
    return found;
  }

  /** Deux téléchargements à la fois : le premier en file démarre. */
  function promote() {
    const running = hub.downloads.filter(
      (d) => d.state !== "queued" && ACTIVE.includes(d.state),
    ).length;
    if (running >= 2) return;
    const next = hub.downloads.find((d) => d.state === "queued");
    if (next) next.state = "starting";
  }

  function failure(command: string) {
    const error = hub.failures.get(command);
    if (error) throw error;
  }

  function problemFor(command: string): HubProblem | null {
    return command in hub.problems
      ? (hub.problems[command] ?? null)
      : hub.problem;
  }

  /** État des données : fraîches, anciennes (problème + cache) ou absentes. */
  function status(command: string): { status: HubStatus; data: boolean } {
    const problem = problemFor(command);
    if (!problem)
      return { status: { fetchedAt: Date.now(), problem }, data: true };
    return hub.cached
      ? { status: { fetchedAt: SAVED_AT, problem }, data: true }
      : { status: { fetchedAt: null, problem }, data: false };
  }

  function fitFor(memory: number): HubFit | null {
    const machine = hub.hardware;
    if (!machine) return null;
    const vram = machine.gpus.reduce((sum, gpu) => sum + gpu.vramTotal, 0);
    const ram = machine.ramTotal - 6 * GIB;
    if (vram > 0) {
      if (memory <= 0.9 * vram) return "fits";
      if (memory <= 0.9 * vram + ram) return "partial";
      return "tooLarge";
    }
    return memory <= ram ? "fits" : "tooLarge";
  }

  /** Mémoire au chargement : poids, cache de contexte, marge. */
  function memoryFor(size: number, parameters: number): number {
    const context = hub.hardware?.context ?? 4096;
    return (
      size + context * (100 * 1024 + 3.3 * 1024 * (parameters / 1e9)) + GIB
    );
  }

  /** Résumé d'un dépôt, estimé pour la machine du moment. */
  function summary(repo: DemoRepo): HubModel {
    const model = { ...repo.model };
    const parameters = model.parameters ?? 0;
    const size = 0.64 * parameters * (model.capabilities.vision ? 1.3 : 1);
    model.estimatedSize = Math.round(size);
    model.estimatedMemory = hub.hardware
      ? Math.round(memoryFor(size, parameters))
      : null;
    model.fit = fitFor(memoryFor(size, parameters));
    return model;
  }

  function quants(repo: DemoRepo): HubQuant[] {
    const parameters = repo.model.parameters ?? 0;
    const name = repo.model.id.split("/")[1].replace(/[-_]gguf$/i, "");
    const projector = repo.model.capabilities.vision ? PROJECTOR : 0;
    const base =
      repo.quants ??
      [...LADDER, ...(repo.extra ?? [])]
        .map((step) => {
          const size = Math.round((parameters * step.bits) / 8);
          const file =
            "file" in step
              ? (step.file as string)
              : `${name}-${step.label}.gguf`;
          // Les gros dépôts découpent leurs versions lourdes.
          const sharded = parameters > 30e9 && step.bits >= 8;
          const subfolder = repo.subfolder === true && step.label === "BF16";
          return {
            label: step.label,
            file: sharded
              ? `${name}-${step.label}/${name}-${step.label}-00001-of-00002.gguf`
              : subfolder
                ? `BF16/${file}`
                : file,
            model:
              sharded || subfolder
                ? null
                : `hf.co/${repo.model.id}:${step.label}`,
            issue: sharded
              ? ("sharded" as const)
              : subfolder
                ? ("subfolder" as const)
                : null,
            size,
            downloadSize: size + projector,
            tier: step.tier,
          };
        })
        .sort((a, b) => b.size - a.size);
    const list: HubQuant[] = base.map((quant) => {
      const memory = memoryFor(quant.downloadSize, parameters);
      return {
        ...quant,
        fit: fitFor(memory),
        memory: hub.hardware ? Math.round(memory) : null,
        recommended: false,
      };
    });
    const available = list.filter((quant) => quant.model !== null);
    const fitting = hub.hardware
      ? PREFERRED.map((label) =>
          available.find((q) => q.label === label && q.fit === "fits"),
        ).find(Boolean)
      : undefined;
    const pick =
      fitting ??
      available.find((q) => q.label === "Q4_K_M") ??
      available.find((q) => q.tier === "balanced");
    if (pick) pick.recommended = true;
    return list;
  }

  function matches(repo: DemoRepo, filters: HubFilters, search: boolean) {
    const model = summary(repo);
    const capabilities = model.capabilities;
    for (const use of filters.uses) {
      if (use === "reasoning" && !capabilities.thinking) return false;
      if (use !== "reasoning" && !capabilities[use]) return false;
    }
    const parameters = model.parameters ?? 0;
    switch (filters.size) {
      case "fits":
        if (hub.hardware && model.fit !== "fits") return false;
        break;
      case "small":
        if (parameters >= 4e9) return false;
        break;
      case "medium":
        if (parameters < 4e9 || parameters > 15e9) return false;
        break;
      case "large":
        if (parameters <= 15e9) return false;
        break;
    }
    const open = ["apache-2.0", "mit", "bsd-3-clause", "isc", "cc-by-4.0"];
    if (filters.openLicense && !open.includes(model.license ?? ""))
      return false;
    if (repo.gated) return false;
    if (!search && model.trust === "unverified") return false;
    if (search && filters.verifiedOnly && model.trust === "unverified") {
      return false;
    }
    return true;
  }

  function catalogFor(filters: HubFilters): DemoRepo[] {
    return hub.catalog.filter((repo) => matches(repo, filters, false));
  }

  const byDownloads = (a: HubModel, b: HubModel) => b.downloads - a.downloads;

  function section(id: HubSectionId, repos: DemoRepo[]): HubModel[] {
    const models = repos.map(summary);
    switch (id) {
      case "forYou": {
        if (!hub.hardware) return [];
        const best = new Map<string, HubModel>();
        for (const model of models) {
          if (model.fit !== "fits") continue;
          const key = model.family ?? model.id;
          const current = best.get(key);
          if (!current || (model.parameters ?? 0) > (current.parameters ?? 0)) {
            best.set(key, model);
          }
        }
        return [...best.values()].sort(byDownloads);
      }
      case "trending":
        return repos
          .slice()
          .sort((a, b) => b.trend - a.trend)
          .map(summary);
      case "popular":
        return models.sort(byDownloads);
      case "coding":
        return models.filter((m) => m.capabilities.code).sort(byDownloads);
      case "vision":
        return models.filter((m) => m.capabilities.vision).sort(byDownloads);
      case "reasoning":
        return models.filter((m) => m.capabilities.thinking).sort(byDownloads);
      case "small":
        return models
          .filter((m) => (m.parameters ?? 0) < 4e9)
          .sort(byDownloads);
    }
  }

  const SECTIONS: HubSectionId[] = [
    "forYou",
    "trending",
    "popular",
    "coding",
    "vision",
    "reasoning",
    "small",
  ];

  simulator.handle("hub_home", (args: Args): HubHome => {
    failure("hub_home");
    const { status: state, data } = status("hub_home");
    if (!data) return { sections: [], families: [], status: state };
    const repos = catalogFor(args.filters as HubFilters);
    const counts = new Map<string, { count: number; downloads: number }>();
    for (const repo of repos) {
      const family = repo.model.family;
      if (!family) continue;
      const current = counts.get(family) ?? { count: 0, downloads: 0 };
      current.count += 1;
      current.downloads += repo.model.downloads;
      counts.set(family, current);
    }
    const families: HubFamily[] = [...counts.entries()]
      .sort((a, b) => b[1].downloads - a[1].downloads)
      .map(([id, { count }]) => ({
        id,
        label: FAMILY_LABELS[id] ?? id,
        count,
      }));
    return {
      sections: SECTIONS.map((id) => ({
        id,
        models: section(id, repos).slice(0, 6),
      })).filter((s) => s.models.length > 0),
      families,
      status: state,
    };
  });

  simulator.handle("hub_browse", (args: Args): HubList => {
    failure("hub_browse");
    const { status: state, data } = status("hub_browse");
    if (!data) return { models: [], nextCursor: null, status: state };
    const scope = args.scope as HubScope;
    const repos = catalogFor(args.filters as HubFilters);
    const models =
      scope.kind === "section"
        ? section(scope.id, repos)
        : repos
            .filter((repo) => repo.model.family === scope.id)
            .map(summary)
            .sort(byDownloads);
    return { models, nextCursor: null, status: state };
  });

  simulator.handle("hub_search", (args: Args): HubList => {
    failure("hub_search");
    const search = args.search as HubSearch;
    const { status: state, data } = status("hub_search");
    if (!data) return { models: [], nextCursor: null, status: state };
    const words = lower(search.text).split(/\s+/).filter(Boolean);
    const found = hub.catalog
      .filter((repo) =>
        words.every((word) => lower(repo.model.id).includes(word)),
      )
      .filter((repo) => matches(repo, search.filters, true))
      .map(summary)
      .sort(byDownloads);
    const start = Number(search.cursor ?? 0);
    const end = start + hub.pageSize;
    return {
      models: found.slice(start, end),
      nextCursor: end < found.length ? String(end) : null,
      status: state,
    };
  });

  simulator.handle("hub_repo", (args: Args): HubRepoResult => {
    failure("hub_repo");
    const id = args.id as string;
    const { status: state, data } = status("hub_repo");
    const repo = hub.catalog.find((r) => lower(r.model.id) === lower(id));
    if (!repo) {
      return {
        repo: null,
        status: {
          fetchedAt: null,
          problem: state.problem ?? { kind: "notFound" },
        },
      };
    }
    if (!data) return { repo: null, status: state };
    const detail: HubRepo = {
      model: summary(repo),
      sha: `sha-${repo.model.id}`,
      gated: repo.gated === true,
      createdAt: "2025-01-15T08:00:00Z",
      languages: repo.languages ?? [],
      quants: repo.gated ? [] : quants(repo),
      contextUsed: hub.hardware
        ? Math.min(hub.hardware.context, repo.model.contextLength ?? Infinity)
        : null,
      otherSources: repo.otherSources ?? [],
    };
    const recommended = detail.quants.find((q) => q.recommended);
    if (recommended) detail.model.estimatedSize = recommended.downloadSize;
    if (recommended) detail.model.estimatedMemory = recommended.memory;
    if (recommended) detail.model.fit = recommended.fit;
    return { repo: detail, status: state };
  });

  simulator.handle("hub_readme", (args: Args): HubReadme => {
    failure("hub_readme");
    const id = args.id as string;
    const problem = problemFor("hub_readme");
    if (problem && !hub.cached) return { text: null, problem };
    const text = hub.readmes.has(id)
      ? (hub.readmes.get(id) ?? null)
      : defaultReadme(id);
    return { text, problem: null };
  });

  simulator.handle("hub_hardware", () => {
    failure("hub_hardware");
    return hub.hardware;
  });

  simulator.handle("hub_download", (args: Args) => {
    failure("hub_download");
    const request = args.request as HubDownloadRequest;
    // Comme valid_name du moteur : hf.co/<auteur>/<dépôt>:<étiquette>, ou un
    // nom de la bibliothèque d'Ollama (« gemma3:4b », onglet Polish).
    if (
      !/^hf\.co\/[^/:\s]+\/[^/:\s]+:[^/:\s]+$/i.test(request.model) &&
      !/^[a-z0-9][a-z0-9._-]*(\/[a-z0-9._-]+)?(:[\w.-]+)?$/i.test(request.model)
    ) {
      throw {
        kind: "invalid",
        message: `invalid model name: ${request.model}`,
      };
    }
    const existing = hub.downloads.find(
      (d) => lower(d.model) === lower(request.model),
    );
    if (existing && ACTIVE.includes(existing.state)) return null;
    // En pause : repris, à sa place.
    if (existing?.state === "paused") {
      existing.state = "queued";
      promote();
      return null;
    }
    if (existing) hub.downloads.splice(hub.downloads.indexOf(existing), 1);
    const running = hub.downloads.filter(
      (d) => d.state !== "queued" && ACTIVE.includes(d.state),
    ).length;
    hub.downloads.push({
      model: request.model,
      repo: request.repo,
      label: request.label,
      state: running >= 2 ? "queued" : "starting",
      completed: 0,
      total: 0,
      bytesPerSecond: null,
      error: null,
      startedAt: Date.now(),
      finishedAt: null,
    });
    return null;
  });

  simulator.handle("hub_cancel_download", (args: Args) => {
    failure("hub_cancel_download");
    const model = args.model as string;
    const download = hub.downloads.find((d) => lower(d.model) === lower(model));
    if (
      download &&
      (ACTIVE.includes(download.state) || download.state === "paused")
    ) {
      download.state = "cancelled";
      download.bytesPerSecond = null;
      download.finishedAt = Date.now();
      promote();
    }
    return null;
  });

  // Comme le moteur : en file ou en cours (pas pendant « finishing ») → en
  // pause, progression gardée, le suivant démarre.
  simulator.handle("hub_pause_download", (args: Args) => {
    failure("hub_pause_download");
    const model = args.model as string;
    const download = hub.downloads.find((d) => lower(d.model) === lower(model));
    if (
      download &&
      ["queued", "starting", "downloading"].includes(download.state)
    ) {
      download.state = "paused";
      download.bytesPerSecond = null;
      promote();
    }
    return null;
  });

  simulator.handle("hub_resume_download", (args: Args) => {
    failure("hub_resume_download");
    const model = args.model as string;
    const download = hub.downloads.find((d) => lower(d.model) === lower(model));
    if (download?.state === "paused") {
      download.state = "queued";
      promote();
    }
    return null;
  });

  // En tête de la file, démarré tout de suite : le dernier en cours (hors
  // « finishing ») retourne en file si les deux places sont prises.
  simulator.handle("hub_prioritize_download", (args: Args) => {
    failure("hub_prioritize_download");
    const model = args.model as string;
    const download = hub.downloads.find((d) => lower(d.model) === lower(model));
    if (
      !download ||
      !(ACTIVE.includes(download.state) || download.state === "paused")
    ) {
      return null;
    }
    hub.downloads.splice(hub.downloads.indexOf(download), 1);
    hub.downloads.unshift(download);
    if (download.state === "paused") download.state = "queued";
    if (download.state === "queued") {
      const running = hub.downloads.filter(
        (d) => d.state !== "queued" && ACTIVE.includes(d.state),
      );
      const last = running.filter((d) => d.state !== "finishing").at(-1);
      if (running.length >= 2 && last) {
        last.state = "queued";
        last.bytesPerSecond = null;
      }
      promote();
    }
    return null;
  });

  simulator.handle("hub_downloads", () => {
    failure("hub_downloads");
    return hub.downloads;
  });

  simulator.handle("hub_dismiss_download", (args: Args) => {
    failure("hub_dismiss_download");
    const model = args.model as string;
    const index = hub.downloads.findIndex(
      (d) =>
        lower(d.model) === lower(model) &&
        !ACTIVE.includes(d.state) &&
        d.state !== "paused",
    );
    if (index !== -1) hub.downloads.splice(index, 1);
    return null;
  });

  simulator.handle("hub_installed", (): HubInstalled[] => {
    failure("hub_installed");
    return host.models.map((name, index) => installed(name, index));
  });

  simulator.handle("hub_delete_model", (args: Args) => {
    failure("hub_delete_model");
    const model = args.model as string;
    const index = host.models.findIndex((name) => lower(name) === lower(model));
    if (index === -1) throw { kind: "notFound", message: "model not found" };
    host.models.splice(index, 1);
    return null;
  });

  /** Modèle installé : détails donnés par le test, sinon déduits du nom. */
  function installed(name: string, index: number): HubInstalled {
    const match = /^hf\.co\/([^/]+\/[^:]+)(?::(.+))?$/i.exec(name);
    const repo = match ? match[1] : null;
    const demoRepo = repo
      ? hub.catalog.find((r) => lower(r.model.id) === lower(repo))
      : undefined;
    const quant = demoRepo
      ? quants(demoRepo).find((q) => lower(q.label) === lower(match?.[2] ?? ""))
      : undefined;
    const capabilities = ["completion"];
    const flags = demoRepo?.model.capabilities;
    if (flags?.tools || !demoRepo) capabilities.push("tools");
    if (flags?.thinking || !demoRepo) capabilities.push("thinking");
    if (flags?.vision) capabilities.push("vision");
    return {
      model: name,
      size: quant?.downloadSize ?? (index + 1) * 0.5 * GIB,
      modifiedAt: new Date(Date.now() - index * 86400 * 1000).toISOString(),
      parameterSize: demoRepo
        ? `${((demoRepo.model.parameters ?? 0) / 1e9).toFixed(1)}B`
        : "751.63M",
      quantizationLevel: match?.[2]?.toUpperCase() ?? "Q4_K_M",
      family: demoRepo?.model.architecture ?? name.split(/[:.]/)[0],
      capabilities,
      repo,
      ...hub.installedInfo.get(lower(name)),
    };
  }

  return hub;
}

/** README généré : titre, tableau, image, lien relatif. */
function defaultReadme(id: string): string {
  const name = id.split("/")[1];
  return [
    `# ${name}`,
    "",
    `GGUF quantizations of **${name}**, ready for llama.cpp and Ollama.`,
    "",
    "## Quick start",
    "",
    "```sh",
    `ollama run hf.co/${id}:Q4_K_M`,
    "```",
    "",
    "| Quant | Use |",
    "|---|---|",
    "| Q8_0 | Best quality |",
    "| Q4_K_M | Recommended |",
    "",
    "See the [license](LICENSE) for details.",
  ].join("\n");
}
