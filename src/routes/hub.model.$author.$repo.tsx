// Fiche d'un dépôt Hugging Face (ModelHub) ; page chargée à part
// (hub.model.$author.$repo.lazy.tsx).

import { createFileRoute } from "@tanstack/react-router";

export const Route = createFileRoute("/hub/model/$author/$repo")({});
