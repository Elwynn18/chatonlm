// Annonce brève de toute l'app (src/lib/notice.ts), en bas au centre de la
// fenêtre, comme « Saved » des réglages ; retirée après quelques secondes.
// La région reste dans la page : seul son contenu change, pour que les
// lecteurs d'écran l'annoncent sûrement ; région vivante sans rôle
// « status », qui reste celui de chaque page (« Saved » des réglages…).

import { useEffect } from "react";
import { useT } from "@/i18n";
import { hideNotice, useNotice } from "@/lib/notice";

/** Durée d'une annonce. */
export const NOTICE_DURATION = 6000;

export function Notice() {
  const t = useT();
  const notice = useNotice();

  useEffect(() => {
    if (!notice) return;
    const timer = window.setTimeout(
      () => hideNotice(notice.id),
      NOTICE_DURATION,
    );
    return () => window.clearTimeout(timer);
  }, [notice]);

  return (
    <div
      aria-live="polite"
      aria-atomic="true"
      data-testid="notice"
      className="pointer-events-none fixed bottom-4 left-1/2 z-50 w-max max-w-[calc(100%-2rem)] -translate-x-1/2"
    >
      {notice && (
        <p
          key={notice.id}
          className="rounded-md bg-green-100 px-3 py-1.5 text-center text-sm/5 font-medium text-green-800 dark:bg-green-800 dark:text-green-100"
        >
          {notice.lines.map((line) => t(line.key, line.params)).join(" ")}
        </p>
      )}
    </div>
  );
}
