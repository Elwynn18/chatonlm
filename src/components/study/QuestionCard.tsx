// Question du lecteur de cours (onglet Study) : entraînement, validation
// d'un palier ou bilan. Saisie selon le type : choix et vrai/faux (boutons
// radio, chiffres 1 à 9), trou (champ dans la phrase), ordre (flèches haut
// et bas), association (une liste par élément de gauche). « Check » (ou
// Entrée) envoie la réponse au moteur, qui la corrige ; le retour s'affiche
// aussitôt, annoncé (aria-live) : juste ou faux par une icône et un texte
// (encouragement varié, choisi par l'identifiant de la question), réponse
// attendue, explication ; éléments justes et faux marqués. Bonne réponse :
// petit rebond ; mauvaise : légère secousse (mouvement réduit : rien). Une
// question déjà répondue se montre en lecture seule avec son retour.

import {
  ArrowDownIcon,
  ArrowUpIcon,
  CheckCircleIcon,
  CheckIcon,
  XCircleIcon,
  XMarkIcon,
} from "@heroicons/react/20/solid";
import { useQueryClient } from "@tanstack/react-query";
import {
  type KeyboardEvent,
  type ReactNode,
  useEffect,
  useId,
  useRef,
  useState,
} from "react";
import type { AnswerValue } from "@/bindings/AnswerValue";
import type { Answered } from "@/bindings/Answered";
import type { Course } from "@/bindings/Course";
import type { Question } from "@/bindings/Question";
import { CardShell, KeyHint } from "@/components/study/cardKit";
import {
  InlineMarkdown,
  Markdown,
  PRIMARY_BUTTON,
} from "@/components/study/ui";
import { expectedText } from "@/components/study/texts";
import { isTextField, useCardKeys } from "@/hooks/useCardKeys";
import { storeAnswer } from "@/hooks/useStudy";
import { useT } from "@/i18n";
import { Trans } from "@/i18n/Trans";
import { errorMessage, studyAnswer } from "@/lib/ipc";
import {
  answerOf,
  plainInline,
  questionPrompt,
  splitBlank,
  variantOf,
} from "@/lib/study";

/** Encouragements d'une bonne réponse (variante par question). */
const PRAISE = [
  "great",
  "exactly",
  "wellDone",
  "spotOn",
  "nice",
  "perfect",
] as const;
/** Mots d'une mauvaise réponse (variante par question). */
const NUDGE = ["notQuite", "almost", "goodTry"] as const;

/** Réponse en cours de saisie, avant « Check ». */
function initialDraft(question: Question): AnswerValue | null {
  const body = question.body;
  switch (body.type) {
    case "choice":
    case "trueFalse":
      return null;
    case "blank":
      return { type: "blank", text: "" };
    case "order":
      return { type: "order", order: body.items.map((_, i) => i) };
    case "match":
      return { type: "match", pairs: body.left.map(() => -1) };
  }
}

/** La réponse saisie peut-elle être envoyée ? */
function ready(draft: AnswerValue | null): draft is AnswerValue {
  if (!draft) return false;
  switch (draft.type) {
    case "blank":
      return draft.text.trim() !== "";
    case "match":
      return draft.pairs.every((pair) => pair >= 0);
    default:
      return true;
  }
}

/** Nombre d'options choisies au chiffre (choix, vrai/faux). */
function optionCount(question: Question): number {
  const body = question.body;
  if (body.type === "choice") return body.options.length;
  if (body.type === "trueFalse") return 2;
  return 0;
}

export function QuestionCard({
  course,
  question,
  eyebrow,
  onContinue,
  continueLabel,
}: {
  course: Course;
  question: Question;
  eyebrow: string;
  /** Carte suivante ; null : pas de suite (fin d'un essai manqué...). */
  onContinue: (() => void) | null;
  continueLabel?: string;
}) {
  const t = useT();
  const queryClient = useQueryClient();
  const promptId = useId();
  const answered = answerOf(course, question);
  const [draft, setDraft] = useState<AnswerValue | null>(() =>
    initialDraft(question),
  );
  const [checking, setChecking] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // Répondue pendant que la carte est montrée : animation du retour.
  const [fresh, setFresh] = useState(false);
  const continueRef = useRef<HTMLButtonElement>(null);
  const value = answered?.given ?? draft;

  const check = () => {
    if (answered || checking || !ready(draft)) return;
    setChecking(true);
    setError(null);
    studyAnswer(course.id, question.id, draft).then(
      (result) => {
        setChecking(false);
        setFresh(true);
        storeAnswer(queryClient, course.id, question.id, result);
      },
      (cause: unknown) => {
        setChecking(false);
        setError(errorMessage(cause));
      },
    );
  };

  // Réponse corrigée : « Continue » prend le focus (Entrée continue), sauf
  // depuis un champ de saisie (trou, question au tuteur).
  const corrected = fresh && answered !== null;
  useEffect(() => {
    if (!corrected || isTextField(document.activeElement)) return;
    continueRef.current?.focus();
  }, [corrected]);

  useCardKeys({
    onEnter: () => {
      if (answered) onContinue?.();
      else check();
    },
    onDigit: (digit) => {
      if (answered || digit > optionCount(question)) return;
      const body = question.body;
      if (body.type === "choice") {
        setDraft({ type: "choice", index: digit - 1 });
      } else if (body.type === "trueFalse") {
        setDraft({ type: "trueFalse", value: digit === 1 });
      }
    },
  });

  const body = question.body;
  const prompt = questionPrompt(question);
  const blankParts = body.type === "blank" ? splitBlank(body.prompt) : null;
  const motion = fresh
    ? answered?.correct
      ? "motion-safe:animate-study-bounce"
      : "motion-safe:animate-study-shake"
    : "";
  const digits = optionCount(question);

  return (
    <CardShell
      testId="study-question"
      eyebrow={eyebrow}
      title={
        body.type === "blank" && blankParts ? (
          <span id={promptId} className="sr-only">
            {t("study.question.blankTitle")}
          </span>
        ) : (
          <span id={promptId}>
            <InlineMarkdown content={prompt} size="lg" />
          </span>
        )
      }
      actions={
        answered ? (
          onContinue && (
            <button
              ref={continueRef}
              type="button"
              className={PRIMARY_BUTTON}
              aria-keyshortcuts="Enter"
              onClick={onContinue}
            >
              {continueLabel ?? t("study.card.continue")}
            </button>
          )
        ) : (
          <button
            type="button"
            className={PRIMARY_BUTTON}
            aria-keyshortcuts="Enter"
            disabled={!ready(draft) || checking}
            onClick={check}
          >
            {t("study.question.check")}
          </button>
        )
      }
      hint={
        answered ? (
          onContinue && <KeyHint k="study.keys.continue" />
        ) : digits > 0 ? (
          <KeyHint k="study.keys.choose" params={{ count: digits }} />
        ) : (
          <KeyHint k="study.keys.check" />
        )
      }
    >
      <div data-question-input="" className={motion}>
        {(body.type === "choice" || body.type === "trueFalse") && (
          <Options
            labelledBy={promptId}
            options={
              body.type === "choice"
                ? body.options.map((option) => (
                    <InlineMarkdown content={option} />
                  ))
                : [t("study.question.true"), t("study.question.false")]
            }
            selected={selectedIndex(value)}
            answered={answered}
            onSelect={(index) =>
              setDraft(
                body.type === "choice"
                  ? { type: "choice", index }
                  : { type: "trueFalse", value: index === 0 },
              )
            }
          />
        )}
        {body.type === "blank" && (
          <Blank
            parts={blankParts}
            text={value?.type === "blank" ? value.text : ""}
            answered={answered}
            onChange={(text) => setDraft({ type: "blank", text })}
            onEnter={() => (answered ? onContinue?.() : check())}
          />
        )}
        {body.type === "order" && value?.type === "order" && (
          <Order
            items={body.items}
            order={value.order}
            answered={answered}
            onChange={(order) => setDraft({ type: "order", order })}
          />
        )}
        {body.type === "match" && value?.type === "match" && (
          <Match
            left={body.left}
            right={body.right}
            pairs={value.pairs}
            answered={answered}
            onChange={(pairs) => setDraft({ type: "match", pairs })}
          />
        )}
      </div>
      {error && (
        <p
          role="alert"
          className="mt-3 rounded-lg bg-red-50 px-3 py-2 text-sm break-words text-red-700 dark:bg-red-950/40 dark:text-red-300"
        >
          {error}
        </p>
      )}
      <div aria-live="polite" data-testid="study-feedback-region">
        {answered && (
          <Feedback question={question} answered={answered} fresh={fresh} />
        )}
      </div>
    </CardShell>
  );
}

/** Indice de l'option montrée choisie (choix, vrai/faux), ou -1. */
function selectedIndex(value: AnswerValue | null): number {
  if (value?.type === "choice") return value.index;
  if (value?.type === "trueFalse") return value.value ? 0 : 1;
  return -1;
}

/** Options (boutons radio natifs) ; après la réponse : attendue et donnée marquées. */
function Options({
  labelledBy,
  options,
  selected,
  answered,
  onSelect,
}: {
  labelledBy: string;
  options: ReactNode[];
  selected: number;
  answered: Answered | null;
  onSelect: (index: number) => void;
}) {
  const t = useT();
  const name = useId();
  const expected = answered ? selectedIndex(answered.expected) : -1;
  return (
    <div
      role="radiogroup"
      aria-labelledby={labelledBy}
      className="flex flex-col gap-2"
    >
      {options.map((option, index) => {
        const isExpected = answered !== null && index === expected;
        const isWrongGiven =
          answered !== null && index === selected && !answered.correct;
        const isSelected = index === selected;
        // Choix fait, pas encore validé : aplat roux, texte encre.
        const text = "text-neutral-800 dark:text-neutral-100";
        const tone = isExpected
          ? `${text} border-emerald-500 bg-emerald-50 dark:border-emerald-500/70 dark:bg-emerald-950/40`
          : isWrongGiven
            ? `${text} border-red-400 bg-red-50 dark:border-red-500/70 dark:bg-red-950/40`
            : isSelected
              ? "border-accent bg-accent text-neutral-900"
              : `${text} border-neutral-200 hover:border-neutral-400 dark:border-neutral-700 dark:hover:border-neutral-500`;
        return (
          <label
            key={index}
            data-testid="study-option"
            data-state={
              isExpected ? "expected" : isWrongGiven ? "wrong" : undefined
            }
            className={`flex items-center gap-3 rounded-xl border-2 px-3 py-2.5 text-sm transition-colors has-[:focus-visible]:outline-2 has-[:focus-visible]:outline-offset-2 has-[:focus-visible]:outline-accent-fg ${answered ? "" : "cursor-pointer"} ${tone}`}
          >
            <input
              type="radio"
              name={name}
              className="sr-only"
              checked={isSelected}
              disabled={answered !== null}
              onChange={() => onSelect(index)}
            />
            <span
              aria-hidden="true"
              className={`flex h-6 w-6 flex-none items-center justify-center rounded-full text-xs font-semibold ${
                isSelected
                  ? "bg-neutral-900 text-white dark:bg-white dark:text-neutral-900"
                  : "bg-neutral-100 text-neutral-600 dark:bg-neutral-700 dark:text-neutral-300"
              }`}
            >
              {index + 1}
            </span>
            <span className="min-w-0 flex-1">{option}</span>
            {isExpected && (
              <>
                <CheckCircleIcon
                  aria-hidden="true"
                  className="h-5 w-5 flex-none text-emerald-600 dark:text-emerald-400"
                />
                <span className="sr-only">
                  {t("study.question.expectedMark")}
                </span>
              </>
            )}
            {isWrongGiven && (
              <>
                <XCircleIcon
                  aria-hidden="true"
                  className="h-5 w-5 flex-none text-red-500"
                />
                <span className="sr-only">{t("study.question.givenMark")}</span>
              </>
            )}
          </label>
        );
      })}
    </div>
  );
}

const FIELD =
  "h-9 rounded-lg border-2 border-neutral-300 bg-white px-2 text-sm text-neutral-900 outline-none focus-visible:border-accent-fg read-only:border-dashed dark:border-neutral-600 dark:bg-neutral-900 dark:text-neutral-100";

/**
 * Trou : champ dans la phrase ; seul sous elle (en titre de la carte) si
 * elle ne peut pas être coupée.
 */
function Blank({
  parts,
  text,
  answered,
  onChange,
  onEnter,
}: {
  parts: [string, string] | null;
  text: string;
  answered: Answered | null;
  onChange: (text: string) => void;
  onEnter: () => void;
}) {
  const t = useT();
  const onKeyDown = (event: KeyboardEvent<HTMLInputElement>) => {
    if (event.key !== "Enter" || event.nativeEvent.isComposing) return;
    event.preventDefault();
    // Touche tenue : un seul envoi, le retour se lit avant de continuer.
    if (event.repeat) return;
    onEnter();
  };
  const tone = answered
    ? answered.correct
      ? "border-emerald-500! dark:border-emerald-500/70!"
      : "border-red-400! dark:border-red-500/70!"
    : "";
  const field = (
    <input
      type="text"
      value={text}
      readOnly={answered !== null}
      aria-label={t("study.question.yourAnswer")}
      autoComplete="off"
      spellCheck={false}
      maxLength={200}
      onChange={(event) => onChange(event.target.value)}
      onKeyDown={onKeyDown}
      className={`${FIELD} mx-1 w-40 align-baseline ${tone}`}
    />
  );
  if (parts) {
    return (
      <div className="text-base/9 text-neutral-800 dark:text-neutral-100">
        <InlineMarkdown content={parts[0]} size="lg" />
        {/* Espaces autour du trou (le Markdown les retire). */}
        {/\s$/.test(parts[0]) && " "}
        {field}
        {/^\s/.test(parts[1]) && " "}
        <InlineMarkdown content={parts[1]} size="lg" />
      </div>
    );
  }
  return <div>{field}</div>;
}

/** Ordre : flèches haut et bas sur chaque élément (clavier compris). */
function Order({
  items,
  order,
  answered,
  onChange,
}: {
  items: string[];
  order: number[];
  answered: Answered | null;
  onChange: (order: number[]) => void;
}) {
  const t = useT();
  const move = (position: number, step: -1 | 1) => {
    const next = [...order];
    const target = position + step;
    if (target < 0 || target >= next.length) return;
    [next[position], next[target]] = [next[target], next[position]];
    onChange(next);
  };
  return (
    <ol
      aria-label={t("study.question.orderList")}
      className="flex flex-col gap-2"
    >
      {order.map((item, position) => {
        const right = answered?.detail[position];
        return (
          <li
            key={item}
            data-testid="study-order-item"
            className={`flex items-center gap-3 rounded-xl border-2 px-3 py-2 text-sm text-neutral-800 dark:text-neutral-100 ${
              right === true
                ? "border-emerald-500 bg-emerald-50 dark:border-emerald-500/70 dark:bg-emerald-950/40"
                : right === false
                  ? "border-red-400 bg-red-50 dark:border-red-500/70 dark:bg-red-950/40"
                  : "border-neutral-200 dark:border-neutral-700"
            }`}
          >
            <span
              aria-hidden="true"
              className="w-5 flex-none text-center text-xs font-semibold text-neutral-500"
            >
              {position + 1}
            </span>
            <span className="min-w-0 flex-1">
              <InlineMarkdown content={items[item]} />
            </span>
            <DetailMark right={right} />
            {!answered && (
              <span className="flex flex-none gap-1">
                <button
                  type="button"
                  className="rounded-full p-1 text-neutral-500 outline-none hover:bg-neutral-100 hover:text-neutral-900 focus-visible:ring-2 focus-visible:ring-accent-fg disabled:opacity-30 dark:hover:bg-neutral-700 dark:hover:text-white"
                  aria-label={t("study.question.moveUp", {
                    item: plainInline(items[item]),
                  })}
                  title={t("study.question.moveUpShort")}
                  disabled={position === 0}
                  onClick={() => move(position, -1)}
                >
                  <ArrowUpIcon aria-hidden="true" className="h-4 w-4" />
                </button>
                <button
                  type="button"
                  className="rounded-full p-1 text-neutral-500 outline-none hover:bg-neutral-100 hover:text-neutral-900 focus-visible:ring-2 focus-visible:ring-accent-fg disabled:opacity-30 dark:hover:bg-neutral-700 dark:hover:text-white"
                  aria-label={t("study.question.moveDown", {
                    item: plainInline(items[item]),
                  })}
                  title={t("study.question.moveDownShort")}
                  disabled={position === order.length - 1}
                  onClick={() => move(position, 1)}
                >
                  <ArrowDownIcon aria-hidden="true" className="h-4 w-4" />
                </button>
              </span>
            )}
          </li>
        );
      })}
    </ol>
  );
}

/**
 * Association : une liste déroulante native par élément de gauche (clavier
 * et lecteurs d'écran du système) ; ses options ne peuvent contenir que du
 * texte : Markdown lu en texte (plainInline), formules comprises.
 */
function Match({
  left,
  right,
  pairs,
  answered,
  onChange,
}: {
  left: string[];
  right: string[];
  pairs: number[];
  answered: Answered | null;
  onChange: (pairs: number[]) => void;
}) {
  const t = useT();
  const baseId = useId();
  return (
    <ul className="flex flex-col gap-2">
      {left.map((item, index) => {
        const mark = answered?.detail[index];
        const id = `${baseId}-${index}`;
        return (
          <li
            key={index}
            data-testid="study-match-row"
            className={`flex flex-wrap items-center gap-3 rounded-xl border-2 px-3 py-2 text-sm text-neutral-800 dark:text-neutral-100 ${
              mark === true
                ? "border-emerald-500 bg-emerald-50 dark:border-emerald-500/70 dark:bg-emerald-950/40"
                : mark === false
                  ? "border-red-400 bg-red-50 dark:border-red-500/70 dark:bg-red-950/40"
                  : "border-neutral-200 dark:border-neutral-700"
            }`}
          >
            <label htmlFor={id} className="min-w-0 flex-1">
              <InlineMarkdown content={item} />
            </label>
            <select
              id={id}
              value={pairs[index]}
              disabled={answered !== null}
              onChange={(event) => {
                const next = [...pairs];
                next[index] = Number(event.target.value);
                onChange(next);
              }}
              className="h-9 max-w-56 min-w-32 rounded-lg border border-neutral-300 bg-white pr-7 pl-2 text-sm text-neutral-900 outline-none focus-visible:border-accent-fg focus-visible:ring-2 focus-visible:ring-accent-fg/40 disabled:opacity-90 dark:border-neutral-700 dark:bg-neutral-800 dark:text-neutral-100"
            >
              <option value={-1} disabled>
                {t("study.question.choose")}
              </option>
              {right.map((choice, choiceIndex) => (
                <option key={choiceIndex} value={choiceIndex}>
                  {plainInline(choice)}
                </option>
              ))}
            </select>
            <DetailMark right={mark} />
          </li>
        );
      })}
    </ul>
  );
}

/** Élément juste ou faux (icône et texte pour les lecteurs d'écran). */
function DetailMark({ right }: { right: boolean | undefined }) {
  const t = useT();
  if (right === undefined) return null;
  return right ? (
    <span className="flex flex-none items-center">
      <CheckIcon
        aria-hidden="true"
        className="h-4 w-4 text-emerald-600 dark:text-emerald-400"
      />
      <span className="sr-only">{t("study.question.itemRight")}</span>
    </span>
  ) : (
    <span className="flex flex-none items-center">
      <XMarkIcon aria-hidden="true" className="h-4 w-4 text-red-500" />
      <span className="sr-only">{t("study.question.itemWrong")}</span>
    </span>
  );
}

/** Suite attendue (ordre) ou paires attendues (association). */
export function ExpectedDetail({
  question,
  value,
}: {
  question: Question;
  value: AnswerValue;
}) {
  const t = useT();
  const body = question.body;
  if (value.type === "order" && body.type === "order") {
    return (
      <>
        <p>{t("study.feedback.expectedOrder")}</p>
        <ol className="mt-1 list-[decimal] pl-5">
          {value.order.map((item) => (
            <li key={item}>
              <InlineMarkdown content={body.items[item] ?? ""} />
            </li>
          ))}
        </ol>
      </>
    );
  }
  if (value.type === "match" && body.type === "match") {
    return (
      <>
        <p>{t("study.feedback.expectedPairs")}</p>
        <ul className="mt-1 list-[disc] pl-5">
          {value.pairs.map((pair, index) => (
            <li key={index}>
              <Trans
                k="study.feedback.pair"
                tags={{
                  left: () => (
                    <InlineMarkdown content={body.left[index] ?? ""} />
                  ),
                  right: () => (
                    <InlineMarkdown content={body.right[pair] ?? ""} />
                  ),
                }}
              />
            </li>
          ))}
        </ul>
      </>
    );
  }
  const text = expectedText(t, question, value);
  if (text === null) return null;
  return (
    <p>
      {t("study.feedback.expected")}{" "}
      <strong className="font-semibold">
        <InlineMarkdown content={text} />
      </strong>
    </p>
  );
}

/** Retour d'une réponse : juste ou faux, attendu, explication. */
function Feedback({
  question,
  answered,
  fresh,
}: {
  question: Question;
  answered: Answered;
  fresh: boolean;
}) {
  const t = useT();
  // Acceptée à une faute près : pas d'éloge, la remarque sur l'orthographe.
  const headline = answered.close
    ? null
    : answered.correct
      ? t(
          `study.feedback.praise.${PRAISE[variantOf(question.id, PRAISE.length)]}`,
        )
      : t(
          `study.feedback.nudge.${NUDGE[variantOf(question.id, NUDGE.length)]}`,
        );
  return (
    <div
      data-testid="study-feedback"
      data-correct={answered.correct}
      className={`mt-4 rounded-2xl px-4 py-3 text-sm ${
        answered.correct
          ? "bg-emerald-50 text-emerald-900 dark:bg-emerald-950/40 dark:text-emerald-100"
          : "bg-red-50 text-red-900 dark:bg-red-950/40 dark:text-red-100"
      } ${fresh ? "motion-safe:animate-study-reveal" : ""}`}
    >
      <p className="flex items-center gap-2 font-semibold">
        {answered.correct ? (
          <CheckCircleIcon
            aria-hidden="true"
            className={`h-5 w-5 text-emerald-600 dark:text-emerald-400 ${fresh ? "motion-safe:animate-study-pop" : ""}`}
          />
        ) : (
          <XCircleIcon aria-hidden="true" className="h-5 w-5 text-red-500" />
        )}
        <span>
          {answered.correct
            ? t("study.feedback.correct")
            : t("study.feedback.incorrect")}
        </span>
        {headline && <span className="font-normal">{headline}</span>}
      </p>
      {answered.close && (
        <p className="mt-1">
          {t("study.feedback.spelling", {
            expected: expectedText(t, question, answered.expected) ?? "",
          })}
        </p>
      )}
      {/* Faux à une faute près (cours de langue : accent, frappe) : la ligne
          sur l'orthographe donne déjà la réponse attendue. */}
      {!answered.correct && !answered.close && (
        <div className="mt-1">
          <ExpectedDetail question={question} value={answered.expected} />
        </div>
      )}
      {answered.explanation && (
        <div className="mt-2 border-t border-current/10 pt-2">
          <Markdown content={answered.explanation} />
        </div>
      )}
    </div>
  );
}
