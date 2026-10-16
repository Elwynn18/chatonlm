// Fiche d'un dépôt de modèles d'images (ModelHub) ; page chargée à part
// (hub.images.$author.$repo.lazy.tsx).

import { createFileRoute } from "@tanstack/react-router";

export const Route = createFileRoute("/hub/images/$author/$repo")({});
