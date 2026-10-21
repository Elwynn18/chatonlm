/*
 * SPDX-License-Identifier: MIT
 * SPDX-FileCopyrightText: Ollama
 * SPDX-FileCopyrightText: 2026 Elwynn18 and ChatonLM contributors
 *
 * Adapted from Ollama (https://github.com/ollama/ollama),
 * file app/ui/app/src/components/Chat.tsx (upgrade address by plan),
 * commit 42e911bc3d05.
 * Modified for ChatonLM: a pure function, with the kinds of the cloud errors
 * of ChatonLM's backend.
 */

// Erreurs des modèles cloud d'Ollama (voir CloudErrorActions.tsx).

import type { ChatErrorKind } from "@/bindings/ChatErrorKind";

/** Erreurs propres aux modèles cloud. */
export type CloudErrorKind = Extract<
  ChatErrorKind,
  "cloudOff" | "cloudSignedOut" | "cloudLimit"
>;

export function isCloudError(
  kind: ChatErrorKind | undefined,
): kind is CloudErrorKind {
  return (
    kind === "cloudOff" || kind === "cloudSignedOut" || kind === "cloudLimit"
  );
}

/**
 * Page d'ollama.com pour passer à un forfait supérieur, comme l'officiel
 * (Chat.tsx) : la facturation pour un compte Pro, sinon la page « upgrade ».
 */
export function upgradeUrl(plan: string | undefined): string {
  return plan === "pro"
    ? "https://ollama.com/settings/billing"
    : "https://ollama.com/upgrade";
}
