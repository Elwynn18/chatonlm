/*
 * SPDX-License-Identifier: MIT
 * SPDX-FileCopyrightText: Ollama
 * SPDX-FileCopyrightText: 2026 Elwynn18 and ChatonLM contributors
 *
 * Copied from Ollama (https://github.com/ollama/ollama),
 * file app/ui/app/src/lib/keyboard.ts, commit 42e911bc3d05.
 */

type KeyboardTarget = EventTarget & {
  tagName?: string;
  isContentEditable?: boolean;
};

function isEditableTarget(target: EventTarget | null) {
  if (!target || typeof target !== "object") {
    return false;
  }

  const { tagName, isContentEditable } = target as KeyboardTarget;
  return (
    isContentEditable === true || tagName === "INPUT" || tagName === "TEXTAREA"
  );
}

export function preventPageSelectAll(event: KeyboardEvent) {
  const isSelectAll =
    (event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "a";

  if (isSelectAll && !isEditableTarget(event.target)) {
    event.preventDefault();
  }
}
