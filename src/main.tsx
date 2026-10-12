import { QueryClientProvider } from "@tanstack/react-query";
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { setLanguage } from "@/i18n";
import { appPlatform, uiLanguage } from "@/lib/ipc";
import { guessPlatform, setPlatform } from "@/lib/platform";
import { App } from "./App";
import { scheduleHighlighterWarmUp } from "./lib/highlighter";
import { queryClient } from "./lib/queryClient";
import "./index.css";

function start() {
  createRoot(document.getElementById("root")!).render(
    <StrictMode>
      <QueryClientProvider client={queryClient}>
        <App />
      </QueryClientProvider>
    </StrictMode>,
  );

  // Surligneur de code (morceau à part de 2 Mo) : chargé puis préchauffé une
  // fois la fenêtre affichée, pendant les temps morts.
  scheduleHighlighterWarmUp();
}

// Langue lue avant le premier affichage (la fenêtre, créée masquée, ne
// montre jamais l'anglais d'abord) ; en cas d'échec, l'anglais, puis celle des
// réglages une fois lus. Pas d'await au niveau du module : il ferait sortir
// du morceau d'entrée les modules qu'il partage avec le surligneur, chargé
// alors avec la page (e2e/markdown.test.mjs).
const language = uiLanguage().then(setLanguage, (cause: unknown) => {
  console.error("ui_language failed", cause);
});
// Système (Linux ou Windows), lu de même avant le premier affichage : textes,
// chemins et adresses des images en dépendent. Sans réponse du moteur, il est
// déduit de la webview.
const system = appPlatform().then(setPlatform, (cause: unknown) => {
  console.error("app_platform failed", cause);
  setPlatform(guessPlatform());
});
// Aucune des deux ne rejette.
void Promise.all([language, system]).finally(start);
