// Un projet de l'Atelier : la page est chargée à part
// (workshop.$projectId.lazy.tsx).

import { createFileRoute } from "@tanstack/react-router";

export const Route = createFileRoute("/workshop/$projectId")({});
