import {
  type AriaAttributes,
  type MouseEvent,
  type ReactNode,
  useCallback,
  useState,
} from "react";
import { ContextMenu } from "@/components/ContextMenu";
import { useT } from "@/i18n";
import { copyText } from "@/lib/clipboard";
import { openLink, reportCopyFailure, webUrl } from "@/lib/openLink";

type DataAttributes = { [name: `data-${string}`]: string | undefined };

/**
 * Lien vers une page web (réponses Markdown, messages d'erreur), ouvert dans
 * le navigateur par le backend : la webview, elle, ne quitte jamais l'app.
 * Clic, Entrée (qui produit un clic) et clic milieu ouvrent ; le clic droit
 * donne un menu maison à la place de celui de WebKit, dont « Download Linked
 * File » téléchargerait l'adresse sans rien demander.
 *
 * Infobulle : le titre du lien Markdown s'il en a un, sinon l'adresse qui
 * sera ouverte (écart à l'officiel) : la fenêtre n'a pas de barre d'état qui
 * la montrerait au survol, et le texte d'un lien peut annoncer une autre
 * adresse que la sienne.
 *
 * Tout autre lien (vide, relatif, ancre, mailto:, javascript:, lien encore
 * incomplet pendant le flux...) devient un <a> sans href : il garde
 * l'apparence d'un lien (styles .prose de `a`), mais il est inerte, hors de
 * la tabulation et n'est pas un lien pour les lecteurs d'écran.
 *
 * id, aria-* et data-* (appels et retours des notes de bas de page) sont
 * posés sur l'une et l'autre forme.
 */
export function ExternalLink({
  href,
  title,
  children,
  className,
  ...attributes
}: {
  href?: string;
  title?: string;
  children: ReactNode;
  className?: string;
  id?: string;
} & AriaAttributes &
  DataAttributes) {
  const t = useT();
  const [menu, setMenu] = useState<{ x: number; y: number } | null>(null);
  const closeMenu = useCallback(() => setMenu(null), []);
  const url = webUrl(href);

  if (!url) {
    return (
      <a {...attributes} title={title} className={className}>
        {children}
      </a>
    );
  }

  const open = (event: MouseEvent) => {
    event.preventDefault();
    event.stopPropagation();
    void openLink(url);
  };

  return (
    <>
      <a
        {...attributes}
        href={href}
        // Un titre vide n'en est pas un : l'adresse reste visible.
        title={title || url}
        rel="noreferrer"
        className={className}
        onClick={open}
        onAuxClick={(event) => {
          // WebKit n'envoie pas de click pour le bouton du milieu, seulement
          // auxclick, puis tenterait une navigation (refusée) sans ceci.
          if (event.button === 1) open(event);
          else event.preventDefault();
        }}
        onContextMenu={(event) => {
          event.preventDefault();
          event.stopPropagation();
          // Touche Menu du clavier : pas de position de souris.
          const fromKeyboard = event.clientX === 0 && event.clientY === 0;
          const box = event.currentTarget.getBoundingClientRect();
          setMenu(
            fromKeyboard
              ? { x: box.left, y: box.bottom }
              : { x: event.clientX, y: event.clientY },
          );
        }}
      >
        {children}
      </a>
      {/* En se fermant, le menu rend le focus là où il était (au lien, si
          on l'a ouvert au clavier). */}
      {menu && (
        <ContextMenu
          x={menu.x}
          y={menu.y}
          onClose={closeMenu}
          items={[
            { label: t("chat.link.open"), onSelect: () => void openLink(url) },
            {
              label: t("chat.link.copy"),
              onSelect: () => {
                copyText(url).catch(() => reportCopyFailure(url));
              },
            },
          ]}
        />
      )}
    </>
  );
}
