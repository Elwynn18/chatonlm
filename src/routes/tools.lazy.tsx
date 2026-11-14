// Page Tools (propre à ChatonLM), chargée à la première visite : barre
// latérale du chat, historique compris (entrée Tools surlignée), titre
// « Tools », puis les cartes des outils.

import { createLazyFileRoute } from "@tanstack/react-router";
import { ChatSidebar } from "@/components/ChatSidebar";
import { SidebarLayout } from "@/components/layout/layout";
import { ToolsPage } from "@/components/tools/ToolsPage";
import { useT } from "@/i18n";

export const Route = createLazyFileRoute("/tools")({
  component: ToolsRoute,
});

function ToolsRoute() {
  const t = useT();
  return (
    <SidebarLayout
      title={t("tools.title")}
      sidebar={<ChatSidebar current="tools" />}
    >
      <ToolsPage />
    </SidebarLayout>
  );
}
