// « Auto » du simulateur, comme le moteur (src-tauri/src/ollama/auto.rs) :
// sans modèle choisi sur la page (Proofread, Study), le modèle en mémoire le
// plus grand en paramètres (noms cloud exclus ; ordre d'Ollama à égalité),
// sinon celui du chat. Tests vitest seulement.

import type { RunningModel } from "@/bindings/RunningModel";

const FACTORS: Record<string, number> = { K: 1e3, M: 1e6, B: 1e9, T: 1e12 };

/** « 8.2B » → 8,2e9 ; 0 si illisible. */
function parameters(size: string): number {
  const match = /^\s*([\d.]+)\s*([KMBT])\s*$/i.exec(size);
  if (!match) return 0;
  return Number(match[1]) * (FACTORS[match[2].toUpperCase()] ?? 0) || 0;
}

/** Nom d'un modèle cloud d'ollama.com (étiquette « cloud » ou « …-cloud »). */
function isCloud(name: string): boolean {
  const tag = name.slice(name.lastIndexOf(":") + 1).toLowerCase();
  return name.includes(":") && (tag === "cloud" || tag.endsWith("-cloud"));
}

/** Modèle de la page : `own`, sinon « Auto » ; "" si aucun. */
export function autoModelOf(
  own: string,
  chat: string,
  running: readonly RunningModel[],
): string {
  if (own) return own;
  const best = running
    .filter((model) => !isCloud(model.name))
    .reduce<RunningModel | null>(
      (best, model) =>
        best === null ||
        parameters(model.parameterSize) > parameters(best.parameterSize)
          ? model
          : best,
      null,
    );
  return best ? best.name.replace(/:latest$/, "") : chat;
}
