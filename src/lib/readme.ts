// README d'un dépôt Hugging Face (fiche du ModelHub, propre à ChatonLM) : le
// Markdown passe par marked, puis le HTML obtenu est relu par DOMParser
// (document inerte : rien n'y est exécuté ni chargé) et reconstruit en un
// arbre de nœuds sûrs, par liste blanche. Les README mêlent Markdown et HTML
// (tableaux d'unsloth, <details> de bartowski) et contiennent du texte qui
// ressemble à des balises (<think>, <file>) : tout élément hors de la liste
// est rendu en texte, jamais interprété. Les images ne sont jamais chargées
// (CSP, pistage) : une pastille lien les remplace (sans lien dans un lien : le
// lien parent garde sa destination, bouton Discord d'unsloth...) ; les badges
// img.shields.io sont retirés. Liens : http(s) seulement, relatifs résolus
// vers la page du fichier sur Hugging Face. Pas de rehype-raw ni de
// Streamdown ici.

import { Marked } from "marked";

/** Éléments gardés (avec leurs enfants). */
const ALLOWED = new Set([
  "p",
  "div",
  "span",
  "a",
  "br",
  "hr",
  "b",
  "strong",
  "i",
  "em",
  "sub",
  "sup",
  "small",
  "h1",
  "h2",
  "h3",
  "h4",
  "h5",
  "h6",
  "ul",
  "ol",
  "li",
  "table",
  "thead",
  "tbody",
  "tr",
  "th",
  "td",
  "details",
  "summary",
  "code",
  "pre",
  "blockquote",
]);

export type ReadmeTag =
  | "p"
  | "div"
  | "span"
  | "a"
  | "br"
  | "hr"
  | "b"
  | "strong"
  | "i"
  | "em"
  | "sub"
  | "sup"
  | "small"
  | "h1"
  | "h2"
  | "h3"
  | "h4"
  | "h5"
  | "h6"
  | "ul"
  | "ol"
  | "li"
  | "table"
  | "thead"
  | "tbody"
  | "tr"
  | "th"
  | "td"
  | "details"
  | "summary"
  | "code"
  | "pre"
  | "blockquote";

/** Éléments sans balise fermante, pour leur rendu en texte. */
const VOID = new Set([
  "area",
  "base",
  "col",
  "embed",
  "input",
  "link",
  "meta",
  "param",
  "source",
  "track",
  "wbr",
]);

const ALIGNMENTS = new Set(["left", "center", "right", "justify"]);

/** Profondeur au-delà de laquelle un élément est aplati en texte. */
const MAX_DEPTH = 48;

export type ReadmeNode =
  | { kind: "text"; text: string }
  | {
      kind: "element";
      tag: ReadmeTag;
      /** `align` d'origine (left, center, right, justify). */
      align?: string;
      /** Lien http(s) d'un `a`, absolu ; absent : lien inerte. */
      href?: string;
      children: ReadmeNode[];
    }
  | {
      kind: "image";
      /** Texte alternatif, sinon nom du fichier. */
      label: string;
      /** Hôte de l'image (« github.com »), null si l'adresse est invalide. */
      host: string | null;
      /**
       * Adresse http(s) de l'image, ouverte dans le navigateur ; null dans un
       * lien (la pastille mène où mène le lien).
       */
      href: string | null;
    };

const markdown = new Marked({ gfm: true });

/** Base des liens relatifs d'un dépôt : la page de ses fichiers. */
function baseOf(repoId: string): string {
  return `https://huggingface.co/${repoId}/blob/main/`;
}

/** Adresse http(s) absolue, ou null (javascript:, mailto:, ancre seule...). */
function resolve(href: string, repoId: string): URL | null {
  const value = href.trim();
  if (!value || value.startsWith("#")) return null;
  try {
    const url = new URL(value, baseOf(repoId));
    return url.protocol === "http:" || url.protocol === "https:" ? url : null;
  } catch {
    return null;
  }
}

/** Badge (img.shields.io) : retiré, il ne dit rien sans son image. */
function isBadge(url: URL | null): boolean {
  return url !== null && /(^|\.)shields\.io$/i.test(url.hostname);
}

function text(value: string): ReadmeNode {
  return { kind: "text", text: value };
}

/** Balise ouvrante d'origine, en texte (« <think> », « <file name="a"> »). */
function openingTag(element: Element): string {
  const attributes = [...element.attributes]
    .map((attribute) => ` ${attribute.name}="${attribute.value}"`)
    .join("");
  return `<${element.localName}${attributes}>`;
}

function image(
  element: Element,
  repoId: string,
  inLink: boolean,
): ReadmeNode | null {
  const src = element.getAttribute("src") ?? "";
  const url = resolve(src, repoId);
  if (isBadge(url)) return null;
  const file = (url?.pathname ?? src).split("/").filter(Boolean).at(-1) ?? "";
  const alt = (element.getAttribute("alt") ?? "").trim();
  return {
    kind: "image",
    label: alt || decodeName(file) || "image",
    host: url?.host ?? null,
    href: inLink ? null : (url?.href ?? null),
  };
}

function decodeName(file: string): string {
  try {
    return decodeURIComponent(file);
  } catch {
    return file;
  }
}

/**
 * Éléments où le HTML n'admet pas de texte (React le signalerait) : les
 * blancs de mise en page y sont retirés.
 */
const NO_TEXT = new Set(["table", "thead", "tbody", "tr", "ul", "ol"]);

/** Contexte de conversion : dépôt, profondeur, dans un lien ou non. */
interface Context {
  repoId: string;
  depth: number;
  inLink: boolean;
}

function convert(nodes: NodeListOf<ChildNode>, context: Context, parent = "") {
  const out: ReadmeNode[] = [];
  for (const node of nodes) {
    if (
      NO_TEXT.has(parent) &&
      node.nodeType === Node.TEXT_NODE &&
      !node.textContent?.trim()
    ) {
      continue;
    }
    out.push(...convertNode(node, context));
  }
  return out;
}

function convertNode(node: Node, context: Context): ReadmeNode[] {
  const { repoId, depth } = context;
  if (node.nodeType === Node.TEXT_NODE) {
    return node.textContent ? [text(node.textContent)] : [];
  }
  if (node.nodeType !== Node.ELEMENT_NODE) return [];
  const element = node as Element;
  const tag = element.localName.toLowerCase();
  if (depth > MAX_DEPTH) return [text(element.textContent ?? "")];
  if (tag === "img") {
    const chip = image(element, repoId, context.inLink);
    return chip ? [chip] : [];
  }
  const inner: Context = {
    repoId,
    depth: depth + 1,
    inLink: context.inLink || tag === "a",
  };
  if (!ALLOWED.has(tag)) {
    // Inconnu (<think>, <script>, <iframe>...) : montré tel qu'écrit.
    const children = convert(element.childNodes, inner);
    return VOID.has(tag)
      ? [text(openingTag(element)), ...children]
      : [text(openingTag(element)), ...children, text(`</${tag}>`)];
  }
  const children = convert(element.childNodes, inner, tag);
  const result: ReadmeNode = {
    kind: "element",
    tag: tag as ReadmeTag,
    children,
  };
  const align = element.getAttribute("align")?.trim().toLowerCase();
  if (align && ALIGNMENTS.has(align)) result.align = align;
  if (tag === "a") {
    // Lien vide (badge retiré) : rien à cliquer.
    if (children.length === 0) return [];
    const url = resolve(element.getAttribute("href") ?? "", repoId);
    if (url) result.href = url.href;
  }
  return [result];
}

/**
 * Arbre sûr du README `markdown` (sans en-tête YAML) du dépôt `repoId`
 * (« auteur/dépôt »).
 */
export function readmeTree(source: string, repoId: string): ReadmeNode[] {
  const html = markdown.parse(source, { async: false });
  const document = new DOMParser().parseFromString(
    `<!doctype html><html><body>${html}</body></html>`,
    "text/html",
  );
  return convert(document.body.childNodes, {
    repoId,
    depth: 0,
    inLink: false,
  });
}

/** Texte brut d'un arbre (tests, longueur affichée). */
export function readmeText(nodes: readonly ReadmeNode[]): string {
  return nodes
    .map((node) => {
      if (node.kind === "text") return node.text;
      if (node.kind === "image") return `Image: ${node.label}`;
      return readmeText(node.children);
    })
    .join("");
}
