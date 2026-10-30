// Remplace src/lib/appHost.ts dans le build du panneau
// (vscode/webview.vite.config.ts) : l'hôte de l'app passe par Tauri, absent
// ici. Jamais employé : le panneau pose le sien (panelHost.ts) par HostContext.
import type { Host } from "@/lib/host";

const unavailable = () => Promise.reject(new Error("no app host in the panel"));

export const appHost: Host = {
  openLink: unavailable,
  copyText: unavailable,
  copyRich: unavailable,
  copyLinkFailed() {},
};
