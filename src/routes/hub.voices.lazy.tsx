// Voix de la lecture à voix haute (ModelHub) : catalogue, téléchargement,
// suppression.

import { createLazyFileRoute } from "@tanstack/react-router";
import { VoiceModels } from "@/components/hub/VoiceModels";

export const Route = createLazyFileRoute("/hub/voices")({
  component: VoiceModels,
});
