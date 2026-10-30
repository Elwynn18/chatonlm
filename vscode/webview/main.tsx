// Point d'entrée du panneau (webview de l'extension VS Code). Pas d'await au
// niveau du module (voir src/main.tsx) : le surligneur reste un morceau à part.
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { scheduleHighlighterWarmUp } from "@/lib/highlighter";
import { Panel } from "./Panel";
import { vscodePost } from "./vscodeApi";
import "./index.css";

const post = vscodePost();

/** Erreur du panneau : dite à l'hôte, qui l'écrit dans sa sortie. */
function report(error: unknown) {
  const message =
    error instanceof Error
      ? `${error.message}\n${error.stack ?? ""}`
      : String(error);
  post({ type: "panelError", message });
}
window.addEventListener("error", (event) =>
  report(event.error ?? event.message),
);
window.addEventListener("unhandledrejection", (event) => report(event.reason));

createRoot(document.getElementById("root")!, {
  onUncaughtError: report,
  onCaughtError: report,
}).render(
  <StrictMode>
    <Panel post={post} />
  </StrictMode>,
);

scheduleHighlighterWarmUp();
