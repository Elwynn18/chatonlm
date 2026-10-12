// Fichiers déposés sur la fenêtre : dépôt natif de Tauri (chemins, comme le
// sélecteur), le WebView ne voit pas ces dépôts. Code propre à ChatonLM :
// l'officiel lit les évènements HTML5 de la zone de chat.

import { getCurrentWebview } from "@tauri-apps/api/webview";
import { useEffect, useRef, useState } from "react";
import { isLocalPath } from "@/lib/attachments";

/**
 * Chemins de fichiers locaux d'un glisser. wry donne aussi les adresses d'un
 * lien ou d'une image glissés depuis un navigateur ou depuis une réponse
 * (https://...) : ce ne sont pas des fichiers, comme pour l'officiel qui ne
 * réagit qu'au type « Files ». Forme d'un chemin selon le système
 * (`/…`, ou `C:\…` sous Windows) : isLocalPath.
 */
function localPaths(paths: string[]): string[] {
  return paths.filter(isLocalPath);
}

/**
 * Écoute les dépôts de fichiers tant que le composant est monté ; `onDrop`
 * reçoit les chemins déposés. Vrai pendant qu'on survole la fenêtre avec des
 * fichiers (voile affiché).
 */
export function useFileDrop(onDrop: (paths: string[]) => void): boolean {
  const [dragging, setDragging] = useState(false);
  const onDropRef = useRef(onDrop);
  useEffect(() => {
    onDropRef.current = onDrop;
  });

  useEffect(() => {
    let active = true;
    let unlisten: (() => void) | null = null;
    getCurrentWebview()
      .onDragDropEvent((event) => {
        const drag = event.payload;
        switch (drag.type) {
          case "enter":
            // Glisser de texte : pas de chemins, le WebView s'en charge.
            setDragging(localPaths(drag.paths).length > 0);
            break;
          case "leave":
            setDragging(false);
            break;
          case "drop": {
            setDragging(false);
            const paths = localPaths(drag.paths);
            if (paths.length > 0) onDropRef.current(paths);
            break;
          }
        }
      })
      .then(
        (stop) => {
          if (active) unlisten = stop;
          else stop();
        },
        (error: unknown) =>
          console.error("Failed to listen for dropped files:", error),
      );
    return () => {
      active = false;
      unlisten?.();
    };
  }, []);

  return dragging;
}
