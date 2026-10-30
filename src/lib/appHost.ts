// Hôte de l'app (voir host.ts) : ouverture des liens par le moteur Tauri,
// presse-papiers de la webview. Le build du panneau VS Code remplace ce
// module (vscode/webview.vite.config.ts) : rien de Tauri n'y est embarqué.
import { copyRich, copyText } from "@/lib/clipboard";
import type { Host } from "@/lib/host";
import { openLink, reportCopyFailure } from "@/lib/openLink";

export const appHost: Host = {
  openLink,
  copyText,
  copyRich,
  copyLinkFailed: reportCopyFailure,
};
