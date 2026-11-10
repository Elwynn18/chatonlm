// Moteur de texte llama.cpp dans les réglages (lot L5b) : clés du cache de
// React Query et textes de la ligne « Engine » et du dossier des modèles,
// sans React ; pannes du moteur dans le chat (EngineErrorActions.tsx).
// Textes par `t` (langue affichée).

import type { ChatErrorKind } from "@/bindings/ChatErrorKind";
import type { EngineModelsDir } from "@/bindings/EngineModelsDir";
import type { EngineStatus } from "@/bindings/EngineStatus";
import type { TFunction } from "@/i18n";
import { formatBytes } from "@/i18n/format";

/** État du moteur (`engine_status`). */
export const engineStatusKey = ["engineStatus"] as const;

/** Pack CUDA (`cuda_status`). */
export const cudaStatusKey = ["cudaStatus"] as const;

/** Dossier des modèles (`engine_models_dir`). */
export const engineModelsDirKey = ["engineModelsDir"] as const;

/** Pannes du moteur dans le chat. */
export type EngineErrorKind = Extract<
  ChatErrorKind,
  "engineFailed" | "engineUnavailable"
>;

export function isEngineError(
  kind: ChatErrorKind | undefined,
): kind is EngineErrorKind {
  return kind === "engineFailed" || kind === "engineUnavailable";
}

/**
 * Version de llama.cpp à montrer : celle de llama.cpp, sans la marque de la
 * construction de ChatonLM (`v0.9.0+chatonlm.1b94fcb0b0db` → `v0.9.0`) ;
 * null si elle est inconnue.
 */
export function shortEngineVersion(version: string | null): string | null {
  const short = version?.split("+")[0]?.trim() ?? "";
  return short === "" ? null : short;
}

/** Version de llama.cpp (« llama.cpp v0.9.0 »), sans numéro si inconnue. */
export function engineVersionText(t: TFunction, status: EngineStatus): string {
  const version = shortEngineVersion(status.version);
  return version
    ? t("settings.engine.version", { version })
    : t("settings.engine.versionUnknown");
}

/**
 * Accélération : la carte NVIDIA par CUDA, la carte graphique nommée, ou le
 * processeur seul.
 */
export function accelerationText(t: TFunction, status: EngineStatus): string {
  if (status.backend === "cpu") return t("settings.engine.accelerationCpu");
  if (status.backend === "cuda") {
    return status.gpuName
      ? t("settings.engine.accelerationCuda", { name: status.gpuName })
      : t("settings.engine.accelerationCudaUnnamed");
  }
  return status.gpuName
    ? t("settings.engine.accelerationGpu", { name: status.gpuName })
    : t("settings.engine.accelerationGpuUnnamed");
}

/** Résumé sous le dossier : par défaut, modèles et leur taille, place libre. */
export function folderSummary(t: TFunction, dir: EngineModelsDir): string[] {
  const parts: string[] = [];
  if (dir.isDefault) parts.push(t("settings.modelsFolder.default"));
  parts.push(
    dir.models > 0
      ? t("settings.modelsFolder.models", {
          count: dir.models,
          size: formatBytes(t, dir.bytes),
        })
      : t("settings.modelsFolder.noModels"),
  );
  if (dir.freeBytes !== null) {
    parts.push(
      t("settings.modelsFolder.free", { size: formatBytes(t, dir.freeBytes) }),
    );
  }
  return parts;
}
