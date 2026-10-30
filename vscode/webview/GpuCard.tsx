// Carte « Graphics card », en haut du panneau : autocomplétion (activer,
// couper, état et raison d'une pause), plan du chef de la carte graphique de
// l'app (modèles en mémoire, mémoire occupée, ensemble ou l'un après l'autre),
// modèles proposés (licence, « non-commercial use » du 3B, installation par
// l'app), « Prepare » et « Measure speed » avec son verdict. Tout vient de
// l'hôte ; les gestes lui sont renvoyés. Contrat :
// .tools/research/91-lot3-gpu-completion-contract.md (X).
import { XMarkIcon } from "@heroicons/react/20/solid";
import { Switch } from "@/components/settings/Row";
import { useT } from "@/i18n";
import { formatBytes, formatInteger, formatPercent } from "@/i18n/format";
import {
  type CompletionCommand,
  type CompletionOption,
  type CompletionView,
  canPrepare,
  type GpuPlan,
  type GpuSlot,
} from "../shared/protocol";
import { completionStatus, gpuModeLabel, slotLabel } from "./labels";

const PILL =
  "shrink-0 cursor-pointer rounded-full border border-neutral-300 px-3 py-1 text-xs text-neutral-700 hover:bg-neutral-200 disabled:cursor-default disabled:opacity-50 dark:border-neutral-600 dark:text-neutral-200 dark:hover:bg-neutral-700/75";

/** Couleurs des parts de la barre de mémoire (et de leurs pastilles dans la
 * légende) : conversation brun foncé (crème en sombre), autocomplétion roux,
 * le reste brun moyen ; chaque part est séparée de sa voisine par un filet
 * de la couleur de la carte, visible contre chacune. */
const CHAT_COLOR = "bg-neutral-700 dark:bg-neutral-300";
const COMPLETION_COLOR = "bg-accent";
const OTHER_COLOR = "bg-neutral-500";

const sameModel = (a: string | null, b: string) =>
  a !== null && a.replace(/:latest$/, "") === b.replace(/:latest$/, "");

function Dot({ color }: { color: string }) {
  return (
    <span
      aria-hidden="true"
      className={`mr-1.5 inline-block h-2 w-2 shrink-0 rounded-full align-middle ${color}`}
    />
  );
}

/** Mémoire de la carte graphique (sans carte reconnue : mémoire vive) :
 * conversation, autocomplétion, le reste ; mémoire libre parfois inconnue. */
function MemoryBar({ plan }: { plan: GpuPlan }) {
  const t = useT();
  const memory = plan.memory;
  if (!memory || memory.total <= 0) return null;
  // Sans carte reconnue, Ollama ne met rien en VRAM : la part d'un modèle
  // est sa taille en mémoire vive.
  const bytes = (slot: GpuSlot | null) =>
    slot?.loaded ? (memory.gpu ? slot.vramBytes : slot.sizeBytes) : 0;
  const chat = bytes(plan.chat);
  const completion = bytes(plan.completion);
  // Libre inconnue (AMD sous Windows) : seulement ce que tiennent les modèles.
  const used =
    memory.free === null
      ? Math.min(memory.total, chat + completion)
      : Math.min(memory.total, Math.max(0, memory.total - memory.free));
  const other = Math.max(0, used - chat - completion);
  const percent = Math.round((used / memory.total) * 100);
  const label = t(memory.gpu ? "vscode.gpu.memory" : "vscode.gpu.ram");
  const parts = [
    { id: "chat", bytes: chat, color: CHAT_COLOR },
    { id: "completion", bytes: completion, color: COMPLETION_COLOR },
    { id: "other", bytes: other, color: OTHER_COLOR },
  ].filter((part) => part.bytes > 0);
  return (
    <div className="mt-2">
      <div
        role="meter"
        aria-label={label}
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={percent}
        aria-valuetext={formatPercent(t.lang, percent)}
        className="flex h-2 w-full overflow-hidden rounded-full bg-neutral-200 dark:bg-neutral-700"
      >
        {parts.map((part) => (
          <div
            key={part.id}
            data-testid={`gpu-part-${part.id}`}
            className={`h-full border-r-2 border-neutral-100 dark:border-neutral-800 ${part.color}`}
            style={{
              width: `${Math.min(100, (part.bytes / memory.total) * 100)}%`,
            }}
          />
        ))}
      </div>
      <p
        className="pt-1 text-xs text-neutral-600 dark:text-neutral-300"
        data-testid="gpu-memory"
        data-gpu={memory.gpu}
      >
        {label}
        {" · "}
        {memory.free === null
          ? t("vscode.gpu.memoryTotal", { total: formatBytes(t, memory.total) })
          : t("vscode.gpu.memoryUsed", {
              used: formatBytes(t, used),
              total: formatBytes(t, memory.total),
            })}
      </p>
      {other > 0 && (
        <p
          className="text-xs text-neutral-600 dark:text-neutral-300"
          data-testid="gpu-other"
        >
          <Dot color={OTHER_COLOR} />
          {t("vscode.gpu.otherUse", { size: formatBytes(t, other) })}
        </p>
      )}
      {!memory.gpu && (
        <p className="text-xs text-neutral-600 dark:text-neutral-300">
          {t("vscode.gpu.ramNote")}
        </p>
      )}
    </div>
  );
}

function OptionRow({
  option,
  view,
  onModel,
  onInstall,
}: {
  option: CompletionOption;
  view: CompletionView;
  onModel: (model: string) => void;
  onInstall: (model: string) => void;
}) {
  const t = useT();
  const inUse = sameModel(view.model, option.model);
  // `fits` null : machine inconnue, rien n'est dit.
  const notes = [
    option.estimatedBytes ? formatBytes(t, option.estimatedBytes) : null,
    option.license,
    option.fits === false ? t("vscode.gpu.mayNotFit") : null,
  ].filter((note): note is string => note !== null);
  return (
    <li
      className="flex items-center gap-2 border-t border-neutral-200 py-1 first:border-t-0 dark:border-neutral-700"
      data-testid="gpu-option"
      data-model={option.model}
    >
      <div className="min-w-0 flex-1">
        <p className="truncate text-xs font-medium text-neutral-900 dark:text-white">
          {option.model}
          {option.model === view.recommended && (
            <span className="font-normal text-neutral-600 dark:text-neutral-300">
              {" · "}
              {t("vscode.gpu.recommended")}
            </span>
          )}
        </p>
        <p className="text-xs text-neutral-600 dark:text-neutral-300">
          {notes.join(" · ")}
        </p>
        {!option.commercialUse && (
          <p
            className="text-xs font-medium text-neutral-800 dark:text-neutral-100"
            data-testid="gpu-non-commercial"
          >
            {t("vscode.gpu.nonCommercial")}
          </p>
        )}
      </div>
      {!option.installed ? (
        <button
          type="button"
          className={PILL}
          aria-label={t("vscode.gpu.installModel", { model: option.model })}
          onClick={() => onInstall(option.model)}
        >
          {t("vscode.gpu.install")}
        </button>
      ) : inUse ? (
        <span className="shrink-0 rounded-full bg-accent px-3 py-1 text-xs text-neutral-900">
          {t("vscode.gpu.inUse")}
        </span>
      ) : (
        <button
          type="button"
          className={PILL}
          aria-label={t("vscode.gpu.useModel", { model: option.model })}
          onClick={() => onModel(option.model)}
        >
          {t("vscode.gpu.use")}
        </button>
      )}
    </li>
  );
}

export function GpuCard({
  view,
  onCommand,
  onModel,
  onInstall,
  onClose,
}: {
  view: CompletionView;
  onCommand: (command: CompletionCommand) => void;
  onModel: (model: string) => void;
  onInstall: (model: string) => void;
  onClose: () => void;
}) {
  const t = useT();
  const { plan, measure } = view;
  // « Prepare » seulement quand il peut réussir (pas « ne tient pas »…).
  const prepare = canPrepare(view);
  const canMeasure = view.status === "ready" || view.status === "slow";
  const other = view.otherProvider ?? view.alsoSuggests;

  return (
    <section
      aria-label={t("vscode.gpu.open")}
      data-testid="gpu"
      data-status={view.status}
      className="mx-3 mt-2 max-h-[60vh] shrink-0 overflow-y-auto rounded-2xl bg-neutral-100 px-3 py-2 text-sm dark:bg-neutral-800"
    >
      <div className="flex items-center gap-2">
        <p className="min-w-0 flex-1 truncate font-medium text-neutral-900 dark:text-white">
          {t("vscode.gpu.open")}
        </p>
        <button
          type="button"
          aria-label={t("vscode.gpu.close")}
          title={t("vscode.gpu.close")}
          onClick={onClose}
          className="flex h-7 w-7 shrink-0 cursor-pointer items-center justify-center rounded-full text-neutral-700 hover:bg-neutral-200 dark:text-neutral-200 dark:hover:bg-neutral-700/75"
        >
          <XMarkIcon className="h-4 w-4" />
        </button>
      </div>

      <div className="mt-1 flex items-center gap-2">
        <span className="min-w-0 flex-1 text-sm text-neutral-800 dark:text-neutral-100">
          {t("vscode.gpu.autocomplete")}
        </span>
        {/* Nom stable : l'état passe par aria-checked. */}
        <Switch
          checked={view.enabled}
          label={t("vscode.gpu.autocomplete")}
          onChange={(on) => onCommand(on ? "enable" : "disable")}
        />
      </div>
      <p
        role="status"
        data-testid="gpu-status"
        className="pt-1 text-xs text-neutral-600 dark:text-neutral-300"
      >
        {completionStatus(t, view)}
      </p>
      {view.alsoSuggests && view.status !== "otherProvider" && (
        <p
          className="text-xs text-neutral-600 dark:text-neutral-300"
          data-testid="gpu-also"
        >
          {t("vscode.gpu.alsoSuggests", { name: view.alsoSuggests })}
        </p>
      )}
      {other && (
        <div className="mt-1">
          <button
            type="button"
            className={PILL}
            onClick={() => onCommand("ignoreOthers")}
          >
            {t("vscode.gpu.ignoreOthers")}
          </button>
        </div>
      )}

      {plan && (
        <div className="mt-2" data-testid="gpu-plan" data-mode={plan.mode}>
          <p className="text-xs font-medium text-neutral-800 dark:text-neutral-100">
            {gpuModeLabel(t, plan.mode)}
          </p>
          <MemoryBar plan={plan} />
          <ul className="pt-1 text-xs text-neutral-600 dark:text-neutral-300">
            {plan.chat && (
              <li data-testid="gpu-slot-chat">
                <Dot color={CHAT_COLOR} />
                <span className="text-neutral-800 dark:text-neutral-100">
                  {t("vscode.gpu.chat")}
                </span>
                {" · "}
                {slotLabel(t, plan.chat)}
              </li>
            )}
            {plan.completion && (
              <li data-testid="gpu-slot-completion">
                <Dot color={COMPLETION_COLOR} />
                <span className="text-neutral-800 dark:text-neutral-100">
                  {t("vscode.gpu.completion")}
                </span>
                {" · "}
                {slotLabel(t, plan.completion)}
              </li>
            )}
          </ul>
        </div>
      )}

      {view.options.length > 0 && (
        <ul aria-label={t("vscode.gpu.models")} className="mt-2">
          {view.options.map((option) => (
            <OptionRow
              key={option.model}
              option={option}
              view={view}
              onModel={onModel}
              onInstall={onInstall}
            />
          ))}
        </ul>
      )}

      {(prepare || canMeasure) && (
        <div className="mt-2 flex flex-wrap gap-1.5">
          {prepare && (
            <button
              type="button"
              className={PILL}
              disabled={view.preparing}
              onClick={() => onCommand("prepare")}
            >
              {t(
                view.preparing ? "vscode.gpu.preparing" : "vscode.gpu.prepare",
              )}
            </button>
          )}
          {canMeasure && (
            <button
              type="button"
              className={PILL}
              disabled={measure?.state === "running"}
              onClick={() => onCommand("measure")}
            >
              {t(
                measure?.state === "running"
                  ? "vscode.gpu.measuring"
                  : "vscode.gpu.measure",
              )}
            </button>
          )}
        </div>
      )}

      {measure?.state === "done" && (
        <div
          className="pt-2 text-xs"
          data-testid="gpu-measure"
          data-rating={measure.result.rating}
        >
          <p className="font-medium text-neutral-800 dark:text-neutral-100">
            {t(
              measure.result.rating === "smooth"
                ? "vscode.gpu.rating.smooth"
                : measure.result.rating === "usable"
                  ? "vscode.gpu.rating.usable"
                  : "vscode.gpu.rating.tooSlow",
            )}
          </p>
          <p className="text-neutral-600 dark:text-neutral-300">
            {t("vscode.gpu.measureDetail", {
              ms: formatInteger(t.lang, measure.result.firstTokenMs),
              speed: formatInteger(t.lang, measure.result.tokensPerSecond),
            })}
            {measure.result.partialGpu && ` ${t("vscode.gpu.measurePartial")}`}
          </p>
        </div>
      )}
      {measure?.state === "failed" && (
        <p
          className="pt-2 text-xs text-neutral-800 dark:text-neutral-100"
          role="alert"
          data-testid="gpu-measure"
        >
          {measure.message}
        </p>
      )}
    </section>
  );
}
