// Onglet « Dictation » des réglages (étape 17, propre à ChatonLM), en cartes :
// « Dictation » (activée ou non, pas sans modèle de transcription installé ;
// raccourci global : touche, changement par la boîte du bureau, ou commande
// à lier soi-même quand le portail manque, ou, sous Windows, saisie de la
// combinaison dans l'app : ShortcutRecorder.tsx ; bascule ou push-to-talk),
// « Transcription » (modèle, langue, dictionnaire, copie automatique dans le
// presse-papiers, puis collage là où se trouve le curseur, activé par défaut
// et grisé sans la copie), « Polish » (modèle et
// consignes du nettoyage), « Notifications » (texte dans les notifications),
// puis l'essai depuis la page et le dernier résultat.
// Chaque changement est enregistré aussitôt (« Saved » de la page, erreur
// sous la ligne) ; les consignes, en quittant le champ, en quittant la page
// ou avant Ctrl+Q. Ouverte pour elle (section=dictation, menu de la zone de
// notification), la carte défile en vue et son titre prend le focus à chaque
// demande, sauf si l'utilisateur a mis le focus ailleurs depuis. Listes pas
// encore lues ou illisibles (Ollama injoignable) : ni « not installed » ni
// téléchargement sur cette base. Serveur Ollama distant : la carte dit que
// la transcription lui est envoyée ; modèles du cloud jamais proposés.
// Modèles que cette machine ne peut pas faire tourner (Whisper sans AVX2...,
// Parakeet sans ONNX Runtime) : grisés dans la liste, jamais proposés en
// activant la dictée, et expliqués sous la liste s'ils sont choisis.
// Textes dans src/locales/<langue>/dictation.json ; langues parlées nommées
// dans la langue de l'interface (Intl), triées par ce nom.

import {
  ArrowPathIcon,
  BellIcon,
  BookOpenIcon,
  ClipboardDocumentCheckIcon,
  ClipboardDocumentIcon,
  CommandLineIcon,
  HandRaisedIcon,
  LanguageIcon,
  MicrophoneIcon,
  PencilSquareIcon,
  SparklesIcon,
  SpeakerWaveIcon,
  XMarkIcon,
} from "@heroicons/react/20/solid";
import { Link } from "@tanstack/react-router";
import {
  type ReactNode,
  useCallback,
  useEffect,
  useId,
  useMemo,
  useRef,
  useState,
} from "react";
import type { DictationMode } from "@/bindings/DictationMode";
import type { DictationPatch } from "@/bindings/DictationPatch";
import type { DictationResult } from "@/bindings/DictationResult";
import type { DictationSettings } from "@/bindings/DictationSettings";
import type { DictationStatus } from "@/bindings/DictationStatus";
import type { RuntimeProblemKind } from "@/bindings/RuntimeProblemKind";
import type { SpeechModelInfo } from "@/bindings/SpeechModelInfo";
import { CommandBlock } from "@/components/CommandBlock";
import { Row, RowError, Switch } from "@/components/settings/Row";
import { AppShortcut } from "@/components/settings/ShortcutRecorder";
import {
  useCopyLastDictation,
  useDictationActions,
  useDictationInfo,
  useDictationSettings,
  useDictationStatus,
  useSpeechModels,
} from "@/hooks/useDictation";
import { useModels } from "@/hooks/useModels";
import { useShortcutsStatus } from "@/hooks/useShortcuts";
import { useSettings } from "@/hooks/useSettings";
import { type TFunction, type TKey, useT } from "@/i18n";
import { formatTimeAgo, languageName } from "@/i18n/format";
import {
  addWord,
  AUTO_LANGUAGE,
  isCloudModel,
  isLocalAddress,
  MAX_PROMPT_CHARS,
  MAX_WORD_CHARS,
  phaseText,
} from "@/lib/dictation";
import { errorMessage } from "@/lib/ipc";
import { sameModel } from "@/lib/modelNames";
import { isWindows } from "@/lib/platform";
import { onBeforeQuit } from "@/lib/quit";

/** Bascule ou push-to-talk, dans la langue de `t`. */
function modes(t: TFunction): { value: DictationMode; label: string }[] {
  return [
    { value: "toggle", label: t("dictation.mode.toggle") },
    { value: "hold", label: t("dictation.mode.hold") },
  ];
}

const MUTED = "text-sm/6 text-zinc-500 dark:text-zinc-400";
const LINK =
  "rounded-md text-sm/6 text-zinc-700 underline underline-offset-2 outline-none hover:text-zinc-950 focus-visible:ring-2 focus-visible:ring-accent-fg dark:text-zinc-300 dark:hover:text-white";
const SELECT =
  "h-9 w-full max-w-sm rounded-lg border border-neutral-300 bg-white pr-7 pl-2 text-sm text-neutral-900 outline-none focus-visible:border-accent-fg focus-visible:ring-2 focus-visible:ring-accent-fg/40 disabled:opacity-50 dark:border-neutral-700 dark:bg-neutral-800 dark:text-neutral-100";
const SMALL_BUTTON =
  "inline-flex flex-none items-center gap-1.5 rounded-lg border border-zinc-950/10 bg-white px-3 py-1 text-sm/6 font-medium text-zinc-950 shadow-sm outline-none hover:bg-zinc-50 focus-visible:ring-2 focus-visible:ring-accent-fg disabled:cursor-default disabled:opacity-50 dark:border-white/15 dark:bg-zinc-900 dark:text-white dark:hover:bg-zinc-800";
const PRIMARY_BUTTON =
  "inline-flex flex-none items-center gap-1.5 rounded-lg border border-zinc-700/90 bg-zinc-800 px-3 py-1 text-sm/6 font-medium text-white shadow-sm outline-none hover:bg-zinc-700 focus-visible:ring-2 focus-visible:ring-accent-fg disabled:cursor-default disabled:opacity-50 dark:border-white/10 dark:bg-white dark:text-zinc-900 dark:hover:bg-zinc-200";
const KBD =
  "inline-flex items-center rounded-md border border-neutral-300 bg-neutral-50 px-1.5 py-0.5 font-sans text-xs font-medium text-neutral-800 shadow-sm dark:border-neutral-600 dark:bg-neutral-900 dark:text-neutral-100";
const ALERT = "mt-2 text-xs break-words text-red-600 dark:text-red-400";

type Field = keyof DictationPatch;

/** Modèle proposé en activant la dictée sans modèle choisi : le recommandé d'abord. */
function preferredModel(installed: SpeechModelInfo[]): SpeechModelInfo | null {
  return installed.find((model) => model.recommended) ?? installed[0] ?? null;
}

/** Explication d'un ONNX Runtime indisponible (Parakeet), par raison. */
const RUNTIME_NOTES: Record<RuntimeProblemKind, TKey> = {
  missing: "dictation.transcription.model.runtime.missing",
  incompatible: "dictation.transcription.model.runtime.incompatible",
  loadFailed: "dictation.transcription.model.runtime.loadFailed",
};

/** Installé, et cette machine peut le faire tourner. */
function usable(model: SpeechModelInfo | undefined): boolean {
  return (
    !!model?.installed &&
    model.missingCpuFeatures.length === 0 &&
    model.runtimeProblem === null
  );
}

/** Nom d'un modèle installé dans la liste, avec ce qui l'empêche de tourner. */
function optionName(t: TFunction, model: SpeechModelInfo): string {
  if (model.missingCpuFeatures.length > 0) {
    return t("dictation.transcription.model.unsupportedOption", {
      name: model.name,
    });
  }
  if (model.runtimeProblem !== null) {
    return t("dictation.transcription.model.unavailableOption", {
      name: model.name,
    });
  }
  return model.name;
}

export function Dictation({
  focusRequest = null,
  onSaved,
}: {
  /**
   * Demande de montrer la carte (une valeur par navigation) ; null : aucune.
   */
  focusRequest?: string | null;
  /** Un changement est enregistré (« Saved » de la page). */
  onSaved: () => void;
}) {
  const t = useT();
  const { settings, error, update, reload } = useDictationSettings({
    refresh: true,
  });
  const info = useDictationInfo();
  const status = useDictationStatus();
  const catalog = useSpeechModels();
  const [errors, setErrors] = useState<Partial<Record<Field, string>>>({});
  const ids = {
    title: useId(),
    description: useId(),
    shortcut: useId(),
    shortcutDescription: useId(),
    mode: useId(),
    modeDescription: useId(),
    speechModel: useId(),
    speechModelDescription: useId(),
    language: useId(),
    languageDescription: useId(),
    polishModel: useId(),
    polishModelDescription: useId(),
    prompt: useId(),
    promptDescription: useId(),
    dictionary: useId(),
    dictionaryDescription: useId(),
    notify: useId(),
    notifyDescription: useId(),
    autoCopy: useId(),
    autoCopyDescription: useId(),
    autoPaste: useId(),
    autoPasteDescription: useId(),
    transcription: useId(),
    polish: useId(),
    notifications: useId(),
  };

  const { settings: app } = useSettings();
  const server = app?.effectiveServerUrl ?? "";
  const remote = server !== "" && !isLocalAddress(server);

  // Toutes les cartes de la dictée (défilement, focus déjà dedans).
  const cardRef = useRef<HTMLDivElement>(null);
  const titleRef = useRef<HTMLHeadingElement>(null);
  // Dernière demande servie, et celle qui attend le contenu (avec le focus
  // d'alors).
  const servedRef = useRef<string | null>(null);
  const waitingRef = useRef<{
    request: string;
    active: Element | null;
  } | null>(null);
  const shown = settings !== undefined || error !== null;
  useEffect(() => {
    if (focusRequest === null || servedRef.current === focusRequest) return;
    if (waitingRef.current?.request !== focusRequest) {
      waitingRef.current = {
        request: focusRequest,
        active: document.activeElement,
      };
    }
    if (!shown) return;
    const before = waitingRef.current.active;
    servedRef.current = focusRequest;
    waitingRef.current = null;
    const card = cardRef.current;
    if (!card) return;
    // La demande l'emporte sur le focus d'avant ; mais l'utilisateur l'a mis
    // ailleurs depuis (contenu long à lire) : ni défilement ni focus volé.
    const active = document.activeElement;
    if (
      active !== before &&
      active !== null &&
      active !== document.body &&
      !card.contains(active)
    ) {
      return;
    }
    // jsdom n'a pas scrollIntoView.
    card.scrollIntoView?.({ block: "start" });
    titleRef.current?.focus({ preventScroll: true });
  }, [focusRequest, shown]);

  /** Enregistre `patch` ; l'erreur s'affiche sous la ligne `field`. */
  const save = async (field: Field, patch: DictationPatch) => {
    setErrors((current) => ({ ...current, [field]: undefined }));
    try {
      await update(patch);
    } catch (cause) {
      setErrors((current) => ({ ...current, [field]: errorMessage(cause) }));
      return false;
    }
    onSaved();
    return true;
  };

  const title = (
    <h2
      ref={titleRef}
      id={ids.title}
      tabIndex={-1}
      className="rounded-md font-medium outline-none"
    >
      {t("dictation.card.title")}
    </h2>
  );

  let body: ReactNode;
  // Cartes Transcription, Polish et Notifications, une fois les réglages lus.
  let more: ReactNode = null;
  if (error && !settings) {
    body = (
      <Row
        icon={MicrophoneIcon}
        label={title}
        description={t("dictation.card.loadFailed")}
        descriptionId={ids.description}
      >
        <button
          type="button"
          className={`${SMALL_BUTTON} mt-2`}
          onClick={() => void reload()}
        >
          <ArrowPathIcon aria-hidden="true" className="h-4 w-4" />
          {t("dictation.common.tryAgain")}
        </button>
      </Row>
    );
  } else if (!settings) {
    body = (
      <Row
        icon={MicrophoneIcon}
        label={title}
        description={t("dictation.card.description")}
        descriptionId={ids.description}
      >
        <div
          data-testid="dictation-loading"
          aria-hidden="true"
          className="mt-3 h-24 animate-pulse rounded-lg bg-neutral-200 dark:bg-neutral-700"
        />
      </Row>
    );
  } else {
    const models = catalog.data ?? [];
    const installed = models.filter((model) => model.installed);
    const ready = installed.filter(usable);
    // Catalogue pas encore lu (ou illisible) : rien n'est dit « not installed ».
    const catalogRead = catalog.data !== undefined;
    const current = models.find((model) => model.id === settings.speechModel);
    const noModel = catalog.isSuccess && ready.length === 0;
    const enable = (enabled: boolean) => {
      if (!enabled) return void save("enabled", { enabled });
      const proposed = usable(current) ? null : preferredModel(ready);
      void save("enabled", {
        enabled,
        ...(proposed && { speechModel: proposed.id }),
      });
    };
    body = (
      <>
        <div className="flex items-start justify-between gap-4">
          <Row
            icon={MicrophoneIcon}
            label={title}
            description={
              remote ? (
                <>
                  {/* Serveur Ollama distant : le nettoyage n'est plus local. */}
                  {t("dictation.card.remoteDescription")}{" "}
                  <span className="text-amber-700 dark:text-amber-400">
                    {t("dictation.card.remoteNote", { server })}
                  </span>
                </>
              ) : (
                t("dictation.card.description")
              )
            }
            descriptionId={ids.description}
          >
            {noModel && (
              <p className="mt-1">
                <Link to="/hub/transcription" className={LINK}>
                  {t("dictation.card.downloadModel")}
                </Link>
              </p>
            )}
            <RowError message={errors.enabled} />
          </Row>
          <Switch
            label={t("dictation.card.enable")}
            checked={settings.enabled}
            describedBy={ids.description}
            // Déjà activée (modèle supprimé depuis) : on peut toujours éteindre.
            disabled={
              !settings.enabled && (!catalog.isSuccess || ready.length === 0)
            }
            onChange={enable}
          />
        </div>

        <Row
          icon={CommandLineIcon}
          label={<span id={ids.shortcut}>{t("dictation.shortcut.label")}</span>}
          description={t("dictation.shortcut.description")}
          descriptionId={ids.shortcutDescription}
        >
          {status.data && <ShortcutLine status={status.data} />}
        </Row>

        <div className="flex flex-wrap items-start justify-between gap-4">
          <Row
            icon={HandRaisedIcon}
            label={<span id={ids.mode}>{t("dictation.mode.label")}</span>}
            description={
              <>
                {t("dictation.mode.description")}
                {status.data?.shortcut.state === "unavailable" && (
                  <>
                    {" "}
                    <span className="text-amber-700 dark:text-amber-400">
                      {t("dictation.mode.holdNeedsShortcut")}
                    </span>
                  </>
                )}
              </>
            }
            descriptionId={ids.modeDescription}
          >
            <RowError message={errors.mode} />
          </Row>
          <div className="flex-shrink-0">
            <Choice
              options={modes(t)}
              value={settings.mode}
              onChange={(mode) => void save("mode", { mode })}
              labelledBy={ids.mode}
              describedBy={ids.modeDescription}
            />
          </div>
        </div>
      </>
    );
    more = (
      <>
        <Card
          title={t("dictation.transcription.title")}
          titleId={ids.transcription}
        >
          <Row
            icon={SpeakerWaveIcon}
            label={
              <label htmlFor={ids.speechModel}>
                {t("dictation.transcription.model.label")}
              </label>
            }
            description={t("dictation.transcription.model.description")}
            descriptionId={ids.speechModelDescription}
          >
            <div className="mt-3 flex flex-wrap items-center gap-x-4 gap-y-2">
              <select
                id={ids.speechModel}
                aria-describedby={ids.speechModelDescription}
                className={SELECT}
                value={settings.speechModel}
                disabled={installed.length === 0}
                onChange={(event) =>
                  void save("speechModel", { speechModel: event.target.value })
                }
              >
                {!catalogRead ? (
                  <option value={settings.speechModel} disabled>
                    {catalog.isError
                      ? settings.speechModel ||
                        t("dictation.transcription.model.none")
                      : t("dictation.transcription.model.loading")}
                  </option>
                ) : (
                  !current?.installed && (
                    // Rien de choisi, ou modèle supprimé depuis.
                    <option value={settings.speechModel} disabled>
                      {settings.speechModel
                        ? t("dictation.common.notInstalled", {
                            name: current?.name ?? settings.speechModel,
                          })
                        : installed.length === 0
                          ? t("dictation.transcription.model.noneInstalled")
                          : t("dictation.transcription.model.choose")}
                    </option>
                  )
                )}
                {installed.map((model) => (
                  <option
                    key={model.id}
                    value={model.id}
                    disabled={!usable(model)}
                  >
                    {optionName(t, model)}
                  </option>
                ))}
              </select>
              <Link to="/hub/transcription" className={LINK}>
                {t("dictation.transcription.model.manage")}
              </Link>
            </div>
            {current?.installed && current.missingCpuFeatures.length > 0 && (
              <p className="mt-2 text-xs break-words text-amber-700 dark:text-amber-400">
                {t("dictation.transcription.model.unsupported", {
                  name: current.name,
                  features: current.missingCpuFeatures.join(", "),
                })}
              </p>
            )}
            {current?.installed && current.runtimeProblem && (
              <p className="mt-2 text-xs break-words text-amber-700 dark:text-amber-400">
                {t(RUNTIME_NOTES[current.runtimeProblem.kind], {
                  name: current.name,
                  detail: current.runtimeProblem.detail,
                })}
              </p>
            )}
            {catalog.isError && !catalogRead && (
              <p role="alert" className={ALERT}>
                {t("dictation.transcription.model.unreadable", {
                  error: errorMessage(catalog.error),
                })}{" "}
                <button
                  type="button"
                  className="underline underline-offset-2"
                  onClick={() => void catalog.refetch()}
                >
                  {t("dictation.common.tryAgain")}
                </button>
              </p>
            )}
            <RowError message={errors.speechModel} />
          </Row>

          <LanguageRow
            id={ids.language}
            descriptionId={ids.languageDescription}
            value={settings.language}
            languages={info.data?.languages ?? []}
            parakeet={current?.engine === "parakeet"}
            onChange={(language) => void save("language", { language })}
            error={errors.language}
          />

          <DictionaryRow
            id={ids.dictionary}
            descriptionId={ids.dictionaryDescription}
            words={settings.dictionary}
            onSave={(dictionary) => save("dictionary", { dictionary })}
            error={errors.dictionary}
          />

          <div className="flex items-start justify-between gap-4">
            <Row
              icon={ClipboardDocumentIcon}
              label={
                <span id={ids.autoCopy}>
                  {t("dictation.transcription.autoCopy.label")}
                </span>
              }
              description={t("dictation.transcription.autoCopy.description")}
              descriptionId={ids.autoCopyDescription}
            >
              <RowError message={errors.autoCopy} />
            </Row>
            <Switch
              label={t("dictation.transcription.autoCopy.label")}
              checked={settings.autoCopy}
              describedBy={ids.autoCopyDescription}
              onChange={(autoCopy) => void save("autoCopy", { autoCopy })}
            />
          </div>

          <div className="flex items-start justify-between gap-4">
            <Row
              icon={ClipboardDocumentCheckIcon}
              label={
                <span id={ids.autoPaste}>
                  {t("dictation.transcription.autoPaste.label")}
                </span>
              }
              description={
                <>
                  {t("dictation.transcription.autoPaste.description")}
                  {/* Linux : le bureau demande une fois l'autorisation
                      d'envoyer la touche (portail RemoteDesktop). */}
                  {!isWindows() && (
                    <> {t("dictation.transcription.autoPaste.askHint")}</>
                  )}
                </>
              }
              descriptionId={ids.autoPasteDescription}
            >
              <RowError message={errors.autoPaste} />
            </Row>
            {/* Sans la copie automatique, le presse-papiers n'a pas le
                texte : rien à coller. */}
            <Switch
              label={t("dictation.transcription.autoPaste.label")}
              checked={settings.autoPaste}
              describedBy={ids.autoPasteDescription}
              disabled={!settings.autoCopy}
              onChange={(autoPaste) => void save("autoPaste", { autoPaste })}
            />
          </div>
        </Card>

        <Card title={t("dictation.polish.title")} titleId={ids.polish}>
          <PolishModelRow
            id={ids.polishModel}
            descriptionId={ids.polishModelDescription}
            value={settings.polishModel}
            onChange={(polishModel) =>
              void save("polishModel", { polishModel })
            }
            error={errors.polishModel}
          />

          <InstructionsRow
            id={ids.prompt}
            descriptionId={ids.promptDescription}
            saved={settings.polishPrompt}
            defaultPrompt={info.data?.defaultPolishPrompt ?? ""}
            onSave={(polishPrompt) => save("polishPrompt", { polishPrompt })}
            error={errors.polishPrompt}
          />
        </Card>

        <Card
          title={t("dictation.notifications.title")}
          titleId={ids.notifications}
        >
          <div className="flex items-start justify-between gap-4">
            <Row
              icon={BellIcon}
              label={
                <span id={ids.notify}>
                  {t("dictation.notifications.showText")}
                </span>
              }
              description={t("dictation.notifications.description")}
              descriptionId={ids.notifyDescription}
            >
              <RowError message={errors.notifyText} />
            </Row>
            <Switch
              label={t("dictation.notifications.showText")}
              checked={settings.notifyText}
              describedBy={ids.notifyDescription}
              onChange={(notifyText) => void save("notifyText", { notifyText })}
            />
          </div>
        </Card>
      </>
    );
  }

  return (
    <div ref={cardRef} data-testid="dictation" className="space-y-4">
      <section
        aria-labelledby={ids.title}
        className="overflow-hidden rounded-xl bg-white dark:bg-neutral-800"
      >
        <div className="space-y-4 p-4">{body}</div>
      </section>
      {more}
      {settings && status.data && (
        <section
          aria-label={t("dictation.try.label")}
          className="overflow-hidden rounded-xl bg-white dark:bg-neutral-800"
        >
          <TryDictation
            settings={settings}
            status={status.data}
            ready={catalog.data?.some(
              (model) => model.id === settings.speechModel && usable(model),
            )}
          />
        </section>
      )}
    </div>
  );
}

/** Carte titrée d'une partie de la dictée. */
function Card({
  title,
  titleId,
  children,
}: {
  title: string;
  titleId: string;
  children: ReactNode;
}) {
  return (
    <section
      aria-labelledby={titleId}
      className="overflow-hidden rounded-xl bg-white dark:bg-neutral-800"
    >
      <div className="space-y-4 p-4">
        <h2
          id={titleId}
          className="text-sm/6 font-medium text-zinc-950 dark:text-white"
        >
          {title}
        </h2>
        {children}
      </div>
    </section>
  );
}

/**
 * Raccourci : touche liée, attente du bureau, refus, ou commande à lier
 * soi-même ; sous Windows (touches choisies dans l'app), touche liée ou
 * raison de l'échec, avec la saisie de la combinaison.
 */
function ShortcutLine({ status }: { status: DictationStatus }) {
  const t = useT();
  const { bindShortcut, configureShortcut } = useDictationActions();
  // Qui choisit les touches (le bureau, ou l'app sous Windows) : statut des
  // raccourcis globaux, tenu à jour par ses évènements.
  const shortcuts = useShortcutsStatus();
  const chosenInApp = shortcuts.data?.chooser === "app";
  // Dans l'app : le statut que la saisie vient de recevoir, sans attendre
  // la relecture de celui de la dictée.
  const shortcut =
    chosenInApp && shortcuts.data ? shortcuts.data.dictation : status.shortcut;
  const failure = bindShortcut.isError
    ? errorMessage(bindShortcut.error)
    : configureShortcut.isError
      ? errorMessage(configureShortcut.error)
      : null;
  const setShortcut = (configure: boolean) => (
    <span className="flex flex-wrap items-center gap-3">
      <span className="text-sm/6 text-zinc-950 dark:text-white">
        {t("dictation.shortcut.notSet")}
      </span>
      <button
        type="button"
        className={SMALL_BUTTON}
        disabled={bindShortcut.isPending || configureShortcut.isPending}
        onClick={() =>
          configure ? configureShortcut.mutate() : bindShortcut.mutate()
        }
      >
        {t("dictation.shortcut.set")}
      </button>
    </span>
  );

  let line: ReactNode;
  if (
    chosenInApp &&
    (shortcut.state === "bound" || shortcut.state === "unavailable")
  ) {
    return (
      <div className="mt-2">
        <AppShortcut
          id="dictation"
          name={t("dictation.card.title")}
          status={shortcut}
          busy={bindShortcut.isPending}
          onRetry={() => bindShortcut.mutate()}
        />
        {bindShortcut.isError && (
          <p role="alert" className={ALERT}>
            {errorMessage(bindShortcut.error)}
          </p>
        )}
      </div>
    );
  }
  switch (shortcut.state) {
    case "disabled":
      line = <p className={MUTED}>{t("dictation.shortcut.turnOnFirst")}</p>;
      break;
    case "binding":
      line = (
        <p className={`${MUTED} flex items-center gap-2`}>
          <ArrowPathIcon
            aria-hidden="true"
            className="h-4 w-4 animate-spin text-zinc-500"
          />
          {t("dictation.shortcut.waiting")}
        </p>
      );
      break;
    case "bound":
      line = shortcut.trigger ? (
        <span className="flex flex-wrap items-center gap-3">
          <kbd className={KBD}>{shortcut.trigger}</kbd>
          {status.canConfigureShortcut && (
            <button
              type="button"
              className={SMALL_BUTTON}
              disabled={configureShortcut.isPending}
              onClick={() => configureShortcut.mutate()}
            >
              {t("dictation.shortcut.change")}
            </button>
          )}
        </span>
      ) : (
        // Lié sans touche : la boîte du bureau la fait choisir, si elle existe.
        setShortcut(status.canConfigureShortcut)
      );
      break;
    case "unavailable":
      line = (
        <div className="space-y-2">
          {shortcut.problem === "refused" ? (
            setShortcut(false)
          ) : (
            <span className="flex flex-wrap items-center gap-3">
              <span className="text-sm/6 text-amber-700 dark:text-amber-400">
                {shortcut.reason}
              </span>
              {/* Échec du bureau ou raccourci perdu : un nouvel essai peut
                  réussir ; sans portail, non. */}
              {shortcut.problem !== "noPortal" && (
                <button
                  type="button"
                  className={SMALL_BUTTON}
                  disabled={bindShortcut.isPending}
                  onClick={() => bindShortcut.mutate()}
                >
                  {t("dictation.common.tryAgain")}
                </button>
              )}
            </span>
          )}
          <p className={MUTED}>{t("dictation.shortcut.bindCommand")}</p>
          <CommandBlock command={status.command} />
        </div>
      );
      break;
  }
  return (
    <div className="mt-2">
      {line}
      {failure && (
        <p role="alert" className={ALERT}>
          {failure}
        </p>
      )}
    </div>
  );
}

/** Choix exclusif : boutons radio natifs (flèches du clavier comprises). */
function Choice<T extends string>({
  options,
  value,
  onChange,
  labelledBy,
  describedBy,
}: {
  options: { value: T; label: string }[];
  value: T;
  onChange: (value: T) => void;
  labelledBy: string;
  describedBy: string;
}) {
  const name = useId();
  return (
    <div
      role="radiogroup"
      aria-labelledby={labelledBy}
      aria-describedby={describedBy}
      className="inline-flex rounded-lg bg-neutral-100 p-0.5 dark:bg-neutral-900"
    >
      {options.map((option) => (
        <label key={option.value} className="relative">
          <input
            type="radio"
            name={name}
            value={option.value}
            checked={value === option.value}
            onChange={() => onChange(option.value)}
            className="peer sr-only"
          />
          <span className="block cursor-pointer rounded-md px-3 py-1 text-sm/6 whitespace-nowrap text-neutral-600 peer-checked:bg-white peer-checked:text-neutral-900 peer-checked:shadow-sm peer-focus-visible:ring-2 peer-focus-visible:ring-accent-fg peer-disabled:cursor-not-allowed peer-disabled:opacity-50 dark:text-neutral-300 dark:peer-checked:bg-neutral-700 dark:peer-checked:text-white">
            {option.label}
          </span>
        </label>
      ))}
    </div>
  );
}

function LanguageRow({
  id,
  descriptionId,
  value,
  languages,
  parakeet,
  onChange,
  error,
}: {
  id: string;
  descriptionId: string;
  value: string;
  languages: { code: string; name: string; parakeet: boolean }[];
  /** Le modèle choisi est Parakeet : ses langues seulement. */
  parakeet: boolean;
  onChange: (language: string) => void;
  error?: string;
}) {
  const t = useT();
  const known =
    value === AUTO_LANGUAGE || languages.some((lang) => lang.code === value);
  // Noms dans la langue de l'interface (pas les noms anglais du moteur),
  // triés par ce nom.
  const named = useMemo(() => {
    const collator = new Intl.Collator(t.lang);
    return languages
      .map((language) => ({
        ...language,
        label: languageName(t.lang, language.code),
      }))
      .sort((a, b) => collator.compare(a.label, b.label));
  }, [languages, t]);
  return (
    <Row
      icon={LanguageIcon}
      label={
        <label htmlFor={id}>
          {t("dictation.transcription.language.label")}
        </label>
      }
      description={
        parakeet
          ? t("dictation.transcription.language.descriptionParakeet")
          : t("dictation.transcription.language.description")
      }
      descriptionId={descriptionId}
    >
      <div className="mt-3">
        <select
          id={id}
          aria-describedby={descriptionId}
          className={SELECT}
          value={value}
          onChange={(event) => onChange(event.target.value)}
        >
          <option value={AUTO_LANGUAGE}>
            {t("dictation.transcription.language.auto")}
          </option>
          {!known && (
            <option value={value}>{languageName(t.lang, value)}</option>
          )}
          {named.map((language) => (
            <option
              key={language.code}
              value={language.code}
              disabled={parakeet && !language.parakeet}
            >
              {language.label}
            </option>
          ))}
        </select>
      </div>
      <RowError message={error} />
    </Row>
  );
}

/** Modèle du nettoyage : « Off » ou un modèle installé dans Ollama. */
function PolishModelRow({
  id,
  descriptionId,
  value,
  onChange,
  error,
}: {
  id: string;
  descriptionId: string;
  value: string;
  onChange: (model: string) => void;
  error?: string;
}) {
  const t = useT();
  const models = useModels();
  // Liste lue (même ancienne) ; sinon on ne sait pas ce qui est installé.
  const read = models.data !== undefined;
  // Échec, même pendant les nouveaux essais de useModels.
  const unreachable = !read && (models.isError || models.failureCount > 0);
  // Jamais le cloud : le texte dicté quitterait la machine.
  const names = (models.data ?? [])
    .map((model) => model.model)
    .filter((name) => !isCloudModel(name));
  const listed = value === "" || names.some((name) => sameModel(name, value));
  const unlisted = !read
    ? value
    : isCloudModel(value)
      ? t("dictation.polish.model.cloud", { name: value })
      : t("dictation.common.notInstalled", { name: value });
  return (
    <Row
      icon={SparklesIcon}
      label={<label htmlFor={id}>{t("dictation.polish.model.label")}</label>}
      description={t("dictation.polish.model.description")}
      descriptionId={descriptionId}
    >
      <div className="mt-3 flex flex-wrap items-center gap-x-4 gap-y-2">
        <select
          id={id}
          aria-describedby={descriptionId}
          className={SELECT}
          value={value}
          onChange={(event) => onChange(event.target.value)}
        >
          <option value="">{t("dictation.polish.model.off")}</option>
          {!listed && <option value={value}>{unlisted}</option>}
          {names.map((name) => (
            <option key={name} value={name}>
              {name}
            </option>
          ))}
        </select>
        <Link to="/hub/polish" className={LINK}>
          {t("dictation.polish.model.recommended")}
        </Link>
      </div>
      {unreachable ? (
        <p className="mt-2 text-xs text-amber-700 dark:text-amber-400">
          {t("dictation.polish.model.unreachable")}
        </p>
      ) : (
        !read && (
          <p className="mt-2 text-xs text-zinc-500 dark:text-zinc-400">
            {t("dictation.polish.model.loading")}
          </p>
        )
      )}
      <RowError message={error} />
    </Row>
  );
}

/**
 * Consignes du nettoyage, enregistrées en quittant le champ, et aussi quand
 * la carte disparaît (autre page) ou avant Ctrl+Q sans que le champ soit
 * quitté ; vides ou identiques à celles par défaut, le moteur garde les
 * consignes par défaut. Un brouillon refusé reste dans le champ.
 */
function InstructionsRow({
  id,
  descriptionId,
  saved,
  defaultPrompt,
  onSave,
  error,
}: {
  id: string;
  descriptionId: string;
  /** Consignes enregistrées ; vide = par défaut. */
  saved: string;
  defaultPrompt: string;
  onSave: (prompt: string) => Promise<boolean>;
  error?: string;
}) {
  const t = useT();
  // null : pas de modification en cours, le champ montre l'enregistré.
  const [draft, setDraft] = useState<string | null>(null);
  const shown = saved || defaultPrompt;
  const value = draft ?? shown;
  // Texte tapé pas encore envoyé, lu hors du rendu (démontage, Ctrl+Q).
  const unsavedRef = useRef<string | null>(null);
  const latestRef = useRef({ shown, defaultPrompt, onSave });
  useEffect(() => {
    latestRef.current = { shown, defaultPrompt, onSave };
  });
  /** Envoie le texte tapé s'il diffère de l'enregistré ; faux si refusé. */
  const flush = useCallback(async () => {
    const text = unsavedRef.current;
    if (text === null) return true;
    unsavedRef.current = null;
    const latest = latestRef.current;
    if (text === latest.shown) return true;
    const saved = await latest.onSave(
      text.trim() === latest.defaultPrompt.trim() ? "" : text,
    );
    // Refusé : à renvoyer, sauf si un texte plus récent attend déjà.
    if (!saved) unsavedRef.current ??= text;
    return saved;
  }, []);
  useEffect(() => onBeforeQuit(flush), [flush]);
  // Démontage (autre page) sans quitter le champ : envoyé quand même.
  useEffect(() => () => void flush(), [flush]);
  const commit = async () => {
    const text = unsavedRef.current;
    if (await flush()) {
      setDraft((current) => (current === text ? null : current));
    }
  };
  return (
    <Row
      icon={PencilSquareIcon}
      label={
        <label htmlFor={id}>{t("dictation.polish.instructions.label")}</label>
      }
      description={t("dictation.polish.instructions.description")}
      descriptionId={descriptionId}
    >
      <textarea
        id={id}
        aria-describedby={descriptionId}
        rows={6}
        maxLength={MAX_PROMPT_CHARS}
        spellCheck={false}
        value={value}
        onChange={(event) => {
          setDraft(event.target.value);
          unsavedRef.current = event.target.value;
        }}
        onBlur={() => void commit()}
        className="mt-3 block w-full resize-y rounded-lg border border-neutral-300 bg-white px-3 py-2 font-mono text-[13px]/5 text-neutral-900 outline-none focus-visible:border-accent-fg focus-visible:ring-2 focus-visible:ring-accent-fg/40 dark:border-neutral-700 dark:bg-neutral-800 dark:text-neutral-100"
      />
      <div className="mt-2">
        <button
          type="button"
          className={SMALL_BUTTON}
          disabled={saved === "" && (draft === null || draft === defaultPrompt)}
          onClick={() => {
            unsavedRef.current = null;
            setDraft(null);
            if (saved !== "") void onSave("");
          }}
        >
          {t("dictation.polish.instructions.reset")}
        </button>
      </div>
      <RowError message={error} />
    </Row>
  );
}

/** Dictionnaire : pastilles retirables, champ où Entrée ajoute un mot. */
function DictionaryRow({
  id,
  descriptionId,
  words,
  onSave,
  error,
}: {
  id: string;
  descriptionId: string;
  words: string[];
  onSave: (words: string[]) => Promise<boolean>;
  error?: string;
}) {
  const t = useT();
  const [word, setWord] = useState("");
  const inputRef = useRef<HTMLInputElement>(null);
  // Composition en cours (méthode de saisie), comme ChatForm.
  const composingRef = useRef(false);
  const add = async () => {
    const next = addWord(words, word);
    if (next === null) {
      // Vide, ou déjà là (sa pastille le montre).
      if (word.trim()) setWord("");
      return;
    }
    if (await onSave(next)) setWord("");
  };
  return (
    <Row
      icon={BookOpenIcon}
      label={
        <label htmlFor={id}>
          {t("dictation.transcription.dictionary.label")}
        </label>
      }
      description={t("dictation.transcription.dictionary.description")}
      descriptionId={descriptionId}
    >
      {words.length > 0 && (
        <ul
          aria-label={t("dictation.transcription.dictionary.words")}
          className="mt-3 flex flex-wrap gap-1.5"
        >
          {words.map((entry) => (
            <li
              key={entry}
              className="inline-flex items-center gap-0.5 rounded-md bg-neutral-100 py-0.5 pr-0.5 pl-2 text-sm text-neutral-800 dark:bg-neutral-900 dark:text-neutral-100"
            >
              {entry}
              <button
                type="button"
                aria-label={t("dictation.transcription.dictionary.remove", {
                  word: entry,
                })}
                title={t("common.actions.remove")}
                className="flex h-5 w-5 items-center justify-center rounded text-neutral-500 outline-none hover:bg-neutral-200 hover:text-neutral-900 focus-visible:ring-2 focus-visible:ring-accent-fg dark:hover:bg-neutral-700 dark:hover:text-white"
                onClick={() => {
                  // Le bouton va disparaître : le focus reste dans la ligne.
                  inputRef.current?.focus();
                  void onSave(words.filter((other) => other !== entry));
                }}
              >
                <XMarkIcon aria-hidden="true" className="h-3.5 w-3.5" />
              </button>
            </li>
          ))}
        </ul>
      )}
      <input
        ref={inputRef}
        id={id}
        type="text"
        aria-describedby={descriptionId}
        placeholder={t("dictation.transcription.dictionary.placeholder")}
        autoComplete="off"
        spellCheck={false}
        maxLength={MAX_WORD_CHARS}
        value={word}
        onChange={(event) => setWord(event.target.value)}
        onKeyDown={(event) => {
          if (event.key !== "Enter") return;
          // L'Entrée qui valide une composition n'ajoute rien.
          if (composingRef.current || event.nativeEvent.isComposing) return;
          event.preventDefault();
          void add();
        }}
        onCompositionStart={() => {
          composingRef.current = true;
        }}
        onCompositionEnd={() => {
          // L'Entrée qui termine la composition passe d'abord.
          setTimeout(() => {
            composingRef.current = false;
          }, 10);
        }}
        className="mt-3 h-9 w-full max-w-sm rounded-lg border border-neutral-300 bg-white px-3 text-sm text-neutral-900 outline-none placeholder:text-neutral-400 focus-visible:border-accent-fg focus-visible:ring-2 focus-visible:ring-accent-fg/40 dark:border-neutral-700 dark:bg-neutral-800 dark:text-neutral-100 dark:placeholder:text-neutral-500"
      />
      <RowError message={error} />
    </Row>
  );
}

/**
 * Pied de la carte : essai depuis la page (le même bouton change de rôle
 * selon la phase, le focus y reste), phase annoncée, dernier résultat.
 */
function TryDictation({
  settings,
  status,
  ready,
}: {
  settings: DictationSettings;
  status: DictationStatus;
  /** Le modèle choisi est installé. */
  ready: boolean | undefined;
}) {
  const t = useT();
  const { toggle, cancel } = useDictationActions();
  const phase = status.phase;
  // Dictée de VS Code : ni l'essai, ni Cancel ne l'arrêtent (le bandeau, si).
  const editor = status.editor;
  const busy = phase === "transcribing" || phase === "polishing";
  const hint = editor
    ? t("dictation.try.editor")
    : !settings.enabled
      ? t("dictation.try.turnOnFirst")
      : ready === false
        ? t("dictation.try.chooseModel")
        : null;
  const failure = toggle.isError
    ? errorMessage(toggle.error)
    : cancel.isError
      ? errorMessage(cancel.error)
      : null;
  return (
    <div className="p-4">
      <div className="flex flex-wrap items-center gap-3">
        <button
          type="button"
          className={PRIMARY_BUTTON}
          disabled={
            editor ||
            (phase === "idle" &&
              (hint !== null || ready !== true || toggle.isPending))
          }
          onClick={() => (busy ? cancel.mutate() : toggle.mutate())}
        >
          {phase === "recording" && !editor && (
            <span
              aria-hidden="true"
              className="h-2 w-2 animate-pulse rounded-full bg-red-500"
            />
          )}
          {phase === "idle" || editor
            ? t("dictation.try.label")
            : busy
              ? t("common.actions.cancel")
              : t("dictation.try.stop")}
        </button>
        {phase === "recording" && !editor && (
          <button
            type="button"
            className={SMALL_BUTTON}
            onClick={() => cancel.mutate()}
          >
            {t("common.actions.cancel")}
          </button>
        )}
        {/* Phase annoncée : région vivante tant que la dictée est activée
            (elle seule peut changer de phase), sans rôle status (celui de
            la page est le « Saved » des réglages). */}
        {settings.enabled && (
          <span aria-live="polite" className={MUTED}>
            {phaseText(t, phase)}
          </span>
        )}
        {(phase === "idle" || editor) && hint && (
          <span className={MUTED}>{hint}</span>
        )}
      </div>
      {failure && (
        <p role="alert" className={ALERT}>
          {failure}
        </p>
      )}
      {status.lastResult && (
        <LastResult
          result={status.lastResult}
          polishing={settings.polishModel !== ""}
        />
      )}
    </div>
  );
}

/**
 * Dernière dictée : texte, copié ou non (copie automatique coupée : rien
 * d'inquiétant ; échec de la copie : bien visible), bouton Copy (copie le
 * texte de nouveau, ou pour la première fois), « not polished » si un modèle
 * du nettoyage est réglé et n'a pas servi, et la raison donnée par le
 * moteur ; ou l'échec de la dictée.
 */
function LastResult({
  result,
  polishing,
}: {
  result: DictationResult;
  /** Un modèle du nettoyage est réglé. */
  polishing: boolean;
}) {
  const t = useT();
  const copyError = result.copyFailed;
  const copyFailed = !result.copied && copyError;
  // Copié depuis (Copy) : l'échec de la copie automatique n'a plus cours.
  const error = result.copied && copyError ? null : result.error;
  return (
    <div data-testid="last-dictation" className="mt-4">
      <p className="flex items-center justify-between gap-2 text-xs text-zinc-500 dark:text-zinc-400">
        <span>{t("dictation.lastResult.title")}</span>
        <span>{formatTimeAgo(t, result.at)}</span>
      </p>
      {result.text ? (
        <>
          <p className="mt-1.5 rounded-lg bg-neutral-100 px-3 py-2 text-sm whitespace-pre-wrap text-neutral-900 select-text dark:bg-neutral-900 dark:text-neutral-100">
            {result.text}
          </p>
          <div className="mt-1.5 flex flex-wrap items-center gap-1.5 text-xs">
            <CopyLast />
            {result.copied && (
              <span className="text-green-700 dark:text-green-400">
                {result.pasted
                  ? t("dictation.lastResult.pasted")
                  : t("dictation.lastResult.copied")}
              </span>
            )}
            {copyFailed && (
              <span className="rounded bg-red-100 px-1.5 font-medium text-red-700 dark:bg-red-900/40 dark:text-red-300">
                {t("dictation.lastResult.notCopied")}
              </span>
            )}
            {polishing && !result.polished && (
              <span className="rounded bg-amber-100 px-1.5 font-medium text-amber-800 dark:bg-amber-900/40 dark:text-amber-300">
                {t("dictation.lastResult.notPolished")}
              </span>
            )}
          </div>
          {error && (
            <p
              className={`mt-1 text-xs break-words ${
                copyFailed
                  ? "text-red-600 dark:text-red-400"
                  : "text-amber-700 dark:text-amber-400"
              }`}
            >
              {error}
            </p>
          )}
          {result.pasteError && (
            <p className="mt-1 text-xs break-words text-amber-700 dark:text-amber-400">
              {result.pasteError}
            </p>
          )}
        </>
      ) : (
        <p className="mt-1.5 text-sm break-words text-red-600 dark:text-red-400">
          {result.error ?? t("dictation.lastResult.failed")}
        </p>
      )}
    </div>
  );
}

/**
 * Copie le texte de la dernière dictée (copie automatique coupée, ou
 * presse-papiers écrasé depuis) : « Copied » deux secondes, ou l'erreur.
 */
function CopyLast() {
  const t = useT();
  const copy = useCopyLastDictation();
  const [copied, setCopied] = useState(false);
  useEffect(() => {
    if (!copied) return;
    const timer = setTimeout(() => setCopied(false), 2000);
    return () => clearTimeout(timer);
  }, [copied]);
  return (
    <>
      <button
        type="button"
        className={SMALL_BUTTON}
        disabled={copy.isPending}
        onClick={() => {
          setCopied(false);
          copy.mutate(undefined, { onSuccess: () => setCopied(true) });
        }}
      >
        {copied ? t("common.actions.copied") : t("common.actions.copy")}
      </button>
      {copy.isError && (
        <span role="alert" className="text-red-600 dark:text-red-400">
          {t("dictation.lastResult.copyFailed", {
            error: errorMessage(copy.error),
          })}
        </span>
      )}
    </>
  );
}
