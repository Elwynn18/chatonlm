// Page d'un cours (onglet Study) ; chargée à part (study.$courseId.lazy.tsx).

import { createFileRoute } from "@tanstack/react-router";

export const Route = createFileRoute("/study/$courseId")({});
