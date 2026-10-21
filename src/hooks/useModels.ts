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
 * order of the list done by the picker (src/lib/models.ts); cloud models only
 * once the "Use cloud models" setting is on (and the server cloud is not
 * turned off): otherwise a selected cloud model gives way to the most recently
 * installed one, saved; they come from the server's recommendations and from a
 * typed name found in the registry, not from /api/tags (the list of installed
 * models stays local); no VRAM-based default; the recommendations do not delay the model list
 * ("Loading..."); the registry is asked about a typed name 300 ms after the
 * last keystroke (debounce), not at each keystroke.
 */

import { useQuery } from "@tanstack/react-query";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { RecommendedModel } from "@/bindings/RecommendedModel";
import { useChat } from "@/hooks/useChats";
import { useCloudStatus } from "@/hooks/useCloudStatus";
import { useSettings } from "@/hooks/useSettings";
import { isCloudModel } from "@/lib/dictation";
import { listModels, modelExistsUpstream, recommendedModels } from "@/lib/ipc";
import { chatModel, defaultModel, selectedEntry } from "@/lib/models";

export const modelsKey = ["models"] as const;
export const recommendedKey = ["recommendedModels"] as const;
export const upstreamKey = (model: string) =>
  ["modelExistsUpstream", model] as const;

/** Attente après la dernière frappe avant d'interroger le registre. */
export const UPSTREAM_DEBOUNCE_MS = 300;

const NO_MODELS: readonly RecommendedModel[] = [];

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
 * Modèles recommandés par le serveur Ollama (cloud compris, avec leur
 * forfait), en tête du sélecteur ; liste vide tant qu'elle n'est pas lue, ou
 * si elle ne peut pas l'être.
 */
export function useRecommendedModels(): readonly RecommendedModel[] {
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
 * Modèles cloud permis : réglage « Use cloud models » activé et cloud du
 * serveur pas coupé (`/api/status`, lu seulement une fois le réglage activé ;
 * inconnu : permis, le moteur refusera ce que le serveur refuse). Réglages
 * pas encore lus : non permis.
 */
export function useCloudModels(): boolean {
  const { settings } = useSettings();
  const on = settings?.cloudModels ?? false;
  const { cloudDisabled } = useCloudStatus({ enabled: on });
  return on && !cloudDisabled;
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
 * choisi, le plus récemment installé est pris ; un modèle cloud choisi alors
 * qu'ils ne sont pas permis laisse la place au plus récemment installé (ou à
 * aucun), comme l'officiel quand le cloud est coupé, mais enregistré.
 */
export function useSelectedModel(chatId?: string) {
  const { settings, setSettings } = useSettings();
  const { data: models, isLoading, isSuccess } = useModels();
  const { data: chat } = useChat(chatId && chatId !== "new" ? chatId : "");
  const cloud = useCloudModels();
  const selectedName = settings?.selectedModel ?? "";
  const installed = useMemo(() => models ?? [], [models]);
  const selected = useMemo(
    () => selectedEntry(installed, selectedName),
    [installed, selectedName],
  );

  // Track which chat we've already restored the model for
  const restoredChatRef = useRef<string | null>(null);

  useEffect(() => {
    if (!chatId || chatId === "new" || !chat || !settings) return;
    if (restoredChatRef.current === chatId) return;
    restoredChatRef.current = chatId;
    const model = chatModel(chat.messages);
    // Modèle cloud pas permis : le modèle choisi reste (le repli plus bas
    // l'aurait remplacé par un autre que celui de l'utilisateur).
    if (model && model !== selectedName && (cloud || !isCloudModel(model))) {
      void setSettings({ selectedModel: model });
    }
  }, [chatId, chat, settings, cloud, selectedName, setSettings]);

  useEffect(() => {
    if (!isSuccess || !settings || selectedName) return;
    const fallback = defaultModel(installed, cloud);
    if (fallback) void setSettings({ selectedModel: fallback });
  }, [isSuccess, settings, cloud, selectedName, installed, setSettings]);

  // Modèle cloud qui n'est pas (ou plus) permis : rien ne doit y partir. Le
  // remplaçant n'est jamais lui-même un nom de modèle cloud (sinon l'effet
  // repartirait sans fin).
  useEffect(() => {
    if (!isSuccess || !settings || cloud || !isCloudModel(selectedName)) return;
    const fallback = defaultModel(installed, false) ?? "";
    if (fallback !== selectedName)
      void setSettings({ selectedModel: fallback });
  }, [isSuccess, settings, cloud, selectedName, installed, setSettings]);

  const select = useCallback(
    (model: string) => setSettings({ selectedModel: model }),
    [setSettings],
  );

  return { selected, models: installed, loading: isLoading, select };
}
