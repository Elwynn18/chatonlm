// Fiche d'un dépôt Hugging Face (ModelHub).

import { createLazyFileRoute } from "@tanstack/react-router";
import { ModelPage } from "@/components/hub/ModelPage";

export const Route = createLazyFileRoute("/hub/model/$author/$repo")({
  component: ModelRoute,
});

function ModelRoute() {
  const { author, repo } = Route.useParams();
  const id = `${author}/${repo}`;
  // Une autre fiche repart de zéro (version choisie, README déplié).
  return <ModelPage key={id} id={id} />;
}
