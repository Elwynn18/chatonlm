// Adresses web ouvrables : module pur, sans Tauri, partagé par l'app et le
// panneau de l'extension VS Code (vscode/webview).

/**
 * Adresse à ouvrir dans le navigateur, normalisée, ou null : seuls http et
 * https sont ouverts (localhost compris, un clic explicite) ; vide, relatif,
 * ancre, mailto: et le reste ne sont pas des liens.
 */
export function webUrl(href: string | undefined): string | null {
  if (!href) return null;
  try {
    const url = new URL(href);
    return url.protocol === "http:" || url.protocol === "https:"
      ? url.href
      : null;
  } catch {
    return null;
  }
}
