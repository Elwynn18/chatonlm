// Le tuteur du lecteur de cours (onglet Study) parle en bulles : « I don't
// understand » sous une section (explication plus simple, en flux, gardée)
// et « Ask a question » sur un palier (mini-conversation : la question de
// l'apprenant à droite, la réponse du tuteur à gauche, en flux). « Stop » ou
// Échap arrête un flux ; Échap ferme aussi le panneau des questions. Une
// réponse finie est enregistrée par le moteur et revient par la relecture du
// cours ; une réponse arrêtée n'est pas gardée. Une réponse enregistrée se
// lit à voix haute (StudyReadAloud, sous sa bulle), comme une réponse du
// chat ; pas une réponse encore en flux.

import {
  ChatBubbleLeftRightIcon,
  QuestionMarkCircleIcon,
  StopIcon,
  XMarkIcon,
} from "@heroicons/react/20/solid";
import {
  type FormEvent,
  type KeyboardEvent,
  useEffect,
  useId,
  useRef,
  useState,
} from "react";
import type { Course } from "@/bindings/Course";
import type { Level } from "@/bindings/Level";
import { StudyReadAloud } from "@/components/study/StudyReadAloud";
import {
  BUTTON,
  LearnerBubble,
  Markdown,
  PRIMARY_BUTTON,
  QUIET_BUTTON,
  TutorBubble,
} from "@/components/study/ui";
import {
  askKey,
  clearStudyStream,
  explainKey,
  isStreaming,
  startStudyStream,
  stopStudyStream,
  type StudyStream,
  useStudyStream,
} from "@/hooks/useStudy";
import { useT } from "@/i18n";
import { overlayOpen } from "@/lib/overlay";
import { MAX_QUESTION_CHARS, passageKey } from "@/lib/study";

/** Échap pendant un flux : arrêt (sinon `onIdle`, s'il est donné). */
function useEscape(
  stream: StudyStream | null,
  key: string,
  onIdle?: () => void,
) {
  const state = useRef({ stream, onIdle });
  useEffect(() => {
    state.current = { stream, onIdle };
  });
  useEffect(() => {
    const onKeyDown = (event: globalThis.KeyboardEvent) => {
      if (event.key !== "Escape" || event.defaultPrevented || overlayOpen()) {
        return;
      }
      const current = state.current;
      if (current.stream?.phase === "streaming") {
        event.preventDefault();
        stopStudyStream(key);
      } else if (current.onIdle && !isStreaming(current.stream)) {
        event.preventDefault();
        current.onIdle();
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [key]);
}

/** Réponse en cours ou pas encore relue avec le cours (sinon : null). */
function liveStream(stream: StudyStream | null, saved: number) {
  if (!stream) return null;
  if (stream.phase === "done" && saved > stream.savedBefore) return null;
  return stream;
}

function StreamNotes({ stream }: { stream: StudyStream }) {
  const t = useT();
  return (
    <>
      {stream.phase === "stopped" && (
        <p className="mt-1 text-xs text-neutral-500 dark:text-neutral-400">
          {t("study.tutor.stopped")}
        </p>
      )}
      {stream.error && (
        <p role="alert" className="mt-1 text-red-700 dark:text-red-300">
          {stream.error.message}
        </p>
      )}
    </>
  );
}

/** « I don't understand » sous une section, et ses explications gardées. */
export function ExplainBlock({
  course,
  level,
  section,
}: {
  course: Course;
  level: Level;
  section: number;
}) {
  const t = useT();
  const key = explainKey(course.id, level.index, section);
  const stream = useStudyStream(key);
  useEscape(stream, key);
  // Avec leur rang dans le palier (clé du texte lu à voix haute).
  const saved = level.explains
    .map((explain, index) => ({ explain, index }))
    .filter(({ explain }) => explain.section === section);
  const live = liveStream(stream, saved.length);
  const running = isStreaming(stream);
  return (
    <div className="mt-5 flex flex-col gap-3">
      {saved.map(({ explain, index }) => (
        <TutorBubble
          key={index}
          testId="study-explain"
          actions={
            <StudyReadAloud
              courseId={course.id}
              passage={passageKey.explain(level.index, index)}
            />
          }
        >
          <p className="mb-1 text-xs font-medium text-neutral-500 dark:text-neutral-400">
            {t("study.tutor.simpler")}
          </p>
          <Markdown content={explain.text} />
        </TutorBubble>
      ))}
      {live && (live.text || running || live.error) && (
        <TutorBubble testId="study-explain-live" busy={running}>
          <p className="mb-1 text-xs font-medium text-neutral-500 dark:text-neutral-400">
            {t("study.tutor.simpler")}
          </p>
          {live.text ? (
            <Markdown content={live.text} streaming={running} />
          ) : (
            running && <TypingDots />
          )}
          <StreamNotes stream={live} />
        </TutorBubble>
      )}
      <div>
        {running ? (
          <button
            type="button"
            className={BUTTON}
            disabled={stream?.phase === "stopping"}
            aria-keyshortcuts="Escape"
            onClick={() => stopStudyStream(key)}
          >
            <StopIcon aria-hidden="true" className="h-4 w-4" />
            {t("study.tutor.stop")}
          </button>
        ) : (
          <button
            type="button"
            className={QUIET_BUTTON}
            onClick={() =>
              startStudyStream(
                key,
                {
                  kind: "explain",
                  courseId: course.id,
                  level: level.index,
                  section,
                },
                saved.length,
              )
            }
          >
            <QuestionMarkCircleIcon aria-hidden="true" className="h-4 w-4" />
            {t("study.tutor.dontUnderstand")}
          </button>
        )}
      </div>
    </div>
  );
}

/**
 * Trois points qui clignotent : le tuteur écrit (décoratif) ; immobiles
 * sous mouvement réduit (variante motion-safe, comme MessageList).
 */
function TypingDots() {
  const t = useT();
  return (
    <span
      data-testid="study-typing"
      className="inline-flex items-center gap-1 py-1"
    >
      <span className="sr-only">{t("study.tutor.writing")}</span>
      {[0, 1, 2].map((i) => (
        <span
          key={i}
          aria-hidden="true"
          className="h-1.5 w-1.5 rounded-full bg-neutral-400 motion-safe:animate-[typing_1.4s_infinite]"
          style={{ animationDelay: `${i * 0.2}s` }}
        />
      ))}
    </span>
  );
}

/** « Ask a question » : mini-conversation avec le tuteur sur un palier. */
export function AskPanel({ course, level }: { course: Course; level: Level }) {
  const t = useT();
  const fieldId = useId();
  const key = askKey(course.id, level.index);
  const stream = useStudyStream(key);
  const [open, setOpen] = useState(false);
  const [question, setQuestion] = useState("");
  const toggleRef = useRef<HTMLButtonElement>(null);
  const fieldRef = useRef<HTMLTextAreaElement>(null);
  const running = isStreaming(stream);
  // Panneau fermé par l'apprenant : le bouton qui l'ouvre reprend le focus.
  const refocusToggle = useRef(false);
  const close = () => {
    refocusToggle.current = true;
    setOpen(false);
    clearStudyStream(key);
  };
  useEscape(stream, key, open ? close : undefined);
  const live = liveStream(stream, level.asks.length);

  // Panneau ouvert : le champ prend le focus ; fermé : le bouton.
  useEffect(() => {
    if (open) {
      fieldRef.current?.focus();
    } else if (refocusToggle.current) {
      refocusToggle.current = false;
      toggleRef.current?.focus();
    }
  }, [open]);

  const send = () => {
    const text = question.trim();
    if (!text || running) return;
    setQuestion("");
    startStudyStream(
      key,
      { kind: "ask", courseId: course.id, level: level.index, question: text },
      level.asks.length,
    );
  };
  const onSubmit = (event: FormEvent) => {
    event.preventDefault();
    send();
  };
  const onKeyDown = (event: KeyboardEvent<HTMLTextAreaElement>) => {
    if (
      event.key !== "Enter" ||
      event.shiftKey ||
      event.nativeEvent.isComposing
    ) {
      return;
    }
    event.preventDefault();
    send();
  };

  if (!open) {
    return (
      <div className="flex justify-center">
        <button
          ref={toggleRef}
          type="button"
          className={QUIET_BUTTON}
          aria-expanded="false"
          onClick={() => setOpen(true)}
        >
          <ChatBubbleLeftRightIcon aria-hidden="true" className="h-4 w-4" />
          {level.asks.length > 0
            ? t("study.ask.openWithCount", { count: level.asks.length })
            : t("study.ask.open")}
        </button>
      </div>
    );
  }

  return (
    <section
      aria-label={t("study.ask.title")}
      data-testid="study-ask"
      className="rounded-2xl border border-neutral-200 bg-white p-4 motion-safe:animate-study-reveal dark:border-neutral-700/70 dark:bg-neutral-800/60"
    >
      <div className="mb-3 flex items-center justify-between">
        <h3 className="flex items-center gap-2 text-sm font-semibold text-neutral-900 dark:text-white">
          <ChatBubbleLeftRightIcon aria-hidden="true" className="h-4 w-4" />
          {t("study.ask.title")}
        </h3>
        <button
          type="button"
          className="rounded-full p-1 text-neutral-500 outline-none hover:bg-neutral-100 hover:text-neutral-900 focus-visible:ring-2 focus-visible:ring-accent-fg dark:hover:bg-neutral-700 dark:hover:text-white"
          aria-label={t("study.ask.close")}
          title={t("study.ask.close")}
          onClick={close}
        >
          <XMarkIcon aria-hidden="true" className="h-4 w-4" />
        </button>
      </div>
      <div className="flex flex-col gap-3" aria-live="polite">
        {level.asks.length === 0 && !live && (
          <p className="text-sm text-neutral-500 dark:text-neutral-400">
            {t("study.ask.empty")}
          </p>
        )}
        {level.asks.map((ask, index) => (
          <div key={index} className="flex flex-col gap-2">
            <LearnerBubble>{ask.question}</LearnerBubble>
            <TutorBubble
              testId="study-ask-answer"
              actions={
                <StudyReadAloud
                  courseId={course.id}
                  passage={passageKey.ask(level.index, index)}
                />
              }
            >
              <Markdown content={ask.answer} />
            </TutorBubble>
          </div>
        ))}
        {live && (
          <div className="flex flex-col gap-2">
            {live.question && <LearnerBubble>{live.question}</LearnerBubble>}
            <TutorBubble testId="study-ask-live" busy={running}>
              {live.text ? (
                <Markdown content={live.text} streaming={running} />
              ) : (
                running && <TypingDots />
              )}
              <StreamNotes stream={live} />
            </TutorBubble>
          </div>
        )}
      </div>
      <form onSubmit={onSubmit} className="mt-3 flex items-end gap-2">
        <label htmlFor={fieldId} className="sr-only">
          {t("study.ask.label")}
        </label>
        <textarea
          ref={fieldRef}
          id={fieldId}
          rows={1}
          value={question}
          maxLength={MAX_QUESTION_CHARS}
          placeholder={t("study.ask.placeholder")}
          onChange={(event) => setQuestion(event.target.value)}
          onKeyDown={onKeyDown}
          className="min-h-9 flex-1 resize-none rounded-xl border border-neutral-300 bg-white px-3 py-1.5 text-sm text-neutral-900 outline-none placeholder:text-neutral-400 focus-visible:border-accent-fg focus-visible:ring-2 focus-visible:ring-accent-fg/40 dark:border-neutral-700 dark:bg-neutral-900 dark:text-neutral-100"
        />
        {running ? (
          <button
            type="button"
            className={BUTTON}
            disabled={stream?.phase === "stopping"}
            aria-keyshortcuts="Escape"
            onClick={() => stopStudyStream(key)}
          >
            <StopIcon aria-hidden="true" className="h-4 w-4" />
            {t("study.tutor.stop")}
          </button>
        ) : (
          <button
            type="submit"
            className={PRIMARY_BUTTON}
            disabled={!question.trim()}
          >
            {t("study.ask.send")}
          </button>
        )}
      </form>
    </section>
  );
}
