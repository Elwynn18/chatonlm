// Jauge de la VRAM, en tête de la carte « Loaded models » de l'onglet Models
// des réglages (propre à ChatonLM : l'app officielle ne montre pas la
// mémoire graphique). Une barre : une part par modèle en mémoire (teinte
// reprise par la pastille de sa ligne), les autres usages (bureau, autres
// programmes), le libre ; sous elle, la légende chiffrée, et la part des
// modèles qui déborde en mémoire vive. Relue avec la liste des modèles
// (toutes les 2 s). Serveur distant, mémoire illisible ou machine sans carte
// graphique : une phrase à la place de la barre.

import type { RunningModel } from "@/bindings/RunningModel";
import { useMachineMemory } from "@/hooks/useMachineMemory";
import { useT } from "@/i18n";
import { formatBytes } from "@/i18n/format";
import { modelShade, share, vramView } from "@/lib/vram";

const MUTED = "text-sm/6 text-zinc-500 dark:text-zinc-400";
/** Autres usages : gris foncé, distinct du roux des modèles et du libre. */
const OTHER = "bg-neutral-500 dark:bg-neutral-400";
/** Fond de la barre : le libre. */
const FREE = "bg-neutral-200 dark:bg-neutral-700";

export function VramGauge({ models }: { models: readonly RunningModel[] }) {
  const t = useT();
  const memory = useMachineMemory();
  // Lecture en cours (premier affichage) : rien, la liste suit aussitôt.
  if (memory.isPending) return null;
  // Échec de la lecture (réglages illisibles) : mémoire inconnue.
  const view = vramView(memory.data, models);

  if (view.kind === "unknown") {
    return (
      <p data-testid="vram-unknown" className={MUTED}>
        {t("settings.models.vram.unknown")}
      </p>
    );
  }
  if (view.kind === "noGpu") {
    return (
      <p data-testid="vram-no-gpu" className={MUTED}>
        {t("settings.models.vram.noGpu")}
      </p>
    );
  }

  const usage = t("settings.models.vram.usage", {
    used: formatBytes(t, view.used),
    total: formatBytes(t, view.total),
  });
  const label = t("settings.models.vram.label");
  return (
    <div data-testid="vram-gauge" className="space-y-2">
      <div className="flex flex-wrap items-baseline justify-between gap-x-4">
        <p className="min-w-0 text-sm/6 text-zinc-950 dark:text-white">
          {label}
          <span
            data-testid="vram-gpus"
            className="ml-2 text-zinc-500 dark:text-zinc-400"
          >
            {view.gpus.join(", ")}
          </span>
        </p>
        <p data-testid="vram-usage" className={MUTED}>
          {usage}
        </p>
      </div>
      <div
        role="meter"
        aria-label={label}
        aria-valuemin={0}
        aria-valuemax={view.total}
        aria-valuenow={view.used}
        aria-valuetext={usage}
        className={`flex h-3 w-full overflow-hidden rounded-full ${FREE}`}
      >
        {view.models.map((model) => (
          <div
            key={model.name}
            data-testid="vram-model"
            data-model={model.name}
            className={`h-full flex-none border-r-2 border-white last:border-r-0 dark:border-neutral-800 ${modelShade(model.index)}`}
            style={{ width: `${share(model.bytes, view.total)}%` }}
          />
        ))}
        {view.other > 0 && (
          <div
            data-testid="vram-other"
            className={`h-full flex-none ${OTHER}`}
            style={{ width: `${share(view.other, view.total)}%` }}
          />
        )}
      </div>
      <ul className="flex flex-wrap gap-x-4 gap-y-1">
        {view.other > 0 && (
          <li className={`flex items-center gap-1.5 ${MUTED}`}>
            <span
              aria-hidden="true"
              className={`h-2.5 w-2.5 flex-none rounded-full ${OTHER}`}
            />
            {t("settings.models.vram.otherSize", {
              size: formatBytes(t, view.other),
            })}
          </li>
        )}
        <li
          data-testid="vram-free"
          className={`flex items-center gap-1.5 ${MUTED}`}
        >
          <span
            aria-hidden="true"
            className={`h-2.5 w-2.5 flex-none rounded-full border border-neutral-300 dark:border-neutral-600 ${FREE}`}
          />
          {t("settings.models.vram.freeSize", {
            size: formatBytes(t, view.free),
          })}
        </li>
      </ul>
      {view.spilled > 0 && (
        <p data-testid="vram-spilled" className={MUTED}>
          {t("settings.models.vram.spilled", {
            size: formatBytes(t, view.spilled),
          })}
        </p>
      )}
    </div>
  );
}
