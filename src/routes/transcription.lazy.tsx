// Outil Transcription (propre à ChatonLM), chargé à la première visite :
// barre latérale du chat, historique compris (entrée Tools surlignée), titre
// « Transcription », puis la page. L'état de la transcription en cours
// (« transcription-status ») et celui de la synthèse
// (« transcription-synthesis ») sont suivis ici, pour les deux pages.

import { createLazyFileRoute, Outlet } from "@tanstack/react-router";
import { ChatSidebar } from "@/components/ChatSidebar";
import { SidebarLayout } from "@/components/layout/layout";
import {
  useSynthesisEvents,
  useTranscriptionEvents,
} from "@/components/transcription/queries";
import { useT } from "@/i18n";

export const Route = createLazyFileRoute("/transcription")({
  component: TranscriptionLayout,
});

function TranscriptionLayout() {
  const t = useT();
  useTranscriptionEvents();
  useSynthesisEvents();
  return (
    <SidebarLayout
      title={t("transcription.title")}
      sidebar={<ChatSidebar current="tools" />}
    >
      <Outlet />
    </SidebarLayout>
  );
}
