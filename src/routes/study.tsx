// Onglet Study (propre à ChatonLM) : gabarit commun de l'accueil (/study) et
// d'un cours (/study/$courseId). Barre latérale du chat, historique compris
// (entrée Study surlignée ; les cours ne vont jamais dans l'historique), titre
// « Study », puis la page. Les changements des cours (« study-updated ») sont
// écoutés ici, pour les deux pages.

import { createFileRoute, Outlet } from "@tanstack/react-router";
import { ChatSidebar } from "@/components/ChatSidebar";
import { SidebarLayout } from "@/components/layout/layout";
import { useStudyEvents } from "@/hooks/useStudy";
import { useT } from "@/i18n";

export const Route = createFileRoute("/study")({
  component: StudyLayout,
});

function StudyLayout() {
  const t = useT();
  useStudyEvents();
  return (
    <SidebarLayout
      title={t("study.title")}
      sidebar={<ChatSidebar current="study" />}
    >
      <Outlet />
    </SidebarLayout>
  );
}
