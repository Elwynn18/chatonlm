// Parcours des paliers du lecteur de cours (onglet Study) : une bulle ronde
// par palier, reliées par un trait vertical. Palier réussi : vert, coche qui
// « pop » ; à revoir : ambre ; en cours : numéro, halo qui respire ;
// verrouillé : grisé, cadenas. Étoiles sous un palier fini ; petit
// indicateur pendant l'écriture d'un palier ou d'un nouvel essai demandé.
// Le bilan final est le dernier nœud (trophée). Un clic sur un palier ouvert
// montre sa première carte. L'état est dit en texte (nom accessible), jamais
// par la seule couleur.

import {
  ArrowPathIcon,
  BookOpenIcon,
  CheckIcon,
  LockClosedIcon,
} from "@heroicons/react/20/solid";
import { TrophyIcon } from "@heroicons/react/24/outline";
import type { Course } from "@/bindings/Course";
import type { Level } from "@/bindings/Level";
import { Stars } from "@/components/study/ui";
import { useT } from "@/i18n";
import {
  allLevelsDone,
  finalKey,
  levelDone,
  levelStars,
  levelStartKey,
} from "@/lib/study";

export function LevelPath({
  course,
  viewedLevel,
  viewingFinal,
  go,
}: {
  course: Course;
  /** Palier de la carte montrée (null : intro, bilan, fin). */
  viewedLevel: number | null;
  viewingFinal: boolean;
  go: (key: string) => void;
}) {
  const t = useT();
  const finalOpen = allLevelsDone(course);
  const finished = course.status === "finished";
  return (
    <nav
      aria-label={t("study.path.label")}
      data-testid="study-path"
      className="w-56 flex-none overflow-y-auto border-r border-neutral-100 px-4 py-6 dark:border-neutral-800"
    >
      <ol className="flex flex-col">
        {course.levels.map((level) => (
          <PathNode
            key={level.index}
            course={course}
            level={level}
            viewed={viewedLevel === level.index}
            go={go}
          />
        ))}
        <li className="relative flex items-start gap-3">
          <button
            type="button"
            data-testid="study-path-final"
            disabled={!finalOpen}
            aria-current={viewingFinal ? "step" : undefined}
            aria-label={t("study.path.finalNode", {
              status: finished
                ? t("study.path.status.done")
                : finalOpen
                  ? t("study.path.status.open")
                  : t("study.path.status.locked"),
            })}
            onClick={() => go(finished ? "end" : finalKey(0))}
            className={`relative flex h-9 w-9 flex-none items-center justify-center rounded-full border-2 outline-none focus-visible:ring-2 focus-visible:ring-accent-fg focus-visible:ring-offset-2 dark:focus-visible:ring-offset-neutral-900 ${
              finished
                ? "border-amber-400 bg-amber-100 text-amber-600 dark:bg-amber-900/40 dark:text-amber-300"
                : finalOpen
                  ? "border-neutral-900 bg-white text-neutral-900 motion-safe:animate-study-pulse dark:border-white dark:bg-neutral-900 dark:text-white"
                  : "border-neutral-200 bg-neutral-100 text-neutral-500 dark:border-neutral-700 dark:bg-neutral-800"
            } ${viewingFinal ? "ring-2 ring-neutral-400 ring-offset-2 dark:ring-offset-neutral-900" : ""}`}
          >
            <TrophyIcon aria-hidden="true" className="h-5 w-5" />
          </button>
          <span
            aria-hidden="true"
            className={`pt-2 text-sm ${finalOpen ? "font-medium text-neutral-900 dark:text-white" : "text-neutral-500"}`}
          >
            {t("study.final.title")}
          </span>
        </li>
      </ol>
    </nav>
  );
}

function PathNode({
  course,
  level,
  viewed,
  go,
}: {
  course: Course;
  level: Level;
  viewed: boolean;
  go: (key: string) => void;
}) {
  const t = useT();
  const title = course.plan?.levels[level.index]?.title ?? "";
  const stars = levelStars(course, level.index);
  const locked = level.status === "locked";
  // Palier en cours d'écriture, ou nouvel essai demandé (en file, puis en
  // cours d'écriture : le moteur le met d'abord à « pending ») ; rien ne
  // s'écrit pour un cours en pause.
  const writing =
    !course.paused &&
    (level.state === "generating" ||
      level.remedy === "pending" ||
      level.remedy === "generating");
  const done = levelDone(level);
  // Au milieu du nom lu : en minuscules (les pastilles ont leur majuscule).
  const status = t(`study.path.status.${level.status}`);
  const tone =
    level.status === "passed"
      ? "border-emerald-500 bg-emerald-500 text-white"
      : level.status === "review"
        ? "border-amber-400 bg-amber-100 text-amber-700 dark:bg-amber-900/40 dark:text-amber-300"
        : level.status === "current"
          ? "border-neutral-900 bg-neutral-900 text-white motion-safe:animate-study-pulse dark:border-white dark:bg-white dark:text-neutral-900"
          : "border-neutral-200 bg-neutral-100 text-neutral-500 dark:border-neutral-700 dark:bg-neutral-800";
  return (
    <li className="relative flex items-start gap-3 pb-6">
      {/* Trait vers le nœud suivant (vert une fois le palier fini). */}
      <span
        aria-hidden="true"
        className={`absolute top-9 bottom-0 left-[17px] w-0.5 ${done ? "bg-emerald-400 dark:bg-emerald-600" : "bg-neutral-200 dark:bg-neutral-700"}`}
      />
      <button
        type="button"
        data-testid="study-path-node"
        data-status={level.status}
        disabled={locked}
        aria-current={viewed ? "step" : undefined}
        aria-label={
          stars !== null
            ? t("study.path.nodeWithStars", {
                number: level.index + 1,
                title,
                status,
                stars: t("study.stars", { count: stars }),
              })
            : t("study.path.node", { number: level.index + 1, title, status })
        }
        onClick={() => go(levelStartKey(level.index))}
        className={`relative flex h-9 w-9 flex-none items-center justify-center rounded-full border-2 text-sm font-semibold outline-none focus-visible:ring-2 focus-visible:ring-accent-fg focus-visible:ring-offset-2 dark:focus-visible:ring-offset-neutral-900 ${tone} ${viewed ? "ring-2 ring-neutral-400 ring-offset-2 dark:ring-offset-neutral-900" : ""}`}
      >
        {level.status === "passed" ? (
          <CheckIcon
            aria-hidden="true"
            className="h-5 w-5 motion-safe:animate-study-pop"
          />
        ) : level.status === "review" ? (
          <BookOpenIcon aria-hidden="true" className="h-4 w-4" />
        ) : locked ? (
          <LockClosedIcon aria-hidden="true" className="h-4 w-4" />
        ) : (
          <span aria-hidden="true">{level.index + 1}</span>
        )}
        {writing && (
          <span
            aria-hidden="true"
            className="absolute -right-1 -bottom-1 flex h-4 w-4 items-center justify-center rounded-full bg-white shadow dark:bg-neutral-800"
          >
            <ArrowPathIcon className="h-3 w-3 text-neutral-500 motion-safe:animate-spin" />
          </span>
        )}
      </button>
      <span aria-hidden="true" className="min-w-0 pt-1">
        <span
          className={`line-clamp-2 text-sm ${
            locked
              ? "text-neutral-500"
              : viewed
                ? "font-semibold text-neutral-900 dark:text-white"
                : "text-neutral-700 dark:text-neutral-200"
          }`}
        >
          {title}
        </span>
        {stars !== null && (
          <span className="mt-0.5 block">
            <Stars count={stars} />
          </span>
        )}
        {writing && (
          <span className="mt-0.5 block text-[11px] text-neutral-500">
            {t("study.path.writing")}
          </span>
        )}
      </span>
    </li>
  );
}
