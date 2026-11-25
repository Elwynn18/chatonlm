// Nouveau projet de l'Atelier (propre à ChatonLM) : grande zone de la tâche,
// pièces jointes (documents texte, par le sélecteur natif ou un dépôt sur la
// fenêtre, comme les documents de Study), curseur d'autonomie (Maj+Tab dans
// la tâche le fait passer au niveau suivant, comme dans Claude Code ; sous
// WebKitGTK Maj+Tab donne la touche « Unidentified », d'où `isTabKey` ;
// Échap puis Maj+Tab quitte la zone comme d'habitude, une fois),
// modèle (Auto, ou un modèle installé, vérifié par `workshop_model_check` :
// modèle retenu, avertissements, refus), stratégie (Auto, ou une de celles
// que le moteur sait mener ; les autres « bientôt »), nom du sous-dossier
// (celui que prendrait le moteur, modifiable). « Start the project » crée le
// projet (pièces jointes reprises par le moteur) et ouvre sa vue.

import {
  ArrowLeftIcon,
  ArrowRightIcon,
  DocumentArrowUpIcon,
  ExclamationTriangleIcon,
  PaperClipIcon,
  XMarkIcon,
} from "@heroicons/react/20/solid";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Link, useNavigate } from "@tanstack/react-router";
import {
  type FormEvent,
  type KeyboardEvent,
  useId,
  useRef,
  useState,
} from "react";
import { createPortal } from "react-dom";
import type { Autonomy } from "@/bindings/Autonomy";
import type { Strategy } from "@/bindings/Strategy";
import { PRIMARY_BUTTON, QUIET_BUTTON } from "@/components/study/ui";
import {
  useWorkshopSettings,
  workshopListKey,
} from "@/components/workshop/queries";
import { AutonomyPicker } from "@/components/workshop/AutonomyPicker";
import { release, useComposerFiles } from "@/hooks/useComposerFiles";
import { useFileDrop } from "@/hooks/useFileDrop";
import { useModels } from "@/hooks/useModels";
import { useT } from "@/i18n";
import {
  errorMessage,
  pickFiles,
  workshopCreate,
  workshopModelCheck,
} from "@/lib/ipc";
import { shortModelName } from "@/lib/modelNames";
import { isTabKey } from "@/lib/shortcuts";
import {
  MAX_ATTACHMENTS,
  MAX_FOLDER_CHARS,
  MAX_TASK_CHARS,
  modelWarningKey,
  nextAutonomy,
  STRATEGIES,
  strategyAvailable,
} from "@/lib/workshop";

const FIELD =
  "rounded-lg border border-neutral-300 bg-white px-2 text-sm text-neutral-900 outline-none focus-visible:border-accent-fg focus-visible:ring-2 focus-visible:ring-accent-fg/40 disabled:opacity-50 dark:border-neutral-700 dark:bg-neutral-800 dark:text-neutral-100";
const SELECT = `${FIELD} h-8 max-w-64 min-w-0 pr-7`;
const LABEL = "text-sm font-medium text-neutral-700 dark:text-neutral-300";

/** Refus de `workshop_model_check` traduits ; les autres : texte commun. */
const MODEL_ERRORS = [
  "noModel",
  "modelNotFound",
  "remoteOff",
  "contextTooSmall",
] as const;

export function NewProject() {
  const t = useT();
  const navigate = useNavigate();
  const client = useQueryClient();
  const ids = {
    task: useId(),
    model: useId(),
    strategy: useId(),
    folder: useId(),
    help: useId(),
    shortcut: useId(),
  };
  // Échap vient d'être tapé dans la tâche : le Maj+Tab suivant garde son
  // rôle habituel (sortir de la zone), une fois.
  const escaped = useRef(false);
  const settings = useWorkshopSettings();
  const [task, setTask] = useState("");
  // Choix de l'utilisateur ; null : l'autonomie des réglages.
  const [chosenAutonomy, setAutonomy] = useState<Autonomy | null>(null);
  const autonomy = chosenAutonomy ?? settings.data?.autonomy ?? "ask";
  const [model, setModel] = useState("");
  const [strategy, setStrategy] = useState<Strategy | "">("");
  // Nom donné par l'utilisateur ; vide : le projet se nomme lui-même au
  // cadrage (jamais d'après le texte de la tâche).
  const [folder, setFolder] = useState("");
  const [creating, setCreating] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const files = useComposerFiles();
  const dragging = useFileDrop((paths) => files.addPaths(paths));
  const check = useQuery({
    queryKey: ["workshop", "modelCheck", model],
    queryFn: () => workshopModelCheck(model),
    retry: false,
  });

  const documents = files.files.filter((file) => file.kind !== "image");
  const images = files.files.filter((file) => file.kind === "image");
  const tooMany = documents.length > MAX_ATTACHMENTS;
  const modelRefused = check.data !== undefined && check.data.error !== null;
  const disabled =
    task.trim() === "" ||
    tooMany ||
    files.busy ||
    creating ||
    modelRefused ||
    !check.data;

  const create = () => {
    if (disabled) return;
    setCreating(true);
    setError(null);
    const name = folder.trim();
    workshopCreate({
      task: task.trim(),
      attachmentIds: documents.map((file) => file.id),
      autonomy,
      ...(strategy ? { strategy } : {}),
      ...(model ? { model } : {}),
      ...(name ? { folderName: name } : {}),
    }).then(
      (id) => {
        // Documents repris par le moteur : ne pas les libérer au départ de
        // la page ; les images refusées, qui ne sont pas parties, si.
        release(
          files
            .take()
            .filter((file) => file.kind === "image")
            .map((file) => file.id),
        );
        void client.invalidateQueries({ queryKey: workshopListKey });
        void navigate({
          to: "/workshop/$projectId",
          params: { projectId: String(id) },
        });
      },
      (cause: unknown) => {
        setCreating(false);
        setError(errorMessage(cause));
      },
    );
  };

  const onSubmit = (event: FormEvent) => {
    event.preventDefault();
    create();
  };

  // Maj+Tab dans la tâche : niveau d'autonomie suivant (le focus reste) ;
  // après Échap, il sort de la zone (une fois).
  const onTaskKeyDown = (event: KeyboardEvent<HTMLTextAreaElement>) => {
    if (event.key === "Escape") {
      escaped.current = true;
      return;
    }
    // Maj seule (enfoncée avant Tab) ne compte pas comme une autre touche.
    if (event.key === "Shift") return;
    const wasEscaped = escaped.current;
    escaped.current = false;
    if (!event.shiftKey || !isTabKey(event.nativeEvent)) return;
    if (event.ctrlKey || event.altKey || event.metaKey) return;
    if (wasEscaped) return;
    event.preventDefault();
    setAutonomy(nextAutonomy(autonomy));
  };

  const attach = () => {
    pickFiles().then(files.addPaths, (cause: unknown) =>
      setError(errorMessage(cause)),
    );
  };

  return (
    <main
      data-testid="workshop-new-project"
      className="flex min-h-0 w-full flex-1 flex-col overflow-y-auto dark:bg-neutral-900"
    >
      <div className="mx-auto flex w-full max-w-3xl flex-col gap-4 p-6 pt-2">
        <Link
          to="/workshop"
          className={`${QUIET_BUTTON} self-start`}
          data-testid="workshop-back"
        >
          <ArrowLeftIcon aria-hidden="true" className="h-4 w-4" />
          {t("workshop.new.back")}
        </Link>
        <form
          onSubmit={onSubmit}
          aria-label={t("workshop.new.title")}
          data-testid="workshop-form"
          className="flex flex-col gap-5 rounded-3xl border border-neutral-200 bg-white p-5 shadow-sm dark:border-neutral-700/70 dark:bg-neutral-800"
        >
          <div>
            <label
              htmlFor={ids.task}
              className="mb-2 block font-rounded text-lg font-medium text-neutral-900 dark:text-white"
            >
              {t("workshop.new.task")}
            </label>
            <textarea
              id={ids.task}
              rows={7}
              value={task}
              maxLength={MAX_TASK_CHARS}
              onChange={(event) => setTask(event.target.value)}
              onKeyDown={onTaskKeyDown}
              onBlur={() => {
                escaped.current = false;
              }}
              placeholder={t("workshop.new.taskPlaceholder")}
              aria-describedby={`${ids.help} ${ids.shortcut}`}
              spellCheck
              data-testid="workshop-task"
              className="w-full resize-y rounded-xl border border-neutral-200 bg-neutral-50 px-3 py-2 text-base text-neutral-900 outline-none placeholder:text-neutral-400 focus-visible:border-accent-fg focus-visible:ring-2 focus-visible:ring-accent-fg/40 dark:border-neutral-700 dark:bg-neutral-900 dark:text-neutral-100 dark:placeholder:text-neutral-500"
            />
            {task.length > MAX_TASK_CHARS - 200 && (
              <p className="mt-1 text-right text-xs text-neutral-500 tabular-nums dark:text-neutral-400">
                {t("workshop.new.characters", {
                  count: task.length,
                  max: MAX_TASK_CHARS,
                })}
              </p>
            )}
            <Attachments files={files} />
            {tooMany && (
              <p
                role="alert"
                className="mt-2 text-sm text-red-600 dark:text-red-400"
              >
                {t("workshop.new.tooMany", { count: MAX_ATTACHMENTS })}
              </p>
            )}
            {images.length > 0 && (
              <ul className="mt-2 space-y-1">
                {images.map((file) => (
                  <li
                    key={file.id}
                    role="alert"
                    className="flex items-center gap-2 text-sm text-red-600 dark:text-red-400"
                  >
                    <span className="min-w-0 truncate">
                      {t("workshop.new.imageRefused", { name: file.filename })}
                    </span>
                    <button
                      type="button"
                      className="rounded-full p-0.5 outline-none hover:bg-red-50 focus-visible:ring-2 focus-visible:ring-accent-fg dark:hover:bg-red-950/40"
                      aria-label={t("workshop.new.removeAttachment", {
                        name: file.filename,
                      })}
                      title={t("workshop.new.removeAttachment", {
                        name: file.filename,
                      })}
                      onClick={() => files.remove(file.id)}
                    >
                      <XMarkIcon aria-hidden="true" className="h-4 w-4" />
                    </button>
                  </li>
                ))}
              </ul>
            )}
            <div className="mt-3 flex flex-wrap items-center gap-2">
              <button
                type="button"
                className={QUIET_BUTTON}
                onClick={attach}
                disabled={documents.length >= MAX_ATTACHMENTS}
                data-testid="workshop-attach"
              >
                <PaperClipIcon aria-hidden="true" className="h-4 w-4" />
                {t("workshop.new.attach")}
              </button>
              <span className="text-xs text-neutral-500 dark:text-neutral-400">
                {t("workshop.new.attachHint")}
              </span>
            </div>
          </div>

          <AutonomyPicker
            value={autonomy}
            onChange={setAutonomy}
            helpId={ids.help}
            shortcutId={ids.shortcut}
          />

          <div className="flex flex-col gap-1.5">
            <div className="flex flex-wrap items-center gap-2">
              <label htmlFor={ids.model} className={LABEL}>
                {t("workshop.new.model")}
              </label>
              <ModelSelect id={ids.model} value={model} onChange={setModel} />
            </div>
            <ModelCheckLine
              model={model}
              checking={check.isPending}
              data={check.data}
              error={check.isError ? errorMessage(check.error) : null}
            />
          </div>

          <div className="flex flex-wrap items-center gap-2">
            <label htmlFor={ids.strategy} className={LABEL}>
              {t("workshop.new.strategy")}
            </label>
            <select
              id={ids.strategy}
              className={SELECT}
              value={strategy}
              data-testid="workshop-strategy-select"
              onChange={(event) =>
                setStrategy(event.target.value as Strategy | "")
              }
            >
              <option value="">{t("workshop.new.strategyAuto")}</option>
              {STRATEGIES.map((value) =>
                strategyAvailable(value) ? (
                  <option key={value} value={value}>
                    {t(`workshop.strategies.${value}`)}
                  </option>
                ) : (
                  <option key={value} value={value} disabled>
                    {t("workshop.new.soon", {
                      strategy: t(`workshop.strategies.${value}`),
                    })}
                  </option>
                ),
              )}
            </select>
          </div>

          <div className="flex flex-col gap-1.5">
            <label htmlFor={ids.folder} className={LABEL}>
              {t("workshop.new.folder")}
            </label>
            <input
              id={ids.folder}
              type="text"
              value={folder}
              maxLength={MAX_FOLDER_CHARS}
              placeholder={t("workshop.new.folderPlaceholder")}
              onChange={(event) => setFolder(event.target.value)}
              data-testid="workshop-folder"
              className={`${FIELD} h-8 w-full max-w-md`}
            />
            {settings.data && settings.data.dir !== "" && (
              <p className="text-xs break-all text-neutral-500 dark:text-neutral-400">
                {t("workshop.new.folderIn", { dir: settings.data.dir })}
              </p>
            )}
          </div>

          <div className="flex justify-end">
            <button
              type="submit"
              className={PRIMARY_BUTTON}
              disabled={disabled}
              data-testid="workshop-create"
            >
              {creating ? t("workshop.new.creating") : t("workshop.new.create")}
              <ArrowRightIcon aria-hidden="true" className="h-4 w-4" />
            </button>
          </div>
          {error && (
            <p
              role="alert"
              data-testid="workshop-create-error"
              className="rounded-lg bg-red-50 px-3 py-2 text-sm break-words text-red-700 dark:bg-red-950/40 dark:text-red-300"
            >
              {error}
            </p>
          )}
        </form>
      </div>
      {dragging && <DropVeil />}
    </main>
  );
}

/** Modèle du projet : Auto, puis les modèles installés (noms courts). */
function ModelSelect({
  id,
  value,
  onChange,
}: {
  id: string;
  value: string;
  onChange: (model: string) => void;
}) {
  const t = useT();
  const models = useModels();
  const names = (models.data ?? []).map((model) => model.id);
  return (
    <select
      id={id}
      className={SELECT}
      value={value}
      data-testid="workshop-model"
      onChange={(event) => onChange(event.target.value)}
    >
      <option value="">{t("workshop.new.modelAuto")}</option>
      {value !== "" && !names.includes(value) && (
        <option value={value}>{shortModelName(value)}</option>
      )}
      {names.map((name) => (
        <option key={name} value={name}>
          {shortModelName(name)}
        </option>
      ))}
    </select>
  );
}

/** Modèle retenu, avertissements montrés avant le lancement, ou refus. */
function ModelCheckLine({
  model,
  checking,
  data,
  error,
}: {
  model: string;
  checking: boolean;
  data: Awaited<ReturnType<typeof workshopModelCheck>> | undefined;
  error: string | null;
}) {
  const t = useT();
  if (error !== null) {
    return (
      <p role="alert" className="text-sm text-red-600 dark:text-red-400">
        {t("workshop.new.modelCheckError", { message: error })}
      </p>
    );
  }
  if (checking || !data) {
    return (
      <p className="text-sm text-neutral-500 dark:text-neutral-400">
        {t("workshop.new.modelChecking")}
      </p>
    );
  }
  if (data.error !== null || data.model === null) {
    const code = data.error ?? "";
    return (
      <div className="flex flex-wrap items-center gap-x-3 gap-y-1">
        <p
          role="alert"
          data-testid="workshop-model-error"
          data-code={code}
          className="text-sm text-red-600 dark:text-red-400"
        >
          {(MODEL_ERRORS as readonly string[]).includes(code)
            ? t(
                `workshop.new.modelErrors.${code as (typeof MODEL_ERRORS)[number]}`,
              )
            : t("workshop.new.modelErrors.unknown")}
        </p>
        {code === "noModel" && (
          <Link
            to="/hub"
            data-testid="workshop-open-hub"
            className="text-sm font-medium text-accent-fg underline underline-offset-2"
          >
            {t("workshop.new.openHub")}
          </Link>
        )}
      </div>
    );
  }
  const name = shortModelName(data.model);
  const warnings = data.warnings.flatMap((code) => {
    const key = modelWarningKey(code);
    return key ? [{ code, text: t(key) }] : [];
  });
  return (
    <div data-testid="workshop-model-check" data-model={data.model}>
      <p className="text-sm text-neutral-600 dark:text-neutral-300">
        {model === "" && data.auto
          ? t("workshop.new.modelAutoChosen", { model: name })
          : t("workshop.new.modelChosen", { model: name })}
      </p>
      {warnings.length > 0 && (
        <ul className="mt-1 flex flex-col gap-1">
          {warnings.map((warning) => (
            <li
              key={warning.code}
              data-testid="workshop-model-warning"
              data-code={warning.code}
              className="flex items-start gap-1.5 text-sm text-accent-fg"
            >
              <ExclamationTriangleIcon
                aria-hidden="true"
                className="mt-0.5 h-4 w-4 flex-none"
              />
              {warning.text}
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}

/** Documents joints (nom, retirer), en lecture, refusés. */
function Attachments({
  files,
}: {
  files: ReturnType<typeof useComposerFiles>;
}) {
  const t = useT();
  const documents = files.files.filter((file) => file.kind !== "image");
  if (
    documents.length === 0 &&
    files.reading.length === 0 &&
    files.errors.length === 0
  ) {
    return null;
  }
  return (
    <ul
      aria-label={t("workshop.new.attachments")}
      className="mt-3 flex flex-wrap gap-2"
    >
      {documents.map((file) => (
        <li
          key={file.id}
          data-testid="workshop-attachment"
          className="flex max-w-64 items-center gap-1.5 rounded-full border border-neutral-200 bg-neutral-50 py-1 pr-1 pl-3 text-sm text-neutral-800 dark:border-neutral-700 dark:bg-neutral-900 dark:text-neutral-100"
        >
          <span className="min-w-0 truncate">{file.filename}</span>
          <button
            type="button"
            className="rounded-full p-0.5 text-neutral-500 outline-none hover:bg-neutral-200 hover:text-neutral-900 focus-visible:ring-2 focus-visible:ring-accent-fg dark:text-neutral-400 dark:hover:bg-neutral-700 dark:hover:text-white"
            aria-label={t("workshop.new.removeAttachment", {
              name: file.filename,
            })}
            title={t("workshop.new.removeAttachment", { name: file.filename })}
            onClick={() => files.remove(file.id)}
          >
            <XMarkIcon aria-hidden="true" className="h-4 w-4" />
          </button>
        </li>
      ))}
      {files.reading.map((file) => (
        <li
          key={file.key}
          aria-busy="true"
          className="max-w-64 truncate rounded-full border border-dashed border-neutral-300 px-3 py-1 text-sm text-neutral-500 dark:border-neutral-600 dark:text-neutral-400"
        >
          {file.filename}
        </li>
      ))}
      {files.errors.map((failure, index) => (
        <li
          key={`${failure.filename}-${index}`}
          role="alert"
          className="flex max-w-80 items-center gap-1.5 rounded-full border border-red-200 bg-red-50 py-1 pr-1 pl-3 text-sm text-red-700 dark:border-red-900 dark:bg-red-950/40 dark:text-red-300"
        >
          <span className="min-w-0 truncate">
            {t("workshop.new.fileError", {
              name: failure.filename,
              error: failure.error ?? t("workshop.new.readError"),
            })}
          </span>
          <button
            type="button"
            className="rounded-full p-0.5 outline-none hover:bg-red-100 focus-visible:ring-2 focus-visible:ring-accent-fg dark:hover:bg-red-900"
            aria-label={t("workshop.new.removeAttachment", {
              name: failure.filename,
            })}
            title={t("workshop.new.removeAttachment", {
              name: failure.filename,
            })}
            onClick={() => files.removeError(index)}
          >
            <XMarkIcon aria-hidden="true" className="h-4 w-4" />
          </button>
        </li>
      ))}
    </ul>
  );
}

/** Voile du dépôt de documents sur la fenêtre. */
function DropVeil() {
  const t = useT();
  return createPortal(
    <div
      data-testid="workshop-drop"
      className="pointer-events-none fixed inset-0 z-[9999] flex items-center justify-center bg-neutral-50 dark:bg-neutral-900"
    >
      <div className="mx-4 max-w-sm rounded-2xl border border-neutral-200 bg-white p-10 text-center shadow-2xl dark:border-neutral-700 dark:bg-neutral-800">
        <DocumentArrowUpIcon
          aria-hidden="true"
          className="mx-auto mb-2 h-8 w-8 text-neutral-900 dark:text-white"
        />
        <p className="text-sm font-medium text-neutral-600 dark:text-neutral-300">
          {t("workshop.new.drop")}
        </p>
      </div>
    </div>,
    document.body,
  );
}
