/*
 * SPDX-License-Identifier: MIT
 * SPDX-FileCopyrightText: Ollama
 * SPDX-FileCopyrightText: 2026 Elwynn18 and ChatonLM contributors
 *
 * Adapted from Ollama (https://github.com/ollama/ollama),
 * file app/ui/app/src/components/CopyButton.tsx, commit 42e911bc3d05.
 * Modified for ChatonLM: named export, cleaned HTML copy (one code block, real
 * bold, no classes), visible "Copy failed" state, status announced to screen
 * readers, timer cleared on unmount; texts translated into English, Spanish
 * and French (src/i18n); clipboard reached through the host layer
 * (src/lib/host.ts), shared with the VS Code extension panel.
 */

import { CheckIcon, XMarkIcon } from "@heroicons/react/20/solid";
import { Square2StackIcon } from "@heroicons/react/24/outline";
import { type RefObject, useEffect, useState } from "react";
import { useT } from "@/i18n";
import { cleanCopyHtml } from "@/lib/copyHtml";
import { useHost } from "@/lib/host";

interface CopyButtonProps {
  content: string;
  copyRef?: RefObject<HTMLElement | null>;
  size?: "sm" | "md";
  showLabels?: boolean;
  className?: string;
  title?: string;
  onCopy?: () => void;
}

/** Durée de « Copied » (et de « Copy failed »), comme l'officiel. */
const FEEDBACK_MS = 2000;

export function CopyButton({
  content,
  copyRef,
  size = "sm",
  showLabels = false,
  className = "",
  title = "",
  onCopy,
}: CopyButtonProps) {
  const t = useT();
  const host = useHost();
  const [state, setState] = useState<"idle" | "copied" | "failed">("idle");

  // Retour à « Copy » ; minuterie annulée si le bouton disparaît avant.
  useEffect(() => {
    if (state === "idle") return;
    const timer = setTimeout(() => setState("idle"), FEEDBACK_MS);
    return () => clearTimeout(timer);
  }, [state]);

  const handleCopy = () => {
    // Écriture lancée tout de suite, dans le clic : WebKitGTK la refuse
    // quelques secondes après le geste de l'utilisateur. Avec copyRef (une
    // réponse entière), HTML nettoyé et Markdown ; sinon le texte seul.
    const root = copyRef?.current;
    const copying = root
      ? host.copyRich(cleanCopyHtml(root), content)
      : host.copyText(content);
    copying.then(
      () => {
        setState("copied");
        onCopy?.();
      },
      () => setState("failed"),
    );
  };

  const iconSize = size === "sm" ? "h-3 w-3" : "h-7 w-7";
  const baseClasses =
    size === "sm"
      ? `text-xs px-4 py-2 z-10 cursor-pointer rounded-lg ${className}`
      : `${iconSize} px-1 py-0.5 text-xs cursor-pointer rounded-lg hover:bg-neutral-100 dark:hover:bg-neutral-800 flex items-center justify-center ${className}`;

  const icon =
    state === "copied" ? (
      <CheckIcon className={iconSize} />
    ) : state === "failed" ? (
      <XMarkIcon className={iconSize} />
    ) : (
      <Square2StackIcon className={iconSize} />
    );
  const label =
    state === "copied"
      ? t("common.actions.copied")
      : state === "failed"
        ? t("chat.copy.failed")
        : t("common.actions.copy");

  return (
    <>
      <button
        type="button"
        className={baseClasses}
        onClick={handleCopy}
        title={title}
        // Icône seule : nom accessible tiré de l'infobulle.
        aria-label={showLabels ? undefined : title || t("common.actions.copy")}
      >
        {showLabels ? (
          <span className="flex items-center gap-1">
            {icon}
            {label}
          </span>
        ) : (
          icon
        )}
      </button>
      {/* Hors du bouton : ses enfants ne sont pas annoncés (rôle button).
          Retirée du HTML copié par cleanCopyHtml. */}
      <span className="sr-only" role="status" data-copy-status="">
        {state === "idle" ? "" : label}
      </span>
    </>
  );
}
