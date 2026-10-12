// README d'un dépôt (fiche du ModelHub) : arbre sûr de src/lib/readme.ts
// rendu en éléments React de la liste blanche seulement. Liens et pastilles
// d'image ouverts dans le navigateur (ExternalLink). Titres décalés sous
// celui de la section About (h3) : « # Titre » devient un h4, plafonné à h6,
// à sa taille d'origine. Au-delà d'une hauteur, le texte est replié derrière
// « Show more », et se déplie quand le clavier y entre (un lien caché ne
// reçoit pas le focus sans se montrer).

import { PhotoIcon } from "@heroicons/react/24/outline";
import {
  createElement,
  type ReactNode,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { ExternalLink } from "@/components/ExternalLink";
import { type TFunction, useT } from "@/i18n";
import { type ReadmeNode, readmeText, readmeTree } from "@/lib/readme";

/** Hauteur repliée (36rem). */
const COLLAPSED_PX = 576;
/** Sans mesure possible (jsdom) : replié au-delà de ce nombre de caractères. */
const COLLAPSED_CHARS = 1500;

/** Titres sous « About » (h3) : un niveau au-dessous, h6 au plus. */
const HEADING_SHIFT = 3;

/** Taille d'un titre selon son niveau d'origine (h1 à h6). */
const HEADING_SIZES = [
  "text-xl",
  "text-lg",
  "text-base",
  "text-sm",
  "text-sm",
  "text-sm",
];

const HEADINGS = new Set(["h1", "h2", "h3", "h4", "h5", "h6"]);

/** Titre du README, décalé sous la section About (`align` gardé). */
function heading(
  tag: string,
  key: string,
  align: string | undefined,
  children: ReactNode[],
) {
  const level = Number(tag.slice(1));
  return createElement(
    `h${Math.min(6, level + HEADING_SHIFT)}`,
    {
      key,
      align,
      className: `mt-6 mb-2 font-semibold text-neutral-900 first:mt-0 dark:text-white ${HEADING_SIZES[level - 1]}`,
    },
    ...children,
  );
}

function render(
  t: TFunction,
  nodes: readonly ReadmeNode[],
  prefix = "",
): ReactNode[] {
  return nodes.map((node, index) => {
    const key = `${prefix}${index}`;
    if (node.kind === "text") return node.text;
    if (node.kind === "image") {
      const content = (
        <>
          <PhotoIcon aria-hidden="true" className="h-3.5 w-3.5 flex-none" />
          <span className="truncate">
            {t("hub.readme.image", { label: node.label })}
          </span>
          {node.host && (
            <span className="flex-none text-neutral-500">{node.host}</span>
          )}
        </>
      );
      const chip =
        "mx-0.5 inline-flex max-w-full items-center gap-1 rounded-md border border-neutral-200 bg-neutral-50 px-1.5 py-0.5 align-middle text-xs font-normal text-neutral-600 no-underline dark:border-neutral-700 dark:bg-neutral-800 dark:text-neutral-300";
      return node.href ? (
        <ExternalLink
          key={key}
          href={node.href}
          className={`${chip} hover:bg-neutral-100 dark:hover:bg-neutral-700`}
        >
          {content}
        </ExternalLink>
      ) : (
        <span key={key} className={chip}>
          {content}
        </span>
      );
    }
    const children = render(t, node.children, `${key}.`);
    if (node.tag === "a") {
      return (
        <ExternalLink key={key} href={node.href}>
          {children}
        </ExternalLink>
      );
    }
    if (HEADINGS.has(node.tag)) {
      return heading(node.tag, key, node.align, children);
    }
    if (node.tag === "table") {
      // Les tableaux larges défilent sans élargir la page.
      return (
        <div key={key} className="overflow-x-auto">
          <table>{children}</table>
        </div>
      );
    }
    const props: Record<string, string> = { key };
    if (node.align) props.align = node.align;
    return createElement(
      node.tag,
      props,
      ...(node.tag === "br" || node.tag === "hr" ? [] : children),
    );
  });
}

export function Readme({
  markdown,
  repoId,
}: {
  markdown: string;
  repoId: string;
}) {
  const t = useT();
  const tree = useMemo(() => readmeTree(markdown, repoId), [markdown, repoId]);
  const long = useMemo(() => readmeText(tree).length > COLLAPSED_CHARS, [tree]);
  const [measured, setMeasured] = useState<boolean | null>(null);
  const [expanded, setExpanded] = useState(false);
  const contentRef = useRef<HTMLDivElement>(null);

  // Hauteur réelle mesurée quand le navigateur la donne (pas jsdom, sans
  // mise en page : hauteur nulle).
  useEffect(() => {
    const element = contentRef.current;
    if (!element || typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver(() => {
      const height = element.scrollHeight;
      setMeasured(height > 0 ? height > COLLAPSED_PX + 48 : null);
    });
    observer.observe(element);
    return () => observer.disconnect();
  }, [tree]);

  const collapsible = measured ?? long;
  const collapsed = collapsible && !expanded;
  // Focus donné par un clic (pointeur enfoncé dans le README) : le lien est
  // visible, rien à déplier ; sinon (Tab, lecteur d'écran) : déplié.
  const pointing = useRef(false);
  const onFocus = () => {
    if (collapsed && !pointing.current) setExpanded(true);
    pointing.current = false;
  };

  return (
    <div>
      <div
        ref={contentRef}
        data-testid="readme"
        onFocus={onFocus}
        onPointerDown={() => (pointing.current = true)}
        onPointerUp={() => (pointing.current = false)}
        className={`allow-context-menu prose prose-sm prose-neutral max-w-none break-words dark:prose-invert prose-headings:font-semibold ${
          collapsed ? "relative max-h-[36rem] overflow-hidden" : ""
        }`}
      >
        {render(t, tree)}
        {collapsed && (
          <div
            aria-hidden="true"
            className="pointer-events-none absolute inset-x-0 bottom-0 h-24 bg-linear-to-t from-white to-transparent dark:from-neutral-900"
          />
        )}
      </div>
      {collapsible && (
        <button
          type="button"
          aria-expanded={expanded}
          onClick={() => setExpanded((value) => !value)}
          className="mt-3 rounded-md text-sm font-medium text-neutral-700 underline-offset-2 outline-none hover:underline focus-visible:ring-2 focus-visible:ring-accent-fg dark:text-neutral-200"
        >
          {expanded ? t("hub.readme.showLess") : t("hub.readme.showMore")}
        </button>
      )}
    </div>
  );
}
