// Atelier (moteur de tâches longues, propre à ChatonLM) : gabarit commun de
// l'accueil (/workshop), d'un nouveau projet (/workshop/new) et d'un projet
// (/workshop/$projectId), chargé à part (workshop.lazy.tsx).

import { createFileRoute } from "@tanstack/react-router";

export const Route = createFileRoute("/workshop")({});
