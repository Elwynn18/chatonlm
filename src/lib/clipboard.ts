// Presse-papiers par l'API du navigateur. WebKitGTK n'accorde l'écriture que
// dans les ~5 s qui suivent un geste de l'utilisateur : ces fonctions doivent
// être appelées directement dans le gestionnaire du clic, sans attente avant.

/**
 * Copie du texte dans le presse-papiers : API asynchrone, sinon l'ancienne
 * commande « copy » sur une zone de texte temporaire. Rejette si les deux
 * échouent.
 */
export async function copyText(text: string): Promise<void> {
  try {
    await navigator.clipboard.writeText(text);
    return;
  } catch {
    // Repli ci-dessous.
  }
  // La zone de texte prend le focus le temps de la copie : il est rendu
  // ensuite à l'élément qui l'avait (bouton Copy, lien après son menu...),
  // sinon il tomberait sur body.
  const focused = document.activeElement;
  const area = document.createElement("textarea");
  area.value = text;
  area.setAttribute("readonly", "");
  area.style.position = "fixed";
  area.style.opacity = "0";
  document.body.append(area);
  area.select();
  try {
    // Dépréciée, mais seul repli quand l'API asynchrone est refusée.
    if (!document.execCommand("copy")) throw new Error("copy refused");
  } finally {
    area.remove();
    if (focused instanceof HTMLElement && focused.isConnected) {
      focused.focus({ preventScroll: true });
    }
  }
}

/**
 * Copie riche (bouton « Copy » d'une réponse) : HTML et texte brut ensemble,
 * pour un collage mis en forme dans un éditeur et en texte dans un terminal.
 * Sans ClipboardItem ou en cas de refus, texte seul par copyText (encore
 * permis juste après le refus). Rejette si tout échoue.
 */
export async function copyRich(html: string, text: string): Promise<void> {
  try {
    // Appel synchrone dès l'entrée : le geste de l'utilisateur est encore actif.
    await navigator.clipboard.write([
      new ClipboardItem({
        "text/html": new Blob([html], { type: "text/html" }),
        "text/plain": new Blob([text], { type: "text/plain" }),
      }),
    ]);
  } catch {
    await copyText(text);
  }
}
