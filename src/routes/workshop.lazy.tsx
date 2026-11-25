// Atelier (propre à ChatonLM), chargé à la première visite : barre latérale
// du chat, historique compris (entrée Tools surlignée), titre « Workshop »,
// puis la page. Les avis du moteur (« workshop-event ») sont suivis ici,
// pour toutes les pages de l'outil.

import { createLazyFileRoute, Outlet } from "@tanstack/react-router";
import { ChatSidebar } from "@/components/ChatSidebar";
import { SidebarLayout } from "@/components/layout/layout";
import { useWorkshopNotices } from "@/components/workshop/queries";
import { useT } from "@/i18n";

export const Route = createLazyFileRoute("/workshop")({
  component: WorkshopLayout,
});

function WorkshopLayout() {
  const t = useT();
  useWorkshopNotices();
  return (
    <SidebarLayout
      title={t("workshop.title")}
      sidebar={<ChatSidebar current="tools" />}
    >
      <Outlet />
    </SidebarLayout>
  );
}
