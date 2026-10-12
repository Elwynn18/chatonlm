/*
 * SPDX-License-Identifier: MIT
 * SPDX-FileCopyrightText: Ollama
 * SPDX-FileCopyrightText: 2026 Elwynn18 and ChatonLM contributors
 *
 * Adapted from Ollama (https://github.com/ollama/ollama),
 * files app/ui/app/src/hooks/useModels.ts,
 * app/ui/app/src/hooks/useSelectedModel.ts and
 * app/ui/app/src/hooks/useFeaturedModels.ts, commit 42e911bc3d05.
 * Modified for ChatonLM: Tauri commands instead of HTTP calls; search and
 * order of the list done by the picker (src/lib/models.ts); no cloud models,
 * no VRAM-based default; the recommendations do not delay the model list
 * ("Loading..."); the registry is asked about a typed name 300 ms after the
 * last keystroke (debounce), not at each keystroke.
 */

import { useQuery } from "@tanstack/react-query";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useChat } from "@/hooks/useChats";
import { useSettings } from "@/hooks/useSettings";
import { listModels, modelExistsUpstream, recommendedModels } from "@/lib/ipc";
import { chatModel, defaultModel, selectedEntry } from "@/lib/models";

export const modelsKey = ["models"] as const;
export const recommendedKey = ["recommendedModels"] as const;
export const upstreamKey = (model: string) =>
  ["modelExistsUpstream", model] as const;

/** Attente après la dernière frappe avant d'interroger le registre. */
export const UPSTREAM_DEBOUNCE_MS = 300;

const NO_MODELS: readonly string[] = [];

/** Modèles installés, rafraîchis toutes les 30 s et au retour sur la fenêtre. */
export function useModels() {
  return useQuery({
    queryKey: modelsKey,
    queryFn: listModels,
    gcTime: 10 * 60 * 1000,
    retry: 10,
    retryDelay: (attemptIndex) => Math.min(100 * 2 ** attemptIndex, 5000),
    refetchOnWindowFocus: true,
    refetchInterval: 30 * 1000,
    refetchIntervalInBackground: true,
  });
}

/**
 * Modèles recommandés par le serveur Ollama, en tête du sélecteur ; liste
 * vide tant qu'elle n'est pas lue, ou si elle ne peut pas l'être.
 */
export function useRecommendedModels(): readonly string[] {
  const { data } = useQuery({
    queryKey: recommendedKey,
    queryFn: recommendedModels,
    staleTime: 5 * 60 * 1000,
    gcTime: 30 * 60 * 1000,
    refetchOnWindowFocus: false,
  });
  return data ?? NO_MODELS;
}

/**
 * `name` s'il existe au registre, sinon null ; vide : rien à chercher. Le
 * registre n'est interrogé qu'après 300 ms sans changement du nom ; une
 * erreur (registre injoignable) vaut « absent ».
 */
export function useUpstreamModel(name: string): string | null {
  // Dernier nom resté 300 ms sans changer.
  const [settled, setSettled] = useState("");
  useEffect(() => {
    if (!name) return;
    const timer = setTimeout(() => setSettled(name), UPSTREAM_DEBOUNCE_MS);
    return () => clearTimeout(timer);
  }, [name]);
  const ready = name !== "" && settled === name;
  // Clé du nom stabilisé : pas d'entrée de cache par frappe.
  const { data } = useQuery({
    queryKey: upstreamKey(settled),
    queryFn: () => modelExistsUpstream(settled),
    enabled: ready,
    staleTime: 5 * 60 * 1000,
    retry: false,
  });
  return ready && data === true ? name : null;
}

/**
 * Modèle choisi (réglage global) et modèles installés. À l'ouverture d'une
 * conversation, son dernier modèle redevient le modèle choisi ; sans modèle
 * choisi, le plus récemment installé est pris.
 */
export function useSelectedModel(chatId?: string) {
  const { settings, setSettings } = useSettings();
  const { data: models, isLoading, isSuccess } = useModels();
  const { data: chat } = useChat(chatId && chatId !== "new" ? chatId : "");
  const selectedName = settings?.selectedModel ?? "";
  const installed = useMemo(() => models ?? [], [models]);
  const selected = useMemo(
    () => selectedEntry(installed, selectedName),
    [installed, selectedName],
  );

  // Track which chat we've already restored the model for
  const restoredChatRef = useRef<string | null>(null);

  useEffect(() => {
    if (!chatId || chatId === "new" || !chat) return;
    if (restoredChatRef.current === chatId) return;
    restoredChatRef.current = chatId;
    const model = chatModel(chat.messages);
    if (model && model !== selectedName) {
      void setSettings({ selectedModel: model });
    }
  }, [chatId, chat, selectedName, setSettings]);

  useEffect(() => {
    if (!isSuccess || !settings || selectedName) return;
    const fallback = defaultModel(installed);
    if (fallback) void setSettings({ selectedModel: fallback });
  }, [isSuccess, settings, selectedName, installed, setSettings]);

  const select = useCallback(
    (model: string) => setSettings({ selectedModel: model }),
    [setSettings],
  );

  return { selected, models: installed, loading: isLoading, select };
}
