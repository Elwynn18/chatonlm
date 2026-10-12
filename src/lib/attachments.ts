// Pièces jointes côté interface : adresses des images servies par le
// protocole chatonlm:// du backend (src-tauri/src/attachments/protocol.rs),
// sans faire passer leurs octets par l'IPC. Sous Windows, ce protocole est
// servi à l'adresse http://chatonlm.localhost/ (voir protocolBase) ; le
// moteur reçoit la même adresse chatonlm://localhost/... sur les deux systèmes.

import { isWindows, protocolBase } from "@/lib/platform";

/** Adresse d'une ressource du protocole chatonlm:// (`chemin` sans « / » initial). */
function protocolUrl(path: string): string {
  return `${protocolBase("chatonlm")}${path}`;
}

/** Vignette d'un fichier préparé (composer) ; 404 une fois envoyé ou retiré. */
export function stagedImageUrl(id: string): string {
  return protocolUrl(`staged/${id}`);
}

/** Image d'une pièce jointe enregistrée (message envoyé). */
export function attachmentImageUrl(id: number): string {
  return protocolUrl(`attachment/${id}`);
}

/** Taille maximale d'un fichier (MAX_FILE_SIZE du backend, comme l'officiel). */
export const MAX_FILE_SIZE = 10 * 1024 * 1024;

/**
 * Nom de base d'un chemin (sélecteur natif, dépôt). Sous Windows, « \ »
 * sépare aussi les dossiers (`C:\Users\moi\photo.png`) ; sous Linux, c'est un
 * caractère de nom comme un autre.
 */
export function basename(path: string): string {
  if (isWindows()) {
    return (
      path
        .replace(/[\\/]+$/, "")
        .split(/[\\/]/)
        .pop() ?? path
    );
  }
  return path.replace(/\/+$/, "").split("/").pop() ?? path;
}

/**
 * Le chemin désigne-t-il un fichier local ? Un glisser donne aussi des
 * adresses (https://...), qui n'en sont pas. Linux : chemin absolu ;
 * Windows : lettre de lecteur (`C:\`) ou partage réseau (`\\serveur\`).
 */
export function isLocalPath(path: string): boolean {
  if (isWindows()) {
    return /^[A-Za-z]:[\\/]/.test(path) || path.startsWith("\\\\");
  }
  return path.startsWith("/");
}
