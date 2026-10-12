// Page Proofread (propre à ChatonLM), chargée à la première visite : barre
// latérale du chat, historique compris (entrée Proofread surlignée), puis
// le volet de correction. Avec `selection`, la dernière correction du
// raccourci global est lue et montrée en revue, une fois par demande : une
// nouvelle action « Show » (état `navigateRequest`) la redemande, un retour
// par l'historique sur une entrée déjà servie, non.

import { createLazyFileRoute, useRouterState } from "@tanstack/react-router";
import { useEffect } from "react";
import { ChatSidebar } from "@/components/ChatSidebar";
import { SidebarLayout } from "@/components/layout/layout";
import { Proofread } from "@/components/proofread/Proofread";
import { claimSelectionRequest, showSelection } from "@/hooks/useProofread";
import { useT } from "@/i18n";
import { proofreadLastSelection } from "@/lib/ipc";
import { validateProofreadSearch } from "@/routes/proofread";

export const Route = createLazyFileRoute("/proofread")({
  component: ProofreadRoute,
});

function ProofreadRoute() {
  const t = useT();
  // Une clé par demande : numéro donné par la route racine aux pages
  // demandées par le moteur, sinon l'entrée d'historique ; null sans
  // `selection`. Lue sur l'adresse elle-même : pendant un retour par
  // l'historique, la page peut être rendue avant que l'adresse courante
  // soit la sienne (celle d'avant n'est pas une demande).
  const request = useRouterState({
    select: ({ location }) =>
      location.pathname === "/proofread" &&
      validateProofreadSearch(location.search).selection
        ? String(
            location.state.navigateRequest ??
              location.state.__TSR_key ??
              location.href,
          )
        : null,
  });
  useEffect(() => {
    if (request === null || !claimSelectionRequest(request)) return;
    proofreadLastSelection().then(
      (result) => {
        if (result) showSelection(result);
      },
      (error: unknown) =>
        console.error("Failed to read the corrected selection:", error),
    );
  }, [request]);
  return (
    <SidebarLayout
      title={t("proofread.title")}
      sidebar={<ChatSidebar current="proofread" />}
    >
      <Proofread />
    </SidebarLayout>
  );
}
