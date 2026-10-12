// Onglet Study : classes et petits éléments communs aux pages (boutons,
// Markdown des contenus générés, avatar et bulles du tuteur, étoiles,
// confettis).

import { AcademicCapIcon, StarIcon } from "@heroicons/react/24/solid";
import { StarIcon as StarOutlineIcon } from "@heroicons/react/24/outline";
import type { ReactNode } from "react";
import { StreamingMarkdownContent } from "@/components/StreamingMarkdownContent";
import { useReducedMotion } from "@/hooks/useReducedMotion";
import { useT } from "@/i18n";

export const BUTTON =
  "inline-flex flex-none items-center justify-center gap-1.5 rounded-full border border-neutral-300 bg-white px-4 py-1.5 text-sm font-medium text-neutral-800 outline-none hover:bg-neutral-50 focus-visible:ring-2 focus-visible:ring-accent-fg disabled:cursor-default disabled:opacity-50 dark:border-neutral-600 dark:bg-neutral-800 dark:text-neutral-100 dark:hover:bg-neutral-700";
export const PRIMARY_BUTTON =
  "inline-flex flex-none items-center justify-center gap-1.5 rounded-full bg-neutral-900 px-4 py-1.5 text-sm font-medium text-white outline-none hover:bg-neutral-700 focus-visible:ring-2 focus-visible:ring-accent-fg focus-visible:ring-offset-2 disabled:cursor-default disabled:opacity-50 dark:bg-white dark:text-neutral-900 dark:hover:bg-neutral-200 dark:focus-visible:ring-offset-neutral-900";
export const QUIET_BUTTON =
  "inline-flex flex-none items-center gap-1.5 rounded-full px-3 py-1.5 text-sm text-neutral-600 outline-none hover:bg-neutral-100 hover:text-neutral-900 focus-visible:ring-2 focus-visible:ring-accent-fg disabled:cursor-default disabled:opacity-50 dark:text-neutral-300 dark:hover:bg-neutral-800 dark:hover:text-white";
export const LINK_BUTTON =
  "rounded-md font-medium underline underline-offset-2 outline-none hover:text-red-800 focus-visible:ring-2 focus-visible:ring-accent-fg dark:hover:text-red-200";
export const CARD =
  "rounded-2xl border border-neutral-200 bg-white p-6 dark:border-neutral-700/70 dark:bg-neutral-800/60";
/** Largeur de lecture des pages du cours. */
export const READING = "mx-auto w-full max-w-[42rem]";
export const EYEBROW =
  "text-xs font-medium tracking-wide text-neutral-500 uppercase dark:text-neutral-400";

/**
 * Texte généré (Markdown, formules, code), taille de lecture. Sans la marge
 * haute du premier bloc ni la marge basse du dernier (structure :
 * `.prose > div > p…`) : les cartes et les bulles ont leur propre
 * espacement, sinon un vide apparaît sous le dernier paragraphe.
 */
export function Markdown({
  content,
  streaming = false,
}: {
  content: string;
  streaming?: boolean;
}) {
  return (
    <div
      data-study-markdown=""
      className="[&_.prose>div>:first-child]:mt-0 [&_.prose>div>:last-child]:mb-0"
    >
      <StreamingMarkdownContent content={content} isStreaming={streaming} />
    </div>
  );
}

/**
 * Markdown d'une ligne (option, élément d'une liste, moitié d'une phrase à
 * trou, énoncé en titre) rendu dans le fil du texte : sans les marges des
 * paragraphes ; `size` : taille du texte autour (petit par défaut).
 */
export function InlineMarkdown({
  content,
  size = "sm",
}: {
  content: string;
  size?: "sm" | "base" | "lg";
}) {
  return (
    <span className="inline [&_.prose]:inline [&_.prose]:max-w-none [&_div]:inline [&_p]:m-0 [&_p]:inline">
      <StreamingMarkdownContent
        content={content}
        size={size === "base" ? undefined : size}
      />
    </span>
  );
}

/** Avatar du tuteur (décoratif). */
export function TutorAvatar() {
  return (
    <span
      aria-hidden="true"
      className="flex h-8 w-8 flex-none items-center justify-center rounded-full bg-neutral-900 text-white shadow-sm dark:bg-white dark:text-neutral-900"
    >
      <AcademicCapIcon className="h-4.5 w-4.5" />
    </span>
  );
}

/**
 * Bulle du tuteur, à gauche, avec son avatar ; `label` : nom lu par les
 * lecteurs d'écran (« Tutor »).
 */
export function TutorBubble({
  children,
  testId,
  busy = false,
}: {
  children: ReactNode;
  testId?: string;
  busy?: boolean;
}) {
  const t = useT();
  return (
    <div
      data-testid={testId}
      data-speaker="tutor"
      className="flex items-start gap-2.5 motion-safe:animate-study-reveal"
    >
      <TutorAvatar />
      <div
        role="group"
        aria-label={t("study.tutor.label")}
        aria-busy={busy || undefined}
        className="min-w-0 flex-1 rounded-2xl rounded-tl-md bg-neutral-100 px-4 py-2.5 text-sm dark:bg-neutral-700/60"
      >
        {children}
      </div>
    </div>
  );
}

/** Bulle de l'apprenant, à droite (question posée). */
export function LearnerBubble({ children }: { children: ReactNode }) {
  const t = useT();
  return (
    <div data-speaker="learner" className="flex justify-end">
      <div
        role="group"
        aria-label={t("study.tutor.you")}
        className="max-w-[85%] rounded-2xl rounded-tr-md bg-neutral-200 px-4 py-2 text-sm break-words whitespace-pre-wrap text-neutral-900 dark:bg-neutral-600 dark:text-white"
      >
        {children}
      </div>
    </div>
  );
}

/**
 * Étoiles d'un palier (0 à 3), lues « 2 stars out of 3 » ; `animate` : elles
 * apparaissent une à une (célébration).
 */
export function Stars({
  count,
  size = "sm",
  animate = false,
}: {
  count: number;
  size?: "sm" | "lg";
  animate?: boolean;
}) {
  const t = useT();
  const icon = size === "lg" ? "h-8 w-8" : "h-3.5 w-3.5";
  return (
    <span
      role="img"
      aria-label={t("study.stars", { count })}
      data-stars={count}
      className="inline-flex items-center gap-0.5"
    >
      {[0, 1, 2].map((i) =>
        i < count ? (
          <StarIcon
            key={i}
            aria-hidden="true"
            className={`${icon} text-amber-400 ${animate ? "motion-safe:animate-study-pop" : ""}`}
            style={
              animate ? { animationDelay: `${200 + i * 250}ms` } : undefined
            }
          />
        ) : (
          <StarOutlineIcon
            key={i}
            aria-hidden="true"
            className={`${icon} text-neutral-300 dark:text-neutral-600`}
          />
        ),
      )}
    </span>
  );
}

// Teintes chaudes de l'app (roux, ambre, sable), plus le vert et le rose.
const CONFETTI_COLORS = [
  "bg-amber-400",
  "bg-emerald-400",
  "bg-accent",
  "bg-rose-400",
  "bg-neutral-300",
];

/**
 * Confettis en CSS (décoratifs, une seule chute) ; rien sous mouvement
 * réduit. Positions et délais fixes : pas de hasard pendant le rendu.
 */
export function Confetti({ pieces = 18 }: { pieces?: number }) {
  const reduced = useReducedMotion();
  if (reduced) return null;
  return (
    <span
      aria-hidden="true"
      data-testid="study-confetti"
      className="pointer-events-none absolute inset-x-0 top-0 h-0 overflow-visible"
    >
      {Array.from({ length: pieces }, (_, i) => (
        <span
          key={i}
          className={`absolute top-0 h-2.5 w-1.5 rounded-sm opacity-0 motion-safe:animate-study-confetti ${CONFETTI_COLORS[i % CONFETTI_COLORS.length]}`}
          style={{
            left: `${(i * 37 + 11) % 100}%`,
            animationDelay: `${(i * 97) % 600}ms`,
          }}
        />
      ))}
    </span>
  );
}
