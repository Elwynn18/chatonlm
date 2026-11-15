// Une transcription (outil Transcription) : la page est chargée à part
// (transcription.$transcriptionId.lazy.tsx).

import { createFileRoute } from "@tanstack/react-router";

export const Route = createFileRoute("/transcription/$transcriptionId")({});
