// Lecture à voix haute d'un texte d'un cours (onglet Study) : le bouton
// « Read aloud » des réponses du chat (ReadAloudButton : préparation, voix à
// télécharger avec accord, « Stop », erreurs dites à côté du bouton, une
// seule lecture dans toute l'app), pour un passage du cours nommé par sa clé
// (`passageKey` de src/lib/study.ts). Le moteur relit le texte en base : la
// page n'envoie que la clé. La lecture s'arrête quand le texte quitte l'écran
// (carte suivante, autre page), son bouton « Stop » avec lui.

import { ReadAloudButton, ReadAloudStatus } from "@/components/ReadAloudButton";
import { useShownStudyPassage } from "@/hooks/useReadAloud";

export function StudyReadAloud({
  courseId,
  passage,
}: {
  courseId: string;
  /** Clé du texte dans le cours (`passageKey`). */
  passage: string;
}) {
  useShownStudyPassage(courseId, passage);
  return (
    <div
      data-testid="study-read-aloud"
      data-passage={passage}
      className="-ml-1 flex flex-wrap items-center"
    >
      <ReadAloudButton
        target={{ kind: "study", courseId, passage }}
        className="text-neutral-500 dark:text-neutral-400"
      />
      <ReadAloudStatus source={{ kind: "study", courseId, passage }} />
    </div>
  );
}
