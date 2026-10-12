// Téléchargements du ModelHub : en cours, en file, terminés.

import { createLazyFileRoute } from "@tanstack/react-router";
import { DownloadsList } from "@/components/hub/DownloadsList";

export const Route = createLazyFileRoute("/hub/downloads")({
  component: DownloadsList,
});
