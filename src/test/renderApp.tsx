// Rendu de l'app pour les tests : vraies routes (src/routes), routeur en
// mémoire et QueryClient neufs à chaque test. À utiliser avec le simulateur
// (src/test/simulator.ts), qui répond aux commandes.

import { QueryClientProvider } from "@tanstack/react-query";
import {
  createMemoryHistory,
  createRouter,
  RouterContextProvider,
  RouterProvider,
} from "@tanstack/react-router";
import { cleanup, render, screen } from "@testing-library/react";
import { onTestFinished, vi } from "vitest";
import { createQueryClient } from "@/lib/queryClient";
import { routeTree } from "@/routeTree.gen";
import { type Send, SendProbe } from "@/test/SendProbe";

/** Affiche l'app à `path` ; démontée à la fin du test. */
export function renderApp(path: string) {
  // Réglé comme celui de l'app (modifications envoyées même hors ligne).
  const queryClient = createQueryClient();
  const router = createRouter({
    routeTree,
    context: { queryClient },
    history: createMemoryHistory({ initialEntries: [path] }),
  });
  // jsdom n'a pas window.scrollTo, que le routeur appelle à chaque navigation.
  const scrollTo = vi.spyOn(window, "scrollTo").mockImplementation(() => {});
  let send: Send | null = null;
  render(
    <QueryClientProvider client={queryClient}>
      <RouterProvider router={router} />
      <RouterContextProvider router={router}>
        <SendProbe onSend={(value) => (send = value)} />
      </RouterContextProvider>
    </QueryClientProvider>,
  );
  // Appelé avant la fin du simulateur (ordre inverse d'inscription).
  onTestFinished(() => {
    cleanup();
    queryClient.clear();
    scrollTo.mockRestore();
  });
  return {
    router,
    queryClient,
    /** Chemin affiché. */
    path: () => router.state.location.pathname,
    /** Envoi direct, sans la zone de saisie. */
    send: (...args: Parameters<Send>) => {
      if (!send) throw new Error("the app is not rendered yet");
      return send(...args);
    },
  };
}

/**
 * Textes des messages affichés d'un rôle, dans l'ordre (sans le bouton
 * « edit » des messages de l'utilisateur).
 */
export function texts(role: "user" | "assistant") {
  return [...document.querySelectorAll(`[data-role='${role}']`)].map(
    (element) => {
      const copy = element.cloneNode(true) as Element;
      for (const button of copy.querySelectorAll(".edit-button")) {
        button.remove();
      }
      return copy.textContent;
    },
  );
}

/** Zone de saisie. */
export function composer() {
  return screen.getByLabelText("Message") as HTMLTextAreaElement;
}
