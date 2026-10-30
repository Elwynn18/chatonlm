// Couche d'hôte du rendu des réponses : ce que les composants partagés
// (CopyButton, ExternalLink, donc StreamingMarkdownContent) demandent hors de
// la page. Dans l'app, le moteur Tauri et le presse-papiers de la webview
// (appHost.ts, valeur par défaut) ; dans le panneau de l'extension VS Code,
// des messages à l'hôte de l'extension (vscode/webview/panelHost.ts), posés
// par <HostContext value={…}>.
import { createContext, useContext } from "react";
import { appHost } from "@/lib/appHost";

export interface Host {
  /**
   * Ouvre une adresse web (http ou https, déjà vérifiée par webUrl) dans le
   * navigateur. Ne rejette jamais : un échec est montré par l'hôte.
   */
  openLink(url: string): Promise<void>;
  /** Copie du texte ; rejette si la copie a échoué. */
  copyText(text: string): Promise<void>;
  /** Copie riche (HTML et texte) d'une réponse ; rejette si tout a échoué. */
  copyRich(html: string, text: string): Promise<void>;
  /** La copie de l'adresse d'un lien a échoué : l'hôte la montre. */
  copyLinkFailed(url: string): void;
}

export const HostContext = createContext<Host>(appHost);

/** Hôte de la page : celui du contexte, l'app par défaut. */
export function useHost(): Host {
  return useContext(HostContext);
}
