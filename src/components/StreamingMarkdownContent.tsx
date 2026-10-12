/*
 * SPDX-License-Identifier: MIT
 * SPDX-FileCopyrightText: Ollama
 * SPDX-FileCopyrightText: 2026 Elwynn18 and ChatonLM contributors
 *
 * Adapted from Ollama (https://github.com/ollama/ollama),
 * file app/ui/app/src/components/StreamingMarkdownContent.tsx, commit 42e911bc3d05.
 * Modified for ChatonLM: named export; no citations or web search results;
 * every Streamdown element replaced, with only the classes the official app
 * actually generates, p and section included (same output as Streamdown's,
 * without their memoization, which kept the rendering of the stream at the
 * end of a reply); multi-line inline code kept inline; web links (http,
 * https) opened in the browser by the backend, with an Open Link / Copy Link
 * context menu instead of WebKit's and their destination as tooltip when the
 * Markdown gives no title; any other link (empty, relative, #anchor, mailto:,
 * javascript:, incomplete while streaming) rendered as an inert <a> without
 * href or target; id and aria attributes of footnote links kept; code
 * colored incrementally as soon as the highlighter is ready (lines beyond
 * the time budget of a task shown plain, then colored during idle time),
 * language names matched case-insensitively (Python), unknown languages,
 * GLSL and very long lines left plain, copied without the final newline,
 * keyboard focus ring on the copy button; tables without the node
 * attribute; while streaming, incomplete Markdown repaired
 * in the last block only and never inside a code block
 * (repairIncompleteMarkdown instead of Streamdown's parseIncompleteMarkdown),
 * so that finished blocks are never rendered again, nothing is remounted at
 * the end of a reply and the final rendering is the text as written; error
 * fallback keeps line breaks.
 */

import clsx from "clsx";
import {
  cloneElement,
  Component,
  type ComponentProps,
  createElement,
  type ErrorInfo,
  Fragment,
  isValidElement,
  type JSX,
  memo,
  type ReactNode,
  useMemo,
  useState,
  useSyncExternalStore,
} from "react";
import {
  Streamdown,
  type StreamdownProps,
  defaultRehypePlugins,
  defaultRemarkPlugins,
} from "streamdown";
import type { ThemedToken } from "shiki/core";
import { CopyButton } from "@/components/CopyButton";
import { ExternalLink } from "@/components/ExternalLink";
import {
  createBlockTokenizer,
  resolveLanguage,
  useHighlighter,
} from "@/lib/highlighter";
import { repairIncompleteMarkdown } from "@/lib/markdownRepair";

interface StreamingMarkdownContentProps {
  content: string;
  isStreaming?: boolean;
  size?: "sm" | "md" | "lg";
}

type Tag = keyof JSX.IntrinsicElements;
/** Propriétés reçues de react-markdown : celles de l'élément, plus son nœud hast. */
type ElementProps<T extends Tag> = ComponentProps<T> & { node?: unknown };

// Helper to extract text from React nodes
const extractText = (node: ReactNode): string => {
  if (typeof node === "string") return node;
  if (typeof node === "number") return String(node);
  if (!node) return "";
  if (isValidElement<{ children?: ReactNode }>(node)) {
    return extractText(node.props.children);
  }
  if (Array.isArray(node)) {
    return node.map(extractText).join("");
  }
  return "";
};

const safeRehypePlugins = [defaultRehypePlugins.katex];
const remarkPlugins = [defaultRemarkPlugins.gfm, defaultRemarkPlugins.math];

// Le surligneur ne lève une exception que pour une expression de grammaire que
// le moteur refuse (déjà signalée par le préchauffage) : une seule trace, le
// bloc reste en texte brut.
let highlightFailureReported = false;
function reportHighlightFailure(error: unknown) {
  if (highlightFailureReported) return;
  highlightFailureReported = true;
  console.error("Failed to highlight code:", error);
}

/**
 * Un élément de Streamdown avec ses classes. Streamdown pose ses propres
 * classes Tailwind, mais Tailwind ne lit pas node_modules : chez l'officiel,
 * seules celles qui apparaissent aussi dans ses sources existent. Chaque
 * élément est donc remplacé par un équivalent qui ne porte que les classes
 * réellement actives chez l'officiel (la liste ci-dessous) ; écrire ici une
 * classe inactive là-bas (taille des h1 et h3, listes, bordure des citations)
 * la ferait générer et changerait le rendu.
 */
function markdownElement<T extends Tag>(tag: T, classes: string, slot: string) {
  return function MarkdownElement(
    props: ElementProps<T> & { className?: string },
  ) {
    return createElement(tag as string, {
      ...withoutNode(props),
      className: clsx(classes, props.className) || undefined,
      "data-streamdown": slot,
    });
  };
}

/**
 * Propriétés à poser sur l'élément : sans le nœud hast, que React écrirait
 * tel quel (node="[object Object]").
 */
function withoutNode<P extends { node?: unknown }>(props: P): Omit<P, "node"> {
  const rest = { ...props };
  delete rest.node;
  return rest;
}

function MarkdownLink(props: ElementProps<"a">) {
  // Les autres attributs posés par react-markdown suivent (id, aria-* et
  // data-* des notes GFM), sauf target et rel : le lien ne s'ouvre jamais
  // dans la webview.
  const { href, title, className, children, ...attributes } =
    withoutNode(props);
  delete attributes.target;
  delete attributes.rel;
  return (
    <ExternalLink
      {...attributes}
      href={href}
      title={title}
      className={clsx("font-medium underline", className)}
      data-streamdown="link"
    >
      {children}
    </ExternalLink>
  );
}

/** Enfants d'un élément en tableau, tels quels (null et "" compris). */
function childList(children: ReactNode): ReactNode[] {
  return Array.isArray(children) ? children : [children];
}

/**
 * Paragraphe : celui de Streamdown (une image seule n'est pas enveloppée
 * dans un <p>), sans sa mémo. Streamdown ne rend un paragraphe de nouveau
 * que si sa position dans le texte change : réparé pendant le flux, il
 * gardait à la fin le rendu de la réparation quand le texte final a la même
 * longueur (adresse de lien de 26 caractères comme le marqueur du lien
 * incomplet, « **gras** » fermé par la réparation au lieu de « **gras ») ou
 * quand seul le reste du texte change (appel de note résolu plus tard).
 */
function MarkdownParagraph(props: ElementProps<"p">) {
  const { children, className, ...attributes } = withoutNode(props);
  const kept = childList(children).filter(
    (child) => child != null && child !== "",
  );
  if (
    kept.length === 1 &&
    isValidElement<{ node?: { tagName?: string } }>(kept[0]) &&
    kept[0].props.node?.tagName === "img"
  ) {
    return <>{children}</>;
  }
  return (
    <p className={className} {...attributes}>
      {children}
    </p>
  );
}

const MarkdownOrderedList = markdownElement("ol", "", "ordered-list");

type FootnoteChildProps = {
  children?: ReactNode;
  "data-footnote-backref"?: unknown;
};

/**
 * Note sans texte (seulement son lien de retour), que Streamdown retire de
 * la liste des notes : même règle que la sienne.
 */
function isEmptyFootnote(item: ReactNode): boolean {
  if (!isValidElement<FootnoteChildProps>(item)) return false;
  let content = false;
  let backref = false;
  for (const child of childList(item.props.children)) {
    if (!child) continue;
    if (typeof child === "string") {
      if (child.trim() !== "") content = true;
    } else if (isValidElement<FootnoteChildProps>(child)) {
      if (child.props["data-footnote-backref"] !== undefined) {
        backref = true;
      } else if (
        childList(child.props.children).some((grandchild) =>
          typeof grandchild === "string"
            ? grandchild.trim() !== ""
            : isValidElement<FootnoteChildProps>(grandchild) &&
              grandchild.props["data-footnote-backref"] === undefined,
        )
      ) {
        content = true;
      }
    }
  }
  return backref && !content;
}

/**
 * Section : celle de Streamdown, sans sa mémo (nœud sans position : jamais
 * rendue de nouveau, les notes gardaient leur texte du premier rendu). Dans
 * celle des notes, les notes vides sont retirées, et la section aussi s'il
 * n'en reste aucune ; la liste des notes est ici MarkdownOrderedList, que la
 * section de Streamdown ne reconnaît pas (elle attend son propre `ol`).
 */
function MarkdownSection(props: ElementProps<"section">) {
  const { children, className, ...attributes } = withoutNode(props);
  if (!("data-footnotes" in attributes)) {
    return (
      <section className={className} {...attributes}>
        {children}
      </section>
    );
  }
  const kept = Array.isArray(children)
    ? children.map((child: ReactNode) => {
        if (!isValidElement<{ children?: ReactNode }>(child)) return child;
        if (child.type !== MarkdownOrderedList) return child;
        const notes = childList(child.props.children).filter(
          (note) => !isEmptyFootnote(note),
        );
        return notes.length === 0 ? null : cloneElement(child, {}, ...notes);
      })
    : children;
  const visible = Array.isArray(kept)
    ? kept.some((child) => child !== null)
    : kept !== null;
  return visible ? (
    <section className={className} {...attributes}>
      {kept}
    </section>
  ) : null;
}

function MarkdownImage({ alt }: ElementProps<"img">) {
  // Jamais d'image distante (pistage) : seulement son texte de remplacement.
  return alt ? <span>{alt}</span> : null;
}

function MarkdownTable(props: ElementProps<"table">) {
  return (
    <div className="overflow-x-auto max-w-full">
      <table
        {...withoutNode(props)}
        className="border-collapse w-full border border-neutral-200 dark:border-neutral-700 rounded-lg overflow-hidden"
      >
        {props.children}
      </table>
    </div>
  );
}

/**
 * Une ligne de code colorée. Pendant le flux, le tokenizer rend les mêmes
 * tableaux pour les lignes déjà terminées : seules la dernière ligne et les
 * nouvelles sont rendues de nouveau, pas tout le bloc à chaque fragment.
 * darkColors : table des couleurs du thème sombre, null pour le thème clair.
 */
const CodeLine = memo(function CodeLine({
  tokens,
  darkColors,
}: {
  tokens: ThemedToken[];
  darkColors: ReadonlyMap<string, string> | null;
}) {
  return tokens.map((token, j) => {
    const color =
      darkColors && token.color ? darkColors.get(token.color) : token.color;
    return (
      <span key={j} style={color ? { color } : undefined}>
        {token.content}
      </span>
    );
  });
});

const CodeBlock = memo(function CodeBlock({ children }: ElementProps<"pre">) {
  // Extract code and language from children : l'élément <code> préparé par
  // react-markdown (jamais rendu ici), « language-xxx » et le texte, qui se
  // termine par "\n".
  const code = isValidElement<{ className?: string; children?: ReactNode }>(
    children,
  )
    ? children.props
    : { className: undefined, children };
  const language = code.className?.replace(/language-/, "") || "";
  const codeText = extractText(code.children);

  // Coloration dès que le surligneur est prêt (rendu de nouveau à ce
  // moment) ; pendant le flux, seules les nouvelles lignes sont tokenisées.
  // Un rendu ne dépasse pas le budget de temps de la tâche : les lignes qui
  // n'ont pas eu le temps restent en texte brut, puis le tokenizer les colore
  // pendant les temps morts et change de version, ce qui rend le bloc de
  // nouveau (les lignes déjà rendues gardent leurs tableaux : CodeLine).
  const highlighter = useHighlighter();
  const [tokenize] = useState(createBlockTokenizer);
  const version = useSyncExternalStore(tokenize.subscribe, tokenize.getVersion);
  const lang = highlighter ? resolveLanguage(highlighter, language) : null;
  const lines = useMemo(() => {
    if (!highlighter || !lang) return null;
    try {
      // Même texte, version nouvelle : le tokenizer rend les lignes colorées
      // depuis le dernier rendu.
      void version;
      return tokenize(highlighter, codeText, lang);
    } catch (error) {
      reportHighlightFailure(error);
      return null;
    }
  }, [highlighter, lang, codeText, tokenize, version]);

  const render = (darkColors: ReadonlyMap<string, string> | null) =>
    lines
      ? lines.map((line, i) => (
          <Fragment key={i}>
            <CodeLine tokens={line} darkColors={darkColors} />
            {i < lines.length - 1 && "\n"}
          </Fragment>
        ))
      : codeText;

  return (
    <div className="relative bg-neutral-100 dark:bg-neutral-800 rounded-2xl overflow-hidden my-6">
      <div className="flex select-none" data-code-block-header="">
        {language && (
          <div className="text-[13px] text-neutral-500 dark:text-neutral-400 font-mono px-4 py-2">
            {language}
          </div>
        )}
        <CopyButton
          // Sans le saut de ligne final : collé dans un terminal, il
          // exécuterait la dernière commande.
          content={codeText.replace(/\n$/, "")}
          showLabels={true}
          className="copy-button text-neutral-500 dark:text-neutral-400 bg-neutral-100 dark:bg-neutral-800 ml-auto focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-accent-fg"
        />
      </div>
      {/* Light mode */}
      <pre
        className="dark:hidden m-0 bg-neutral-100 text-sm overflow-x-auto p-4"
        data-theme="light"
      >
        <code className="font-mono text-sm">{render(null)}</code>
      </pre>
      {/* Dark mode */}
      <pre
        className="hidden dark:block m-0 bg-neutral-800 text-sm overflow-x-auto p-4"
        data-theme="dark"
      >
        <code className="font-mono text-sm">
          {render(highlighter?.lightToDark ?? null)}
        </code>
      </pre>
    </div>
  );
});

// Composants au niveau du module : Streamdown et ses blocs sont mémoïsés sur
// le texte seulement, un objet recréé à chaque rendu serait ignoré. Aucun
// n'est mémoïsé : un bloc rendu de nouveau l'est entièrement (voir
// MarkdownParagraph).
const components: NonNullable<StreamdownProps["components"]> = {
  h1: markdownElement("h1", "mt-6 mb-2 font-semibold", "heading-1"),
  h2: markdownElement("h2", "mt-6 mb-2 font-semibold text-2xl", "heading-2"),
  h3: markdownElement("h3", "mt-6 mb-2 font-semibold", "heading-3"),
  h4: markdownElement("h4", "mt-6 mb-2 font-semibold text-lg", "heading-4"),
  h5: markdownElement("h5", "mt-6 mb-2 font-semibold text-base", "heading-5"),
  h6: markdownElement("h6", "mt-6 mb-2 font-semibold text-sm", "heading-6"),
  ul: markdownElement("ul", "", "unordered-list"),
  ol: MarkdownOrderedList,
  li: markdownElement("li", "py-1", "list-item"),
  hr: markdownElement("hr", "my-6", "horizontal-rule"),
  // Gras en <span>, comme Streamdown (couleur du texte, pas celle de
  // prose-strong) ; cleanCopyHtml en fait un <strong> dans la copie.
  strong: markdownElement("span", "font-semibold", "strong"),
  blockquote: markdownElement("blockquote", "my-4 italic", "blockquote"),
  // Tout code en ligne, même coupé par un saut de ligne (Streamdown en ferait
  // un bloc Shiki dans le paragraphe). Le <code> d'un bloc n'est jamais rendu :
  // CodeBlock lit ses propriétés.
  code: markdownElement(
    "code",
    "rounded px-1.5 py-0.5 font-mono text-sm",
    "inline-code",
  ),
  thead: markdownElement("thead", "", "table-header"),
  tbody: markdownElement("tbody", "", "table-body"),
  tr: markdownElement("tr", "border-b", "table-row"),
  th: markdownElement(
    "th",
    "whitespace-nowrap px-4 py-2 text-left font-semibold text-sm",
    "table-header-cell",
  ),
  td: markdownElement("td", "px-4 py-2 text-sm", "table-cell"),
  sup: markdownElement("sup", "text-sm", "superscript"),
  sub: markdownElement("sub", "text-sm", "subscript"),
  a: MarkdownLink,
  img: MarkdownImage,
  table: MarkdownTable,
  pre: CodeBlock,
  p: MarkdownParagraph,
  section: MarkdownSection,
};

export const StreamingMarkdownContent = memo(function StreamingMarkdownContent({
  content,
  isStreaming = false,
  size,
}: StreamingMarkdownContentProps) {
  return (
    <div
      className={`
        max-w-full
        ${size === "sm" ? "prose-sm" : size === "lg" ? "prose-lg" : ""}
        prose
        prose-neutral
        prose-headings:font-semibold
        prose:text-neutral-800
        prose-strong:font-semibold
        prose-ul:marker:text-neutral-700
        prose-li:marker:text-neutral-700
        prose-headings:text-neutral-800
        prose-pre:bg-transparent
        prose-pre:rounded-xl
        prose-pre:text-neutral-800
        prose-pre:font-normal
        prose-pre:my-0
        prose-pre:max-w-full
        prose-pre:pt-1
        [&_table]:border-collapse
        [&_table]:w-full
        [&_table]:border
        [&_table]:border-neutral-200
        [&_table]:rounded-lg
        [&_table]:overflow-hidden
        [&_th]:px-3
        [&_th]:py-2
        [&_th]:text-left
        [&_th]:font-semibold
        [&_th]:border-b
        [&_th]:border-r
        [&_th]:border-neutral-200
        [&_th:last-child]:border-r-0
        [&_td]:px-3
        [&_td]:py-2
        [&_td]:border-r
        [&_td]:border-neutral-200
        [&_td:last-child]:border-r-0
        [&_tbody_tr:not(:last-child)_td]:border-b
        [&_code:not(pre_code)]:text-neutral-700
        [&_code:not(pre_code)]:bg-neutral-100
        [&_code:not(pre_code)]:font-normal
        [&_code:not(pre_code)]:px-1.5
        [&_code:not(pre_code)]:py-0.5
        [&_code:not(pre_code)]:rounded-md
        [&_code:not(pre_code)]:text-[90%]
        [&_code:not(pre_code)]:before:hidden
        [&_code:not(pre_code)]:after:hidden
        dark:prose-invert
        dark:prose:text-neutral-200
        dark:prose-pre:bg-none
        dark:prose-headings:text-neutral-200
        dark:prose-strong:text-neutral-200
        dark:prose-pre:text-neutral-200
        dark:prose:pre:text-neutral-200
        dark:[&_table]:border-neutral-700
        dark:[&_thead]:bg-neutral-800
        dark:[&_th]:border-neutral-700
        dark:[&_td]:border-neutral-700
        dark:[&_code:not(pre_code)]:text-neutral-200
        dark:[&_code:not(pre_code)]:bg-neutral-800
        dark:[&_code:not(pre_code)]:font-normal
        dark:prose-ul:marker:text-neutral-300
        dark:prose-li:marker:text-neutral-300
        break-words
      `}
    >
      <StreamingMarkdownErrorBoundary
        content={content}
        isStreaming={isStreaming}
      >
        <MarkdownBlocks content={content} isStreaming={isStreaming} />
      </StreamingMarkdownErrorBoundary>
    </div>
  );
});

/**
 * Streamdown et la réparation du flux, tous deux sous l'ErrorBoundary : le
 * lexer de marked peut lever une exception, ici comme dans Streamdown.
 */
function MarkdownBlocks({
  content,
  isStreaming,
}: {
  content: string;
  isStreaming: boolean;
}) {
  return (
    <Streamdown
      // Pas de réparation par Streamdown : il répare chaque bloc, y compris
      // les blocs finis et l'intérieur d'un bloc de code ouvert, et ne
      // réanalyse rien à la fin du flux (ses réparations resteraient).
      // Réparé ici, seul le dernier bloc change ; les autres, mémoïsés sur
      // leur texte, ne sont jamais rendus de nouveau, et la fin du flux ne
      // refait que le dernier bloc s'il était réparé (sélection, focus et état
      // des boutons gardés).
      parseIncompleteMarkdown={false}
      isAnimating={isStreaming}
      rehypePlugins={safeRehypePlugins}
      remarkPlugins={remarkPlugins}
      controls={false}
      components={components}
    >
      {isStreaming ? repairIncompleteMarkdown(content) : content}
    </Streamdown>
  );
}

interface StreamingMarkdownErrorBoundaryProps {
  content: string;
  children: ReactNode;
  isStreaming: boolean;
}

// Sometimes remark will throw errors, particularly when rendering math. We add
// this fallback to show the plain text content if there's an error, and then we
// retry rendering the content if the content changes OR if we change our
// streaming state (because we render things differently when in streaming mode
// v. not, so for some cases we'll automatically recover once streaming is over)
//
// This should not be relied on for anything known to be broken (any known
// errors should be fixed!), but it's necessary to not break the full UI because
// of some bad markdown
class StreamingMarkdownErrorBoundary extends Component<
  StreamingMarkdownErrorBoundaryProps,
  { hasError: boolean }
> {
  constructor(props: StreamingMarkdownErrorBoundaryProps) {
    super(props);
    this.state = { hasError: false };
  }

  componentDidUpdate(prevProps: StreamingMarkdownErrorBoundaryProps) {
    if (
      this.state.hasError &&
      (prevProps.isStreaming !== this.props.isStreaming ||
        prevProps.content !== this.props.content)
    ) {
      this.setState({ hasError: false });
    }
  }

  static getDerivedStateFromError(/*_error: Error*/) {
    return { hasError: true };
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
    console.error(
      "StreamingMarkdownContent: caught rendering error",
      error,
      info,
    );
  }

  render() {
    if (this.state.hasError) {
      // Texte brut, retours à la ligne gardés.
      return (
        <div className="whitespace-pre-wrap break-words">
          {this.props.content}
        </div>
      );
    }

    return this.props.children;
  }
}
