/*
 * SPDX-License-Identifier: MIT
 * SPDX-FileCopyrightText: Ollama
 * SPDX-FileCopyrightText: 2026 Elwynn18 and ChatonLM contributors
 *
 * Adapted from Ollama (https://github.com/ollama/ollama),
 * file app/ui/app/src/components/Thinking.tsx, commit 42e911bc3d05.
 * Modified for ChatonLM: named export; Heroicons light bulb and chevron instead
 * of the SF Symbols and Feather glyphs; thinking without an end time is only
 * "Thinking..." while the reply is being written: a finished reply (stopped
 * while thinking, or saved without timestamps) is shown as finished and
 * folded, never stuck on "Thinking..." with an open preview; the collapsed
 * preview shows the end of the thinking by layout (reversed column) instead
 * of a translation measured after each chunk; no internal scrolling of the
 * expanded body (it has no height limit, the official scrolling had no
 * effect); the header is a real button inside the heading (aria-expanded,
 * aria-controls, keyboard, blue focus ring as in the chat form, chevron shown
 * on keyboard focus too); the folded body is hidden from keyboard and screen
 * readers once folded, and marked (data-folded) so that the message list
 * counts it at its final height while it folds; title translated into
 * English, Spanish and French (src/i18n).
 */

import { ChevronDownIcon, LightBulbIcon } from "@heroicons/react/24/outline";
import { useEffect, useId, useRef, useState } from "react";
import { StreamingMarkdownContent } from "@/components/StreamingMarkdownContent";
import { type TFunction, useT } from "@/i18n";
import { formatDecimal } from "@/i18n/format";

/**
 * Titre du bloc. Sans horodatages, le raisonnement est fini mais sa durée
 * inconnue : texte officiel le plus vague plutôt qu'un « Thinking... » figé.
 */
function thinkingTitle(t: TFunction, startTime?: Date, endTime?: Date) {
  if (startTime && !endTime) return t("chat.thinking.active");
  if (!startTime || !endTime) return t("chat.thinking.moment");
  const thinkingTime = (endTime.getTime() - startTime.getTime()) / 1000;
  return thinkingTime < 2
    ? t("chat.thinking.moment")
    : t("chat.thinking.seconds", {
        seconds: formatDecimal(t.lang, thinkingTime, 1),
      });
}

export function Thinking({
  thinking,
  startTime,
  endTime,
}: {
  thinking: string;
  startTime?: Date;
  endTime?: Date;
}) {
  const t = useT();
  // Seul l'utilisateur déplie le bloc. Sans clic, il reste replié : aperçu
  // pendant le raisonnement, fermé à la fin (repli automatique) ; déplié
  // pendant le raisonnement, il le reste à la fin (comme l'officiel, dont le
  // repli automatique ne touche jamais un bloc déplié par un clic).
  const [isExpanded, setIsExpanded] = useState(false);
  const [hasOverflow, setHasOverflow] = useState(false);
  const wrapperRef = useRef<HTMLDivElement>(null);
  const contentRef = useRef<HTMLDivElement>(null);
  const bodyId = useId();

  const activelyThinking = startTime !== undefined && endTime === undefined;
  const isCollapsed = !isExpanded;
  // Replié et fini : hauteur et opacité à 0 (300 ms), puis masqué.
  const isFolded = isCollapsed && !activelyThinking;

  // Le contenu dépasse-t-il de l'aperçu ? (dégradé en haut). Mesuré quand le
  // texte ou la zone change de taille, pas à chaque fragment.
  useEffect(() => {
    const wrapper = wrapperRef.current;
    const content = contentRef.current;
    if (!wrapper || !content || typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver(() => {
      setHasOverflow(content.offsetHeight > wrapper.clientHeight + 1);
    });
    observer.observe(wrapper);
    observer.observe(content);
    return () => observer.disconnect();
  }, []);

  return (
    <div
      className={`mb-4 flex w-full flex-col ${activelyThinking || !isCollapsed ? "text-neutral-800 dark:text-neutral-200" : "text-neutral-600 dark:text-neutral-400"} transition-colors hover:text-neutral-800 dark:hover:text-neutral-200`}
    >
      <h3 className="self-start">
        <button
          type="button"
          className="group/thinking relative flex cursor-pointer items-center rounded-md text-left outline-none select-text focus-visible:ring-2 focus-visible:ring-accent-fg"
          aria-expanded={!isCollapsed}
          aria-controls={bodyId}
          onClick={() => setIsExpanded((expanded) => !expanded)}
        >
          {/* Light bulb */}
          <LightBulbIcon
            className={`absolute top-1/2 -left-1 h-5 w-5 -translate-y-1/2 transition-opacity ${
              isCollapsed ? "opacity-100" : "opacity-0"
            } group-hover/thinking:opacity-0 group-focus-visible/thinking:opacity-0`}
          />
          {/* Arrow */}
          <ChevronDownIcon
            strokeWidth={2}
            className={`absolute h-4 w-4 transition-all ${
              isCollapsed
                ? "-rotate-90 opacity-0 group-hover/thinking:opacity-100 group-focus-visible/thinking:opacity-100"
                : "rotate-0 opacity-100"
            } will-change-[opacity,transform]`}
          />
          <span className="ml-6 select-text">
            {thinkingTitle(t, startTime, endTime)}
          </span>
        </button>
      </h3>
      {/* Colonne inversée : le contenu part du bas de la zone et déborde par
          le haut, l'aperçu montre donc toujours la fin du raisonnement. */}
      <div
        id={bodyId}
        ref={wrapperRef}
        className={`relative mt-2 ml-6 flex flex-col-reverse rounded-md text-xs text-neutral-500 transition-[max-height,opacity,visibility] duration-300 ease-in-out dark:text-neutral-500 ${
          isCollapsed ? "overflow-hidden" : "overflow-y-auto"
        }`}
        style={{
          maxHeight: isCollapsed ? (isFolded ? "0px" : "12rem") : undefined,
          opacity: isFolded ? 0 : 1,
          // Masqué à la fin du repli (visibility suit la transition) : ni
          // tabulation ni lecteur d'écran dans un bloc fermé.
          visibility: isFolded ? "hidden" : undefined,
        }}
        // Repli en cours ou fait : la liste des messages compte ce corps à sa
        // hauteur finale (nulle) pendant l'animation (useMessageAutoscroll).
        data-folded={isFolded ? "" : undefined}
      >
        <div ref={contentRef} className="shrink-0 opacity-75 select-text">
          <StreamingMarkdownContent
            content={thinking}
            isStreaming={activelyThinking}
            size="sm"
          />
        </div>

        {/* Gradient overlay for fade effect when collapsed and scrolled */}
        {isCollapsed && hasOverflow && (
          <div
            data-testid="thinking-fade"
            className="pointer-events-none absolute inset-x-0 -top-1 h-8 bg-gradient-to-b from-white to-transparent dark:from-neutral-900"
          />
        )}
      </div>
    </div>
  );
}
