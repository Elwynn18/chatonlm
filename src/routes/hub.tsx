// Gabarit du ModelHub (propre à ChatonLM) : titre, onglets Discover, Installed
// et Downloads, puis la page, qui défile sous les onglets. Barre latérale du
// chat, historique compris (entrée ModelHub surlignée, aucune conversation).
//
// La zone qui défile reste montée d'une page à l'autre, et le routeur ne remet
// en haut que la fenêtre (qui ne défile pas ici) : sa position est notée par
// entrée d'historique avant chaque navigation, puis rendue une fois la page
// affichée. Une entrée nouvelle (carte, onglet, rubrique, filtre) part du
// haut ; Back et Forward retrouvent la position quittée. Rien de cela hors du
// ModelHub : le chat garde son propre défilement (useMessageAutoscroll).

import {
  createFileRoute,
  Outlet,
  type ParsedLocation,
  useRouter,
} from "@tanstack/react-router";
import { type RefObject, useLayoutEffect, useRef } from "react";
import { ChatSidebar } from "@/components/ChatSidebar";
import { HubTabs } from "@/components/hub/HubTabs";
import { SidebarLayout } from "@/components/layout/layout";
import { useT } from "@/i18n";

export const Route = createFileRoute("/hub")({
  component: HubLayout,
});

/** Positions gardées au plus (les plus anciennes oubliées d'abord). */
const KEPT_POSITIONS = 100;

/** Position de la zone par entrée d'historique (clé du routeur). */
const positions = new Map<string, number>();

function entryKey(location: ParsedLocation): string {
  return location.state.__TSR_key ?? location.href;
}

function remember(key: string, top: number) {
  positions.delete(key);
  positions.set(key, top);
  if (positions.size > KEPT_POSITIONS) {
    positions.delete(positions.keys().next().value!);
  }
}

/**
 * Note la position de `ref` avant chaque navigation et la rend après
 * (0 pour une entrée jamais vue). Inscrit pendant la mise en page, avant
 * que le routeur annonce le rendu (onRendered, dans une microtâche qui peut
 * précéder les effets passifs d'une transition) : le retour dans le ModelHub
 * depuis une autre page est vu aussi.
 */
function useScrollMemory(ref: RefObject<HTMLElement | null>) {
  const router = useRouter();
  useLayoutEffect(() => {
    const element = ref.current;
    if (!element) return;
    const stopBefore = router.subscribe("onBeforeLoad", ({ fromLocation }) => {
      if (fromLocation) remember(entryKey(fromLocation), element.scrollTop);
    });
    const stopRendered = router.subscribe(
      "onRendered",
      ({ fromLocation, toLocation }) => {
        const key = entryKey(toLocation);
        if (fromLocation && entryKey(fromLocation) === key) return;
        element.scrollTop = positions.get(key) ?? 0;
      },
    );
    return () => {
      stopBefore();
      stopRendered();
    };
  }, [router, ref]);
}

function HubLayout() {
  const t = useT();
  const scrollRef = useRef<HTMLDivElement>(null);
  useScrollMemory(scrollRef);
  return (
    <SidebarLayout
      title={t("hub.title")}
      sidebar={<ChatSidebar current="hub" />}
    >
      <div className="flex min-h-0 flex-1 flex-col dark:bg-neutral-900">
        <HubTabs />
        <div
          ref={scrollRef}
          data-testid="hub-scroll"
          className="min-h-0 flex-1 overflow-y-auto overscroll-contain"
        >
          <Outlet />
        </div>
      </div>
    </SidebarLayout>
  );
}
