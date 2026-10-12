/*
 * SPDX-License-Identifier: MIT
 * SPDX-FileCopyrightText: Ollama
 * SPDX-FileCopyrightText: 2026 Elwynn18 and ChatonLM contributors
 *
 * Adapted from Ollama (https://github.com/ollama/ollama),
 * file app/ui/app/index.html, commit 42e911bc3d05.
 * Modified for ChatonLM: as a function tested apart and wired in the root
 * route; editable fields keep the menu anywhere (to paste an address or a
 * title); no custom items passed to a native menu (the app draws its own).
 */

const EDITABLE =
  "input, textarea, [contenteditable]:not([contenteditable='false'])";

/**
 * Le menu contextuel de WebKit (copier, coller...) n'est permis que dans les
 * zones marquées `allow-context-menu` (messages, zone de saisie, historique)
 * et dans les champs éditables ; ailleurs, aucun menu.
 */
export function allowsNativeMenu(target: EventTarget | null): boolean {
  return (
    target instanceof Element &&
    target.closest(`.allow-context-menu, ${EDITABLE}`) !== null
  );
}
