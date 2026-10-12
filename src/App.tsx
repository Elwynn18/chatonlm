import { useQuery, useQueryClient } from "@tanstack/react-query";
import { RouterProvider } from "@tanstack/react-router";
import { useEffect } from "react";
import type { DatabaseProblem } from "@/bindings/DatabaseProblem";
import { DatabaseProblemScreen } from "@/components/DatabaseProblemScreen";
import { ServerGate } from "@/components/ServerGate";
import { useSettings } from "@/hooks/useSettings";
import { useTheme } from "@/hooks/useTheme";
import { useT } from "@/i18n";
import { databaseProblem, errorMessage, recoverDatabase } from "@/lib/ipc";
import { showMainWindow } from "@/lib/window";
import { router } from "@/router";

const databaseProblemKey = ["databaseProblem"] as const;

/** La fenêtre est créée masquée ; on l'affiche une fois le contenu prêt. */
function useShowWindow(ready: boolean) {
  useEffect(() => {
    if (!ready) return;
    showMainWindow().catch((cause: unknown) => {
      console.error("show() failed", cause);
    });
  }, [ready]);
}

export function App() {
  const queryClient = useQueryClient();
  const { data: problem, error } = useQuery({
    queryKey: databaseProblemKey,
    queryFn: databaseProblem,
    staleTime: Infinity,
  });

  // Si la vérification elle-même échoue, l'app continue : les commandes
  // suivantes signaleront l'erreur.
  if (problem === undefined && !error) return null;
  if (problem) {
    return (
      <ProblemWindow
        problem={problem}
        onRecovered={() => queryClient.setQueryData(databaseProblemKey, null)}
      />
    );
  }
  return <MainApp />;
}

function ProblemWindow({
  problem,
  onRecovered,
}: {
  problem: DatabaseProblem;
  onRecovered: () => void;
}) {
  useTheme("system");
  useShowWindow(true);
  return (
    <DatabaseProblemScreen
      problem={problem}
      onRecover={recoverDatabase}
      onRecovered={onRecovered}
    />
  );
}

function MainApp() {
  const t = useT();
  const { settings, error } = useSettings();
  const ready = settings !== undefined || error !== null;
  useTheme(settings?.theme);
  // Après les effets de mise en page du thème : pas de flash au premier rendu.
  useShowWindow(ready);

  // Seulement si rien n'a jamais été lu : une relecture ratée (page
  // Settings) laisse l'app affichée avec les réglages en cache.
  if (error && !settings) {
    return (
      <main className="flex h-screen items-center justify-center p-6 text-center text-sm text-red-600">
        {t("chat.app.settingsUnreadable", { error: errorMessage(error) })}
      </main>
    );
  }
  if (!settings) return null;

  return (
    <ServerGate>
      <RouterProvider router={router} />
    </ServerGate>
  );
}
