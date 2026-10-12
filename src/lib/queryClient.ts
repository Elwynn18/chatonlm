import { QueryClient } from "@tanstack/react-query";

/**
 * Client de l'app ; les tests qui affichent l'app entière (renderApp) en
 * prennent un neuf, réglé de même.
 */
export function createQueryClient() {
  return new QueryClient({
    // Le backend est local : pas de mode hors ligne du navigateur à gérer.
    // Sans réseau, WebKit le dit hors ligne : requêtes et modifications
    // (suppression, renommage, favoris, réglages) partent quand même.
    defaultOptions: {
      queries: { networkMode: "always", retry: false },
      mutations: { networkMode: "always" },
    },
  });
}

export const queryClient = createQueryClient();
