/*
 * SPDX-License-Identifier: MIT
 * SPDX-FileCopyrightText: Ollama
 * SPDX-FileCopyrightText: 2026 Elwynn18 and ChatonLM contributors
 *
 * Adapted from Ollama (https://github.com/ollama/ollama),
 * file app/ui/app/src/hooks/useCloudStatus.ts, commit 42e911bc3d05.
 * Modified for ChatonLM: Tauri command reading the server's /api/status
 * instead of the app's /api/v1/cloud; it can be left unread (enabled); a
 * settled flag (read, or failed) instead of the loading and error states.
 */

// Nuage d'Ollama coupé ou non sur le serveur, comme useCloudStatus de
// l'officiel (lu par /api/status : ChatonLM ne le règle pas).

import { useQuery } from "@tanstack/react-query";
import { cloudStatus } from "@/lib/ipc";

export const cloudStatusKey = ["cloudStatus"] as const;

/**
 * État du nuage ; inconnu (serveur qui n'en dit rien, pas encore lu, ou
 * `enabled` faux) : pas coupé, comme l'officiel.
 */
export function useCloudStatus({ enabled = true }: { enabled?: boolean } = {}) {
  const query = useQuery({
    queryKey: cloudStatusKey,
    queryFn: cloudStatus,
    retry: false,
    staleTime: 60 * 1000,
    enabled,
  });
  return {
    cloudStatus: query.data,
    cloudDisabled: query.data?.disabled ?? false,
    isKnown: query.data !== null && query.data !== undefined,
    /** Lecture finie (réussie ou non) : ce qui en dépend peut partir. */
    isSettled: query.status !== "pending",
  };
}
