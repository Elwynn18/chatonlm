// Reprise des modèles d'Ollama (lot L5b, écran de Settings › Models) :
// règles pures de l'écran, sans React. Textes par `t` (langue affichée).

import type { ImportCandidate } from "@/bindings/ImportCandidate";
import type { ImportProblem } from "@/bindings/ImportProblem";
import type { ImportProgress } from "@/bindings/ImportProgress";
import type { ImportSource } from "@/bindings/ImportSource";
import type { TFunction, TKey } from "@/i18n";

/** Requêtes de la reprise dans le cache de React Query (état, dossiers, ménage). */
export const modelImportKey = ["modelImport"] as const;

/** Raison d'un modèle non repris tel quel : une clé par code du moteur. */
const PROBLEMS: Record<ImportProblem, TKey> = {
  ollamaFormat: "settings.modelImport.problems.ollamaFormat",
  notGguf: "settings.modelImport.problems.notGguf",
  blobMissing: "settings.modelImport.problems.blobMissing",
  cloud: "settings.modelImport.problems.cloud",
  adapter: "settings.modelImport.problems.adapter",
  invalidName: "settings.modelImport.problems.invalidName",
  unreadable: "settings.modelImport.problems.unreadable",
  embedding: "settings.modelImport.problems.embedding",
  unsupported: "settings.modelImport.problems.unsupported",
};

/** Texte d'un code `ImportProblem`. */
export function problemText(t: TFunction, problem: ImportProblem): string {
  return t(PROBLEMS[problem]);
}

/** Ce que l'écran dit d'un modèle (statut, puis sa raison s'il y en a une). */
export function candidateStatusText(
  t: TFunction,
  candidate: ImportCandidate,
): string | null {
  switch (candidate.status) {
    case "ready":
      return null;
    case "installed":
      return t("settings.modelImport.status.installed");
    case "failed":
      return t("settings.modelImport.status.failed");
    case "redownload":
      return candidate.problem
        ? problemText(t, candidate.problem)
        : t("settings.modelImport.status.redownload");
    case "skipped":
      return candidate.problem
        ? problemText(t, candidate.problem)
        : t("settings.modelImport.status.skipped");
  }
}

/** Seuls les modèles prêts se cochent (cochés par défaut). */
export function isSelectable(candidate: ImportCandidate): boolean {
  return candidate.status === "ready";
}

/** Clé d'une case : dossier et nom (un même nom dans deux dossiers). */
export function selectionKey(source: ImportSource, name: string): string {
  return `${source.dir}\n${name}`;
}

/** Noms cochés d'un dossier : les modèles prêts, moins ceux décochés. */
export function selectedNames(
  source: ImportSource,
  unchecked: ReadonlySet<string>,
): string[] {
  return source.models
    .filter(
      (candidate) =>
        isSelectable(candidate) &&
        !unchecked.has(selectionKey(source, candidate.name)),
    )
    .map((candidate) => candidate.name);
}

/** Octets des modèles nommés. */
export function neededBytes(source: ImportSource, names: readonly string[]) {
  return source.models
    .filter((candidate) => names.includes(candidate.name))
    .reduce((total, candidate) => total + candidate.sizeBytes, 0);
}

/**
 * Les modèles de ce dossier seront copiés : toujours depuis l'Ollama de la
 * machine (lecture seule) ou un dossier partagé avec lui ; depuis l'intégré,
 * quand il n'est pas sur le même disque (sinon liens physiques).
 */
export function copiesFiles(source: ImportSource): boolean {
  return source.origin === "machine" || source.shared || !source.sameDrive;
}

/** La place libre connue ne suffit pas aux copies. */
export function lacksSpace(source: ImportSource, need: number): boolean {
  return (
    copiesFiles(source) && source.freeBytes !== null && need > source.freeBytes
  );
}

/**
 * Avancement de la reprise (0 à 100) : les octets copiés, ou les modèles
 * faits quand rien n'est copié (liens physiques) ; 0 sans rien à compter.
 */
export function importPercent(progress: ImportProgress): number {
  const [done, total] =
    progress.totalBytes > 0
      ? [progress.doneBytes, progress.totalBytes]
      : [progress.doneModels, progress.totalModels];
  if (total <= 0) return 0;
  return Math.min(100, Math.floor((done / total) * 100));
}
