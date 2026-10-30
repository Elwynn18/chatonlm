// Zone de saisie du panneau : Entrée envoie, Maj+Entrée va à la ligne, le
// bouton devient Stop pendant une réponse ; actions rapides au-dessus
// (le texte de chacune est construit par l'hôte, avec le code de l'éditeur).
import { ArrowUpIcon, StopIcon } from "@heroicons/react/20/solid";
import { type FormEvent, type KeyboardEvent, useState } from "react";
import { useT } from "@/i18n";
import type { Notice, QuickAction } from "../shared/protocol";

const ACTIONS: readonly QuickAction[] = [
  "explain",
  "fix",
  "tests",
  "commitMessage",
];

function QuickActions({
  disabled,
  onAction,
}: {
  disabled: boolean;
  onAction: (action: QuickAction) => void;
}) {
  const t = useT();
  const label = (action: QuickAction) => {
    switch (action) {
      case "explain":
        return t("vscode.actions.explain");
      case "fix":
        return t("vscode.actions.fix");
      case "tests":
        return t("vscode.actions.tests");
      case "commitMessage":
        return t("vscode.actions.commitMessage");
    }
  };
  return (
    <div
      role="group"
      aria-label={t("vscode.actions.label")}
      className="flex flex-wrap gap-1.5 pb-2"
    >
      {ACTIONS.map((action) => (
        <button
          key={action}
          type="button"
          disabled={disabled}
          onClick={() => onAction(action)}
          className="cursor-pointer rounded-full border border-neutral-300 px-3 py-1 text-xs text-neutral-700 hover:bg-neutral-100 disabled:cursor-default disabled:opacity-50 dark:border-neutral-600 dark:text-neutral-200 dark:hover:bg-neutral-800"
        >
          {label(action)}
        </button>
      ))}
    </div>
  );
}

export function Composer({
  busy,
  canSend,
  ephemeral,
  keepable,
  notice,
  onSend,
  onStop,
  onAction,
  onKeep,
}: {
  busy: boolean;
  /** Liaison prête et modèle choisi. */
  canSend: boolean;
  ephemeral: boolean;
  /** Conversation éphémère commencée : « Keep » la garde. */
  keepable: boolean;
  notice: Notice | null;
  onSend: (text: string) => void;
  onStop: () => void;
  onAction: (action: QuickAction) => void;
  onKeep: () => void;
}) {
  const t = useT();
  const [text, setText] = useState("");
  const ready = canSend && !busy && text.trim() !== "";

  const submit = () => {
    if (!ready) return;
    // Retours à la ligne finaux retirés, comme l'app.
    onSend(text.replace(/\s+$/, ""));
    setText("");
  };

  const onKeyDown = (event: KeyboardEvent<HTMLTextAreaElement>) => {
    if (
      event.key === "Enter" &&
      !event.shiftKey &&
      !event.nativeEvent.isComposing
    ) {
      event.preventDefault();
      submit();
    }
  };

  const onSubmit = (event: FormEvent) => {
    event.preventDefault();
    submit();
  };

  return (
    <div className="px-3 pt-2 pb-3">
      <QuickActions disabled={!canSend || busy} onAction={onAction} />
      {notice === "offMachineNewChat" && (
        <p
          className="pb-2 text-xs text-neutral-700 dark:text-neutral-200"
          role="status"
          data-testid="notice"
        >
          {t("vscode.composer.offMachineNewChat")}
        </p>
      )}
      {ephemeral && (
        <div
          className="flex items-center justify-between gap-2 pb-2 text-xs text-neutral-500 dark:text-neutral-400"
          data-testid="ephemeral-notice"
        >
          <p>{t("vscode.composer.ephemeral")}</p>
          {keepable && (
            <button
              type="button"
              onClick={onKeep}
              className="shrink-0 cursor-pointer rounded-full border border-neutral-300 px-3 py-1 text-neutral-700 hover:bg-neutral-100 dark:border-neutral-600 dark:text-neutral-200 dark:hover:bg-neutral-800"
            >
              {t("vscode.composer.keep")}
            </button>
          )}
        </div>
      )}
      <form
        onSubmit={onSubmit}
        className={`relative flex items-end gap-2 rounded-3xl bg-white py-2 pr-2 pl-4 after:pointer-events-none after:absolute after:inset-0 after:rounded-3xl dark:bg-neutral-800 ${
          ephemeral
            ? "after:border-[1.5px] after:border-dashed after:border-neutral-500 dark:after:border-neutral-400"
            : "after:ring-[1.5px] after:ring-neutral-300 after:ring-inset dark:after:ring-white/10"
        }`}
      >
        <textarea
          aria-label={t("vscode.composer.label")}
          placeholder={t("vscode.composer.placeholder")}
          rows={1}
          value={text}
          onChange={(event) => setText(event.target.value)}
          onKeyDown={onKeyDown}
          className="max-h-48 min-h-[24px] flex-1 resize-none self-center border-none bg-transparent text-sm leading-6 text-neutral-700 outline-none [field-sizing:content] placeholder:text-neutral-400 dark:text-white dark:placeholder:text-neutral-500"
        />
        {busy ? (
          <button
            type="button"
            onClick={onStop}
            aria-label={t("vscode.composer.stop")}
            title={t("vscode.composer.stop")}
            className="flex h-8 w-8 shrink-0 cursor-pointer items-center justify-center rounded-full bg-black text-white focus:outline-2 focus:outline-offset-2 focus:outline-accent-fg dark:bg-white dark:text-black"
          >
            <StopIcon className="h-3.5 w-3.5" />
          </button>
        ) : (
          <button
            type="submit"
            disabled={!ready}
            aria-label={t("vscode.composer.send")}
            title={t("vscode.composer.send")}
            className="flex h-8 w-8 shrink-0 cursor-pointer items-center justify-center rounded-full bg-black text-white focus:outline-2 focus:outline-offset-2 focus:outline-accent-fg disabled:cursor-default disabled:opacity-10 dark:bg-white dark:text-black"
          >
            <ArrowUpIcon className="h-4 w-4" />
          </button>
        )}
      </form>
    </div>
  );
}
