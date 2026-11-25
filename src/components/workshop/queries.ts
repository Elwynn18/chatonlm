// Atelier (moteur de tâches longues, propre à ChatonLM) : état lu par React
// Query et tenu à jour par l'événement du moteur « workshop-event »
// (`WorkshopNotice`, débit borné). La base du moteur est la source de vérité :
// liste (`workshop_list`), projet (`workshop_get`), journal
// (`workshop_events`, lu par morceaux après le dernier numéro connu),
// parties (`workshop_parts`, `workshop_part`) et réglages.
//
// Avis reçu pendant une lecture : React Query v5 ne relance pas une
// première lecture en cours (sans données), il la reprend ; l'avis serait
// perdu. D'où, par projet, le plus grand numéro et le dernier état annoncés
// (le journal est relu jusqu'à ce numéro, le projet relu une fois de plus
// si son état diffère), et une nouvelle invalidation à la fin d'une
// première lecture en cours quand un avis arrive.

import {
  type QueryClient,
  type QueryKey,
  useQuery,
  useQueryClient,
} from "@tanstack/react-query";
import { useEffect, useMemo } from "react";
import type { ProjectState } from "@/bindings/ProjectState";
import type { WorkshopEvent } from "@/bindings/WorkshopEvent";
import type { WorkshopNotice } from "@/bindings/WorkshopNotice";
import {
  listenWorkshop,
  workshopEvents,
  workshopGet,
  workshopList,
  workshopPart,
  workshopParts,
  workshopSettings,
  workshopSpec,
} from "@/lib/ipc";

/** Événements rendus au plus par lecture (MAX_EVENTS_PER_READ du moteur). */
const EVENTS_PER_READ = 1_000;

export const workshopListKey = ["workshop", "list"] as const;
export const workshopSettingsKey = ["workshop", "settings"] as const;
export const workshopProjectKey = (id: number) =>
  ["workshop", "project", id] as const;
export const workshopEventsKey = (id: number) =>
  ["workshop", "events", id] as const;
export const workshopSpecKey = (id: number) =>
  ["workshop", "spec", id] as const;
export const workshopPartsKey = (id: number) =>
  ["workshop", "parts", id] as const;
/** Toutes les parties lues d'un projet (préfixe de `workshopPartKey`). */
const workshopPartPrefix = (id: number) => ["workshop", "part", id] as const;
export const workshopPartKey = (id: number, brickId: number) =>
  ["workshop", "part", id, brickId] as const;

/** Ce que les avis du moteur ont annoncé d'un projet. */
interface Announced {
  /** Plus grand numéro d'événement annoncé (0 : aucun). */
  lastSeq: number;
  /** Dernier état annoncé (null : aucun). */
  state: ProjectState | null;
}

/** Annonces par client (un client par essai : rien ne fuit de l'un à l'autre). */
const announcements = new WeakMap<QueryClient, Map<number, Announced>>();

function announcedOf(client: QueryClient, id: number): Announced {
  return announcements.get(client)?.get(id) ?? { lastSeq: 0, state: null };
}

function announce(client: QueryClient, notice: WorkshopNotice) {
  let byProject = announcements.get(client);
  if (!byProject) {
    byProject = new Map();
    announcements.set(client, byProject);
  }
  const id = notice.projectId;
  if (notice.deleted) {
    byProject.delete(id);
    return;
  }
  const known = byProject.get(id) ?? { lastSeq: 0, state: null };
  byProject.set(id, {
    lastSeq: Math.max(known.lastSeq, notice.lastSeq ?? 0),
    state: notice.state ?? known.state,
  });
}

/** Lectures vides de suite tolérées avant d'abandonner le numéro annoncé. */
const MAX_EMPTY_READS = 2;

/** Projets, du plus récent au plus ancien. */
export function useWorkshopList() {
  return useQuery({ queryKey: workshopListKey, queryFn: workshopList });
}

/** Projet complet (erreur s'il a été supprimé). */
export function useWorkshopProject(id: number) {
  const client = useQueryClient();
  return useQuery({
    queryKey: workshopProjectKey(id),
    queryFn: async () => {
      const detail = await workshopGet(id);
      // Lu avant un changement d'état annoncé entre-temps : relu une fois.
      const { state } = announcedOf(client, id);
      if (state !== null && detail.project.state !== state) {
        return workshopGet(id);
      }
      return detail;
    },
    retry: false,
  });
}

/**
 * Journal du projet : chaque lecture ne demande que les événements après
 * le dernier déjà reçu (relus tant qu'il en vient un morceau plein, ou
 * qu'un avis annonce un numéro plus grand), et les ajoute à ceux du cache.
 */
export function useWorkshopEvents(id: number) {
  const client = useQueryClient();
  return useQuery({
    queryKey: workshopEventsKey(id),
    queryFn: async () => {
      let events =
        client.getQueryData<WorkshopEvent[]>(workshopEventsKey(id)) ?? [];
      let empty = 0;
      for (;;) {
        const since = events.length === 0 ? 0 : events[events.length - 1].seq;
        const more = await workshopEvents(id, since);
        // Un numéro déjà reçu (deux lectures croisées) n'est pas repris.
        const fresh = more.filter((event) => event.seq > since);
        events = [...events, ...fresh];
        if (more.length >= EVENTS_PER_READ && fresh.length > 0) continue;
        // Un avis arrivé pendant la lecture annonce un numéro plus grand :
        // le relire (borné, si le moteur ne le rend jamais).
        const last = events.length === 0 ? 0 : events[events.length - 1].seq;
        if (last >= announcedOf(client, id).lastSeq) break;
        empty = fresh.length === 0 ? empty + 1 : 0;
        if (empty >= MAX_EMPTY_READS) break;
      }
      return events;
    },
    retry: false,
  });
}

/** Parties du plan (état sauvé par le moteur). */
export function useWorkshopParts(id: number) {
  return useQuery({
    queryKey: workshopPartsKey(id),
    queryFn: () => workshopParts(id),
    retry: false,
  });
}

/** Une partie : texte sauvé et vérifications (`enabled` faux : rien). */
export function useWorkshopPart(id: number, brickId: number | null) {
  return useQuery({
    queryKey: workshopPartKey(id, brickId ?? -1),
    queryFn: () => workshopPart(id, brickId ?? 0),
    enabled: brickId !== null,
    retry: false,
  });
}

/**
 * Critères de la spécification retenue (identifiant → texte), vides avant
 * qu'elle soit écrite.
 */
export function useWorkshopCriteria(id: number): ReadonlyMap<string, string> {
  const spec = useQuery({
    queryKey: workshopSpecKey(id),
    queryFn: () => workshopSpec(id),
    retry: false,
  });
  return useMemo(
    () =>
      new Map(
        (spec.data?.criteria ?? []).map((criterion) => [
          criterion.id,
          criterion.text,
        ]),
      ),
    [spec.data],
  );
}

/** Réglages de l'Atelier. */
export function useWorkshopSettings() {
  return useQuery({
    queryKey: workshopSettingsKey,
    queryFn: workshopSettings,
  });
}

/**
 * Relit les requêtes de `queryKey` (préfixe). Une première lecture en cours
 * (sans données) est reprise par React Query, pas relancée : elle a pu
 * partir avant le changement annoncé, donc la relire encore à sa fin.
 */
function refresh(client: QueryClient, queryKey: QueryKey) {
  for (const query of client.getQueryCache().findAll({ queryKey })) {
    const { fetchStatus, data } = query.state;
    const running = query.promise;
    if (fetchStatus !== "fetching" || data !== undefined || !running) {
      continue;
    }
    const again = () =>
      void client.invalidateQueries({
        queryKey: query.queryKey,
        exact: true,
      });
    running.then(again, again);
  }
  void client.invalidateQueries({ queryKey });
}

/** Ce qu'un avis du moteur fait relire. */
export function applyNotice(client: QueryClient, notice: WorkshopNotice) {
  const id = notice.projectId;
  announce(client, notice);
  refresh(client, workshopListKey);
  if (notice.deleted) {
    // Projet montré : relu, il dit qu'il n'existe plus ; sinon oublié.
    void client.resetQueries({ queryKey: workshopProjectKey(id) });
    client.removeQueries({ queryKey: workshopEventsKey(id) });
    client.removeQueries({ queryKey: workshopPartsKey(id) });
    client.removeQueries({ queryKey: workshopSpecKey(id) });
    client.removeQueries({ queryKey: workshopPartPrefix(id) });
    return;
  }
  refresh(client, workshopProjectKey(id));
  if (notice.lastSeq !== null || notice.state !== null) {
    refresh(client, workshopEventsKey(id));
    refresh(client, workshopPartsKey(id));
    refresh(client, workshopPartPrefix(id));
    refresh(client, workshopSpecKey(id));
  }
}

/**
 * Suit « workshop-event » tant que le composant est monté (pages de
 * l'Atelier, carte de l'outil) : relit ce qui a changé.
 */
export function useWorkshopNotices() {
  const client = useQueryClient();
  useEffect(() => {
    let active = true;
    let stop: (() => void) | null = null;
    listenWorkshop((notice) => applyNotice(client, notice)).then(
      (unlisten) => {
        if (active) stop = unlisten;
        else unlisten();
      },
      (error: unknown) =>
        console.error("Failed to listen to the workshop:", error),
    );
    return () => {
      active = false;
      stop?.();
    };
  }, [client]);
}
