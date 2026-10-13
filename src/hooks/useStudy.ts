// Cours de l'onglet Study (propre à ChatonLM) : la base du moteur est la
// source de vérité. La page lit la liste et chaque cours par React Query et
// les relit à chaque « study-updated » (cours changé ou supprimé) ; une
// réponse corrigée est posée tout de suite dans le cours lu (retour
// immédiat), la relecture qui suit donne les statuts.
//
// Réponses en flux (« I don't understand », « Ask a question ») : magasin de
// module, par clé (cours, palier, section) : un flux continue quand on
// change de carte ou de page ; sa réponse complète est enregistrée par le
// moteur, qui émet alors « study-updated ».

import {
  type QueryClient,
  useQuery,
  useQueryClient,
} from "@tanstack/react-query";
import { Channel } from "@tauri-apps/api/core";
import { useEffect, useRef, useState, useSyncExternalStore } from "react";
import type { Answered } from "@/bindings/Answered";
import type { Course } from "@/bindings/Course";
import type { StudyErrorCode } from "@/bindings/StudyErrorCode";
import type { StudyStreamEvent } from "@/bindings/StudyStreamEvent";
import {
  errorMessage,
  listenStudyUpdated,
  studyAsk,
  studyExplain,
  studyGet,
  studyList,
  studyStreamCancel,
} from "@/lib/ipc";

export const studyListKey = ["study", "list"] as const;
export const courseKey = (courseId: string) =>
  ["study", "course", courseId] as const;

/** Cours de l'onglet, du plus récemment actif au plus ancien. */
export function useCourses() {
  return useQuery({ queryKey: studyListKey, queryFn: studyList });
}

/** Cours complet (erreur `notFound` s'il a été supprimé). */
export function useCourse(courseId: string) {
  return useQuery({
    queryKey: courseKey(courseId),
    queryFn: () => studyGet(courseId),
  });
}

/** Relit la liste et le cours après un changement fait par la page. */
export function refreshCourse(client: QueryClient, courseId: string) {
  void client.invalidateQueries({ queryKey: courseKey(courseId) });
  void client.invalidateQueries({ queryKey: studyListKey });
}

/** Réponse corrigée posée dans le cours lu, avant sa relecture. */
export function storeAnswer(
  client: QueryClient,
  courseId: string,
  questionId: string,
  answered: Answered,
) {
  client.setQueryData<Course>(courseKey(courseId), (course) =>
    course
      ? { ...course, answers: { ...course.answers, [questionId]: answered } }
      : course,
  );
}

/**
 * Nouvel essai demandé (« Explain differently ») : en file dans le moteur
 * (`pending`) dès la réponse de la commande, qui reprend aussi un cours en
 * pause ; montré tout de suite, avant la relecture du cours (le bouton ne
 * revient pas entre les deux).
 */
export function storeRemedyRequested(
  client: QueryClient,
  courseId: string,
  level: number,
) {
  client.setQueryData<Course>(courseKey(courseId), (course) =>
    course
      ? {
          ...course,
          paused: false,
          levels: course.levels.map((item) =>
            item.index === level
              ? { ...item, remedy: "pending", remedyError: null }
              : item,
          ),
        }
      : course,
  );
}

/**
 * Action d'une page du cours : appel du moteur, puis relecture ; l'erreur
 * est gardée pour être montrée. Une seule à la fois : une seconde demande
 * pendant la première (double clic, Entrée deux fois avant le rendu qui
 * désactive le bouton) est ignorée.
 */
export function useCourseAction(courseId: string) {
  const queryClient = useQueryClient();
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // Garde lue hors du rendu (l'état `pending` n'est vu qu'au rendu suivant).
  const running = useRef(false);
  const run = (action: () => Promise<unknown>, after?: () => void) => {
    if (running.current) return;
    running.current = true;
    setPending(true);
    setError(null);
    action().then(
      () => {
        running.current = false;
        setPending(false);
        after?.();
        refreshCourse(queryClient, courseId);
      },
      (cause: unknown) => {
        running.current = false;
        setPending(false);
        setError(errorMessage(cause));
      },
    );
  };
  return { run, pending, error };
}

/**
 * Écoute « study-updated » tant que l'onglet est affiché : liste et cours
 * relus ; un cours supprimé est effacé du cache (relu, donc introuvable,
 * s'il est affiché).
 */
export function useStudyEvents() {
  const client = useQueryClient();
  useEffect(() => {
    let active = true;
    let unlisten: (() => void) | null = null;
    listenStudyUpdated(({ courseId, deleted }) => {
      if (deleted) {
        // Relu s'il est affiché (« introuvable »), oublié sinon.
        void client.resetQueries({ queryKey: courseKey(courseId) });
        void client.invalidateQueries({ queryKey: studyListKey });
        return;
      }
      refreshCourse(client, courseId);
    }).then(
      (stop) => {
        if (active) unlisten = stop;
        else stop();
      },
      (error: unknown) =>
        console.error("Failed to listen for course changes:", error),
    );
    return () => {
      active = false;
      unlisten?.();
    };
  }, [client]);
}

/** Attente de l'évènement final après la réponse de la commande. */
const FINAL_EVENT_GRACE_MS = 2000;

export type StreamPhase =
  "streaming" | "stopping" | "done" | "stopped" | "error";

export interface StudyStream {
  id: string;
  /** Question posée (« Ask a question ») ; null pour une explication. */
  question: string | null;
  text: string;
  phase: StreamPhase;
  error: { code: StudyErrorCode; message: string } | null;
  /**
   * Réponses déjà enregistrées au départ : la réponse finie reste montrée
   * jusqu'à ce que la relecture du cours la donne.
   */
  savedBefore: number;
}

const streams = new Map<string, StudyStream>();
const listeners = new Set<() => void>();

function setStream(key: string, stream: StudyStream | null) {
  if (stream) streams.set(key, stream);
  else streams.delete(key);
  for (const listener of listeners) listener();
}

function subscribe(listener: () => void) {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

/** Clé d'une explication (section d'un palier). */
export const explainKey = (courseId: string, level: number, section: number) =>
  `explain:${courseId}:${level}:${section}`;
/** Clé des questions posées sur un palier. */
export const askKey = (courseId: string, level: number) =>
  `ask:${courseId}:${level}`;

/** Flux d'une clé (null : aucun) ; rendu de nouveau à chaque fragment. */
export function useStudyStream(key: string): StudyStream | null {
  return useSyncExternalStore(
    subscribe,
    () => streams.get(key) ?? null,
    () => streams.get(key) ?? null,
  );
}

export function isStreaming(stream: StudyStream | null): boolean {
  return stream?.phase === "streaming" || stream?.phase === "stopping";
}

/** Applique un évènement au flux `id` de `key` (ignoré s'il a été remplacé). */
function apply(key: string, id: string, event: StudyStreamEvent) {
  const stream = streams.get(key);
  if (!stream || stream.id !== id || !isStreaming(stream)) return;
  switch (event.event) {
    case "delta":
      setStream(key, { ...stream, text: stream.text + event.text });
      break;
    case "done":
      setStream(key, {
        ...stream,
        text: event.stopped ? stream.text : event.text,
        phase: event.stopped ? "stopped" : "done",
      });
      break;
    case "error":
      setStream(key, {
        ...stream,
        phase: "error",
        error: { code: event.code, message: event.message },
      });
      break;
  }
}

/**
 * Lance une explication ou une réponse en flux sous `key` (une à la fois par
 * clé ; sans effet si un flux y est en cours).
 */
export function startStudyStream(
  key: string,
  request:
    | { kind: "explain"; courseId: string; level: number; section: number }
    | { kind: "ask"; courseId: string; level: number; question: string },
  savedBefore: number,
) {
  if (isStreaming(streams.get(key) ?? null)) return;
  const id = crypto.randomUUID();
  setStream(key, {
    id,
    question: request.kind === "ask" ? request.question : null,
    text: "",
    phase: "streaming",
    error: null,
    savedBefore,
  });
  let finalEvent: () => void = () => {};
  const ended = new Promise<void>((resolve) => (finalEvent = resolve));
  const channel = new Channel<StudyStreamEvent>();
  channel.onmessage = (event) => {
    if (event.event === "done" || event.event === "error") finalEvent();
    apply(key, id, event);
  };
  const running =
    request.kind === "explain"
      ? studyExplain(
          request.courseId,
          request.level,
          request.section,
          id,
          channel,
        )
      : studyAsk(
          request.courseId,
          request.level,
          request.question,
          id,
          channel,
        );
  running.then(
    async () => {
      // Tauri n'ordonne pas la réponse et les derniers messages du canal.
      await Promise.race([
        ended,
        new Promise((resolve) => setTimeout(resolve, FINAL_EVENT_GRACE_MS)),
      ]);
      const stream = streams.get(key);
      if (stream?.id === id && isStreaming(stream)) {
        setStream(key, { ...stream, phase: "done" });
      }
    },
    (cause: unknown) => {
      const stream = streams.get(key);
      if (stream?.id !== id) return;
      setStream(key, {
        ...stream,
        phase: "error",
        error: { code: "failed", message: errorMessage(cause) },
      });
    },
  );
}

/** Stop : le moteur répond par `done { stopped: true }`. */
export function stopStudyStream(key: string) {
  const stream = streams.get(key);
  if (!stream || stream.phase !== "streaming") return;
  setStream(key, { ...stream, phase: "stopping" });
  studyStreamCancel(stream.id).catch((cause: unknown) =>
    console.error("Failed to stop the answer:", cause),
  );
}

/** Oublie un flux fini (nouvelle question, panneau fermé). */
export function clearStudyStream(key: string) {
  const stream = streams.get(key);
  if (stream && !isStreaming(stream)) setStream(key, null);
}

/**
 * Oublie les flux d'un palier réécrit (« Report a mistake ») : leurs
 * réponses parlaient de l'ancienne leçon. Un flux en cours est arrêté, ses
 * derniers évènements ignorés.
 */
export function forgetLevelStreams(courseId: string, level: number) {
  const explains = `explain:${courseId}:${level}:`;
  const ask = askKey(courseId, level);
  for (const [key, stream] of [...streams]) {
    if (key !== ask && !key.startsWith(explains)) continue;
    if (stream.phase === "streaming") {
      studyStreamCancel(stream.id).catch((cause: unknown) =>
        console.error("Failed to stop the answer:", cause),
      );
    }
    setStream(key, null);
  }
}

/** Essais : aucun flux (le magasin vit aussi longtemps que le module). */
export function resetStudyStreamsForTests() {
  streams.clear();
  for (const listener of listeners) listener();
}
