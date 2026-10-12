// Accueil de l'onglet Study (propre à ChatonLM) : en tête, le modèle des
// cours (réglage `studyModel`, vide = celui du chat) et un avertissement
// discret pour un petit modèle ; puis le formulaire d'un nouveau cours
// (sujet, niveau, durée, documents, recherche web) et les cours, en cours
// puis terminés (jamais dans l'historique de la barre latérale).
//
// Documents : sélecteur natif puis zone de préparation, comme les pièces
// jointes du chat (useComposerFiles) ; une image ou plus de trois documents
// sont signalés et ne partent pas. La création reprend les fichiers préparés
// (le moteur les retire de la zone) puis ouvre la page du cours.

import {
  ArrowRightIcon,
  ExclamationTriangleIcon,
  PaperClipIcon,
  TrashIcon,
  XMarkIcon,
} from "@heroicons/react/20/solid";
import { useQueryClient } from "@tanstack/react-query";
import { Link, useNavigate } from "@tanstack/react-router";
import {
  type FormEvent,
  type KeyboardEvent,
  useId,
  useRef,
  useState,
} from "react";
import type { CourseSummary } from "@/bindings/CourseSummary";
import type { StudyLevel } from "@/bindings/StudyLevel";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { ModelSettingSelect } from "@/components/ModelSettingSelect";
import { PRIMARY_BUTTON, QUIET_BUTTON } from "@/components/study/ui";
import { WebSearchButton } from "@/components/WebSearchButton";
import { useCloudStatus } from "@/hooks/useCloudStatus";
import { release, useComposerFiles } from "@/hooks/useComposerFiles";
import { useModels } from "@/hooks/useModels";
import { useSettings } from "@/hooks/useSettings";
import { courseKey, studyListKey, useCourses } from "@/hooks/useStudy";
import { useT } from "@/i18n";
import { formatTimeAgo } from "@/i18n/format";
import { errorMessage, pickFiles, studyCreate, studyDelete } from "@/lib/ipc";
import { sameModel } from "@/lib/modelNames";
import {
  DEFAULT_MINUTES,
  isSmallModel,
  MAX_DOCUMENTS,
  MAX_TOPIC_CHARS,
  STUDY_LEVELS,
  STUDY_MINUTES,
  type StudyMinutes,
  type StudySearch,
} from "@/lib/study";

export function StudyHome({ search }: { search: StudySearch }) {
  const t = useT();
  const modelId = useId();
  return (
    <main
      data-testid="study-home"
      className="flex min-h-0 w-full flex-1 flex-col overflow-y-auto dark:bg-neutral-900"
    >
      <div className="mx-auto flex w-full max-w-3xl flex-col gap-6 p-6 pt-2">
        <div className="flex flex-wrap items-start justify-between gap-x-6 gap-y-2">
          <p className="min-w-0 flex-1 text-sm/6 text-zinc-500 select-none dark:text-zinc-400">
            {t("study.home.description")}
          </p>
          <div className="flex flex-col items-end gap-1">
            <StudyModelSelect id={modelId} />
            <SmallModelNote />
          </div>
        </div>
        <NewCourseForm search={search} />
        <CourseLists />
      </div>
    </main>
  );
}

/** Modèle des cours (réglage `studyModel`). */
export function StudyModelSelect({ id }: { id: string }) {
  const t = useT();
  return (
    <ModelSettingSelect
      id={id}
      setting="studyModel"
      texts={{
        label: t("study.model.label"),
        sameAsChat: (model) => t("study.model.sameAsChat", { model }),
        sameAsChatNone: t("study.model.sameAsChatNone"),
        notInstalled: (name) => t("study.model.notInstalled", { name }),
        saveError: (message) => t("study.model.saveError", { message }),
      }}
    />
  );
}

/** Modèle des cours effectif : le sien, sinon celui du chat ("" : aucun). */
function useStudyModel(): { model: string; known: boolean } {
  const { settings } = useSettings();
  return {
    model: settings?.studyModel || settings?.selectedModel || "",
    known: settings !== undefined,
  };
}

/** Avertissement discret : petit modèle, cours moins sûrs. */
function SmallModelNote() {
  const t = useT();
  const { model } = useStudyModel();
  const models = useModels();
  const info = (models.data ?? []).find((m) => sameModel(m.model, model));
  if (!isSmallModel(info)) return null;
  return (
    <p
      data-testid="study-small-model"
      className="flex max-w-xs items-start gap-1.5 text-right text-xs text-amber-700 dark:text-amber-400"
    >
      <ExclamationTriangleIcon
        aria-hidden="true"
        className="mt-0.5 h-3.5 w-3.5 flex-none"
      />
      {t("study.model.small")}
    </p>
  );
}

function NewCourseForm({ search }: { search: StudySearch }) {
  const t = useT();
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const ids = { topic: useId(), level: useId(), minutes: useId() };
  const [topic, setTopic] = useState(search.topic ?? "");
  const [level, setLevel] = useState<StudyLevel>(search.level ?? "beginner");
  const [minutes, setMinutes] = useState<StudyMinutes>(
    search.minutes ?? DEFAULT_MINUTES,
  );
  const [web, setWeb] = useState(false);
  const [creating, setCreating] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const files = useComposerFiles();
  const { model, known } = useStudyModel();
  const { cloudDisabled } = useCloudStatus();
  const topicRef = useRef<HTMLTextAreaElement>(null);

  const documents = files.files.filter((file) => file.kind !== "image");
  const images = files.files.filter((file) => file.kind === "image");
  const tooMany = documents.length > MAX_DOCUMENTS;
  const noModel = known && model === "";
  const empty = topic.trim() === "" && documents.length === 0;
  const disabled = empty || noModel || tooMany || files.busy || creating;

  const create = () => {
    if (disabled) return;
    setCreating(true);
    setError(null);
    studyCreate({
      topic: topic.trim(),
      level,
      minutes,
      web: web && !cloudDisabled,
      documentIds: documents.map((file) => file.id),
    }).then(
      (courseId) => {
        // Documents repris par le moteur : ne pas les libérer au départ de
        // la page ; les images refusées, qui ne sont pas parties, si.
        release(
          files
            .take()
            .filter((file) => file.kind === "image")
            .map((file) => file.id),
        );
        void queryClient.invalidateQueries({ queryKey: studyListKey });
        void navigate({ to: "/study/$courseId", params: { courseId } });
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

  const onTopicKeyDown = (event: KeyboardEvent<HTMLTextAreaElement>) => {
    if (
      event.key !== "Enter" ||
      event.shiftKey ||
      event.nativeEvent.isComposing
    ) {
      return;
    }
    event.preventDefault();
    create();
  };

  const addDocuments = () => {
    pickFiles().then(files.addPaths, (cause: unknown) =>
      setError(errorMessage(cause)),
    );
  };

  return (
    <form
      onSubmit={onSubmit}
      aria-label={t("study.form.label")}
      data-testid="study-form"
      className="rounded-3xl border border-neutral-200 bg-white p-5 shadow-sm dark:border-neutral-700/70 dark:bg-neutral-800"
    >
      <label
        htmlFor={ids.topic}
        className="mb-2 block font-rounded text-lg font-medium text-neutral-900 dark:text-white"
      >
        {t("study.form.topic")}
      </label>
      <textarea
        ref={topicRef}
        id={ids.topic}
        rows={2}
        value={topic}
        maxLength={MAX_TOPIC_CHARS}
        onChange={(event) => setTopic(event.target.value)}
        onKeyDown={onTopicKeyDown}
        placeholder={t("study.form.topicPlaceholder")}
        spellCheck
        className="w-full resize-none rounded-xl border border-neutral-200 bg-neutral-50 px-3 py-2 text-base text-neutral-900 outline-none placeholder:text-neutral-400 focus-visible:border-accent-fg focus-visible:ring-2 focus-visible:ring-accent-fg/40 dark:border-neutral-700 dark:bg-neutral-900 dark:text-neutral-100 dark:placeholder:text-neutral-500"
      />
      {topic.length > MAX_TOPIC_CHARS - 50 && (
        <p className="mt-1 text-right text-xs text-neutral-500 tabular-nums dark:text-neutral-400">
          {t("study.form.characters", {
            count: topic.length,
            max: MAX_TOPIC_CHARS,
          })}
        </p>
      )}

      <div className="mt-4 flex flex-wrap gap-x-8 gap-y-4">
        <Segmented
          id={ids.level}
          legend={t("study.form.level")}
          value={level}
          options={STUDY_LEVELS.map((value) => ({
            value,
            label: t(`study.levels.${value}`),
          }))}
          onChange={setLevel}
        />
        <Segmented
          id={ids.minutes}
          legend={t("study.form.duration")}
          value={minutes}
          options={STUDY_MINUTES.map((value) => ({
            value,
            label: t("study.form.minutes", { count: value }),
          }))}
          onChange={setMinutes}
        />
      </div>

      <DocumentChips files={files} />
      {tooMany && (
        <p role="alert" className="mt-2 text-sm text-red-600 dark:text-red-400">
          {t("study.form.tooManyDocuments", { count: MAX_DOCUMENTS })}
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
                {t("study.form.imageRefused", { name: file.filename })}
              </span>
              <button
                type="button"
                className="rounded-full p-0.5 outline-none hover:bg-red-50 focus-visible:ring-2 focus-visible:ring-accent-fg dark:hover:bg-red-950/40"
                aria-label={t("chat.files.remove", { name: file.filename })}
                title={t("chat.files.remove", { name: file.filename })}
                onClick={() => files.remove(file.id)}
              >
                <XMarkIcon aria-hidden="true" className="h-4 w-4" />
              </button>
            </li>
          ))}
        </ul>
      )}

      <div className="mt-4 flex flex-wrap items-center justify-between gap-3">
        <div className="flex items-center gap-2">
          <button
            type="button"
            className={QUIET_BUTTON}
            onClick={addDocuments}
            disabled={documents.length >= MAX_DOCUMENTS}
          >
            <PaperClipIcon aria-hidden="true" className="h-4 w-4" />
            {t("study.form.addDocument")}
          </button>
          <WebSearchButton
            isVisible={!cloudDisabled}
            isActive={web}
            onToggle={() => setWeb((on) => !on)}
          />
          {web && !cloudDisabled && (
            <span className="text-xs text-neutral-500 dark:text-neutral-400">
              {t("study.form.webOn")}
            </span>
          )}
        </div>
        <button
          type="submit"
          className={PRIMARY_BUTTON}
          disabled={disabled}
          data-testid="study-create"
        >
          {creating ? t("study.form.creating") : t("study.form.create")}
          <ArrowRightIcon aria-hidden="true" className="h-4 w-4" />
        </button>
      </div>
      {noModel && (
        <p className="mt-3 text-sm text-neutral-500 dark:text-neutral-400">
          {t("study.form.noModel")}
        </p>
      )}
      {error && (
        <p
          role="alert"
          className="mt-3 rounded-lg bg-red-50 px-3 py-2 text-sm break-words text-red-700 dark:bg-red-950/40 dark:text-red-300"
        >
          {error}
        </p>
      )}
    </form>
  );
}

/** Choix exclusif en pastilles (boutons radio natifs, flèches du clavier). */
function Segmented<T extends string | number>({
  id,
  legend,
  value,
  options,
  onChange,
}: {
  id: string;
  legend: string;
  value: T;
  options: { value: T; label: string }[];
  onChange: (value: T) => void;
}) {
  return (
    <fieldset className="min-w-0">
      <legend className="mb-1.5 text-sm font-medium text-neutral-700 dark:text-neutral-300">
        {legend}
      </legend>
      <div className="inline-flex flex-wrap gap-1 rounded-full bg-neutral-100 p-1 dark:bg-neutral-900">
        {options.map((option) => (
          <label
            key={String(option.value)}
            className={`cursor-pointer rounded-full px-3 py-1 text-sm transition-colors has-[:focus-visible]:ring-2 has-[:focus-visible]:ring-accent-fg ${
              option.value === value
                ? "bg-white font-medium text-neutral-900 shadow-sm dark:bg-neutral-700 dark:text-white"
                : "text-neutral-600 hover:text-neutral-900 dark:text-neutral-400 dark:hover:text-white"
            }`}
          >
            <input
              type="radio"
              name={id}
              className="sr-only"
              checked={option.value === value}
              onChange={() => onChange(option.value)}
            />
            {option.label}
          </label>
        ))}
      </div>
    </fieldset>
  );
}

/** Documents préparés (nom, retirer), en lecture, refusés. */
function DocumentChips({
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
      aria-label={t("study.form.documents")}
      className="mt-4 flex flex-wrap gap-2"
    >
      {documents.map((file) => (
        <li
          key={file.id}
          data-testid="study-document"
          className="flex max-w-64 items-center gap-1.5 rounded-full border border-neutral-200 bg-neutral-50 py-1 pr-1 pl-3 text-sm text-neutral-800 dark:border-neutral-700 dark:bg-neutral-900 dark:text-neutral-100"
        >
          <span className="min-w-0 truncate">{file.filename}</span>
          <button
            type="button"
            className="rounded-full p-0.5 text-neutral-500 outline-none hover:bg-neutral-200 hover:text-neutral-900 focus-visible:ring-2 focus-visible:ring-accent-fg dark:hover:bg-neutral-700 dark:hover:text-white"
            aria-label={t("chat.files.remove", { name: file.filename })}
            title={t("chat.files.remove", { name: file.filename })}
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
          className="max-w-64 truncate rounded-full border border-dashed border-neutral-300 px-3 py-1 text-sm text-neutral-500 motion-safe:animate-pulse dark:border-neutral-600"
        >
          {file.filename}
        </li>
      ))}
      {files.errors.map((error, index) => (
        <li
          key={`${error.filename}-${index}`}
          role="alert"
          className="flex max-w-80 items-center gap-1.5 rounded-full border border-red-200 bg-red-50 py-1 pr-1 pl-3 text-sm text-red-700 dark:border-red-900 dark:bg-red-950/40 dark:text-red-300"
        >
          <span className="min-w-0 truncate">
            {t("study.form.fileError", {
              name: error.filename,
              error: error.error ?? t("chat.files.readError"),
            })}
          </span>
          <button
            type="button"
            className="rounded-full p-0.5 outline-none hover:bg-red-100 focus-visible:ring-2 focus-visible:ring-accent-fg dark:hover:bg-red-900"
            aria-label={t("chat.files.remove", { name: error.filename })}
            title={t("chat.files.remove", { name: error.filename })}
            onClick={() => files.removeError(index)}
          >
            <XMarkIcon aria-hidden="true" className="h-4 w-4" />
          </button>
        </li>
      ))}
    </ul>
  );
}

/** Cours en cours, puis terminés ; invitation quand il n'y en a aucun. */
function CourseLists() {
  const t = useT();
  const courses = useCourses();
  const [deleting, setDeleting] = useState<CourseSummary | null>(null);
  const [deleteError, setDeleteError] = useState<string | null>(null);
  const queryClient = useQueryClient();
  if (courses.isPending) return null;
  if (courses.isError) {
    return (
      <p role="alert" className="text-sm text-red-600 dark:text-red-400">
        {t("study.home.listError", { message: errorMessage(courses.error) })}
      </p>
    );
  }
  const all = courses.data;
  const inProgress = all.filter((course) => course.status !== "finished");
  const completed = all.filter((course) => course.status === "finished");
  const confirmDelete = () => {
    const course = deleting;
    setDeleting(null);
    if (!course) return;
    setDeleteError(null);
    studyDelete(course.id).then(
      () => {
        queryClient.removeQueries({ queryKey: courseKey(course.id) });
        void queryClient.invalidateQueries({ queryKey: studyListKey });
      },
      (cause: unknown) => setDeleteError(errorMessage(cause)),
    );
  };
  return (
    <>
      {all.length === 0 ? (
        <p
          data-testid="study-empty"
          className="px-1 text-center text-sm text-neutral-500 dark:text-neutral-400"
        >
          {t("study.home.empty")}
        </p>
      ) : (
        <>
          {inProgress.length > 0 && (
            <CourseList
              title={t("study.home.inProgress")}
              testId="study-in-progress"
              courses={inProgress}
              onDelete={setDeleting}
            />
          )}
          {completed.length > 0 && (
            <CourseList
              title={t("study.home.completed")}
              testId="study-completed"
              courses={completed}
              onDelete={setDeleting}
            />
          )}
        </>
      )}
      {deleteError && (
        <p role="alert" className="text-sm text-red-600 dark:text-red-400">
          {deleteError}
        </p>
      )}
      {deleting && (
        <ConfirmDialog
          title={t("study.delete.title", { title: deleting.title })}
          message={t("study.delete.message")}
          confirmLabel={t("common.actions.delete")}
          onConfirm={confirmDelete}
          onCancel={() => setDeleting(null)}
        />
      )}
    </>
  );
}

function CourseList({
  title,
  testId,
  courses,
  onDelete,
}: {
  title: string;
  testId: string;
  courses: CourseSummary[];
  onDelete: (course: CourseSummary) => void;
}) {
  const headingId = useId();
  return (
    <section aria-labelledby={headingId} data-testid={testId}>
      <h2
        id={headingId}
        className="mb-2 px-1 font-rounded text-sm font-medium text-neutral-500 dark:text-neutral-400"
      >
        {title}
      </h2>
      <ul className="flex flex-col gap-1.5">
        {courses.map((course) => (
          <CourseRow key={course.id} course={course} onDelete={onDelete} />
        ))}
      </ul>
    </section>
  );
}

function CourseRow({
  course,
  onDelete,
}: {
  course: CourseSummary;
  onDelete: (course: CourseSummary) => void;
}) {
  const t = useT();
  const total = course.levelsTotal;
  const percent = total > 0 ? Math.round((course.levelsDone / total) * 100) : 0;
  const finished = course.status === "finished";
  const status =
    course.status === "planning"
      ? t("study.status.planning")
      : course.status === "unclear"
        ? t("study.status.unclear")
        : course.status === "planned"
          ? t("study.status.planned")
          : course.status === "failed"
            ? t("study.status.failed")
            : t("study.status.levels", {
                count: total,
                done: course.levelsDone,
              });
  return (
    <li
      data-testid="study-course"
      className="group relative rounded-2xl border border-neutral-200 bg-white hover:border-neutral-300 dark:border-neutral-700/70 dark:bg-neutral-800/60 dark:hover:border-neutral-600"
    >
      <Link
        to="/study/$courseId"
        params={{ courseId: course.id }}
        draggable={false}
        className="flex flex-col gap-1.5 rounded-2xl px-4 py-3 pr-12 outline-none focus-visible:ring-2 focus-visible:ring-accent-fg"
      >
        <span className="flex items-baseline justify-between gap-3">
          <span className="min-w-0 truncate font-medium text-neutral-900 dark:text-white">
            {course.title}
          </span>
          <span className="flex-none text-xs text-neutral-500">
            {formatTimeAgo(t, course.updatedAt)}
          </span>
        </span>
        <span className="flex flex-wrap items-center gap-x-3 gap-y-1 text-xs text-neutral-500 dark:text-neutral-400">
          <span>
            {t("study.home.meta", {
              level: t(`study.levels.${course.level}`),
              minutes: t("study.form.minutes", { count: course.minutes }),
            })}
          </span>
          {finished && course.finalCorrect !== null && course.finalTotal ? (
            <span className="font-medium text-emerald-700 dark:text-emerald-400">
              {t("study.home.score", {
                correct: course.finalCorrect,
                total: course.finalTotal,
              })}
            </span>
          ) : (
            <span>{status}</span>
          )}
        </span>
        {total > 0 && (
          <span
            role="progressbar"
            aria-label={t("study.home.progress")}
            aria-valuemin={0}
            aria-valuemax={100}
            aria-valuenow={percent}
            aria-valuetext={t("study.status.levels", {
              count: total,
              done: course.levelsDone,
            })}
            className="block h-1 overflow-hidden rounded-full bg-neutral-200 dark:bg-neutral-700"
          >
            <span
              className={`block h-full rounded-full ${finished ? "bg-emerald-500" : "bg-accent"}`}
              style={{ width: `${percent}%` }}
            />
          </span>
        )}
      </Link>
      <button
        type="button"
        className="absolute top-2.5 right-2.5 rounded-full p-1.5 text-neutral-500 opacity-0 outline-none group-focus-within:opacity-100 group-hover:opacity-100 hover:bg-neutral-100 hover:text-neutral-900 focus-visible:opacity-100 focus-visible:ring-2 focus-visible:ring-accent-fg dark:text-neutral-400 dark:hover:bg-neutral-700 dark:hover:text-white"
        aria-label={t("study.delete.label", { title: course.title })}
        title={t("study.delete.label", { title: course.title })}
        onClick={() => onDelete(course)}
      >
        <TrashIcon aria-hidden="true" className="h-4 w-4" />
      </button>
    </li>
  );
}
