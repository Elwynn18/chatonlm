// Outil Transcription (propre à ChatonLM) : état lu par React Query et tenu
// à jour par les événements du moteur. La base du moteur est la source de
// vérité : historique (`transcription_list`), transcription
// (`transcription_get`), état de celle en cours (`transcription_status`,
// passages en delta par « transcription-status »), outils yt-dlp et Deno
// (`tools_status`, « tools-status »), synthèse (`transcription_synthesis_status`,
// « transcription-synthesis »).

import {
  type QueryClient,
  useQuery,
  useQueryClient,
} from "@tanstack/react-query";
import { useEffect } from "react";
import type { SynthesisStatus } from "@/bindings/SynthesisStatus";
import type { ToolsStatus } from "@/bindings/ToolsStatus";
import type { TranscriptionStatus } from "@/bindings/TranscriptionStatus";
import { mergeStatus } from "@/components/transcription/model";
import {
  listenToolsStatus,
  listenTranscriptionSynthesis,
  listenTranscriptionStatus,
  toolsStatus,
  transcriptionSynthesisStatus,
  transcriptionGet,
  transcriptionList,
  transcriptionOptions,
  transcriptionStatus,
} from "@/lib/ipc";

export const transcriptionListKey = ["transcription", "list"] as const;
export const transcriptionStatusKey = ["transcription", "status"] as const;
export const transcriptionOptionsKey = ["transcription", "options"] as const;
export const transcriptionKey = (id: string) =>
  ["transcription", "detail", id] as const;
export const toolsStatusKey = ["transcription", "tools"] as const;
export const synthesisKey = ["transcription", "synthesis"] as const;

/** Historique, du plus récent au plus ancien. */
export function useTranscriptions() {
  return useQuery({
    queryKey: transcriptionListKey,
    queryFn: transcriptionList,
  });
}

/** Transcription complète (erreur si elle a été supprimée). */
export function useTranscription(id: string) {
  return useQuery({
    queryKey: transcriptionKey(id),
    queryFn: () => transcriptionGet(id),
    retry: false,
  });
}

/** Modèles proposés, modèle et langue d'office. */
export function useTranscriptionOptions() {
  return useQuery({
    queryKey: transcriptionOptionsKey,
    queryFn: transcriptionOptions,
  });
}

/** Transcription en cours, ou la dernière finie depuis le lancement. */
export function useTranscriptionStatus() {
  return useQuery({
    queryKey: transcriptionStatusKey,
    queryFn: transcriptionStatus,
  });
}

/** Relit l'historique et une transcription après un changement. */
export function refreshTranscription(client: QueryClient, id: string) {
  void client.invalidateQueries({ queryKey: transcriptionKey(id) });
  void client.invalidateQueries({ queryKey: transcriptionListKey });
}

/**
 * Suit « transcription-status » tant que les pages de l'outil sont
 * montées : passages ajoutés à ceux déjà reçus (état complet relu s'il en
 * manque), historique et transcription relus à chaque changement d'étape
 * (nouvelle ligne au départ, texte enregistré à la fin).
 */
export function useTranscriptionEvents() {
  const client = useQueryClient();
  useEffect(() => {
    let active = true;
    let stop: (() => void) | null = null;
    listenTranscriptionStatus((event) => {
      const known = client.getQueryData<TranscriptionStatus>(
        transcriptionStatusKey,
      );
      const merged = mergeStatus(known, event);
      const job = event.job;
      if (merged) {
        client.setQueryData(transcriptionStatusKey, merged);
      } else {
        void client.invalidateQueries({ queryKey: transcriptionStatusKey });
      }
      const before = known?.job;
      if (job && (before?.id !== job.id || before.stage !== job.stage)) {
        refreshTranscription(client, job.id);
      }
    }).then(
      (unlisten) => {
        if (active) stop = unlisten;
        else unlisten();
      },
      (error: unknown) =>
        console.error("Failed to listen to the transcription:", error),
    );
    return () => {
      active = false;
      stop?.();
    };
  }, [client]);
}

/**
 * État de yt-dlp et de Deno, relevé discret de leur dernière version à la
 * première lecture (au plus une fois par jour, par le moteur), tenu à jour
 * par « tools-status ».
 */
export function useToolsStatus() {
  const client = useQueryClient();
  useEffect(() => {
    let active = true;
    let stop: (() => void) | null = null;
    listenToolsStatus((status) => {
      client.setQueryData<ToolsStatus>(toolsStatusKey, status);
    }).then(
      (unlisten) => {
        if (active) stop = unlisten;
        else unlisten();
      },
      (error: unknown) =>
        console.error("Failed to listen to the tools:", error),
    );
    return () => {
      active = false;
      stop?.();
    };
  }, [client]);
  return useQuery({
    queryKey: toolsStatusKey,
    queryFn: () => toolsStatus(true),
    staleTime: Infinity,
  });
}

/**
 * Synthèse en cours ou la dernière (une seule à la fois), tenue à jour par
 * « transcription-synthesis » ; le document fini est relu avec la
 * transcription.
 */
export function useSynthesisEvents() {
  const client = useQueryClient();
  useEffect(() => {
    let active = true;
    let stop: (() => void) | null = null;
    listenTranscriptionSynthesis((status) => {
      client.setQueryData<SynthesisStatus | null>(synthesisKey, status);
      if (status.stage !== "running") refreshTranscription(client, status.id);
    }).then(
      (unlisten) => {
        if (active) stop = unlisten;
        else unlisten();
      },
      (cause: unknown) =>
        console.error("Failed to listen to the synthesis:", cause),
    );
    return () => {
      active = false;
      stop?.();
    };
  }, [client]);
}

/**
 * Synthèse de cette transcription (ou null), et vrai si une autre tourne
 * (une seule à la fois : le moteur refuserait).
 */
export function useSynthesis(id: string): {
  synthesis: SynthesisStatus | null;
  otherRunning: boolean;
} {
  const query = useQuery({
    queryKey: synthesisKey,
    queryFn: transcriptionSynthesisStatus,
  });
  const data = query.data ?? null;
  return {
    synthesis: data?.id === id ? data : null,
    otherRunning: data !== null && data.id !== id && data.stage === "running",
  };
}
