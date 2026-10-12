import { XMarkIcon } from "@heroicons/react/20/solid";
import {
  type FocusEvent,
  useEffect,
  useRef,
  useState,
  useSyncExternalStore,
} from "react";
import { useT } from "@/i18n";
import { Trans } from "@/i18n/Trans";
import { copyText } from "@/lib/clipboard";
import {
  dismissLinkProblem,
  type LinkProblem,
  linkProblem,
  subscribeLinkProblem,
} from "@/lib/openLink";

/** Durée de l'avis, prolongée tant que le pointeur ou le focus y sont. */
const NOTICE_MS = 8000;

/**
 * Avis discret en haut de la zone de contenu quand un lien n'a pas pu être
 * ouvert (ou copié) : la raison donnée par le backend et l'adresse, à
 * sélectionner ou à copier. En haut, et non en bas, pour ne jamais couvrir la
 * zone de saisie (Stop, Send, choix du modèle). Monté une seule fois, dans le
 * <main> de SidebarLayout, qui le positionne ; un nouvel échec remplace l'avis
 * précédent.
 */
export function LinkErrorNotice() {
  const problem = useSyncExternalStore(subscribeLinkProblem, linkProblem);
  if (!problem) return null;
  // Clé : chaque échec repart d'un état neuf (minuterie, bouton de copie).
  return <Notice key={problem.id} problem={problem} />;
}

function Notice({ problem }: { problem: LinkProblem }) {
  const t = useT();
  const [hovered, setHovered] = useState(false);
  const [focused, setFocused] = useState(false);
  const [copy, setCopy] = useState<"idle" | "copied" | "failed">("idle");
  const urlRef = useRef<HTMLSpanElement>(null);
  const noticeRef = useRef<HTMLDivElement>(null);
  // Élément qui avait le focus avant que l'avis ne le prenne.
  const origin = useRef<HTMLElement | null>(null);
  const held = hovered || focused;

  useEffect(() => {
    if (held) return;
    const timer = setTimeout(() => dismissLinkProblem(problem.id), NOTICE_MS);
    return () => clearTimeout(timer);
  }, [held, problem.id]);

  const copyLink = () => {
    copyText(problem.url).then(
      () => setCopy("copied"),
      () => {
        setCopy("failed");
        // Sélectionnée pour une copie à la main.
        const url = urlRef.current;
        if (url) window.getSelection()?.selectAllChildren(url);
      },
    );
  };

  const enter = (event: FocusEvent<HTMLDivElement>) => {
    setFocused(true);
    // Arrivée depuis l'extérieur (pas d'un bouton de l'avis à l'autre).
    const from = event.relatedTarget;
    if (!event.currentTarget.contains(from)) {
      origin.current = from instanceof HTMLElement ? from : null;
    }
  };

  // Dismiss ou Échap. Si l'avis a le focus, il le rend à l'élément qui
  // l'avait avant lui, au lieu de le laisser tomber sur la page à son
  // retrait ; sinon rien ne bouge. La fermeture seule (8 s) n'a pas ce
  // souci : elle attend que le focus soit parti.
  const dismiss = () => {
    const target = origin.current;
    if (
      noticeRef.current?.contains(document.activeElement) &&
      target?.isConnected
    ) {
      target.focus({ preventScroll: true });
    }
    dismissLinkProblem(problem.id);
  };

  const leave = (event: FocusEvent<HTMLDivElement>) => {
    if (!event.currentTarget.contains(event.relatedTarget as Node | null)) {
      setFocused(false);
    }
  };

  return (
    // top-14 : sous la barre du haut (h-13) et ses boutons, au-dessus des
    // messages. Seul l'avis reçoit le pointeur, pas la bande qui le centre.
    <div className="pointer-events-none absolute inset-x-0 top-14 z-50 flex justify-center px-4">
      <div
        ref={noticeRef}
        role="alert"
        onMouseEnter={() => setHovered(true)}
        onMouseLeave={() => setHovered(false)}
        onFocus={enter}
        onBlur={leave}
        onKeyDown={(event) => {
          if (event.key !== "Escape") return;
          // Consommée : Échap n'arrête pas aussi la réponse en cours.
          event.preventDefault();
          dismiss();
        }}
        className="pointer-events-auto flex w-full max-w-[768px] items-center gap-3 rounded-2xl border border-neutral-200 bg-white py-2.5 pr-2 pl-4 text-sm text-neutral-700 shadow-lg dark:border-neutral-700 dark:bg-neutral-800 dark:text-neutral-200"
      >
        <p className="min-w-0 flex-1 break-words">
          <Trans
            k={
              problem.kind === "open"
                ? "chat.linkError.open"
                : "chat.linkError.copy"
            }
            params={problem.kind === "open" ? { detail: problem.detail } : {}}
            tags={{
              url: () => (
                <span ref={urlRef} className="break-all select-text">
                  {problem.url}
                </span>
              ),
            }}
          />
        </p>
        <button
          type="button"
          onClick={copyLink}
          className="shrink-0 rounded-lg px-2.5 py-1 text-xs font-medium text-neutral-600 outline-none hover:bg-neutral-100 focus-visible:ring-2 focus-visible:ring-accent-fg dark:text-neutral-300 dark:hover:bg-neutral-700"
        >
          {copy === "copied"
            ? t("common.actions.copied")
            : copy === "failed"
              ? t("chat.copy.failed")
              : t("chat.linkError.copyLink")}
        </button>
        <button
          type="button"
          onClick={dismiss}
          aria-label={t("chat.linkError.dismiss")}
          title={t("chat.linkError.dismiss")}
          className="flex h-7 w-7 shrink-0 items-center justify-center rounded-lg text-neutral-500 outline-none hover:bg-neutral-100 focus-visible:ring-2 focus-visible:ring-accent-fg dark:text-neutral-400 dark:hover:bg-neutral-700"
        >
          <XMarkIcon className="h-4 w-4" />
        </button>
      </div>
    </div>
  );
}
