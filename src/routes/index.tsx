/*
 * SPDX-License-Identifier: MIT
 * SPDX-FileCopyrightText: Ollama
 * SPDX-FileCopyrightText: 2026 Elwynn18 and ChatonLM contributors
 *
 * Adapted from Ollama (https://github.com/ollama/ollama),
 * file app/ui/app/src/routes/index.tsx, commit 42e911bc3d05.
 * Modified for ChatonLM: no onboarding, always opens a new chat.
 */

import { createFileRoute, redirect } from "@tanstack/react-router";

export const Route = createFileRoute("/")({
  beforeLoad: () => {
    throw redirect({
      to: "/c/$chatId",
      params: { chatId: "new" },
      mask: { to: "/" },
    });
  },
});
