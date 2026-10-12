/*
 * SPDX-License-Identifier: MIT
 * SPDX-FileCopyrightText: Ollama
 * SPDX-FileCopyrightText: 2026 Elwynn18 and ChatonLM contributors
 *
 * Adapted from Ollama (https://github.com/ollama/ollama),
 * file app/ui/app/src/components/ui/slider.tsx, commit 42e911bc3d05.
 * Modified for ChatonLM: rewritten, same look (rail, evenly spread steps in
 * equal-width columns, the rail from the first to the last step, knob
 * on the chosen step, labels below, faded when disabled); one onChange per
 * gesture and only when the value changes (the official click calls it twice,
 * once with the old value); pointer events with pointer capture (mouse, touch,
 * pen) instead of mouse events, cancelled without onChange on pointercancel or
 * lost capture; keyboard (arrows, Home, End, PageUp, PageDown) and slider ARIA
 * on the focusable knob; no effect copying the value prop (local state only
 * while dragging); a value matching no step shows no knob (the step row then
 * carries the slider role); the knob stays visible when disabled.
 */

import {
  type KeyboardEvent,
  type PointerEvent,
  type FocusEvent,
  useLayoutEffect,
  useRef,
  useState,
} from "react";

/**
 * Largeur de chaque colonne (w-12) : toutes égales, pour que les crans soient
 * régulièrement espacés quelle que soit la longueur de leur étiquette
 * (« Default » est plus large que « 4k »).
 */
const COLUMN_PX = 48;

export interface SliderOption {
  value: number;
  label: string;
}

export interface SliderProps {
  /** Crans, dans l'ordre croissant de leur valeur. */
  options: SliderOption[];
  value: number;
  /** Appelé une fois par geste, seulement si la valeur change. */
  onChange: (value: number) => void;
  disabled?: boolean;
  labelledBy?: string;
  describedBy?: string;
}

/** Glisser en cours : pointeur capturé et cran affiché. */
interface Drag {
  pointerId: number;
  index: number;
}

export function Slider({
  options,
  value,
  onChange,
  disabled = false,
  labelledBy,
  describedBy,
}: SliderProps) {
  // Zone des crans : sert au calcul du cran le plus proche et porte les
  // gestionnaires de pointeur et de clavier.
  const trackRef = useRef<HTMLDivElement>(null);
  // Pastille, absente si aucune option ne correspond à la valeur : la zone des
  // crans porte alors le rôle slider.
  const knobRef = useRef<HTMLDivElement>(null);
  // Le glisser est aussi gardé dans une ref : un relâchement peut arriver
  // avant le rendu du dernier déplacement.
  const dragRef = useRef<Drag | null>(null);
  const [dragIndex, setDragIndex] = useState<number | null>(null);
  // Le curseur a le focus (pastille ou zone des crans).
  const focusedRef = useRef(false);

  const valueIndex = options.findIndex((option) => option.value === value);
  // Pendant un glisser, le cran sous le pointeur ; sinon celui de la valeur.
  const shownIndex = dragIndex ?? valueIndex;
  const onStep = shownIndex >= 0;

  // Quand la pastille change de cran, l'élément focalisé est démonté et le
  // focus retombe sur le document : il est rendu au nouvel élément (le blur
  // d'un départ volontaire a remis focusedRef à faux).
  useLayoutEffect(() => {
    if (!focusedRef.current) return;
    const target = knobRef.current ?? trackRef.current;
    if (target && document.activeElement !== target) {
      target.focus({ preventScroll: true });
    }
  });

  if (options.length === 0) return null;

  /** Cran le plus proche d'une abscisse, d'après la zone des crans. */
  function closestIndex(clientX: number): number | null {
    const track = trackRef.current;
    if (!track) return null;
    if (options.length === 1) return 0;
    const rect = track.getBoundingClientRect();
    if (!(rect.width > COLUMN_PX)) return null;
    // Colonnes de même largeur : centres réguliers, du premier au dernier.
    const segment = (rect.width - COLUMN_PX) / (options.length - 1);
    const index = Math.round((clientX - rect.left - COLUMN_PX / 2) / segment);
    return Math.max(0, Math.min(index, options.length - 1));
  }

  function endDrag() {
    dragRef.current = null;
    setDragIndex(null);
  }

  function handlePointerDown(event: PointerEvent<HTMLDivElement>) {
    if (disabled || event.button !== 0 || dragRef.current) return;
    const index = closestIndex(event.clientX);
    if (index === null) return;
    event.preventDefault();
    event.currentTarget.setPointerCapture(event.pointerId);
    dragRef.current = { pointerId: event.pointerId, index };
    setDragIndex(index);
    (knobRef.current ?? event.currentTarget).focus({ preventScroll: true });
  }

  function handlePointerMove(event: PointerEvent<HTMLDivElement>) {
    const drag = dragRef.current;
    if (!drag || drag.pointerId !== event.pointerId) return;
    const index = closestIndex(event.clientX);
    if (index === null || index === drag.index) return;
    dragRef.current = { ...drag, index };
    setDragIndex(index);
  }

  function handlePointerUp(event: PointerEvent<HTMLDivElement>) {
    const drag = dragRef.current;
    if (!drag || drag.pointerId !== event.pointerId) return;
    endDrag();
    const next = options[drag.index]?.value;
    if (!disabled && next !== undefined && next !== value) onChange(next);
  }

  // pointercancel ou perte de la capture : retour à la valeur, sans onChange.
  // (Après un relâchement, la perte de capture ne trouve plus de glisser.)
  function handlePointerCancel(event: PointerEvent<HTMLDivElement>) {
    const drag = dragRef.current;
    if (!drag || drag.pointerId !== event.pointerId) return;
    endDrag();
  }

  /** Cran visé par une touche : undefined si la touche n'est pas gérée. */
  function keyIndex(key: string): number | undefined {
    const last = options.length - 1;
    switch (key) {
      case "ArrowLeft":
      case "ArrowDown":
      case "PageDown":
        if (valueIndex >= 0) return Math.max(0, valueIndex - 1);
        // Hors crans : le dernier cran inférieur à la valeur.
        return options.findLastIndex((option) => option.value < value);
      case "ArrowRight":
      case "ArrowUp":
      case "PageUp":
        if (valueIndex >= 0) return Math.min(last, valueIndex + 1);
        // Hors crans : le premier cran supérieur à la valeur.
        return options.findIndex((option) => option.value > value);
      case "Home":
        return 0;
      case "End":
        return last;
      default:
        return undefined;
    }
  }

  function handleKeyDown(event: KeyboardEvent<HTMLDivElement>) {
    if (disabled || event.altKey || event.ctrlKey || event.metaKey) return;
    const index = keyIndex(event.key);
    if (index === undefined) return;
    event.preventDefault();
    const next = options[index]?.value;
    if (next !== undefined && next !== value) onChange(next);
  }

  function handleFocus() {
    focusedRef.current = true;
  }

  function handleBlur(event: FocusEvent<HTMLDivElement>) {
    if (!event.currentTarget.contains(event.relatedTarget)) {
      focusedRef.current = false;
    }
  }

  const sliderProps = {
    role: "slider",
    tabIndex: disabled ? undefined : 0,
    "aria-valuemin": 0,
    "aria-valuemax": options.length - 1,
    "aria-valuenow": onStep ? shownIndex : undefined,
    "aria-valuetext": onStep ? options[shownIndex]?.label : String(value),
    "aria-labelledby": labelledBy,
    "aria-describedby": describedBy,
    "aria-disabled": disabled,
  } as const;

  return (
    <div className={disabled ? "opacity-50" : undefined}>
      <div className="relative">
        <div className="pointer-events-none absolute top-[9px] right-6 left-6 h-1 rounded-full bg-neutral-300 dark:bg-neutral-600" />
        <div
          ref={trackRef}
          className={`flex touch-none justify-between rounded-md outline-none focus-visible:ring-2 focus-visible:ring-accent-fg ${
            disabled ? "cursor-not-allowed" : "cursor-pointer"
          }`}
          onPointerDown={handlePointerDown}
          onPointerMove={handlePointerMove}
          onPointerUp={handlePointerUp}
          onPointerCancel={handlePointerCancel}
          onLostPointerCapture={handlePointerCancel}
          onKeyDown={handleKeyDown}
          onFocus={handleFocus}
          onBlur={handleBlur}
          {...(onStep ? {} : sliderProps)}
        >
          {options.map((option, index) => (
            <div key={option.value} className="flex w-12 flex-col items-center">
              <div className="relative z-10 flex h-5 w-5 items-center justify-center">
                {index === shownIndex && (
                  <div
                    ref={knobRef}
                    {...sliderProps}
                    className={`h-4 w-4 rounded-full border border-neutral-400 bg-white outline-none focus-visible:ring-2 focus-visible:ring-accent-fg focus-visible:ring-offset-2 dark:border-neutral-500 dark:focus-visible:ring-offset-neutral-800 ${
                      disabled ? "" : "cursor-grab active:cursor-grabbing"
                    }`}
                  />
                )}
              </div>
              <div className="text-xs whitespace-nowrap text-neutral-500 dark:text-neutral-400">
                {option.label}
              </div>
            </div>
          ))}
        </div>
      </div>
    </div>
  );
}
