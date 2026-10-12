// Fichiers collés, comme l'officiel (FileUpload.tsx) : pris seulement si le
// presse-papiers n'a pas de texte (sinon collage de texte normal). Écarts :
// chaque fichier par getAsFile() (l'officiel passe par webkitGetAsEntry(),
// qui ne donne rien pour une image collée), avec repli sur
// clipboardData.files ; une image collée sans nom utilisable reçoit le nom
// « image.png » (ou .jpg, .webp), sans lequel elle serait refusée ; un
// collage sans texte ni fichier (capture d'écran : WebKitGTK 2.52 ne donne à
// la page ni type ni fichier pour une image) est confié au backend, qui lit
// l'image dans le presse-papiers GTK.

/** Noms donnés aux images collées sans extension, par type. */
const IMAGE_NAMES: Record<string, string> = {
  "image/png": "image.png",
  "image/jpeg": "image.jpg",
  "image/webp": "image.webp",
};

/**
 * Ce que contient un collage : du texte (collage normal), des fichiers à
 * joindre, ou rien que la page puisse lire (peut-être une image).
 */
export type Paste =
  { kind: "text" } | { kind: "files"; files: File[] } | { kind: "unreadable" };

export function classifyPaste(data: DataTransfer): Paste {
  const hasText =
    data.types.includes("text/plain") &&
    data.getData("text/plain").trim().length > 0;
  if (hasText) return { kind: "text" };
  const items = Array.from(data.items)
    .filter((item) => item.kind === "file")
    .map((item) => item.getAsFile())
    .filter((file): file is File => file !== null);
  const files = items.length > 0 ? items : Array.from(data.files);
  return files.length > 0 ? { kind: "files", files } : { kind: "unreadable" };
}

/** Nom d'un fichier collé (voir IMAGE_NAMES). */
export function pastedName(file: File): string {
  if (file.name.includes(".")) return file.name;
  return IMAGE_NAMES[file.type] ?? file.name;
}
