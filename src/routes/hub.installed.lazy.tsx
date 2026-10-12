// Modèles installés sur le serveur (ModelHub).

import { createLazyFileRoute } from "@tanstack/react-router";
import { InstalledList } from "@/components/hub/InstalledList";

export const Route = createLazyFileRoute("/hub/installed")({
  component: InstalledList,
});
