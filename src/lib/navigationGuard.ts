/*
 * SPDX-License-Identifier: MIT
 * SPDX-FileCopyrightText: Ollama
 * SPDX-FileCopyrightText: 2026 Elwynn18 and ChatonLM contributors
 *
 * Adapted from Ollama (https://github.com/ollama/ollama),
 * files app/ui/app/index.html and app/cmd/app/webview.go, commit 42e911bc3d05.
 * Modified for ChatonLM: as functions tested apart and wired in the root
 * route; Ctrl+R blocked with or without Shift; editable fields found with
 * closest() (a click inside a contenteditable child counts); the back and
 * forward mouse buttons also blocked on mouseup and auxclick; no popstate
 * rewrite (the router listens to it).
 */

const EDITABLE =
  "input, textarea, select, [contenteditable]:not([contenteditable='false'])";

function inEditable(target: EventTarget | null) {
  return target instanceof Element && target.closest(EDITABLE) !== null;
}

/**
 * Touche qui rechargerait la page ou naviguerait dans l'historique : F5 et
 * Ctrl+R (avec ou sans Maj) partout ; hors champ éditable, Retour arrière et
 * Ctrl, Méta ou Alt avec ← ou →.
 */
export function shouldBlockKey(event: KeyboardEvent): boolean {
  if (event.key === "F5") return true;
  if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "r") {
    return true;
  }
  if (inEditable(event.target)) return false;
  if (event.key === "Backspace") return true;
  return (
    (event.ctrlKey || event.metaKey || event.altKey) &&
    (event.key === "ArrowLeft" || event.key === "ArrowRight")
  );
}

/** Boutons « précédent » (3) et « suivant » (4) de la souris. */
export function shouldBlockMouse(event: MouseEvent): boolean {
  return event.button === 3 || event.button === 4;
}
