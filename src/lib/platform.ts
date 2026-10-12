// Système sur lequel tourne l'app (propre à ChatonLM ; l'officiel injecte
// window.OLLAMA_PLATFORM, lu par son lib/platform.ts). Le moteur le dit au
// démarrage (commande `app_platform`, lue dans main.tsx avec la langue, avant
// le premier affichage) ; il ne change plus ensuite. Posé aussi sur <html>
// (`data-platform`), pour les règles CSS propres à un système (barres de
// défilement de Windows, index.css). Hors de l'app (essais) : « linux ».

import type { Platform } from "@/bindings/Platform";

export type { Platform };

let current: Platform = "linux";

/** Système de l'app. */
export function platform(): Platform {
  return current;
}

/** L'app tourne sous Windows (WebView2). */
export function isWindows(): boolean {
  return current === "windows";
}

/** Note le système et le pose sur <html> ; toute autre valeur : « linux ». */
export function setPlatform(value: Platform): void {
  current = value === "windows" ? "windows" : "linux";
  document.documentElement.dataset.platform = current;
}

/**
 * Pour les essais : système par défaut, attribut retiré (sans page dans les
 * essais des scripts, lancés hors de jsdom).
 */
export function resetPlatformForTests(): void {
  current = "linux";
  if (typeof document !== "undefined") {
    delete document.documentElement.dataset.platform;
  }
}

interface TauriInternals {
  convertFileSrc?: (path: string, protocol?: string) => string;
}

/** Fonction de Tauri qui fabrique l'adresse d'un protocole, si elle existe. */
function convertFileSrc(): TauriInternals["convertFileSrc"] {
  const internals = (
    window as unknown as { __TAURI_INTERNALS__?: TauriInternals }
  ).__TAURI_INTERNALS__;
  return typeof internals?.convertFileSrc === "function"
    ? internals.convertFileSrc
    : undefined;
}

/**
 * Racine des adresses d'un protocole du moteur : `<protocole>://localhost/`,
 * et sous Windows `http://<protocole>.localhost/` (WebView2 ne connaît pas
 * les protocoles personnalisés : wry les sert ainsi, et suit le réglage
 * `useHttpsScheme`). Fabriquée par Tauri (`convertFileSrc`, chemin vide) ;
 * hors de l'app, la forme de Linux.
 */
export function protocolBase(protocol: string): string {
  return convertFileSrc()?.("", protocol) ?? `${protocol}://localhost/`;
}

/**
 * Système déduit de la webview quand le moteur ne répond pas : seuls
 * Windows (et Android) servent les protocoles en http(s).
 */
export function guessPlatform(): Platform {
  return protocolBase("chatonlm").startsWith("http") ? "windows" : "linux";
}
