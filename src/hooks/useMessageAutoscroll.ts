/*
 * SPDX-License-Identifier: MIT
 * SPDX-FileCopyrightText: Ollama
 * SPDX-FileCopyrightText: 2026 Elwynn18 and ChatonLM contributors
 *
 * Adapted from Ollama (https://github.com/ollama/ollama),
 * file app/ui/app/src/hooks/useMessageAutoscroll.ts, commit 42e911bc3d05.
 * Modified for ChatonLM: one view per chat (key) instead of a chatId reset;
 * jump to the bottom on opening moved here; the spacer only applies once a
 * message has been sent from this view, and is recomputed before paint; the
 * scroll to the sent message is scheduled once, never repeated or cancelled by
 * the renders that follow (only by unmounting); observers set up once,
 * message elements observed as they appear; a folding element (finished
 * thinking) is counted at its final height, both in the spacer and in the
 * scroll target.
 */

import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
} from "react";
import type { Message } from "@/bindings/Message";

/** Au-delà de 70 % de la hauteur visible, on montre la fin du message. */
const TALL_MESSAGE = 0.7;

/** Pendant une réponse, place réservée sous le message : 40 % de la hauteur. */
const STREAMING_ROOM = 0.4;

/** Anti-rebond des changements de taille (texte qui s'allonge...). */
const RESIZE_DEBOUNCE_MS = 100;

/** Attributs qui plient ou déplient un contenu (raisonnement, bloc de code). */
const TOGGLE_ATTRIBUTES = ["class", "style", "open", "data-expanded"];

/**
 * Élément replié (corps d'un raisonnement fini) : sa hauteur finale est nulle,
 * mais son repli dure 300 ms (max-height). Pendant ce temps, il est compté à sa
 * hauteur finale : sinon l'espaceur, calculé sur la hauteur du moment, laisse
 * le contenu raccourcir sous la position de défilement (le navigateur rogne
 * alors scrollTop et le message envoyé quitte le haut), et un défilement vers
 * un message situé sous un repli en cours dépasse sa place finale.
 */
const FOLDED = "[data-folded]";

/** Hauteur que l'élément va encore perdre par les replis en cours. */
function foldingHeight(element: HTMLElement) {
  let height = 0;
  for (const folded of element.querySelectorAll<HTMLElement>(FOLDED)) {
    height += folded.offsetHeight;
  }
  return height;
}

/** Hauteur de l'élément une fois les replis en cours terminés. */
function finalHeight(element: HTMLElement) {
  return element.offsetHeight - foldingHeight(element);
}

function messageElements(container: HTMLElement) {
  return [
    ...container.querySelectorAll<HTMLElement>("[data-message-index]"),
  ].map((element) => ({
    element,
    index: Number(element.dataset.messageIndex),
  }));
}

function lastUserIndex(messages: Message[]) {
  for (let i = messages.length - 1; i >= 0; i--) {
    if (messages[i].role === "user") return i;
  }
  return -1;
}

function messageElement(container: HTMLElement, index: number) {
  return container.querySelector<HTMLElement>(
    `[data-message-index="${index}"]`,
  );
}

/**
 * Défilement de la conversation, comme l'officiel :
 * - à l'ouverture, saut en bas ;
 * - à l'envoi, le message envoyé remonte en haut de la zone visible, et un
 *   espace en bas de liste rend ce défilement possible même si la réponse est
 *   encore courte ;
 * - pendant la réponse, aucun défilement automatique.
 * La vue est recréée pour chaque conversation (clé), ce qui remet tout à zéro.
 */
export function useMessageAutoscroll({
  messages,
  isStreaming,
}: {
  messages: Message[];
  isStreaming: boolean;
}) {
  const containerRef = useRef<HTMLElement>(null);
  const [spacerHeight, setSpacerHeight] = useState(0);
  // Un message a été envoyé depuis cette vue (isActiveInteraction officiel).
  const activeRef = useRef(false);
  const pendingScrollRef = useRef(false);
  const scrollFrameRef = useRef(0);

  // Opening a chat: jump to the bottom
  useLayoutEffect(() => {
    const container = containerRef.current;
    if (container) container.scrollTop = container.scrollHeight;
  }, []);

  const scrollToMessage = useCallback((messageIndex: number) => {
    const container = containerRef.current;
    if (!container || messageIndex < 0) return;
    // select the exact element by its data-message-index to avoid index mismatches
    const target = messageElement(container, messageIndex);
    if (!target) return;
    const containerHeight = container.clientHeight;
    const paddingTop = parseFloat(getComputedStyle(container).paddingTop) || 0;
    const maxScroll = container.scrollHeight - containerHeight;
    // Place finale du message : un raisonnement plus haut qui se replie
    // encore (réponse précédente tout juste finie) le fera remonter d'autant.
    const foldingAbove = messageElements(container)
      .filter((item) => item.index < messageIndex)
      .reduce((sum, item) => sum + foldingHeight(item.element), 0);
    // when the message is large scroll to the bottom of it
    const top =
      target.offsetHeight > containerHeight * TALL_MESSAGE
        ? maxScroll
        : target.offsetTop - foldingAbove - paddingTop;
    container.scrollTo?.({
      top: Math.min(Math.max(0, top), maxScroll),
      behavior: "smooth",
    });
  }, []);

  // Calculate and set the spacer height based on container dimensions
  const updateSpacerHeight = useCallback(() => {
    const container = containerRef.current;
    if (!container) return;
    const index = lastUserIndex(messages);
    const target = index < 0 ? null : messageElement(container, index);
    // Seulement après un envoi depuis cette vue : une conversation simplement
    // consultée n'a pas d'espace en plus
    if (!target || !activeRef.current) {
      setSpacerHeight(0);
      return;
    }
    // Messages suivants à leur hauteur finale (replis en cours terminés) :
    // l'espaceur couvre d'avance ce qu'un repli va retirer.
    const following = messageElements(container)
      .filter((item) => item.index > index)
      .reduce((sum, item) => sum + finalHeight(item.element), 0);
    const containerHeight = container.clientHeight;
    const base = Math.max(0, containerHeight - target.offsetHeight - following);
    // Add extra space for assistant response only when streaming
    const room = isStreaming ? containerHeight * STREAMING_ROOM : 0;
    setSpacerHeight(Math.round(base + room));
  }, [messages, isStreaming]);

  // Les observateurs, posés une fois, appellent toujours la dernière version.
  const updateRef = useRef(updateSpacerHeight);

  // Espace recalculé à chaque changement des messages, avant l'affichage : il
  // est en place avant tout défilement vers le message envoyé
  useLayoutEffect(() => {
    updateRef.current = updateSpacerHeight;
    updateSpacerHeight();
  }, [updateSpacerHeight]);

  // Défilement vers le message envoyé, deux images après son affichage (mise
  // en page stabilisée). Programmé une fois : les rendus de la réponse, toutes
  // les 16 ms, ne le répètent ni ne l'annulent ; seul le démontage l'annule.
  useLayoutEffect(() => {
    if (!pendingScrollRef.current) return;
    pendingScrollRef.current = false;
    const index = lastUserIndex(messages);
    if (index < 0) return;
    cancelAnimationFrame(scrollFrameRef.current);
    scrollFrameRef.current = requestAnimationFrame(() => {
      scrollFrameRef.current = requestAnimationFrame(() => {
        scrollFrameRef.current = 0;
        scrollToMessage(index);
      });
    });
  }, [messages, scrollToMessage]);

  useEffect(() => () => cancelAnimationFrame(scrollFrameRef.current), []);

  // Contenu qui bouge (fenêtre ou zone de saisie redimensionnée, raisonnement
  // replié, code coloré...) : tailles de la zone et de chaque message
  // (anti-rebond de 100 ms), attributs de pliage du sous-arbre (immédiat).
  // Posés une fois, avant l'effet qui suit.
  const resizeObserverRef = useRef<ResizeObserver | null>(null);
  const observedRef = useRef(new Set<Element>());
  useLayoutEffect(() => {
    const container = containerRef.current;
    if (!container) return;
    const observed = observedRef.current;
    let timer = 0;
    // Après un pliage, le changement de taille qui suit n'attend pas.
    let immediate = false;
    // L'officiel traite aussi comme immédiat un écart de plus de 50 px entre
    // contentRect et offsetHeight ; lus sur la même mise en page, ils ne
    // diffèrent que de la marge intérieure et de la bordure (nulles ici) :
    // test omis.
    const resizeObserver =
      typeof ResizeObserver === "undefined"
        ? null
        : new ResizeObserver(() => {
            window.clearTimeout(timer);
            if (immediate) {
              immediate = false;
              updateRef.current();
            } else {
              timer = window.setTimeout(
                () => updateRef.current(),
                RESIZE_DEBOUNCE_MS,
              );
            }
          });
    const mutationObserver = new MutationObserver((mutations) => {
      // L'espaceur change de hauteur (style) à chaque calcul : ignoré, sinon
      // chaque calcul en relancerait un autre et annulerait l'anti-rebond.
      const toggled = mutations.some(
        (mutation) =>
          mutation.type === "attributes" &&
          TOGGLE_ATTRIBUTES.includes(mutation.attributeName ?? "") &&
          !(
            mutation.target instanceof Element &&
            mutation.target.hasAttribute("data-spacer")
          ),
      );
      if (toggled) {
        immediate = true;
        updateRef.current();
      }
    });
    resizeObserver?.observe(container);
    mutationObserver.observe(container, {
      attributes: true,
      subtree: true,
      attributeFilter: TOGGLE_ATTRIBUTES,
    });
    resizeObserverRef.current = resizeObserver;
    return () => {
      window.clearTimeout(timer);
      resizeObserver?.disconnect();
      mutationObserver.disconnect();
      resizeObserverRef.current = null;
      observed.clear();
    };
  }, []);

  // Chaque message observé dès son apparition ; ceux qui ont disparu sont
  // relâchés
  useLayoutEffect(() => {
    const container = containerRef.current;
    const observer = resizeObserverRef.current;
    if (!container || !observer) return;
    const observed = observedRef.current;
    for (const element of observed) {
      if (!container.contains(element)) {
        observer.unobserve(element);
        observed.delete(element);
      }
    }
    for (const element of container.querySelectorAll("[data-message-index]")) {
      if (!observed.has(element)) {
        observer.observe(element);
        observed.add(element);
      }
    }
  }, [messages]);

  /** À appeler à l'envoi d'un message depuis cette vue. */
  const onMessageSent = useCallback(() => {
    activeRef.current = true;
    pendingScrollRef.current = true;
  }, []);

  return { containerRef, spacerHeight, onMessageSent };
}
