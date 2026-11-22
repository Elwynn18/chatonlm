// Jauge de la VRAM, en tête de la carte « Loaded models » de l'onglet Models
// des réglages (propre à ChatonLM : l'app officielle ne montre pas la
// mémoire graphique). Une barre : une part par modèle en mémoire (teinte
// reprise par la pastille de sa ligne), le moteur d'images et Whisper quand
// ils sont sur la carte et que leur VRAM est connue, les autres usages
// (bureau, autres programmes), le libre ; sous elle, la légende chiffrée, et
// la part des modèles qui déborde en mémoire vive. Mémoire illisible ou
// machine sans carte graphique : une phrase à la place de la barre. Puis,
// seulement pour ce qui est chargé, où calculent le modèle de transcription
// et le moteur d'images (vue des ressources du moteur). Relue avec la liste
// des modèles (toutes les 2 s).

import { useQuery } from "@tanstack/react-query";
import type { EngineRunningModel } from "@/bindings/EngineRunningModel";
import type { ResourcesView } from "@/bindings/ResourcesView";
import { speechModelsKey } from "@/hooks/useDictation";
import { useMachineMemory } from "@/hooks/useMachineMemory";
import { useResources } from "@/hooks/useResources";
import { type TFunction, useT } from "@/i18n";
import { formatBytes } from "@/i18n/format";
import { speechModels } from "@/lib/ipc";
import {
  EXTRA_SHADES,
  type VramExtra,
  modelShade,
  share,
  vramView,
} from "@/lib/vram";

const MUTED = "text-sm/6 text-zinc-500 dark:text-zinc-400";
/** Autres usages : gris foncé, distinct du roux des modèles et du libre. */
const OTHER = "bg-neutral-500 dark:bg-neutral-400";
/** Fond de la barre : le libre. */
const FREE = "bg-neutral-200 dark:bg-neutral-700";

/**
 * Où calcule chaque partie chargée : modèle de transcription, puis moteur
 * d'images ; machine sans carte : sur le processeur. Moteur d'images sans
 * VRAM dite (pilote qui ne la donne pas par processus) : lieu tu.
 */
function resourceLines(
  t: TFunction,
  resources: ResourcesView | undefined,
  speechName: string,
  noGpu: boolean,
): { key: string; text: string }[] {
  const lines: { key: string; text: string }[] = [];
  const speech = resources?.speech;
  if (speech) {
    const bytes = speech.vramBytes ?? 0;
    lines.push({
      key: "speech",
      text:
        noGpu || speech.device === "processor"
          ? t("settings.models.resources.speechProcessor", { name: speechName })
          : bytes > 0
            ? t("settings.models.resources.speechCard", {
                name: speechName,
                size: formatBytes(t, bytes),
              })
            : t("settings.models.resources.speechCardUnmeasured", {
                name: speechName,
              }),
    });
  }
  const images = resources?.images;
  if (images) {
    const bytes = images.vramBytes ?? 0;
    const size = formatBytes(t, bytes);
    let text: string;
    if (noGpu) {
      text = images.busy
        ? t("settings.models.resources.imagesProcessorBusy")
        : t("settings.models.resources.imagesProcessorIdle");
    } else if (bytes > 0) {
      text = images.busy
        ? t("settings.models.resources.imagesCardBusy", { size })
        : t("settings.models.resources.imagesCardIdle", { size });
    } else {
      text = images.busy
        ? t("settings.models.resources.imagesBusy")
        : t("settings.models.resources.imagesIdle");
    }
    lines.push({ key: "images", text });
  }
  return lines;
}

export function VramGauge({
  models,
}: {
  models: readonly EngineRunningModel[];
}) {
  const t = useT();
  const memory = useMachineMemory();
  const resources = useResources();
  const speechId = resources.data?.speech?.modelId ?? null;
  // Nom lisible du modèle de transcription : catalogue de la dictée (lu
  // seulement s'il en faut un) ; illisible ou inconnu : l'identifiant.
  const catalog = useQuery({
    queryKey: speechModelsKey,
    queryFn: speechModels,
    enabled: speechId !== null,
  });
  // Lecture en cours (premier affichage) : rien, la liste suit aussitôt.
  if (memory.isPending) return null;
  // Échec de la lecture (réglages illisibles) : mémoire inconnue ; vue des
  // ressources illisible : rien d'autre que les modèles.
  const view = vramView(memory.data, models, resources.data);
  const speechName =
    catalog.data?.find((model) => model.id === speechId)?.name ??
    speechId ??
    "";
  const lines = resourceLines(
    t,
    resources.data,
    speechName,
    view.kind === "noGpu",
  );
  const main = <MemoryView view={view} speechName={speechName} />;
  if (lines.length === 0) return main;
  return (
    <div className="space-y-2">
      {main}
      <ul
        data-testid="resources-list"
        aria-label={t("settings.models.resources.label")}
        className="space-y-0.5"
      >
        {lines.map((line) => (
          <li
            key={line.key}
            data-testid={`resources-${line.key}`}
            className={MUTED}
          >
            {line.text}
          </li>
        ))}
      </ul>
    </div>
  );
}

/** Légende d'une autre partie de l'app sur la carte. */
function extraLabel(t: TFunction, extra: VramExtra, speechName: string) {
  const size = formatBytes(t, extra.bytes);
  return extra.kind === "speech"
    ? t("settings.models.vram.speechSize", { name: speechName, size })
    : t("settings.models.vram.imagesSize", { size });
}

function MemoryView({
  view,
  speechName,
}: {
  view: ReturnType<typeof vramView>;
  speechName: string;
}) {
  const t = useT();
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
        {view.extras.map((extra) => (
          <div
            key={extra.kind}
            data-testid="vram-extra"
            data-kind={extra.kind}
            className={`h-full flex-none border-r-2 border-white last:border-r-0 dark:border-neutral-800 ${EXTRA_SHADES[extra.kind]}`}
            style={{ width: `${share(extra.bytes, view.total)}%` }}
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
        {view.extras.map((extra) => (
          <li
            key={extra.kind}
            data-testid={`vram-${extra.kind}-legend`}
            className={`flex items-center gap-1.5 ${MUTED}`}
          >
            <span
              aria-hidden="true"
              className={`h-2.5 w-2.5 flex-none rounded-full ${EXTRA_SHADES[extra.kind]}`}
            />
            {extraLabel(t, extra, speechName)}
          </li>
        ))}
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
