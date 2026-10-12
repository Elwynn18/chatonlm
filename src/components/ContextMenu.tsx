import {
  type KeyboardEvent,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
} from "react";
import { createPortal } from "react-dom";

export interface MenuItem {
  label: string;
  onSelect: () => void;
}

/**
 * Rend le focus à `opener`, l'élément qui l'avait à l'ouverture du menu (lien
 * ouvert au clavier, champ de saisie...), s'il est encore dans la page.
 * Seulement si le focus est encore dans le menu : rien n'est volé si
 * l'utilisateur l'a mis ailleurs entre-temps. Appelée juste avant la
 * fermeture, menu encore affiché ; un choix qui déplace lui-même le focus
 * (champ de renommage, confirmation) le fait ensuite, dans ses effets.
 */
function giveFocusBack(menu: HTMLElement | null, opener: Element | null) {
  if (!menu?.contains(document.activeElement)) return;
  if (
    opener instanceof HTMLElement &&
    opener !== document.body &&
    opener.isConnected
  ) {
    // Sans défilement : la page a pu bouger pendant que le menu était là.
    opener.focus({ preventScroll: true });
  }
}

/**
 * Menu contextuel affiché au point du clic droit. Il remplace le menu natif de
 * l'app officielle (mal placé par Tauri sous Wayland) : il se ferme au clic
 * ailleurs, avec Échap ou quand la fenêtre perd le focus, et se pilote au
 * clavier (flèches, Entrée). Comme un menu natif, il rend le focus en se
 * fermant à l'élément qui l'avait à l'ouverture.
 */
export function ContextMenu({
  x,
  y,
  items,
  onClose,
}: {
  x: number;
  y: number;
  items: MenuItem[];
  onClose: () => void;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const opener = useRef<Element | null>(null);
  const [position, setPosition] = useState({ left: x, top: y });

  // Reste dans la fenêtre, puis met le focus sur le premier élément, après
  // avoir noté qui l'avait. Pas quand le menu l'a déjà (nouvelle position,
  // effet rejoué par StrictMode) : ce serait un de ses éléments.
  useLayoutEffect(() => {
    const menu = ref.current;
    if (!menu) return;
    if (!menu.contains(document.activeElement)) {
      opener.current = document.activeElement;
    }
    const { width, height } = menu.getBoundingClientRect();
    setPosition({
      left: Math.max(4, Math.min(x, window.innerWidth - width - 4)),
      top: Math.max(4, Math.min(y, window.innerHeight - height - 4)),
    });
    menu.querySelector<HTMLElement>("[role=menuitem]")?.focus();
  }, [x, y]);

  useEffect(() => {
    const close = () => {
      giveFocusBack(ref.current, opener.current);
      onClose();
    };
    const closeOutside = (event: MouseEvent) => {
      // Le clic donne ensuite le focus là où il tombe, comme d'habitude.
      if (!ref.current?.contains(event.target as Node)) close();
    };
    const closeOnEscape = (event: globalThis.KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault();
        close();
      }
    };
    document.addEventListener("mousedown", closeOutside);
    document.addEventListener("keydown", closeOnEscape);
    window.addEventListener("blur", close);
    window.addEventListener("resize", close);
    return () => {
      document.removeEventListener("mousedown", closeOutside);
      document.removeEventListener("keydown", closeOnEscape);
      window.removeEventListener("blur", close);
      window.removeEventListener("resize", close);
    };
  }, [onClose]);

  const moveFocus = (event: KeyboardEvent<HTMLDivElement>) => {
    if (event.key !== "ArrowDown" && event.key !== "ArrowUp") return;
    event.preventDefault();
    const entries = [
      ...(ref.current?.querySelectorAll<HTMLElement>("[role=menuitem]") ?? []),
    ];
    const index = entries.indexOf(document.activeElement as HTMLElement);
    const step = event.key === "ArrowDown" ? 1 : -1;
    entries[(index + step + entries.length) % entries.length]?.focus();
  };

  return createPortal(
    <div
      ref={ref}
      role="menu"
      onKeyDown={moveFocus}
      onContextMenu={(event) => event.preventDefault()}
      style={{ left: position.left, top: position.top }}
      className="fixed z-50 min-w-36 rounded-lg border border-neutral-200 bg-white py-1 text-sm shadow-lg select-none dark:border-neutral-700 dark:bg-neutral-800"
    >
      {items.map((item) => (
        <button
          key={item.label}
          type="button"
          role="menuitem"
          onClick={() => {
            giveFocusBack(ref.current, opener.current);
            onClose();
            item.onSelect();
          }}
          className="block w-full px-3 py-1.5 text-left text-neutral-800 outline-none hover:bg-neutral-100 focus:bg-neutral-100 dark:text-neutral-100 dark:hover:bg-neutral-700 dark:focus:bg-neutral-700"
        >
          {item.label}
        </button>
      ))}
    </div>,
    document.body,
  );
}
