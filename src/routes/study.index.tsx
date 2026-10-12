// Accueil de l'onglet Study : paramètres d'adresse `topic`, `level`,
// `minutes` (préremplissage du formulaire, « Go further » de la fin d'un
// cours) ; la page est chargée à part (study.index.lazy.tsx).

import { createFileRoute } from "@tanstack/react-router";
import { validateStudySearch } from "@/lib/study";

export const Route = createFileRoute("/study/")({
  validateSearch: validateStudySearch,
});
