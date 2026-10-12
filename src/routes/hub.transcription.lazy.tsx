// Modèles de transcription de la dictée (ModelHub) : catalogue, téléchargement,
// choix, suppression.

import { createLazyFileRoute } from "@tanstack/react-router";
import { SpeechModels } from "@/components/hub/SpeechModels";

export const Route = createLazyFileRoute("/hub/transcription")({
  component: SpeechModels,
});
