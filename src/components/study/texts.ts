// Onglet Study : textes composés des cartes (sur-titre d'un palier, réponse
// attendue), dans la langue de `t`.

import type { AnswerValue } from "@/bindings/AnswerValue";
import type { Course } from "@/bindings/Course";
import type { Question } from "@/bindings/Question";
import type { TFunction } from "@/i18n";

/** Sur-titre d'une carte de palier : « Level 2 · Title ». */
export function levelEyebrow(
  t: TFunction,
  course: Course,
  index: number,
): string {
  return t("study.card.levelEyebrow", {
    number: index + 1,
    title: course.plan?.levels[index]?.title ?? "",
  });
}

/** Texte de la réponse attendue (choix : Markdown de l'option). */
export function expectedText(
  t: TFunction,
  question: Question,
  value: AnswerValue,
): string | null {
  const body = question.body;
  if (value.type === "choice" && body.type === "choice") {
    return body.options[value.index] ?? null;
  }
  if (value.type === "trueFalse") {
    return value.value ? t("study.question.true") : t("study.question.false");
  }
  if (value.type === "blank") return value.text;
  return null;
}
