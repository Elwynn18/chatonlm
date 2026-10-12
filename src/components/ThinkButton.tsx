/*
 * SPDX-License-Identifier: MIT
 * SPDX-FileCopyrightText: Ollama
 * SPDX-FileCopyrightText: 2026 Elwynn18 and ChatonLM contributors
 *
 * Adapted from Ollama (https://github.com/ollama/ollama),
 * file app/ui/app/src/components/ThinkButton.tsx, commit 42e911bc3d05.
 * Modified for ChatonLM: shown for any model that can think, from the values
 * Ollama allows (src/lib/think.ts): toggle, levels in Ollama's order with
 * "Off" and "On" when allowed (grey when off), and no button for a model
 * that always thinks (Ollama says so, or it thought with think off);
 * Heroicons instead of SF Symbols, the light
 * bulb of the pill keeping the official spacing; the toggle is a
 * labelled toggle button (aria-pressed); the level menu is a real menu
 * (arrows, Home/End, Escape that stops nothing else, focus given back) that
 * closes when the focus leaves it or the window loses it; texts translated
 * into English, Spanish and French (src/i18n).
 */

import { ChevronDownIcon, LightBulbIcon } from "@heroicons/react/24/outline";
import {
  type KeyboardEvent,
  useCallback,
  useEffect,
  useId,
  useRef,
  useState,
} from "react";
import type { Think } from "@/bindings/Think";
import { useT } from "@/i18n";
import { type ThinkControl, thinkLabel } from "@/lib/think";

const BUTTON =
  "flex items-center justify-center rounded-full border border-transparent whitespace-nowrap transition-all select-none focus:outline-2 focus:outline-offset-2 focus:outline-accent-fg";
// Actif : aplat roux, texte encre (dans les deux thèmes) ; sinon pastille sable.
const ACTIVE = "bg-accent text-neutral-900";
const INACTIVE =
  "bg-neutral-200 text-neutral-500 dark:bg-neutral-700 dark:text-neutral-400";

export function ThinkButton({
  control,
  onChange,
  onDropdownToggle,
}: {
  control: Exclude<ThinkControl, { kind: "none" }>;
  onChange: (value: Think) => void;
  /** Menu des niveaux ouvert ou fermé par son bouton. */
  onDropdownToggle?: (isOpen: boolean) => void;
}) {
  const t = useT();
  if (control.kind === "levels") {
    return (
      <LevelMenu
        value={control.value}
        options={control.options}
        onChange={onChange}
        onDropdownToggle={onDropdownToggle}
      />
    );
  }

  if (control.kind === "fixed") {
    // Raisonne toujours : rien à régler, pas de bouton (son raisonnement
    // s'affiche comme d'habitude).
    return null;
  }

  const active = control.value;
  return (
    <button
      type="button"
      aria-label={t("chat.think.label")}
      aria-pressed={active}
      title={t(active ? "chat.think.disable" : "chat.think.enable")}
      onClick={() => onChange(!active)}
      className={`${BUTTON} h-9 w-9 cursor-pointer ${active ? ACTIVE : INACTIVE}`}
    >
      <LightBulbIcon className="h-5 w-5" />
    </button>
  );
}

function LevelMenu({
  value,
  options,
  onChange,
  onDropdownToggle,
}: {
  value: Think;
  options: { value: Think; label: string }[];
  onChange: (value: Think) => void;
  onDropdownToggle?: (isOpen: boolean) => void;
}) {
  const [isOpen, setIsOpen] = useState(false);
  const wrapperRef = useRef<HTMLDivElement>(null);
  const buttonRef = useRef<HTMLButtonElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  const menuId = useId();
  const t = useT();
  const label = thinkLabel(t, value);

  /** Ferme ; le focus revient au bouton s'il était dans le menu. */
  const close = useCallback((giveFocusBack: boolean) => {
    if (giveFocusBack && menuRef.current?.contains(document.activeElement)) {
      buttonRef.current?.focus();
    }
    setIsOpen(false);
  }, []);

  const toggle = () => {
    const next = !isOpen;
    setIsOpen(next);
    onDropdownToggle?.(next);
  };

  // Ouverture : focus sur le niveau courant, les flèches marchent aussitôt.
  useEffect(() => {
    if (!isOpen) return;
    const items = menuItems(menuRef.current);
    (
      items.find((item) => item.getAttribute("aria-checked") === "true") ??
      items[0]
    )?.focus();
  }, [isOpen]);

  // Clic ailleurs, Échap (consommée : elle n'arrête pas la réponse), fenêtre
  // qui perd le focus.
  useEffect(() => {
    if (!isOpen) return;
    const onMouseDown = (event: MouseEvent) => {
      if (!wrapperRef.current?.contains(event.target as Node)) close(false);
    };
    const onKeyDown = (event: globalThis.KeyboardEvent) => {
      if (event.key !== "Escape") return;
      event.preventDefault();
      buttonRef.current?.focus();
      close(false);
    };
    const onBlur = () => close(false);
    document.addEventListener("mousedown", onMouseDown);
    document.addEventListener("keydown", onKeyDown);
    window.addEventListener("blur", onBlur);
    return () => {
      document.removeEventListener("mousedown", onMouseDown);
      document.removeEventListener("keydown", onKeyDown);
      window.removeEventListener("blur", onBlur);
    };
  }, [isOpen, close]);

  const onMenuKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    const items = menuItems(menuRef.current);
    const index = items.indexOf(document.activeElement as HTMLButtonElement);
    let next: number;
    switch (event.key) {
      case "ArrowDown":
        next = (index + 1) % items.length;
        break;
      case "ArrowUp":
        next = (index - 1 + items.length) % items.length;
        break;
      case "Home":
        next = 0;
        break;
      case "End":
        next = items.length - 1;
        break;
      case "Tab":
        // Le Tab part du bouton : il mène au contrôle suivant ou précédent.
        close(true);
        return;
      default:
        return;
    }
    event.preventDefault();
    items[next]?.focus();
  };

  return (
    <div
      ref={wrapperRef}
      className="relative"
      onBlur={(event) => {
        // Focus parti ailleurs (Tab...) ; pas quand il part dans le vide
        // (clic sur un élément qui ne le prend pas : le clic ailleurs s'en
        // charge).
        const to = event.relatedTarget;
        if (isOpen && to && !wrapperRef.current?.contains(to)) close(false);
      }}
    >
      <button
        ref={buttonRef}
        type="button"
        aria-label={t("chat.think.level", { level: label })}
        aria-haspopup="menu"
        aria-expanded={isOpen}
        aria-controls={isOpen ? menuId : undefined}
        title={t("chat.think.level", { level: label })}
        onClick={toggle}
        onKeyDown={(event) => {
          if (
            !isOpen &&
            (event.key === "ArrowDown" || event.key === "ArrowUp")
          ) {
            event.preventDefault();
            toggle();
          }
        }}
        className={`${BUTTON} h-9 cursor-pointer gap-1 px-3 ${value === false ? INACTIVE : ACTIVE}`}
      >
        <div className="flex items-center justify-center space-x-2">
          {/* Le dessin n'occupe qu'environ 12,5 px de la boîte de 20 px :
              -mx-1 lui rend la largeur du svg officiel (w-3), donc les écarts
              officiels avec le bord et le libellé. */}
          <LightBulbIcon className="-mx-1 h-5 w-5 flex-none" />
          <span className="text-sm">{label}</span>
        </div>
        <ChevronDownIcon className="h-3 w-3" strokeWidth={2} />
      </button>

      {isOpen && (
        <div
          ref={menuRef}
          id={menuId}
          role="menu"
          aria-label={t("chat.think.levelMenu")}
          onKeyDown={onMenuKeyDown}
          // Le clic ne déplace pas le focus : le menu ne se ferme pas avant.
          onMouseDown={(event) => event.preventDefault()}
          className="absolute bottom-full mb-2 min-w-[120px] overflow-hidden rounded-2xl border border-neutral-100 bg-white text-[15px] text-neutral-800 shadow-xl shadow-black/5 backdrop-blur-lg dark:border-neutral-600/40 dark:bg-neutral-800 dark:text-white dark:ring-black/20"
        >
          {options.map((option) => (
            <button
              key={`${typeof option.value}:${option.value}`}
              type="button"
              role="menuitemradio"
              aria-checked={option.value === value}
              tabIndex={-1}
              onClick={() => {
                onChange(option.value);
                close(true);
              }}
              className={`w-full cursor-pointer px-3 py-2 text-left text-neutral-700 transition-colors hover:bg-neutral-100 focus:outline-none focus-visible:ring-2 focus-visible:ring-accent-fg focus-visible:ring-inset dark:text-neutral-300 dark:hover:bg-neutral-700 ${
                option.value === value
                  ? "bg-neutral-100 dark:bg-neutral-700/60"
                  : ""
              }`}
            >
              {option.label}
            </button>
          ))}
        </div>
      )}
    </div>
  );
}

function menuItems(menu: HTMLElement | null): HTMLButtonElement[] {
  return [
    ...(menu?.querySelectorAll<HTMLButtonElement>("[role=menuitemradio]") ??
      []),
  ];
}
