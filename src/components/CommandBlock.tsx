import { useEffect, useRef, useState } from "react";
import { useT } from "@/i18n";
import { copyText } from "@/lib/clipboard";

/**
 * Commande de terminal à copier. « Copied » s'affiche 2 s ; si la copie
 * échoue, « Copy failed » s'affiche et la commande est sélectionnée pour une
 * copie à la main. Plusieurs lignes (Settings > Model location) : gardées
 * telles quelles, le bouton en haut.
 */
export function CommandBlock({ command }: { command: string }) {
  const t = useT();
  const multiline = command.includes("\n");
  const [state, setState] = useState<"idle" | "copied" | "failed">("idle");
  const codeRef = useRef<HTMLElement>(null);

  useEffect(() => {
    if (state === "idle") return;
    const timer = setTimeout(() => setState("idle"), 2000);
    return () => clearTimeout(timer);
  }, [state]);

  const copy = async () => {
    try {
      await copyText(command);
      setState("copied");
    } catch {
      setState("failed");
      const code = codeRef.current;
      if (code) window.getSelection()?.selectAllChildren(code);
    }
  };

  return (
    <div
      className={`flex ${multiline ? "items-start" : "items-center"} gap-3 rounded-xl bg-neutral-100 py-2 pr-2 pl-4 text-left dark:bg-neutral-800`}
    >
      <code
        ref={codeRef}
        className={`min-w-0 flex-1 overflow-x-auto font-mono text-[13px] ${multiline ? "whitespace-pre" : "whitespace-nowrap"} text-neutral-800 select-text dark:text-neutral-200`}
      >
        {command}
      </code>
      <button
        type="button"
        onClick={copy}
        aria-label={t("chat.copy.command")}
        title={t("chat.copy.command")}
        className="shrink-0 rounded-lg px-2.5 py-1 text-xs font-medium text-neutral-600 outline-none hover:bg-neutral-200 focus-visible:ring-2 focus-visible:ring-accent-fg dark:text-neutral-300 dark:hover:bg-neutral-700"
      >
        {state === "copied"
          ? t("common.actions.copied")
          : state === "failed"
            ? t("chat.copy.failed")
            : t("common.actions.copy")}
      </button>
      {/* Annonce hors du bouton : les enfants d'un bouton sont présentationnels
          (ARIA), une zone live placée dedans risque de ne pas être lue. */}
      <span className="sr-only" role="status">
        {state === "copied"
          ? t("common.actions.copied")
          : state === "failed"
            ? t("chat.copy.failed")
            : ""}
      </span>
    </div>
  );
}
