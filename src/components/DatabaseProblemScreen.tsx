import { useState } from "react";
import type { DatabaseProblem } from "@/bindings/DatabaseProblem";
import { Logo } from "@/components/Logo";
import { type TFunction, useT } from "@/i18n";

interface Props {
  problem: DatabaseProblem;
  /** Nouvel essai (`fresh` : base neuve) ; renvoie le problème restant. */
  onRecover: (fresh: boolean) => Promise<DatabaseProblem | null>;
  /** Appelé quand la base est ouverte. */
  onRecovered: () => void;
}

/** Titre et explication d'un problème, dans la langue de `t`. */
function texts(t: TFunction, problem: DatabaseProblem) {
  switch (problem.kind) {
    case "newer":
      return {
        title: t("chat.database.newer.title"),
        description: t("chat.database.newer.description"),
      };
    case "corrupt":
      return {
        title: t("chat.database.corrupt.title"),
        description: t("chat.database.corrupt.description"),
      };
    case "locked":
      return {
        title: t("chat.database.locked.title"),
        description: t("chat.database.locked.description"),
      };
    case "unavailable":
      return {
        title: t("chat.database.unavailable.title"),
        description: t("chat.database.unavailable.description", {
          message: problem.message,
        }),
      };
  }
}

/** Écran affiché à la place de l'app quand la base ne s'ouvre pas. */
export function DatabaseProblemScreen({
  problem: initial,
  onRecover,
  onRecovered,
}: Props) {
  const t = useT();
  const [problem, setProblem] = useState(initial);
  const [busy, setBusy] = useState<"retry" | "fresh" | null>(null);
  const [error, setError] = useState<string | null>(null);
  const { title, description } = texts(t, problem);

  const recover = async (fresh: boolean) => {
    setBusy(fresh ? "fresh" : "retry");
    setError(null);
    try {
      const remaining = await onRecover(fresh);
      if (remaining) {
        setProblem(remaining);
      } else {
        onRecovered();
      }
    } catch (cause) {
      setError(
        typeof cause === "object" && cause !== null && "message" in cause
          ? String(cause.message)
          : String(cause),
      );
    } finally {
      setBusy(null);
    }
  };

  return (
    <main className="flex h-screen overflow-y-auto bg-white px-6 dark:bg-neutral-900">
      <div className="m-auto w-full max-w-md py-10 text-center">
        <Logo />
        <h1 className="text-xl font-medium text-neutral-900 dark:text-neutral-100">
          {title}
        </h1>
        <p className="mt-2 text-sm text-neutral-500 dark:text-neutral-400">
          {description}
        </p>
        <p className="mt-3 text-xs break-words text-neutral-500">
          {problem.path}
        </p>
        {problem.kind !== "newer" && (
          <div className="mt-6 flex items-center justify-center gap-4">
            {problem.kind === "corrupt" && (
              <button
                type="button"
                disabled={busy !== null}
                onClick={() => void recover(true)}
                className="h-10 rounded-full bg-neutral-900 px-6 text-sm font-medium text-white outline-none hover:bg-neutral-700 focus-visible:ring-2 focus-visible:ring-accent-fg focus-visible:ring-offset-2 disabled:opacity-60 dark:bg-white dark:text-neutral-900 dark:hover:bg-neutral-200 dark:focus-visible:ring-offset-neutral-900"
              >
                {busy === "fresh"
                  ? t("chat.database.starting")
                  : t("chat.database.startFresh")}
              </button>
            )}
            <button
              type="button"
              disabled={busy !== null}
              onClick={() => void recover(false)}
              className="h-10 min-w-28 rounded-full border border-neutral-300 px-6 text-sm font-medium text-neutral-800 outline-none hover:bg-neutral-100 focus-visible:ring-2 focus-visible:ring-accent-fg disabled:opacity-60 dark:border-neutral-700 dark:text-neutral-200 dark:hover:bg-neutral-800"
            >
              {busy === "retry"
                ? t("chat.actions.checking")
                : t("common.actions.retry")}
            </button>
          </div>
        )}
        {error && (
          <p
            role="alert"
            className="mt-3 text-xs break-words text-red-600 dark:text-red-400"
          >
            {error}
          </p>
        )}
      </div>
    </main>
  );
}
