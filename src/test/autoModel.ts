// « Auto » du simulateur, comme le moteur (src-tauri/src/engine/auto.rs) :
// sans modèle choisi sur la page (Proofread, Study), le modèle prêt du
// moteur le plus grand en paramètres, puis en mémoire (ordre du moteur à
// égalité), sinon celui du chat. Le moteur écarte aussi les modèles de code
// d'autocomplétion : le simulateur n'en a pas. Tests vitest seulement.

import type { EngineRunningModel } from "@/bindings/EngineRunningModel";

const FACTORS: Record<string, number> = { K: 1e3, M: 1e6, B: 1e9, T: 1e12 };

/** « 8.2B » → 8,2e9 ; 0 si illisible. */
function parameters(size: string | null): number {
  const match = /^\s*([\d.]+)\s*([KMBT])\s*$/i.exec(size ?? "");
  if (!match) return 0;
  return Number(match[1]) * (FACTORS[match[2].toUpperCase()] ?? 0) || 0;
}

/** Modèle de la page : `own`, sinon « Auto » ; "" si aucun. */
export function autoModelOf(
  own: string,
  chat: string,
  running: readonly EngineRunningModel[],
): string {
  if (own) return own;
  const best = running
    .filter((model) => model.state === "ready")
    .reduce<EngineRunningModel | null>((best, model) => {
      if (best === null) return model;
      const size = parameters(model.paramsLabel) - parameters(best.paramsLabel);
      if (size !== 0) return size > 0 ? model : best;
      return model.sizeBytes > best.sizeBytes ? model : best;
    }, null);
  return best ? best.id : chat;
}
