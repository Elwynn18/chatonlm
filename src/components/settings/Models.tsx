// Onglet « Models » des réglages (propre à ChatonLM : l'officiel n'a ni
// réglage de chargement, ni pré-prompt, ni liste des modèles en mémoire), en
// trois cartes.
// La première règle le chargement : durée avant déchargement (keep_alive
// envoyé par ChatonLM avec chaque requête qui charge un modèle, le service
// d'Ollama n'est pas modifié), préchargement du modèle choisi et
// déchargement des modèles en quittant l'app (activé par défaut ; pas à la
// fermeture de la fenêtre dans la zone de notification), puis la
// rubrique « Uncensored » du ModelHub (masquée par défaut) ; chaque
// changement est enregistré aussitôt (« Saved » de la page, erreur sous la
// ligne), « Reset to defaults » (General) n'y touche pas. La deuxième règle le
// pré-prompt, envoyé en message système au début de chaque conversation
// (PromptField.tsx : enregistré en quittant le champ). La troisième montre
// d'abord la jauge de la VRAM (VramGauge.tsx), puis les modèles en mémoire (/api/ps, relu toutes les 2 s tant que l'onglet est
// affiché et après chaque action), avec Unload et Unload all, et charge un
// modèle installé (Load) ; un escargot (SlowModelSnail.tsx) suit le nom d'un
// modèle en partie sur le processeur. Le bouton d'une action en cours reste focalisable
// (aria-disabled) ; une ligne déchargée passe le focus à la ligne suivante
// (ou précédente), sinon au titre de la carte.

import {
  ArrowPathIcon,
  BoltIcon,
  ClockIcon,
  PowerIcon,
  ShieldExclamationIcon,
} from "@heroicons/react/20/solid";
import { useQueryClient } from "@tanstack/react-query";
import { useCallback, useId, useRef, useState } from "react";
import type { RunningModel } from "@/bindings/RunningModel";
import type { SettingsPatch } from "@/bindings/SettingsPatch";
import { ModelLabel } from "@/components/ModelLabel";
import { SlowModelSnail } from "@/components/SlowModelSnail";
import { PromptField } from "@/components/settings/PromptField";
import { Row, RowError, Switch } from "@/components/settings/Row";
import { Slider } from "@/components/settings/Slider";
import { VramGauge } from "@/components/settings/VramGauge";
import { useFocusRescue } from "@/hooks/useFocusRescue";
import { machineMemoryKey } from "@/hooks/useMachineMemory";
import { useModels } from "@/hooks/useModels";
import { forgetPreloads } from "@/hooks/usePreloadModel";
import { runningModelsKey, useRunningModels } from "@/hooks/useRunningModels";
import { useSettings } from "@/hooks/useSettings";
import { type TFunction, useT } from "@/i18n";
import { isCloudModel } from "@/lib/dictation";
import {
  errorMessage,
  loadModel,
  unloadAllModels,
  unloadModel,
} from "@/lib/ipc";
import { sameModel } from "@/lib/modelNames";
import {
  cpuShare,
  KEEP_ALIVE_STEPS,
  keepAliveLabel,
  loadedSummary,
  runningDetails,
  sameRunningModel,
} from "@/lib/runningModels";
import { modelShade } from "@/lib/vram";

const MUTED = "text-sm/6 text-zinc-500 dark:text-zinc-400";
const SELECT =
  "h-9 w-full max-w-sm min-w-0 flex-1 rounded-lg border border-neutral-300 bg-white pr-7 pl-2 text-sm text-neutral-900 outline-none focus-visible:border-accent-fg focus-visible:ring-2 focus-visible:ring-accent-fg/40 disabled:opacity-50 dark:border-neutral-700 dark:bg-neutral-800 dark:text-neutral-100";
// aria-disabled comme disabled : le bouton d'une action en cours garde le
// focus (un bouton désactivé le perd).
const SMALL_BUTTON =
  "inline-flex flex-none items-center gap-1.5 rounded-lg border border-zinc-950/10 bg-white px-3 py-1 text-sm/6 font-medium text-zinc-950 shadow-sm outline-none hover:bg-zinc-50 focus-visible:ring-2 focus-visible:ring-accent-fg disabled:cursor-default disabled:opacity-50 aria-disabled:cursor-default aria-disabled:opacity-50 dark:border-white/15 dark:bg-zinc-900 dark:text-white dark:hover:bg-zinc-800";
const ALERT = "text-xs break-words text-red-600 dark:text-red-400";

/** Crans du curseur : leur rang (« Always », -1, est le dernier). */
function keepAliveOptions(t: TFunction) {
  return KEEP_ALIVE_STEPS.map((step, index) => ({
    value: index,
    label: keepAliveLabel(t, step.seconds),
  }));
}

type Field =
  | "keepAlive"
  | "preloadModel"
  | "unloadOnQuit"
  | "showUncensored"
  | "systemPrompt";

/** Longueur maximale du pré-prompt (`MAX_SYSTEM_PROMPT_CHARS` du moteur). */
const MAX_SYSTEM_PROMPT_CHARS = 10_000;

/**
 * Échec d'une action, montré tant que son modèle est dans le même état :
 * `model` null (Unload all) : tant qu'un modèle reste chargé. `message` :
 * écrit au rendu, dans la langue affichée.
 */
interface ActionError {
  model: string | null;
  message: (t: TFunction) => string;
}

export function Models({
  onSaved,
}: {
  /** Un changement est enregistré (« Saved » de la page). */
  onSaved: () => void;
}) {
  const t = useT();
  const { settings, setSettings } = useSettings();
  const [errors, setErrors] = useState<Partial<Record<Field, string>>>({});
  // Cran choisi en attente de la réponse : la pastille y reste (comme
  // Context length).
  const [pendingKeepAlive, setPendingKeepAlive] = useState<number | null>(null);
  const pendingSaves = useRef(0);
  const ids = {
    keepAlive: useId(),
    keepAliveDescription: useId(),
    preload: useId(),
    preloadDescription: useId(),
    unloadOnQuit: useId(),
    unloadOnQuitDescription: useId(),
    uncensored: useId(),
    uncensoredDescription: useId(),
    systemPrompt: useId(),
    systemPromptDescription: useId(),
    systemPromptTitle: useId(),
  };

  if (!settings) return null;

  /** Enregistre `patch` ; faux s'il est refusé (erreur sous la ligne). */
  const save = async (field: Field, patch: SettingsPatch) => {
    setErrors((current) => ({ ...current, [field]: undefined }));
    try {
      await setSettings(patch);
    } catch (cause) {
      setErrors((current) => ({ ...current, [field]: errorMessage(cause) }));
      return false;
    }
    onSaved();
    return true;
  };

  const saveKeepAlive = async (keepAlive: number) => {
    setPendingKeepAlive(keepAlive);
    pendingSaves.current += 1;
    await save("keepAlive", { keepAlive });
    pendingSaves.current -= 1;
    if (pendingSaves.current === 0) setPendingKeepAlive(null);
  };

  const keepAlive = pendingKeepAlive ?? settings.keepAlive;

  return (
    <div data-testid="models-settings" className="space-y-4">
      <div className="overflow-hidden rounded-xl bg-white dark:bg-neutral-800">
        <div className="space-y-4 p-4">
          <Row
            icon={ClockIcon}
            label={
              <span id={ids.keepAlive}>
                {t("settings.models.keepAlive.label")}
              </span>
            }
            description={t("settings.models.keepAlive.description")}
            descriptionId={ids.keepAliveDescription}
          >
            <div className="mt-3">
              <Slider
                options={keepAliveOptions(t)}
                value={KEEP_ALIVE_STEPS.findIndex(
                  (step) => step.seconds === keepAlive,
                )}
                onChange={(index) =>
                  void saveKeepAlive(KEEP_ALIVE_STEPS[index].seconds)
                }
                labelledBy={ids.keepAlive}
                describedBy={ids.keepAliveDescription}
              />
            </div>
            <RowError message={errors.keepAlive} />
          </Row>

          <div className="flex items-start justify-between gap-4">
            <Row
              icon={BoltIcon}
              label={
                <span id={ids.preload}>
                  {t("settings.models.preload.label")}
                </span>
              }
              description={t("settings.models.preload.description")}
              descriptionId={ids.preloadDescription}
            >
              <RowError message={errors.preloadModel} />
            </Row>
            <Switch
              label={t("settings.models.preload.label")}
              checked={settings.preloadModel}
              describedBy={ids.preloadDescription}
              onChange={(preloadModel) => {
                // Activé : le modèle courant est préchargé aussitôt, même
                // s'il l'a déjà été plus tôt dans la session.
                if (preloadModel) forgetPreloads();
                void save("preloadModel", { preloadModel });
              }}
            />
          </div>

          <div className="flex items-start justify-between gap-4">
            <Row
              icon={PowerIcon}
              label={
                <span id={ids.unloadOnQuit}>
                  {t("settings.models.unloadOnQuit.label")}
                </span>
              }
              description={t("settings.models.unloadOnQuit.description")}
              descriptionId={ids.unloadOnQuitDescription}
            >
              <RowError message={errors.unloadOnQuit} />
            </Row>
            <Switch
              label={t("settings.models.unloadOnQuit.label")}
              checked={settings.unloadOnQuit}
              describedBy={ids.unloadOnQuitDescription}
              onChange={(unloadOnQuit) =>
                void save("unloadOnQuit", { unloadOnQuit })
              }
            />
          </div>

          <div className="flex items-start justify-between gap-4">
            <Row
              icon={ShieldExclamationIcon}
              label={
                <span id={ids.uncensored}>
                  {t("settings.models.uncensored.label")}
                </span>
              }
              description={t("settings.models.uncensored.description")}
              descriptionId={ids.uncensoredDescription}
            >
              <RowError message={errors.showUncensored} />
            </Row>
            <Switch
              label={t("settings.models.uncensored.label")}
              checked={settings.showUncensored}
              describedBy={ids.uncensoredDescription}
              onChange={(showUncensored) =>
                void save("showUncensored", { showUncensored })
              }
            />
          </div>
        </div>
      </div>

      <section
        aria-labelledby={ids.systemPromptTitle}
        data-testid="system-prompt-settings"
        className="overflow-hidden rounded-xl bg-white dark:bg-neutral-800"
      >
        <div className="space-y-4 p-4">
          <h2
            id={ids.systemPromptTitle}
            className="text-sm/6 font-medium text-zinc-950 dark:text-white"
          >
            {t("settings.models.systemPrompt.title")}
          </h2>
          <PromptField
            id={ids.systemPrompt}
            descriptionId={ids.systemPromptDescription}
            data-testid="system-prompt"
            label={t("settings.models.systemPrompt.label")}
            description={t("settings.models.systemPrompt.description")}
            saved={settings.systemPrompt}
            placeholder={t("settings.models.systemPrompt.placeholder")}
            maxLength={MAX_SYSTEM_PROMPT_CHARS}
            resetLabel={t("settings.models.systemPrompt.clear")}
            onSave={(systemPrompt) => save("systemPrompt", { systemPrompt })}
            error={errors.systemPrompt}
          />
        </div>
      </section>

      <LoadedModels selectedModel={settings.selectedModel} />
    </div>
  );
}

/**
 * Carte « Loaded models » : modèles en mémoire, Unload et Unload all, puis
 * chargement d'un modèle installé. `selectedModel` : proposé d'abord au
 * chargement s'il n'est pas en mémoire.
 */
function LoadedModels({ selectedModel }: { selectedModel: string }) {
  const t = useT();
  const queryClient = useQueryClient();
  const running = useRunningModels();
  const installed = useModels();
  const titleId = useId();
  const selectId = useId();
  const cardRef = useRef<HTMLElement>(null);
  const titleRef = useRef<HTMLHeadingElement>(null);
  // Modèles en cours de déchargement (bouton « Unloading… »).
  const [unloading, setUnloading] = useState<ReadonlySet<string>>(new Set());
  const [unloadingAll, setUnloadingAll] = useState(false);
  const [unloadError, setUnloadError] = useState<ActionError | null>(null);
  // Modèle en cours de chargement par Load.
  const [loading, setLoading] = useState<string | null>(null);
  const [loadError, setLoadError] = useState<ActionError | null>(null);
  const [choice, setChoice] = useState<string | null>(null);

  // Bouton focalisé retiré (ligne déchargée, Unload all, Try again) : la
  // ligne venue à sa place, sinon la précédente, sinon le titre.
  const rescue = useCallback((lost: Element) => {
    const row = lost.closest<HTMLElement>("[data-testid=loaded-model]");
    const buttons = cardRef.current?.querySelectorAll<HTMLElement>(
      "[data-testid=unload-model]",
    );
    if (row && buttons) {
      const index = Number(row.dataset.index);
      const next = buttons[index] ?? buttons[index - 1];
      if (next) return next;
    }
    return titleRef.current;
  }, []);
  useFocusRescue(cardRef, rescue);

  /** Une nouvelle action efface les échecs des précédentes. */
  const clearErrors = () => {
    setUnloadError(null);
    setLoadError(null);
  };

  /**
   * Relit la liste et la VRAM après une action ; une relecture en échec
   * s'affiche d'elle-même.
   */
  const refresh = () =>
    Promise.all([
      queryClient.invalidateQueries({ queryKey: runningModelsKey }),
      queryClient.invalidateQueries({ queryKey: machineMemoryKey }),
    ]);

  const unload = async (model: string) => {
    clearErrors();
    setUnloading((current) => new Set(current).add(model));
    try {
      await unloadModel(model);
    } catch (cause) {
      const message = errorMessage(cause);
      setUnloadError({
        model,
        message: (t) =>
          t("settings.models.loaded.unloadError", { model, message }),
      });
    }
    // Le bouton garde « Unloading… » jusqu'à la liste relue : la ligne
    // disparaît sans revenir un instant à « Unload ».
    await refresh();
    setUnloading((current) => {
      const next = new Set(current);
      next.delete(model);
      return next;
    });
  };

  const unloadAll = async () => {
    clearErrors();
    setUnloadingAll(true);
    try {
      await unloadAllModels();
    } catch (cause) {
      const message = errorMessage(cause);
      setUnloadError({
        model: null,
        message: (t) => t("settings.models.loaded.unloadAllError", { message }),
      });
    }
    await refresh();
    setUnloadingAll(false);
  };

  const load = async (model: string) => {
    clearErrors();
    setLoading(model);
    try {
      await loadModel(model);
    } catch (cause) {
      const message = errorMessage(cause);
      setLoadError({
        model,
        message: (t) =>
          t("settings.models.loaded.loadError", { model, message }),
      });
    }
    await refresh();
    setLoading(null);
  };

  // Une relecture automatique en cours est rejointe, pas annulée.
  const retry = () => {
    void running.refetch({ cancelRefetch: false });
    void installed.refetch({ cancelRefetch: false });
  };

  const title = (
    <h2
      ref={titleRef}
      id={titleId}
      tabIndex={-1}
      className="rounded-md text-sm/6 font-medium text-zinc-950 outline-none dark:text-white"
    >
      {t("settings.models.loaded.title")}
    </h2>
  );

  let body;
  if (running.failure !== null) {
    body = (
      <div className="flex items-start justify-between gap-4">
        <p
          role="alert"
          data-testid="loaded-models-error"
          className="min-w-0 text-sm/6 break-words text-red-600 dark:text-red-400"
        >
          {t("settings.models.loaded.reachError", {
            message: errorMessage(running.failure),
          })}
        </p>
        <button type="button" className={SMALL_BUTTON} onClick={retry}>
          <ArrowPathIcon aria-hidden="true" className="h-4 w-4" />
          {t("settings.tryAgain")}
        </button>
      </div>
    );
  } else if (!running.data) {
    body = (
      <div
        data-testid="loaded-models-loading"
        aria-hidden="true"
        className="h-12 animate-pulse rounded-lg bg-neutral-200 dark:bg-neutral-700"
      />
    );
  } else {
    const models = running.data;
    // Heure de la dernière lecture réussie : elle change à chaque relecture,
    // même d'un résultat identique, et les échéances sont recalculées.
    const now = running.dataUpdatedAt;
    // Installés, pas dans le nuage (rien à mettre en mémoire ici), pas déjà
    // en mémoire.
    const candidates = (installed.data ?? [])
      .map((info) => info.model)
      .filter(
        (name) =>
          !isCloudModel(name) &&
          !models.some((model) => sameRunningModel(model.name, name)),
      );
    const chosen =
      candidates.find((name) => name === choice) ??
      candidates.find((name) => sameModel(name, selectedModel)) ??
      candidates[0] ??
      "";
    const loaded = (name: string) =>
      models.some((model) => sameRunningModel(model.name, name));
    // Échecs dépassés : modèle chargé depuis (chat...), modèle déchargé
    // depuis, plus rien en mémoire après un Unload all raté.
    const shownLoadError =
      loadError && !loaded(loadError.model ?? "") ? loadError.message(t) : null;
    const shownUnloadError =
      unloadError &&
      (unloadError.model === null
        ? models.length > 0
        : loaded(unloadError.model))
        ? unloadError.message(t)
        : null;
    const unloadAllBusy = unloadingAll || unloading.size > 0;
    const loadUnavailable = chosen === "" || loading !== null;
    body = (
      <>
        <VramGauge models={models} />
        {models.length === 0 ? (
          <p className={MUTED}>{t("settings.models.loaded.none")}</p>
        ) : (
          <>
            <div className="flex items-center justify-between gap-4">
              <p data-testid="loaded-models-summary" className={MUTED}>
                {loadedSummary(t, models)}
              </p>
              <button
                type="button"
                data-testid="unload-all"
                className={SMALL_BUTTON}
                aria-disabled={unloadAllBusy}
                onClick={() => {
                  if (!unloadAllBusy) void unloadAll();
                }}
              >
                {unloadingAll
                  ? t("settings.models.loaded.unloading")
                  : t("settings.models.loaded.unloadAll")}
              </button>
            </div>
            {/* Séparateurs par bordure sur chaque ligne : la classe de
                séparation de Tailwind est interdite (check-bundle.mjs). */}
            <ul>
              {models.map((model, index) => (
                <LoadedModelRow
                  key={model.name}
                  index={index}
                  model={model}
                  now={now}
                  unloading={unloadingAll || unloading.has(model.name)}
                  onUnload={() => void unload(model.name)}
                />
              ))}
            </ul>
          </>
        )}
        {shownUnloadError && (
          <p role="alert" data-testid="unload-error" className={ALERT}>
            {shownUnloadError}
          </p>
        )}

        <div className="border-t border-zinc-950/5 pt-4 dark:border-white/10">
          <div className="flex flex-wrap items-center gap-x-3 gap-y-2">
            <label
              htmlFor={selectId}
              className="text-sm/6 text-zinc-950 dark:text-white"
            >
              {t("settings.models.loaded.loadLabel")}
            </label>
            <select
              id={selectId}
              data-testid="load-model-select"
              className={SELECT}
              value={chosen}
              disabled={candidates.length === 0 || loading !== null}
              onChange={(event) => setChoice(event.target.value)}
            >
              {candidates.length === 0 ? (
                <option value="" disabled>
                  {installed.data
                    ? t("settings.models.loaded.noOther")
                    : t("settings.models.loaded.loading")}
                </option>
              ) : (
                candidates.map((name) => (
                  <option key={name} value={name}>
                    {name}
                  </option>
                ))
              )}
            </select>
            <button
              type="button"
              data-testid="load-model-button"
              className={SMALL_BUTTON}
              aria-disabled={loadUnavailable}
              onClick={() => {
                if (!loadUnavailable) void load(chosen);
              }}
            >
              {loading !== null
                ? t("settings.models.loaded.loading")
                : t("settings.models.loaded.load")}
            </button>
          </div>
          {shownLoadError && (
            <p
              role="alert"
              data-testid="load-error"
              className={`mt-2 ${ALERT}`}
            >
              {shownLoadError}
            </p>
          )}
        </div>
      </>
    );
  }

  return (
    <section
      ref={cardRef}
      data-testid="loaded-models"
      aria-labelledby={titleId}
      className="overflow-hidden rounded-xl bg-white dark:bg-neutral-800"
    >
      <div className="space-y-4 p-4">
        {title}
        {body}
      </div>
    </section>
  );
}

/** Un modèle en mémoire : nom lisible, détails, Unload. */
function LoadedModelRow({
  index,
  model,
  now,
  unloading,
  onUnload,
}: {
  /** Rang dans la liste (focus rendu à la ligne venue à sa place). */
  index: number;
  model: RunningModel;
  now: number;
  /** Déchargement en cours (ce modèle, ou tous). */
  unloading: boolean;
  onUnload: () => void;
}) {
  const t = useT();
  const nameId = useId();
  const cpu = cpuShare([model], model.name);
  return (
    <li
      data-testid="loaded-model"
      data-model={model.name}
      data-index={index}
      className="flex items-center justify-between gap-4 border-t border-zinc-950/5 py-3 first:border-t-0 first:pt-0 last:pb-0 dark:border-white/10"
    >
      <div className="min-w-0">
        <div
          id={nameId}
          className="flex items-center gap-2 text-sm/6 text-zinc-950 dark:text-white"
          title={model.name}
        >
          {/* Teinte de sa part sur la jauge de la VRAM. */}
          {model.sizeVram > 0 && (
            <span
              aria-hidden="true"
              data-testid="vram-swatch"
              className={`h-2.5 w-2.5 flex-none rounded-full ${modelShade(index)}`}
            />
          )}
          <span className="min-w-0">
            <ModelLabel model={model.name} />
          </span>
          {cpu !== null && <SlowModelSnail model={model.name} cpu={cpu} />}
        </div>
        <p className={MUTED}>{runningDetails(t, model, now)}</p>
      </div>
      <button
        type="button"
        data-testid="unload-model"
        className={SMALL_BUTTON}
        aria-describedby={nameId}
        aria-disabled={unloading}
        onClick={() => {
          if (!unloading) onUnload();
        }}
      >
        {unloading
          ? t("settings.models.loaded.unloading")
          : t("settings.models.loaded.unload")}
      </button>
    </li>
  );
}
