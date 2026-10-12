/*
 * SPDX-License-Identifier: MIT
 * SPDX-FileCopyrightText: Ollama
 * SPDX-FileCopyrightText: 2026 Elwynn18 and ChatonLM contributors
 *
 * Adapted from Ollama (https://github.com/ollama/ollama),
 * files app/ui/app/src/hooks/useUser.ts and
 * app/ui/app/src/components/Settings.tsx, commit 42e911bc3d05.
 * Modified for ChatonLM: Tauri commands instead of HTTP calls (the backend
 * opens the sign-in page itself, no connect URL query); the account is a
 * signed-in or signed-out state rather than a user or null; it can be left
 * unread (enabled); sign out reads it again instead of reloading the window;
 * the waiting for a sign-in of Settings (polling, focus) is a hook of its
 * own, starts with one read (a server signed in from a terminal opens no
 * page), and stops as soon as the account is signed in, whatever read it; a
 * read started before a sign-out is ignored; an account that cannot be read
 * (Ollama in error, unreachable) is a failure with its reason from the first
 * failed try, while the official retries go on, never a signed-out account;
 * once they are over, it can be read again (retry), the reason staying until
 * the answer.
 */

// Compte ollama.com du serveur Ollama, comme useUser de l'officiel : état lu
// par /api/me, connexion par la page d'ollama.com ouverte dans le navigateur,
// déconnexion.

import {
  type QueryClient,
  queryOptions,
  useMutation,
  useQuery,
  useQueryClient,
} from "@tanstack/react-query";
import { useCallback, useEffect, useRef, useState } from "react";
import type { AccountStatus } from "@/bindings/AccountStatus";
import {
  accountSignIn,
  accountSignOut,
  accountStatus,
  errorMessage,
} from "@/lib/ipc";

export const accountKey = ["account"] as const;

/** Relecture du compte pendant l'attente d'une connexion (Settings). */
export const SIGN_IN_POLL_MS = 5000;

const SIGNED_OUT: AccountStatus = { state: "signedOut" };

/**
 * Déconnexions lancées : une relecture (refresh) partie avant la dernière
 * rendrait le compte d'avant la déconnexion ; son résultat est ignoré.
 */
let signOuts = 0;

/**
 * Dernière raison d'échec de la lecture, par client : React Query l'efface
 * au départ d'une nouvelle lecture d'une requête sans données (Try again,
 * relecture en arrivant) ; elle reste montrée jusqu'à la réponse.
 */
const lastFailures = new WeakMap<QueryClient, string>();

/** Comme userQueryOptions de l'officiel (délais, nouveaux essais). */
export const accountQueryOptions = queryOptions({
  queryKey: accountKey,
  queryFn: async ({ client }) => {
    try {
      const account = await accountStatus();
      lastFailures.delete(client);
      return account;
    } catch (error) {
      lastFailures.set(client, errorMessage(error));
      throw error;
    }
  },
  staleTime: 5 * 60 * 1000,
  gcTime: 10 * 60 * 1000,
  retry: 10,
  retryDelay: (attemptIndex: number) => Math.min(500 * attemptIndex, 2000),
});

/**
 * Compte du serveur Ollama. `enabled` : lu seulement si utile (pas quand
 * le nuage du serveur est coupé, voir Settings). `refresh` : relecture
 * hors de la requête, comme refreshUser de l'officiel (sans nouveaux essais,
 * `isRefreshing` pendant ce temps) ; résolue avec le compte lu (undefined si
 * une déconnexion l'a rendu périmé : il n'est pas gardé). `signOut` :
 * compte montré délié tout de suite, rétabli si la déconnexion échoue, puis
 * relu (l'officiel recharge la fenêtre). `failure` : raison de l'échec de la
 * lecture, dès le premier essai manqué, tant qu'aucun compte n'est lu (un
 * échec n'est jamais « non relié ») ; `retrying` : les nouveaux essais
 * continuent ; `retry` : nouvelle lecture une fois les essais épuisés (la
 * raison reste montrée pendant ce temps, comme pendant la relecture faite en
 * arrivant).
 */
export function useAccount({
  enabled = true,
  fresh = false,
}: {
  enabled?: boolean;
  /** Relu au montage même s'il est récent (on vient pour se connecter). */
  fresh?: boolean;
} = {}) {
  const queryClient = useQueryClient();
  const query = useQuery({
    ...accountQueryOptions,
    enabled,
    refetchOnMount: fresh ? "always" : true,
  });

  const refreshMutation = useMutation({
    mutationFn: async () => {
      const generation = signOuts;
      const account = await accountStatus();
      // Déconnexion lancée pendant la lecture : résultat périmé.
      return generation === signOuts ? account : undefined;
    },
    onSuccess: (data) => {
      if (data) queryClient.setQueryData(accountKey, data);
    },
  });

  const signOutMutation = useMutation({
    mutationFn: accountSignOut,
    onMutate: async () => {
      signOuts += 1;
      await queryClient.cancelQueries({ queryKey: accountKey });
      const previous = queryClient.getQueryData<AccountStatus>(accountKey);
      queryClient.setQueryData(accountKey, SIGNED_OUT);
      return { previous };
    },
    onError: (_error, _variables, context) => {
      if (context?.previous) {
        queryClient.setQueryData(accountKey, context.previous);
      }
    },
    // Relu sans faire attendre la fin de la déconnexion.
    onSettled: () => {
      void queryClient.invalidateQueries({ queryKey: accountKey });
    },
  });

  const { mutateAsync: refreshAsync } = refreshMutation;
  const refresh = useCallback(() => refreshAsync(), [refreshAsync]);

  // Raison de l'échec en cours ; pendant une nouvelle lecture d'un compte
  // jamais lu après un échec (React Query l'a effacée), celle d'avant.
  const unread = enabled && query.data === undefined;
  const failure = !unread
    ? null
    : query.failureReason
      ? errorMessage(query.failureReason)
      : query.isPending && query.errorUpdateCount > 0
        ? (lastFailures.get(queryClient) ?? null)
        : null;
  const { refetch } = query;
  const retry = useCallback(() => void refetch(), [refetch]);

  return {
    account: query.data,
    /** Pas encore lu (premier essai et nouveaux essais compris). */
    isLoading: query.isPending && enabled,
    /** Raison de l'échec de la lecture, tant qu'aucun compte n'est lu. */
    failure,
    /** Nouveaux essais en cours après un échec. */
    retrying: failure !== null && query.fetchStatus === "fetching",
    retry,
    /** Lecture en cours (première ou relecture). */
    isFetching: query.isFetching,
    isAuthenticated: query.data?.state === "signedIn",
    refresh,
    isRefreshing: refreshMutation.isPending,
    signOut: signOutMutation.mutateAsync,
  };
}

/**
 * Connexion depuis un bouton « Sign In » : la page de connexion d'ollama.com
 * s'ouvre dans le navigateur (par le moteur), puis le compte est attendu,
 * d'abord par une relecture immédiate (serveur déjà relié, au terminal :
 * aucune page ne s'ouvre, le cache était périmé), puis toutes les 5 s ; au
 * retour du focus sur la fenêtre, l'attente s'arrête sur une dernière
 * relecture. Elle s'arrête aussi dès que le compte
 * est relié. `starting` : ouverture en cours (xdg-open et /api/me, jusqu'à
 * quelques secondes), un second clic n'ouvre pas une autre page. `start`
 * rejette si la page ne peut pas être ouverte.
 */
export function useSignIn({
  isAuthenticated,
  refresh,
}: {
  isAuthenticated: boolean;
  /** Relecture du compte ; résolue avec le compte lu. */
  refresh: () => Promise<AccountStatus | undefined>;
}) {
  const [awaiting, setAwaiting] = useState(false);
  const [starting, setStarting] = useState(false);
  // Garde du double clic, avant même le rendu qui désactive le bouton.
  const startingRef = useRef(false);
  const waiting = awaiting && !isAuthenticated;

  // Résultat de chaque relecture : relié, l'attente prend fin.
  const reread = useCallback(() => {
    refresh().then(
      (account) => {
        if (account?.state === "signedIn") setAwaiting(false);
      },
      (error: unknown) =>
        console.error("Failed to read the Ollama account:", error),
    );
  }, [refresh]);

  useEffect(() => {
    if (!waiting) return;
    const timer = window.setInterval(reread, SIGN_IN_POLL_MS);
    const onFocus = () => {
      setAwaiting(false);
      reread();
    };
    window.addEventListener("focus", onFocus);
    return () => {
      window.clearInterval(timer);
      window.removeEventListener("focus", onFocus);
    };
  }, [waiting, reread]);

  const start = useCallback(async () => {
    if (startingRef.current) return;
    startingRef.current = true;
    setStarting(true);
    setAwaiting(false);
    try {
      await accountSignIn();
    } finally {
      startingRef.current = false;
      setStarting(false);
    }
    setAwaiting(true);
    reread();
  }, [reread]);

  /** Fin de l'attente sans relecture (déconnexion...). */
  const stop = useCallback(() => setAwaiting(false), []);

  return { awaiting: waiting, starting, start, stop };
}
