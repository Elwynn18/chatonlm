// Une transcription, chargée à la première visite : progression si elle
// tourne, puis son texte, ses exports et son document de lecture.

import { createLazyFileRoute } from "@tanstack/react-router";
import { TranscriptionView } from "@/components/transcription/TranscriptionView";

export const Route = createLazyFileRoute("/transcription/$transcriptionId")({
  component: TranscriptionRoute,
});

function TranscriptionRoute() {
  const { transcriptionId } = Route.useParams();
  // Clé : une autre transcription repart d'un état neuf (renommage…).
  return <TranscriptionView key={transcriptionId} id={transcriptionId} />;
}
