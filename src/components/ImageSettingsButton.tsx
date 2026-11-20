// Réglages avancés de la génération d'images (propre à ChatonLM ; l'officiel
// n'a pas d'images) : bouton à côté du sélecteur du modèle d'images, en mode
// Image. Il ouvre un panneau au-dessus de la zone de saisie, avec, pour le
// modèle choisi : étapes, guidage (CFG) et prompt négatif (seulement pour une
// famille qui emploie le guidage ; sinon une phrase dit qu'ils seraient sans
// effet), graine aléatoire ou fixe. Champ vide : valeur du modèle (rappelée
// dans l'aide). Chaque changement valide est enregistré tout de suite (à la
// sortie du champ pour les textes), par modèle, et vaut partout où ce modèle
// peint (conversation, Easy Link, VS Code). « Model values » efface tout.
// Conduite du panneau reprise de la jauge du contexte (ContextGauge.tsx) :
// Échap et clic dehors le ferment, le focus revient au bouton.

import { AdjustmentsHorizontalIcon } from "@heroicons/react/24/outline";
import {
  type RefObject,
  useCallback,
  useEffect,
  useId,
  useRef,
  useState,
} from "react";
import type { ImageSettings } from "@/bindings/ImageSettings";
import type { ImageSettingsView } from "@/bindings/ImageSettingsView";
import { useImageSettings } from "@/hooks/useImageModels";
import { useT } from "@/i18n";
import {
  cfgText,
  MAX_CFG,
  MAX_SEED,
  parseCfg,
  parseWhole,
} from "@/lib/imageSettings";
import { errorMessage } from "@/lib/ipc";
import { isEscapeKey } from "@/lib/shortcuts";

export function ImageSettingsButton() {
  const t = useT();
  const { view } = useImageSettings();
  const [open, setOpen] = useState(false);
  const wrapperRef = useRef<HTMLDivElement>(null);
  const buttonRef = useRef<HTMLButtonElement>(null);
  const panelRef = useRef<HTMLDivElement>(null);
  const panelId = useId();

  const close = useCallback(() => {
    if (panelRef.current?.contains(document.activeElement)) {
      buttonRef.current?.focus();
    }
    setOpen(false);
  }, []);

  useEffect(() => {
    if (open) panelRef.current?.focus();
  }, [open]);

  // Clic dehors, Échap (consommée : elle n'arrête pas la réponse).
  useEffect(() => {
    if (!open) return;
    const onMouseDown = (event: MouseEvent) => {
      if (!wrapperRef.current?.contains(event.target as Node)) close();
    };
    const onKeyDown = (event: KeyboardEvent) => {
      if (!isEscapeKey(event) || event.defaultPrevented) return;
      event.preventDefault();
      buttonRef.current?.focus();
      setOpen(false);
    };
    document.addEventListener("mousedown", onMouseDown);
    document.addEventListener("keydown", onKeyDown);
    return () => {
      document.removeEventListener("mousedown", onMouseDown);
      document.removeEventListener("keydown", onKeyDown);
    };
  }, [open, close]);

  // Aucun modèle choisi : rien à régler.
  if (!view) return null;

  return (
    <div
      ref={wrapperRef}
      className="relative"
      onBlur={(event) => {
        const to = event.relatedTarget;
        if (open && to && !wrapperRef.current?.contains(to)) setOpen(false);
      }}
    >
      <button
        ref={buttonRef}
        type="button"
        data-testid="image-settings-button"
        aria-label={t("chat.imageSettings.button")}
        title={t("chat.imageSettings.button")}
        aria-haspopup="dialog"
        aria-expanded={open}
        aria-controls={open ? panelId : undefined}
        onClick={() => (open ? close() : setOpen(true))}
        className="flex h-9 w-9 cursor-pointer items-center justify-center rounded-full border border-transparent bg-neutral-200 focus:outline-2 focus:outline-offset-2 focus:outline-accent-fg dark:bg-neutral-700"
      >
        <AdjustmentsHorizontalIcon
          aria-hidden="true"
          className="h-4.5 w-4.5 stroke-2 text-neutral-500 dark:text-neutral-400"
        />
      </button>
      {open && (
        // Remonté à chaque ouverture et à chaque modèle : brouillon neuf,
        // pris des réglages enregistrés.
        <SettingsPanel
          key={view.modelId}
          view={view}
          panelId={panelId}
          panelRef={panelRef}
        />
      )}
    </div>
  );
}

function SettingsPanel({
  view,
  panelId,
  panelRef,
}: {
  view: ImageSettingsView;
  panelId: string;
  panelRef: RefObject<HTMLDivElement | null>;
}) {
  const t = useT();
  const { save } = useImageSettings();
  const titleId = useId();
  const stepsId = useId();
  const cfgId = useId();
  const negativeId = useId();
  const seedId = useId();
  const { settings } = view;
  const [steps, setSteps] = useState(settings.steps?.toString() ?? "");
  const [cfg, setCfg] = useState(
    settings.cfgScale === null ? "" : cfgText(t.lang, settings.cfgScale),
  );
  const [negative, setNegative] = useState(settings.negativePrompt);
  const [fixedSeed, setFixedSeed] = useState(settings.seed !== null);
  const [seed, setSeed] = useState(settings.seed?.toString() ?? "");
  // Dernières valeurs envoyées (référence : la sortie du champ et le départ
  // du panneau peuvent se suivre sans rendu entre eux).
  const saved = useRef<ImageSettings>(settings);

  const stepsValue = parseWhole(steps, 1, view.maxSteps);
  const cfgValue = parseCfg(cfg);
  const seedValue = fixedSeed ? parseWhole(seed, 0, MAX_SEED) : null;
  const tooLong = [...negative].length > view.maxPrompt;

  /** Enregistre `next` s'il change quelque chose. */
  const commit = (next: ImageSettings) => {
    if (
      next.steps === saved.current.steps &&
      next.cfgScale === saved.current.cfgScale &&
      next.negativePrompt === saved.current.negativePrompt &&
      next.seed === saved.current.seed
    ) {
      return;
    }
    saved.current = next;
    save.mutate({ id: view.modelId, settings: next });
  };

  /** Réglages du brouillon, ceux qui sont invalides laissés à leur valeur enregistrée. */
  const draft = (changes: Partial<ImageSettings> = {}): ImageSettings => ({
    steps: stepsValue === undefined ? saved.current.steps : stepsValue,
    cfgScale: cfgValue === undefined ? saved.current.cfgScale : cfgValue,
    negativePrompt: tooLong ? saved.current.negativePrompt : negative,
    seed: seedValue === undefined ? saved.current.seed : seedValue,
    ...changes,
  });

  const reset = () => {
    setSteps("");
    setCfg("");
    setNegative("");
    setFixedSeed(false);
    setSeed("");
    commit({ steps: null, cfgScale: null, negativePrompt: "", seed: null });
  };

  const field =
    "mt-1 w-full rounded-lg border border-neutral-200 bg-white px-2 py-1 text-sm text-neutral-900 outline-none focus:border-accent-fg dark:border-neutral-600 dark:bg-neutral-900 dark:text-white";
  const hint = "mt-1 text-xs text-neutral-500 dark:text-neutral-400";
  const invalid = "mt-1 text-xs text-red-600 dark:text-red-400";

  return (
    <div
      ref={panelRef}
      id={panelId}
      role="dialog"
      aria-labelledby={titleId}
      tabIndex={-1}
      data-testid="image-settings-panel"
      className="absolute bottom-full left-0 mb-2 max-h-[70vh] w-80 overflow-y-auto rounded-2xl border border-neutral-100 bg-white p-4 text-sm text-neutral-800 shadow-xl shadow-black/5 outline-none dark:border-neutral-600/40 dark:bg-neutral-800 dark:text-white"
    >
      <h2 id={titleId} className="font-medium">
        {t("chat.imageSettings.title")}
      </h2>
      <p className="mt-0.5 truncate text-xs text-neutral-500 dark:text-neutral-400">
        {t("chat.imageSettings.forModel", { model: view.modelName })}
      </p>

      <label htmlFor={stepsId} className="mt-4 block font-medium">
        {t("chat.imageSettings.steps")}
      </label>
      <input
        id={stepsId}
        inputMode="numeric"
        value={steps}
        placeholder={view.defaultSteps.toString()}
        aria-invalid={stepsValue === undefined}
        onChange={(event) => setSteps(event.target.value)}
        onBlur={() => commit(draft())}
        className={field}
      />
      <p className={stepsValue === undefined ? invalid : hint}>
        {stepsValue === undefined
          ? t("chat.imageSettings.invalidSteps", { max: view.maxSteps })
          : t("chat.imageSettings.stepsHint", { value: view.defaultSteps })}
      </p>

      {view.guidance ? (
        <>
          <label htmlFor={cfgId} className="mt-4 block font-medium">
            {t("chat.imageSettings.cfg")}
          </label>
          <input
            id={cfgId}
            inputMode="decimal"
            value={cfg}
            placeholder={cfgText(t.lang, view.defaultCfgScale)}
            aria-invalid={cfgValue === undefined}
            onChange={(event) => setCfg(event.target.value)}
            onBlur={() => commit(draft())}
            className={field}
          />
          <p className={cfgValue === undefined ? invalid : hint}>
            {cfgValue === undefined
              ? t("chat.imageSettings.invalidCfg", { max: MAX_CFG })
              : t("chat.imageSettings.cfgHint", {
                  value: cfgText(t.lang, view.defaultCfgScale),
                })}
          </p>

          <label htmlFor={negativeId} className="mt-4 block font-medium">
            {t("chat.imageSettings.negative")}
          </label>
          <textarea
            id={negativeId}
            rows={3}
            value={negative}
            placeholder={t("chat.imageSettings.negativePlaceholder")}
            aria-invalid={tooLong}
            onChange={(event) => setNegative(event.target.value)}
            onBlur={() => commit(draft())}
            className={`${field} resize-y`}
          />
          {tooLong && (
            <p className={invalid}>
              {t("chat.imageSettings.tooLong", { max: view.maxPrompt })}
            </p>
          )}
        </>
      ) : (
        <p data-testid="image-settings-no-guidance" className={`mt-4 ${hint}`}>
          {t("chat.imageSettings.noGuidance")}
        </p>
      )}

      <fieldset className="mt-4 min-w-0">
        <legend className="font-medium">{t("chat.imageSettings.seed")}</legend>
        <div className="mt-1 flex items-center gap-4">
          <label className="flex items-center gap-1.5">
            <input
              type="radio"
              name={seedId}
              checked={!fixedSeed}
              onChange={() => {
                setFixedSeed(false);
                commit(draft({ seed: null }));
              }}
              className="accent-neutral-900 dark:accent-white"
            />
            {t("chat.imageSettings.seedRandom")}
          </label>
          <label className="flex items-center gap-1.5">
            <input
              type="radio"
              name={seedId}
              checked={fixedSeed}
              onChange={() => setFixedSeed(true)}
              className="accent-neutral-900 dark:accent-white"
            />
            {t("chat.imageSettings.seedFixed")}
          </label>
        </div>
        {fixedSeed && (
          <input
            inputMode="numeric"
            aria-label={t("chat.imageSettings.seedValue")}
            value={seed}
            aria-invalid={seedValue === undefined}
            onChange={(event) => setSeed(event.target.value)}
            onBlur={() => commit(draft())}
            className={field}
          />
        )}
        <p className={fixedSeed && seedValue === undefined ? invalid : hint}>
          {fixedSeed && seedValue === undefined
            ? t("chat.imageSettings.invalidSeed", { max: MAX_SEED })
            : t("chat.imageSettings.seedHint")}
        </p>
      </fieldset>

      <button
        type="button"
        onClick={reset}
        className="mt-4 rounded-full border border-neutral-200 px-3 py-1 text-sm font-medium text-neutral-800 outline-none hover:bg-neutral-100 focus-visible:ring-2 focus-visible:ring-accent-fg dark:border-neutral-600 dark:text-white dark:hover:bg-neutral-700"
      >
        {t("chat.imageSettings.reset")}
      </button>
      {save.isError && (
        <p
          role="alert"
          className="mt-2 text-xs break-words text-red-600 dark:text-red-400"
        >
          {t("chat.imageSettings.saveError", {
            message: errorMessage(save.error),
          })}
        </p>
      )}
    </div>
  );
}
