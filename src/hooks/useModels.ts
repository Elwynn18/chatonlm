/*
 * SPDX-License-Identifier: MIT
 * SPDX-FileCopyrightText: Ollama
 * SPDX-FileCopyrightText: 2026 Elwynn18 and ChatonLM contributors
 *
 * Adapted from Ollama (https://github.com/ollama/ollama),
 * files app/ui/app/src/hooks/useModels.ts,
 * app/ui/app/src/hooks/useSelectedModel.ts and
 * app/ui/app/src/hooks/useFeaturedModels.ts, commit 42e911bc3d05.
 * Modified for ChatonLM: Tauri commands instead of HTTP calls, over the
 * models of ChatonLM's own engine; search and order of the list done by the
 * picker (src/lib/models.ts); remote models only while the "remote models"
 * setting is on: otherwise a selected remote model gives way to the most
 * recently installed local one, saved; recommendations from ChatonLM's own
 * list (Hugging Face ids), read again when the context length setting
 * changes (their memory needs depend on it); no VRAM-based default; the
 * recommendations do not delay the model list ("Loading..."); a typed
 * Hugging Face id is looked up on Hugging Face (repository card of the
 * ModelHub) 300 ms after the last keystroke (debounce), not at each
 * keystroke; the model of a chat from before ChatonLM 0.10 (an old Ollama
 * name) becomes the selected model under its new id, never under a name the
 * engine no longer knows.
 */

import { useQuery } from "@tanstack/react-query";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { RecommendedModel } from "@/bindings/RecommendedModel";
import { useChat } from "@/hooks/useChats";
import { useSettings } from "@/hooks/useSettings";
import { hubRepo, listModels, recommendedModels } from "@/lib/ipc";
import { isRemoteModel, sameModel } from "@/lib/modelNames";
import {
  chatModel,
  defaultModel,
  restoredModel,
  selectedEntry,
  type TypedRepo,
  upstreamModel,
} from "@/lib/models";

export const modelsKey = ["models"] as const;
export const recommendedKey = ["recommendedModels"] as const;
export const upstreamKey = (repo: string) => ["modelUpstream", repo] as const;

/** Attente après la dernière frappe avant d'interroger Hugging Face. */
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
 * Modèles recommandés (identifiants Hugging Face, avec la mémoire qu'il leur
 * faut au contexte réglé, et la pastille « tient ou non »), en tête du
 * sélecteur ; liste vide tant qu'elle n'est pas lue, ou si elle ne peut pas
 * l'être. Le contexte réglé fait partie de la clé : le changer relit la
 * liste (comme les estimations du ModelHub, useHub.ts).
 */
export function useRecommendedModels(): readonly RecommendedModel[] {
  const { settings } = useSettings();
  const { data } = useQuery({
    queryKey: [...recommendedKey, settings?.contextLength ?? null],
    queryFn: recommendedModels,
    // Réglages pas encore lus : la clé ne serait pas la bonne.
    enabled: settings !== undefined,
    staleTime: 5 * 60 * 1000,
    gcTime: 30 * 60 * 1000,
    refetchOnWindowFocus: false,
  });
  return data ?? NO_MODELS;
}

/**
 * Identifiant à proposer pour un dépôt de Hugging Face tapé dans le
 * sélecteur (version demandée, sinon celle conseillée pour la machine), ou
 * null ; `typed` null : rien à chercher. Hugging Face n'est interrogé
 * qu'après 300 ms sans changement ; une erreur (hors ligne) vaut « absent ».
 */
export function useUpstreamModel(typed: TypedRepo | null): string | null {
  const repo = typed?.repo ?? "";
  // Dernier dépôt resté 300 ms sans changer.
  const [settled, setSettled] = useState("");
  useEffect(() => {
    if (!repo) return;
    const timer = setTimeout(() => setSettled(repo), UPSTREAM_DEBOUNCE_MS);
    return () => clearTimeout(timer);
  }, [repo]);
  const ready = repo !== "" && settled === repo;
  // Clé du dépôt stabilisé : pas d'entrée de cache par frappe.
  const { data } = useQuery({
    queryKey: upstreamKey(settled),
    queryFn: () => hubRepo(settled),
    enabled: ready,
    staleTime: 5 * 60 * 1000,
    retry: false,
  });
  return ready && data ? upstreamModel(data.repo, typed?.quant ?? null) : null;
}

/**
 * Modèle choisi (réglage global) et modèles installés. À l'ouverture d'une
 * conversation, son dernier modèle redevient le modèle choisi (un ancien nom
 * d'Ollama, retrouvé sous son nouvel identifiant, voir restoredModel) ; sans modèle
 * choisi, le plus récemment installé est pris ; un modèle distant choisi
 * alors qu'ils ne sont pas permis laisse la place au plus récemment
 * installé (ou à aucun), comme l'officiel quand le cloud est coupé, mais
 * enregistré.
 */
export function useSelectedModel(chatId?: string) {
  const { settings, setSettings } = useSettings();
  const { data: models, isLoading, isSuccess } = useModels();
  const { data: chat } = useChat(chatId && chatId !== "new" ? chatId : "");
  // Réglages pas encore lus : non permis.
  const remote = settings?.remoteModels ?? false;
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
    // Liste pas encore lue : un ancien nom ne serait pas retrouvé.
    if (!isSuccess) return;
    if (restoredChatRef.current === chatId) return;
    restoredChatRef.current = chatId;
    const name = chatModel(chat.messages);
    // Modèle distant pas permis, modèle repris puis supprimé : le modèle
    // choisi reste (le repli plus bas l'aurait remplacé par un autre que
    // celui de l'utilisateur).
    const restored = name ? restoredModel(installed, name, remote) : null;
    if (!restored || sameModel(restored.model, selectedName)) return;
    if (restored.kind === "adopt") {
      void setSettings({ selectedModel: restored.model });
      return;
    }
    // Ancien nom d'une conversation d'avant la 0.10 : le moteur le traduit
    // à l'enregistrement s'il a été repris (réglages rendus) ; resté tel
    // quel, il ne désigne aucun modèle (l'envoi échouerait) : le modèle
    // choisi d'avant revient.
    const previous = selectedName;
    void setSettings({ selectedModel: restored.model }).then(
      (saved) => {
        if (sameModel(saved.selectedModel, restored.model)) {
          void setSettings({ selectedModel: previous });
        }
      },
      () => {},
    );
  }, [
    chatId,
    chat,
    settings,
    isSuccess,
    installed,
    remote,
    selectedName,
    setSettings,
  ]);

  useEffect(() => {
    if (!isSuccess || !settings || selectedName) return;
    const fallback = defaultModel(installed);
    if (fallback) void setSettings({ selectedModel: fallback });
  }, [isSuccess, settings, selectedName, installed, setSettings]);

  // Modèle distant qui n'est pas (ou plus) permis : rien ne doit y partir. Le
  // remplaçant n'est jamais lui-même un modèle distant (sinon l'effet
  // repartirait sans fin).
  useEffect(() => {
    if (!isSuccess || !settings || remote || !isRemoteModel(selectedName)) {
      return;
    }
    const fallback = defaultModel(installed) ?? "";
    if (fallback !== selectedName)
      void setSettings({ selectedModel: fallback });
  }, [isSuccess, settings, remote, selectedName, installed, setSettings]);

  const select = useCallback(
    (model: string) => setSettings({ selectedModel: model }),
    [setSettings],
  );

  return { selected, models: installed, loading: isLoading, select };
}
