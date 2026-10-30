// États vides du panneau : liaison pas prête (app fermée, lien coupé, pas
// appairé, version incompatible, erreur), aucun modèle, conversation vide.
import type { ReactNode } from "react";
import { Logo } from "@/components/Logo";
import { useT } from "@/i18n";
import type { PanelMessage, PanelState } from "../shared/protocol";
import { connectionLabel } from "./labels";
import type { Post } from "./vscodeApi";

const BUTTON =
  "inline-flex cursor-pointer rounded-full bg-neutral-900 px-4 py-1.5 text-sm font-medium text-white hover:bg-neutral-700 dark:bg-white dark:text-neutral-900 dark:hover:bg-neutral-200";

function Screen({
  title,
  body,
  children,
  testId,
}: {
  title: string;
  body?: string;
  children?: ReactNode;
  testId: string;
}) {
  return (
    <div
      className="flex flex-1 flex-col items-center justify-center px-6 py-8 text-center"
      data-testid={testId}
    >
      <Logo size={48} className="mb-5" />
      <h2 className="text-base font-medium text-neutral-900 dark:text-white">
        {title}
      </h2>
      {body && (
        <p className="mt-2 max-w-xs text-sm text-neutral-500 dark:text-neutral-400">
          {body}
        </p>
      )}
      {children && <div className="mt-5">{children}</div>}
    </div>
  );
}

/** Liaison avec l'app pas prête : ce qu'il faut faire. */
export function ConnectionScreen({
  state,
  post,
}: {
  state: PanelState;
  post: Post;
}) {
  const t = useT();
  const title = connectionLabel(t, state.connection);
  const button = (label: string, message: PanelMessage) => (
    <button type="button" className={BUTTON} onClick={() => post(message)}>
      {label}
    </button>
  );
  const openApp = t("vscode.screen.openApp");
  switch (state.connection) {
    case "appClosed":
      return (
        <Screen
          testId="screen-appClosed"
          title={title}
          body={t("vscode.screen.appClosed")}
        >
          {button(openApp, { type: "launchApp" })}
        </Screen>
      );
    case "bridgeOff":
      return (
        <Screen
          testId="screen-bridgeOff"
          title={title}
          body={t("vscode.screen.bridgeOff")}
        >
          {button(openApp, { type: "openApp" })}
        </Screen>
      );
    case "notPaired":
      return (
        <Screen
          testId="screen-notPaired"
          title={title}
          body={t("vscode.screen.notPaired")}
        >
          {button(t("vscode.screen.pair"), { type: "pair" })}
        </Screen>
      );
    case "incompatible":
      return (
        <Screen
          testId="screen-incompatible"
          title={title}
          body={t("vscode.screen.incompatible")}
        />
      );
    case "error":
      return (
        <Screen testId="screen-error" title={title} body={state.error}>
          {button(openApp, { type: "launchApp" })}
        </Screen>
      );
    default:
      return <Screen testId="screen-connecting" title={title} />;
  }
}

const SECONDARY =
  "inline-flex cursor-pointer rounded-full border border-neutral-300 px-4 py-1.5 text-sm font-medium text-neutral-800 hover:bg-neutral-100 dark:border-neutral-600 dark:text-neutral-200 dark:hover:bg-neutral-800";

/** Liaison prête, mais aucun modèle installé dans l'app (relu à l'ouverture
 * du panneau, au retour dans la fenêtre, ou sur demande). */
export function NoModelsScreen({ post }: { post: Post }) {
  const t = useT();
  return (
    <Screen
      testId="screen-noModels"
      title={t("vscode.header.noModels")}
      body={t("vscode.screen.noModels")}
    >
      <div className="flex flex-wrap justify-center gap-2">
        <button
          type="button"
          className={BUTTON}
          onClick={() => post({ type: "openApp" })}
        >
          {t("vscode.screen.openApp")}
        </button>
        <button
          type="button"
          className={SECONDARY}
          onClick={() => post({ type: "refresh" })}
        >
          {t("vscode.screen.checkAgain")}
        </button>
      </div>
    </Screen>
  );
}

/** Conversation vide. */
export function EmptyChat() {
  const t = useT();
  return (
    <Screen
      testId="screen-empty"
      title={t("vscode.empty.title")}
      body={t("vscode.empty.body")}
    />
  );
}
