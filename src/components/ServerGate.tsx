import { useQueryClient } from "@tanstack/react-query";
import { type ReactNode, useEffect, useState } from "react";
import { Logo } from "@/components/Logo";
import { ServerStatusScreen } from "@/components/ServerStatusScreen";
import {
  ollamaRuntimeKey,
  switchOllamaSource,
  useOllamaRuntime,
  useOllamaTargetEvents,
} from "@/hooks/useOllamaRuntime";
import { serverStatusKey, useServerStatus } from "@/hooks/useServerStatus";
import { useSettings } from "@/hooks/useSettings";
import { useT } from "@/i18n";
import {
  errorMessage,
  openBuiltinLog,
  openExternal,
  restartBuiltinOllama,
  startOllamaService,
} from "@/lib/ipc";

/** Erreur du moteur rendue lisible par l'écran (message). */
async function readable<T>(action: () => Promise<T>): Promise<T> {
  try {
    return await action();
  } catch (error) {
    throw new Error(errorMessage(error), { cause: error });
  }
}

/**
 * Affiche l'app quand Ollama répond, sinon l'écran d'accueil qui explique
 * comment l'installer ou le démarrer (ou passer à l'Ollama intégré). Une
 * fois Ollama joint, l'app reste affichée : une perte de connexion se
 * signale dans la conversation, sans revenir à l'accueil.
 */
export function ServerGate({ children }: { children: ReactNode }) {
  const queryClient = useQueryClient();
  const { settings, setSettings } = useSettings();
  const { data: status, error, refetch } = useServerStatus();
  const [connected, setConnected] = useState(false);
  const [retrying, setRetrying] = useState(false);
  // Échec de la relance de l'intégré demandée par Retry.
  const [retryError, setRetryError] = useState<string | null>(null);
  // Cible changée par le moteur : tout ce qui en dépend est relu.
  useOllamaTargetEvents();
  // Réglage `source` : seulement quand l'intégré sert.
  const builtin =
    !connected && status?.builtin === true && status.state !== "running";
  const { data: runtime } = useOllamaRuntime({ enabled: builtin });

  if (status?.state === "running" && !connected) setConnected(true);
  if (connected || status?.state === "running") return children;

  // Un clic pendant une vérification automatique la rejoint, sans l'annuler.
  // L'intégré en échec n'est relancé que sur demande : Retry le relance.
  const retry = async () => {
    setRetrying(true);
    setRetryError(null);
    try {
      if (status?.state === "builtinFailed") {
        try {
          await restartBuiltinOllama();
        } catch (cause) {
          setRetryError(errorMessage(cause));
        }
      }
      await refetch({ cancelRefetch: false });
    } finally {
      setRetrying(false);
    }
  };

  if (!status) {
    return error ? (
      <CheckFailedScreen
        message={errorMessage(error)}
        retrying={retrying}
        onRetry={() => void retry()}
      />
    ) : (
      <ConnectingScreen />
    );
  }

  const saveAddress = async (url: string) => {
    await readable(() => setSettings({ serverUrl: url }));
    await Promise.all([
      queryClient.invalidateQueries({ queryKey: serverStatusKey }),
      queryClient.invalidateQueries({ queryKey: ollamaRuntimeKey }),
    ]);
  };

  return (
    <ServerStatusScreen
      status={status}
      savedUrl={settings?.serverUrl ?? ""}
      isChecking={retrying}
      onRetry={() => void retry()}
      onSaveAddress={saveAddress}
      onOpen={(url) => readable(() => openExternal(url))}
      runtime={builtin ? runtime : null}
      retryError={retryError}
      onStart={async () => {
        const outcome = await readable(startOllamaService);
        if (outcome.kind === "started") {
          await queryClient.invalidateQueries({ queryKey: serverStatusKey });
        }
        return outcome;
      }}
      onOpenLog={() => readable(openBuiltinLog)}
      onSetSource={(source) =>
        readable(() => switchOllamaSource(queryClient, source))
      }
    />
  );
}

/** Première vérification : le logo, puis un message si elle dure. */
function ConnectingScreen() {
  const t = useT();
  const [slow, setSlow] = useState(false);
  useEffect(() => {
    const timer = setTimeout(() => setSlow(true), 400);
    return () => clearTimeout(timer);
  }, []);

  return (
    <main
      aria-busy="true"
      className="flex h-screen bg-white px-6 dark:bg-neutral-900"
    >
      <div className="m-auto text-center">
        <Logo />
        <p
          role="status"
          className={`text-sm text-neutral-500 dark:text-neutral-400 ${slow ? "" : "invisible"}`}
        >
          {t("chat.server.connecting")}
        </p>
      </div>
    </main>
  );
}

/** La vérification elle-même a échoué (erreur du backend). */
function CheckFailedScreen({
  message,
  retrying,
  onRetry,
}: {
  message: string;
  retrying: boolean;
  onRetry: () => void;
}) {
  const t = useT();
  return (
    <main className="flex h-screen bg-white px-6 dark:bg-neutral-900">
      <div className="m-auto w-full max-w-md text-center">
        <Logo />
        <h1 className="text-xl font-medium text-neutral-900 dark:text-neutral-100">
          {t("chat.server.checkFailed")}
        </h1>
        <p
          role="alert"
          className="mt-2 text-sm break-words text-neutral-500 dark:text-neutral-400"
        >
          {message}
        </p>
        <button
          type="button"
          onClick={onRetry}
          disabled={retrying}
          className="mt-6 h-10 min-w-28 rounded-full bg-neutral-900 px-6 text-sm font-medium text-white outline-none hover:bg-neutral-700 focus-visible:ring-2 focus-visible:ring-accent-fg focus-visible:ring-offset-2 disabled:opacity-60 dark:bg-white dark:text-neutral-900 dark:hover:bg-neutral-200 dark:focus-visible:ring-offset-neutral-900"
        >
          {retrying ? t("chat.actions.checking") : t("common.actions.retry")}
        </button>
      </div>
    </main>
  );
}
