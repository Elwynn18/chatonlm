// Accueil de l'outil Transcription, chargé à la première visite : nouvelle
// transcription (adresse d'une vidéo ou fichier), celle en cours et
// l'historique.

import { createLazyFileRoute } from "@tanstack/react-router";
import { TranscriptionHome } from "@/components/transcription/TranscriptionHome";

export const Route = createLazyFileRoute("/transcription/")({
  component: TranscriptionHome,
});
