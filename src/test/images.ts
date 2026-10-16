// Faux moteur des modèles d'images pour le simulateur (tests vitest
// seulement), aligné sur le moteur (src-tauri/src/image_models/service.rs ;
// contrat .tools/research/83-image-models-contract.md) : dépôts de Hugging
// Face (klein 4B et Z-Image-Turbo, recommandés ; klein 9B, non commercial ;
// un Z-Image non censuré ; une famille inconnue ; un dépôt restreint ; un
// SDXL à fichier unique),
// lignes de la base (par date d'enregistrement), fichiers sur le disque
// (`disk`, une pièce partagée y est une fois), téléchargements en mémoire,
// temps mesurés (`measured`) et machine (RTX 3090, 24 Gio). Comme le moteur
// (contrat 85 § 2) :
// - usage : celui imposé par la famille (klein 9B, FLUX.1 dev : non
//   commercial), sinon libre si la licence l'est, sinon licence à lire ;
// - VRAM estimée = fichiers du modèle et pièces + tampons de calcul de la
//   famille ; temps estimé = mesure de référence de la famille (×3 si la
//   VRAM estimée dépasse celle de la carte), rien sans carte graphique ni
//   pour une famille sans mesure ;
// - image_search sans showUnsupported retire les dépôts sans famille ou
//   restreints et les compte dans hiddenUnsupported (non censurés retirés
//   avant, non comptés) ;
// - image_models rend d'abord les lignes qui ne sont pas de simples
//   recommandations (non recommandées, installées, ou avec un téléchargement
//   même fini, échoué ou annulé), du plus ancien au plus récent, puis les
//   autres lignes, puis les recommandations jamais enregistrées ; installé =
//   ligne, tous ses fichiers sur le disque et pas de téléchargement actif ;
// - image_repo : un fichier est « installé » si sa ligne existe et que tous
//   ses fichiers sont sur le disque, ou s'il se télécharge ; dépôt inconnu :
//   notFound ;
// - image_download : rien si déjà en cours ; révision vide ou égale à celle
//   de la ligne, de la recommandation ou de la fiche (une recommandation
//   accepte aussi la révision plus récente de sa fiche) ; refus d'un dépôt
//   restreint ou non pris en charge, d'un non censuré sans le réglage, du
//   manque de place (octets manquants + 100 Mio > place libre) ; la ligne
//   est enregistrée (gardée même si le téléchargement échoue), puis le
//   téléchargement (fichiers qui manquent) émis avant la réponse ;
// - image_dismiss_download retire le téléchargement (sauf actif) et oublie la
//   ligne d'un modèle ni installé ni recommandé (ses fichiers qui ne servent
//   plus avec) ; image_delete_model refuse pendant le téléchargement, répond
//   Ok pour un identifiant inconnu, retire la ligne, son téléchargement, le
//   choix s'il le désignait et les fichiers qui ne servent plus ;
//   image_use_model refuse un modèle non installé (`""` : aucun).
// Le test fait avancer un téléchargement par progress(), finish() et fail()
// (pièces finies avant l'échec comprises), force une panne par `failures`
// et Hugging Face injoignable par `problem` (et `stale` : données du cache).

import type { AppError } from "@/bindings/AppError";
import type { HubProblem } from "@/bindings/HubProblem";
import type { ImageComponentInfo } from "@/bindings/ImageComponentInfo";
import type { ImageFamily } from "@/bindings/ImageFamily";
import type { ImageFileChoice } from "@/bindings/ImageFileChoice";
import type { ImageMachine } from "@/bindings/ImageMachine";
import type { ImageModelInfo } from "@/bindings/ImageModelInfo";
import type { ImageRepoDetail } from "@/bindings/ImageRepoDetail";
import type { ImageRepoSummary } from "@/bindings/ImageRepoSummary";
import type { ImageRepoUnsupported } from "@/bindings/ImageRepoUnsupported";
import type { ImageSearchResult } from "@/bindings/ImageSearchResult";
import type { ImageUsage } from "@/bindings/ImageUsage";
import type { SpeechDownload } from "@/bindings/SpeechDownload";
import type { Args, Simulator } from "@/test/simulator";

const GIB = 1024 ** 3;
const MIB = 1024 ** 2;
/** Marge gardée par le moteur sur le disque. */
const SPACE_MARGIN = 100 * MIB;

export const KLEIN: ImageFamily = {
  id: "flux2-klein-4b",
  name: "FLUX.2 klein 4B",
};
export const Z_IMAGE: ImageFamily = {
  id: "z-image-turbo",
  name: "Z-Image-Turbo",
};
export const KLEIN_9B: ImageFamily = {
  id: "flux2-klein-9b",
  name: "FLUX.2 klein 9B",
};
export const SDXL: ImageFamily = { id: "sdxl", name: "Stable Diffusion XL" };
export const FLUX1_DEV: ImageFamily = { id: "flux1-dev", name: "FLUX.1 dev" };

/**
 * Familles.json du moteur : temps de référence (image 1024×1024, RTX 3090),
 * tampons de calcul, usage imposé.
 */
const FAMILY_FACTS: Record<
  string,
  { seconds: number | null; computeBytes: number; usage?: ImageUsage }
> = {
  [KLEIN.id]: { seconds: 5, computeBytes: 3.1 * 1024 ** 3 },
  [KLEIN_9B.id]: {
    seconds: 10,
    computeBytes: 3.5 * 1024 ** 3,
    usage: "nonCommercial",
  },
  [Z_IMAGE.id]: { seconds: 12, computeBytes: 3.5 * 1024 ** 3 },
  [SDXL.id]: { seconds: 8, computeBytes: 2 * 1024 ** 3 },
  [FLUX1_DEV.id]: {
    seconds: 35,
    computeBytes: 3 * 1024 ** 3,
    usage: "nonCommercial",
  },
};

/** Usage permis : celui de la famille, sinon selon la licence. */
export function imageUsage(
  family: ImageFamily | null,
  openLicense: boolean,
): ImageUsage {
  const imposed = family ? FAMILY_FACTS[family.id]?.usage : undefined;
  return imposed ?? (openLicense ? "free" : "checkLicense");
}

export const KLEIN_REPO = "leejet/FLUX.2-klein-4B-GGUF";
export const KLEIN_FILE = "flux-2-klein-4b-Q4_0.gguf";
export const KLEIN_ID = `${KLEIN_REPO}/${KLEIN_FILE}`;
export const KLEIN_Q8_FILE = "flux-2-klein-4b-Q8_0.gguf";
export const KLEIN_Q8_ID = `${KLEIN_REPO}/${KLEIN_Q8_FILE}`;
export const KLEIN_REVISION = "3b1f5a9d0c2e4f6a8b1c3d5e7f9a0b2c4d6e8f0a";
export const KLEIN_9B_REPO = "leejet/FLUX.2-klein-9B-GGUF";
export const KLEIN_9B_FILE = "flux-2-klein-9b-Q8_0.gguf";
export const KLEIN_9B_ID = `${KLEIN_9B_REPO}/${KLEIN_9B_FILE}`;
export const Z_IMAGE_REPO = "leejet/Z-Image-Turbo-GGUF";
export const Z_IMAGE_FILE = "z_image_turbo-Q4_K.gguf";
export const Z_IMAGE_ID = `${Z_IMAGE_REPO}/${Z_IMAGE_FILE}`;
export const Z_IMAGE_REVISION = "c61c0e42a1b3c5d7e9f1a3b5c7d9e1f3a5b7c9d1";
export const UNCENSORED_REPO = "someone/Z-Image-Turbo-Uncensored-GGUF";
export const UNCENSORED_FILE = "z-image-turbo-uncensored-Q4_K_M.gguf";
export const UNCENSORED_ID = `${UNCENSORED_REPO}/${UNCENSORED_FILE}`;
export const UNKNOWN_REPO = "someone/mystery-diffusion-GGUF";
export const GATED_REPO = "black-forest-labs/FLUX.1-dev-GGUF";
export const SDXL_REPO = "chatonlm/sdxl-single";
export const SDXL_FILE = "sd_xl_base_1.0.safetensors";

/** Pièces annexes de chaque famille (sans `present`). */
type Piece = Omit<ImageComponentInfo, "present">;

const QWEN3_4B: Piece = {
  role: "textEncoder",
  repo: "unsloth/Qwen3-4B-GGUF",
  file: "Qwen3-4B-Q4_K_M.gguf",
  sizeBytes: 2_497_281_120,
  license: "apache-2.0",
};
const KLEIN_VAE: Piece = {
  role: "vae",
  repo: "black-forest-labs/FLUX.2-small-decoder",
  file: "full_encoder_small_decoder.safetensors",
  sizeBytes: 168_000_000,
  license: "apache-2.0",
};
const QWEN3_8B: Piece = {
  role: "textEncoder",
  repo: "unsloth/Qwen3-8B-GGUF",
  file: "Qwen3-8B-Q4_K_M.gguf",
  sizeBytes: 5_027_783_488,
  license: "apache-2.0",
};
const QWEN3_4B_INSTRUCT: Piece = {
  role: "textEncoder",
  repo: "unsloth/Qwen3-4B-Instruct-2507-GGUF",
  file: "Qwen3-4B-Instruct-2507-Q4_K_M.gguf",
  sizeBytes: 2_500_000_000,
  license: "apache-2.0",
};
const AE: Piece = {
  role: "vae",
  repo: "Comfy-Org/z_image_turbo",
  file: "split_files/vae/ae.safetensors",
  sizeBytes: 335_000_000,
  license: "apache-2.0",
};
const SDXL_VAE: Piece = {
  role: "vae",
  repo: "madebyollin/sdxl-vae-fp16-fix",
  file: "sdxl_vae.safetensors",
  sizeBytes: 334_643_268,
  license: "mit",
};

const PIECES: Record<string, Piece[]> = {
  [KLEIN.id]: [QWEN3_4B, KLEIN_VAE],
  [KLEIN_9B.id]: [QWEN3_8B, KLEIN_VAE],
  [Z_IMAGE.id]: [QWEN3_4B_INSTRUCT, AE],
  [SDXL.id]: [SDXL_VAE],
};

/** Dépôt du faux Hugging Face. */
export interface FakeImageRepo {
  summary: ImageRepoSummary;
  revision: string;
  /** Fichiers (sans `installed` ni les estimations, calculés). */
  files: Omit<
    ImageFileChoice,
    "installed" | "vramBytes" | "estimatedSeconds"
  >[];
  unsupported: ImageRepoUnsupported | null;
}

/** Recommandation (épinglée, sans réseau). */
export interface FakeImageRecommendation {
  repo: string;
  file: string;
  revision: string;
  name: string;
}

/** Ligne de la base. */
export interface FakeImageRow {
  id: string;
  repo: string;
  file: string;
  revision: string;
  name: string;
}

export const RECOMMENDATIONS: FakeImageRecommendation[] = [
  {
    repo: KLEIN_REPO,
    file: KLEIN_FILE,
    revision: KLEIN_REVISION,
    name: "FLUX.2 klein 4B",
  },
  {
    repo: Z_IMAGE_REPO,
    file: Z_IMAGE_FILE,
    revision: Z_IMAGE_REVISION,
    name: "Z-Image-Turbo",
  },
];

function summary(
  id: string,
  fields: Partial<ImageRepoSummary> = {},
): ImageRepoSummary {
  const [author, name] = id.split("/");
  const base = {
    id,
    author,
    name,
    family: null,
    downloads: 1000,
    likes: 10,
    lastModified: "2026-10-01T10:00:00Z",
    license: "apache-2.0",
    openLicense: true,
    uncensored: false,
    gated: false,
    ...fields,
  };
  return { ...base, usage: imageUsage(base.family, base.openLicense) };
}

/** Dépôts d'essai, par identifiant. */
export function imageRepos(): Map<string, FakeImageRepo> {
  const repos: FakeImageRepo[] = [
    {
      summary: summary(KLEIN_REPO, { family: KLEIN, downloads: 120_000 }),
      revision: KLEIN_REVISION,
      files: [
        {
          file: KLEIN_FILE,
          sizeBytes: 2_460_378_560,
          quant: "Q4_0",
          recommended: true,
        },
        {
          file: KLEIN_Q8_FILE,
          sizeBytes: 4_300_000_000,
          quant: "Q8_0",
          recommended: false,
        },
        {
          file: "flux-2-klein-4b-BF16.gguf",
          sizeBytes: 7_750_000_000,
          quant: "BF16",
          recommended: false,
        },
      ],
      unsupported: null,
    },
    {
      summary: summary(KLEIN_9B_REPO, {
        family: KLEIN_9B,
        downloads: 60_000,
        license: "other",
        openLicense: false,
      }),
      revision: "e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5",
      files: [
        {
          file: "flux-2-klein-9b-Q4_0.gguf",
          sizeBytes: 5_400_000_000,
          quant: "Q4_0",
          recommended: true,
        },
        {
          file: KLEIN_9B_FILE,
          sizeBytes: 9_900_000_000,
          quant: "Q8_0",
          recommended: false,
        },
      ],
      unsupported: null,
    },
    {
      summary: summary(Z_IMAGE_REPO, { family: Z_IMAGE, downloads: 98_000 }),
      revision: Z_IMAGE_REVISION,
      files: [
        {
          file: Z_IMAGE_FILE,
          sizeBytes: 3_864_250_304,
          quant: "Q4_K",
          recommended: true,
        },
        {
          file: "z_image_turbo-Q8_0.gguf",
          sizeBytes: 7_200_000_000,
          quant: "Q8_0",
          recommended: false,
        },
      ],
      unsupported: null,
    },
    {
      summary: summary(UNCENSORED_REPO, {
        family: Z_IMAGE,
        downloads: 50_000,
        uncensored: true,
      }),
      revision: "a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1",
      files: [
        {
          file: UNCENSORED_FILE,
          sizeBytes: 3_900_000_000,
          quant: "Q4_K_M",
          recommended: true,
        },
      ],
      unsupported: null,
    },
    {
      summary: summary(UNKNOWN_REPO, { downloads: 4_000, license: null }),
      revision: "b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2",
      files: [
        {
          file: "mystery-Q4_0.gguf",
          sizeBytes: 1_500_000_000,
          quant: "Q4_0",
          recommended: true,
        },
      ],
      unsupported: { kind: "unknownFamily" },
    },
    {
      summary: summary(GATED_REPO, {
        family: FLUX1_DEV,
        downloads: 30_000,
        license: "other",
        openLicense: false,
        gated: true,
      }),
      revision: "c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3",
      files: [],
      unsupported: { kind: "gated" },
    },
    {
      summary: summary(SDXL_REPO, {
        family: SDXL,
        downloads: 2_000,
        license: "openrail++",
        openLicense: false,
      }),
      revision: "d4d4d4d4d4d4d4d4d4d4d4d4d4d4d4d4d4d4d4d4",
      files: [
        {
          file: SDXL_FILE,
          sizeBytes: 6_938_078_334,
          quant: null,
          recommended: true,
        },
      ],
      unsupported: null,
    },
  ];
  return new Map(repos.map((repo) => [repo.summary.id, repo]));
}

export interface FakeImages {
  machine: ImageMachine;
  repos: Map<string, FakeImageRepo>;
  /** Recommandations (dans l'ordre du moteur). */
  recommendations: FakeImageRecommendation[];
  /** Lignes de la base, par date d'enregistrement. */
  rows: FakeImageRow[];
  /** Fichiers sur le disque, `<dépôt>/<chemin>`. */
  disk: Set<string>;
  /** Modèle choisi pour la génération (`""` : aucun). */
  selected: string;
  /** Temps de la dernière image, en secondes, par modèle. */
  measured: Map<string, number>;
  /** Téléchargements, dans l'ordre de demande. */
  downloads: SpeechDownload[];
  /** Hugging Face injoignable (recherche et fiches) ; null : à jour. */
  problem: HubProblem | null;
  /** Avec `problem` : données du cache rendues quand même. */
  stale: boolean;
  /** Erreur (panne) renvoyée par une commande, par nom. */
  failures: Map<string, AppError>;
  /** Enregistre la ligne d'un modèle sans ses fichiers (`<dépôt>/<fichier>`). */
  record(id: string): FakeImageRow;
  /** Enregistre des modèles et met tous leurs fichiers sur le disque. */
  install(...ids: string[]): void;
  /** Le modèle de la génération. */
  select(id: string): void;
  /** Modèle tel que le rend image_models. */
  model(id: string): ImageModelInfo;
  /** Fichiers d'un modèle (`<dépôt>/<chemin>` : le sien, puis ses pièces). */
  filesOf(id: string): string[];
  /** Émet la liste des téléchargements (« image-downloads »). */
  emitDownloads(): Promise<void>;
  /** Fait avancer un téléchargement (état downloading), puis émet. */
  progress(id: string, completed: number, total?: number): Promise<void>;
  /** Termine un téléchargement : tous ses fichiers sur le disque, puis émet. */
  finish(id: string): Promise<void>;
  /**
   * Fait échouer un téléchargement avec ce message ; `done` : fichiers finis
   * avant l'échec (restés sur le disque). Puis émet.
   */
  fail(id: string, error: string, done?: string[]): Promise<void>;
}

const ACTIVE: SpeechDownload["state"][] = [
  "queued",
  "downloading",
  "verifying",
];

function invalid(message: string): AppError {
  return { kind: "invalid", message };
}

/** Messages du moteur (backend.json, en anglais). */
const REFUSALS = {
  gated:
    "This repository is restricted (a Hugging Face account is needed): ChatonLM can't download it",
  uncensoredHidden:
    "This model is uncensored: turn on “Show uncensored models” in Settings to download it",
  unknownFamily:
    "ChatonLM doesn't know this model's family, so it can't run it",
  unsupportedFamily:
    "This model's family isn't supported by ChatonLM's image engine",
  notFound: "This repository doesn't exist or is private",
  downloading: "This model is downloading: cancel the download first",
  notInstalled: "This model isn't installed",
  revisionChanged:
    "This repository has changed since its page was opened: open it again",
};

/** Gestionnaires des commandes image_*. */
export function fakeImages(
  simulator: Simulator,
  showUncensored: () => boolean,
): FakeImages {
  const images: FakeImages = {
    machine: {
      engineAvailable: true,
      gpu: true,
      missingCpuFeatures: [],
      freeBytes: 200 * GIB,
      vramBytes: 24 * GIB,
      gpuName: "NVIDIA GeForce RTX 3090",
    },
    repos: imageRepos(),
    recommendations: RECOMMENDATIONS.map((entry) => ({ ...entry })),
    rows: [],
    disk: new Set(),
    selected: "",
    measured: new Map(),
    downloads: [],
    problem: null,
    stale: false,
    failures: new Map(),
    record(id) {
      const found = images.rows.find((row) => row.id === id);
      if (found) return found;
      const { repo, file } = split(id);
      const recommendation = recommendationOf(repo, file);
      const row: FakeImageRow = {
        id,
        repo,
        file,
        revision: recommendation?.revision ?? repoOf(repo).revision,
        name: recommendation?.name ?? stem(file),
      };
      images.rows.push(row);
      return row;
    },
    install(...ids) {
      for (const id of ids) {
        images.record(id);
        for (const file of images.filesOf(id)) images.disk.add(file);
      }
    },
    select(id) {
      images.selected = id;
    },
    model: (id) => {
      const row = images.rows.find((entry) => entry.id === id);
      if (row) return info(row, true);
      const { repo, file } = split(id);
      const recommendation = recommendationOf(repo, file);
      if (!recommendation) throw simulator.fail(`no image model "${id}"`);
      return info(fromRecommendation(recommendation), false);
    },
    filesOf(id) {
      const { repo } = split(id);
      const family = repoOf(repo).summary.family?.id ?? "";
      return [
        id,
        ...(PIECES[family] ?? []).map((piece) => `${piece.repo}/${piece.file}`),
      ];
    },
    emitDownloads: () =>
      simulator.emitEvent("image-downloads", images.downloads),
    async progress(id, completed, total) {
      const download = entry(id);
      download.state = "downloading";
      download.completedBytes = completed;
      if (total !== undefined) download.totalBytes = total;
      await images.emitDownloads();
    },
    async finish(id) {
      const download = entry(id);
      download.state = "done";
      download.completedBytes = download.totalBytes;
      for (const file of images.filesOf(id)) images.disk.add(file);
      await images.emitDownloads();
    },
    async fail(id, error, done = []) {
      const download = entry(id);
      download.state = "failed";
      download.error = error;
      for (const file of done) images.disk.add(file);
      await images.emitDownloads();
    },
  };

  function split(id: string): { repo: string; file: string } {
    const parts = id.split("/");
    return {
      repo: parts.slice(0, 2).join("/"),
      file: parts.slice(2).join("/"),
    };
  }

  function stem(file: string): string {
    const name = file.split("/").at(-1) ?? file;
    return name.replace(/\.[^.]+$/, "");
  }

  function recommendationOf(
    repo: string,
    file: string,
  ): FakeImageRecommendation | undefined {
    return images.recommendations.find(
      (entry) => entry.repo === repo && entry.file === file,
    );
  }

  function fromRecommendation(
    recommendation: FakeImageRecommendation,
  ): FakeImageRow {
    return {
      id: `${recommendation.repo}/${recommendation.file}`,
      repo: recommendation.repo,
      file: recommendation.file,
      revision: recommendation.revision,
      name: recommendation.name,
    };
  }

  function repoOf(id: string): FakeImageRepo {
    const found = images.repos.get(id);
    if (!found) throw simulator.fail(`no image repo "${id}"`);
    return found;
  }

  function entry(id: string): SpeechDownload {
    const found = images.downloads.find((d) => d.id === id);
    if (!found) throw simulator.fail(`no image download of "${id}"`);
    return found;
  }

  function failure(command: string) {
    const error = images.failures.get(command);
    if (error) throw error;
  }

  function active(id: string): boolean {
    return images.downloads.some(
      (d) => d.id === id && ACTIVE.includes(d.state),
    );
  }

  function allPresent(id: string): boolean {
    return images.filesOf(id).every((file) => images.disk.has(file));
  }

  /** Taille d'un fichier d'un modèle (`<dépôt>/<chemin>`). */
  function sizeOf(id: string, key: string): number {
    const { repo } = split(id);
    const found = repoOf(repo);
    const own = found.files.find((f) => `${repo}/${f.file}` === key);
    if (own) return own.sizeBytes;
    const piece = (PIECES[found.summary.family?.id ?? ""] ?? []).find(
      (entry) => `${entry.repo}/${entry.file}` === key,
    );
    if (!piece) throw simulator.fail(`no file "${key}" for "${id}"`);
    return piece.sizeBytes;
  }

  /** Mémoire de la carte estimée (fichiers et pièces + calcul de la famille). */
  function vramOf(family: ImageFamily | null, filesBytes: number): number {
    return (
      filesBytes + (family ? (FAMILY_FACTS[family.id]?.computeBytes ?? 0) : 0)
    );
  }

  /** Temps estimé d'une image ; null sans carte ou sans mesure de référence. */
  function estimatedOf(
    family: ImageFamily | null,
    vramBytes: number,
  ): number | null {
    const seconds = family ? (FAMILY_FACTS[family.id]?.seconds ?? null) : null;
    const { gpu, vramBytes: card } = images.machine;
    if (!gpu || seconds === null) return null;
    return card !== null && vramBytes > card ? seconds * 3 : seconds;
  }

  function missingBytes(id: string): number {
    return images
      .filesOf(id)
      .filter((file) => !images.disk.has(file))
      .reduce((total, file) => total + sizeOf(id, file), 0);
  }

  function info(row: FakeImageRow, recorded: boolean): ImageModelInfo {
    const repo = repoOf(row.repo);
    const family = repo.summary.family;
    if (!family) throw simulator.fail(`image model "${row.id}" has no family`);
    const pieces = PIECES[family.id] ?? [];
    const components: ImageComponentInfo[] = [
      {
        role: "model",
        repo: row.repo,
        file: row.file,
        sizeBytes: sizeOf(row.id, row.id),
        license: repo.summary.license,
        present: images.disk.has(row.id),
      },
      ...pieces.map((piece) => ({
        ...piece,
        present: images.disk.has(`${piece.repo}/${piece.file}`),
      })),
    ];
    const sizeBytes = components.reduce(
      (total, piece) => total + piece.sizeBytes,
      0,
    );
    const vramBytes = vramOf(family, sizeBytes);
    return {
      id: row.id,
      name: row.name,
      repo: row.repo,
      file: row.file,
      revision: row.revision,
      family,
      license: repo.summary.license,
      openLicense: repo.summary.openLicense,
      uncensored: repo.summary.uncensored,
      recommended: recommendationOf(row.repo, row.file) !== undefined,
      installed: recorded && allPresent(row.id) && !active(row.id),
      sizeBytes,
      missingBytes: missingBytes(row.id),
      selected: recorded && row.id === images.selected,
      components,
      usage: imageUsage(family, repo.summary.openLicense),
      quant: repo.files.find((file) => file.file === row.file)?.quant ?? null,
      vramBytes,
      estimatedSeconds: estimatedOf(family, vramBytes),
      measuredSeconds: (recorded && images.measured.get(row.id)) || null,
    };
  }

  /** Fichiers qu'aucune autre ligne ni téléchargement actif n'emploie, retirés. */
  function removeUnused(id: string) {
    const used = new Set<string>();
    for (const row of images.rows) {
      if (row.id !== id) images.filesOf(row.id).forEach((f) => used.add(f));
    }
    for (const download of images.downloads) {
      if (ACTIVE.includes(download.state)) {
        images.filesOf(download.id).forEach((f) => used.add(f));
      }
    }
    for (const file of images.filesOf(id)) {
      if (!used.has(file)) images.disk.delete(file);
    }
  }

  /** Retire le téléchargement `id` (pas actif) ; vrai s'il y en avait un. */
  function dropDownload(id: string): boolean {
    const index = images.downloads.findIndex(
      (d) => d.id === id && !ACTIVE.includes(d.state),
    );
    if (index === -1) return false;
    images.downloads.splice(index, 1);
    return true;
  }

  /** Le moteur émet pendant la commande, avant sa réponse. */
  function emitNow() {
    void images.emitDownloads();
  }

  simulator.handle("image_models", () => {
    failure("image_models");
    const listed = (id: string) => images.downloads.some((d) => d.id === id);
    const first: ImageModelInfo[] = [];
    const last: ImageModelInfo[] = [];
    for (const row of images.rows) {
      const model = info(row, true);
      if (!model.recommended || model.installed || listed(row.id)) {
        first.push(model);
      } else {
        last.push(model);
      }
    }
    for (const recommendation of images.recommendations) {
      const id = `${recommendation.repo}/${recommendation.file}`;
      if (!images.rows.some((row) => row.id === id)) {
        last.push(info(fromRecommendation(recommendation), false));
      }
    }
    return [...first, ...last];
  });
  simulator.handle("image_machine", () => {
    failure("image_machine");
    return { ...images.machine };
  });
  simulator.handle("image_search", (args: Args): ImageSearchResult => {
    failure("image_search");
    if (
      typeof args.query !== "string" ||
      typeof args.showUnsupported !== "boolean"
    ) {
      throw simulator.fail(
        `image_search: bad arguments ${JSON.stringify(args)}`,
      );
    }
    if (images.problem && !images.stale) {
      return {
        repos: [],
        problem: images.problem,
        stale: false,
        hiddenUnsupported: 0,
      };
    }
    const query = args.query.trim().toLowerCase();
    const found = [...images.repos.values()]
      .map((repo) => repo.summary)
      .filter((repo) => !repo.uncensored || showUncensored())
      .filter((repo) => repo.id.toLowerCase().includes(query))
      .sort((a, b) => b.downloads - a.downloads);
    const supported = (repo: ImageRepoSummary) =>
      repo.family !== null && !repo.gated;
    const repos = args.showUnsupported ? found : found.filter(supported);
    return {
      repos,
      problem: images.problem,
      stale: images.stale,
      hiddenUnsupported: found.length - repos.length,
    };
  });
  simulator.handle("image_repo", (args: Args): ImageRepoDetail => {
    failure("image_repo");
    const id = args.repo;
    if (typeof id !== "string") {
      throw simulator.fail(`image_repo: bad arguments ${JSON.stringify(args)}`);
    }
    const repo = images.repos.get(id);
    if (!repo || (images.problem && !images.stale)) {
      return {
        summary: summary(id, { license: null, openLicense: false }),
        revision: "",
        files: [],
        components: [],
        unsupported: null,
        problem: images.problem ?? { kind: "notFound" },
        stale: false,
      };
    }
    const family = repo.summary.family?.id ?? "";
    return {
      summary: repo.summary,
      revision: repo.revision,
      files: repo.files.map((file) => {
        const modelId = `${id}/${file.file}`;
        const recorded = images.rows.some((row) => row.id === modelId);
        const pieces = (PIECES[family] ?? []).reduce(
          (total, piece) => total + piece.sizeBytes,
          0,
        );
        const vramBytes = vramOf(repo.summary.family, file.sizeBytes + pieces);
        return {
          ...file,
          installed: (recorded && allPresent(modelId)) || active(modelId),
          vramBytes,
          estimatedSeconds: estimatedOf(repo.summary.family, vramBytes),
        };
      }),
      components: (PIECES[family] ?? []).map((piece) => ({
        ...piece,
        present: images.disk.has(`${piece.repo}/${piece.file}`),
      })),
      unsupported: repo.unsupported,
      problem: images.problem,
      stale: images.problem !== null && images.stale,
    };
  });
  simulator.handle("image_download", (args: Args) => {
    failure("image_download");
    const { repo: repoId, file, revision } = args;
    if (
      typeof repoId !== "string" ||
      typeof file !== "string" ||
      typeof revision !== "string"
    ) {
      throw simulator.fail(
        `image_download: bad arguments ${JSON.stringify(args)}`,
      );
    }
    const id = `${repoId}/${file}`;
    if (active(id)) return null;
    const repo = images.repos.get(repoId);
    const known = images.rows.find((row) => row.id === id);
    const recommendation = recommendationOf(repoId, file);
    // Révision plus récente de la fiche d'une recommandation : acceptée.
    const fromPage = repo !== undefined && revision === repo.revision;
    let accepted: string;
    if (known) {
      if (
        revision &&
        revision !== known.revision &&
        !(recommendation && fromPage)
      ) {
        throw invalid(REFUSALS.revisionChanged);
      }
      accepted = revision || known.revision;
    } else if (recommendation) {
      if (revision && revision !== recommendation.revision && !fromPage) {
        throw invalid(REFUSALS.revisionChanged);
      }
      accepted = revision || recommendation.revision;
    } else {
      if (!repo) throw invalid(REFUSALS.notFound);
      if (repo.summary.gated) throw invalid(REFUSALS.gated);
      if (repo.summary.uncensored && !showUncensored()) {
        throw invalid(REFUSALS.uncensoredHidden);
      }
      if (repo.unsupported?.kind === "unsupportedFamily") {
        throw invalid(REFUSALS.unsupportedFamily);
      }
      if (repo.unsupported || !repo.summary.family) {
        throw invalid(REFUSALS.unknownFamily);
      }
      if (!repo.files.some((f) => f.file === file)) {
        throw invalid(`${file} isn't in this repository`);
      }
      if (revision && revision !== repo.revision) {
        throw invalid(REFUSALS.revisionChanged);
      }
      accepted = repo.revision;
    }
    if (repo?.summary.uncensored && !showUncensored()) {
      throw invalid(REFUSALS.uncensoredHidden);
    }
    const missing = missingBytes(id);
    const free = images.machine.freeBytes;
    if (free !== null && missing + SPACE_MARGIN > free) {
      throw invalid(
        `Not enough disk space: ${missing} bytes needed, ${free} free`,
      );
    }
    images.record(id).revision = accepted;
    const previous = images.downloads.findIndex((d) => d.id === id);
    if (previous !== -1) images.downloads.splice(previous, 1);
    images.downloads.push({
      id,
      name: images.record(id).name,
      state: "queued",
      completedBytes: 0,
      totalBytes: missing,
      error: null,
    });
    emitNow();
    return null;
  });
  simulator.handle("image_downloads", () => {
    failure("image_downloads");
    return images.downloads;
  });
  simulator.handle("image_cancel_download", (args: Args) => {
    failure("image_cancel_download");
    const download = images.downloads.find((d) => d.id === args.id);
    if (download && ACTIVE.includes(download.state)) {
      download.state = "cancelled";
      emitNow();
    }
    return null;
  });
  simulator.handle("image_dismiss_download", (args: Args) => {
    failure("image_dismiss_download");
    const id = args.id as string;
    if (active(id)) return null;
    if (dropDownload(id)) emitNow();
    const index = images.rows.findIndex((row) => row.id === id);
    if (index === -1) return null;
    const { repo, file } = split(id);
    // Installé ou recommandé : la ligne reste.
    if (allPresent(id) || recommendationOf(repo, file)) return null;
    removeUnused(id);
    images.rows.splice(index, 1);
    return null;
  });
  simulator.handle("image_delete_model", (args: Args) => {
    failure("image_delete_model");
    const id = args.id as string;
    if (active(id)) throw invalid(REFUSALS.downloading);
    const index = images.rows.findIndex((row) => row.id === id);
    if (index !== -1) {
      removeUnused(id);
      images.rows.splice(index, 1);
      if (images.selected === id) images.selected = "";
    }
    if (dropDownload(id)) emitNow();
    return null;
  });
  simulator.handle("image_use_model", (args: Args) => {
    failure("image_use_model");
    const id = args.id as string;
    if (id !== "") {
      const recorded = images.rows.some((row) => row.id === id);
      if (!recorded || !allPresent(id) || active(id)) {
        throw invalid(REFUSALS.notInstalled);
      }
    }
    images.selected = id;
    return null;
  });

  return images;
}
