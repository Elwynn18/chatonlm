// Rendu des réponses avec le vrai Streamdown (alias de vite.config.ts :
// llm-math, shiki et mermaid neutralisés), dans jsdom. Les réglages passés à
// Streamdown sont vérifiés à part (StreamingMarkdownContent.test.tsx).

import {
  act,
  cleanup,
  fireEvent,
  render,
  renderHook,
  waitFor,
} from "@testing-library/react";
import type { ThemedToken } from "shiki/core";
import { afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import longReply from "../../src-tauri/crates/fake-ollama/fixtures/markdown-long.md?raw";
import streamReply from "../../src-tauri/crates/fake-ollama/fixtures/markdown-stream.md?raw";
import fullReply from "../../src-tauri/crates/fake-ollama/fixtures/markdown.md?raw";
import type * as HighlighterModule from "@/lib/highlighter";
import { preloadHighlighter, useHighlighter } from "@/lib/highlighter";
import { resetLinks } from "@/lib/openLink";
import { StreamingMarkdownContent } from "./StreamingMarkdownContent";

// Rendus complets de réponses entières, fragment par fragment : quelques
// secondes seuls, davantage quand toute la suite tourne en parallèle.
vi.setConfig({ testTimeout: 30_000 });

const openExternal = vi.hoisted(() => vi.fn(() => Promise.resolve()));
vi.mock("@/lib/ipc", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/lib/ipc")>()),
  openExternal,
}));

const clipboard = vi.hoisted(() => ({
  copyText: vi.fn<(text: string) => Promise<void>>(() => Promise.resolve()),
  copyRich: vi.fn<(html: string, text: string) => Promise<void>>(() =>
    Promise.resolve(),
  ),
}));
vi.mock("@/lib/clipboard", () => clipboard);

// Tokenizers des blocs de code : nombre créé (un par bloc monté) et textes
// reçus, pour suivre ce que CodeBlock leur donne ; `failing` : langage pour
// lequel ils lèvent une exception (moteur d'expressions trop ancien) ;
// `lineRenders` : si non null, rendus de CodeLine comptés (voir counted).
const tokenizers = vi.hoisted(() => ({
  created: 0,
  texts: [] as string[],
  failing: null as string | null,
  lineRenders: null as number | null,
}));
vi.mock("@/lib/highlighter", async (importOriginal) => {
  const actual = await importOriginal<typeof HighlighterModule>();
  // Une ligne rendue par le tokenizer est remplacée par une copie dont le
  // premier jeton compte la lecture de son texte : une par rendu de CodeLine.
  // Même ligne, même copie : la mémo de CodeLine voit ce que le tokenizer
  // lui donne.
  const copies = new WeakMap<ThemedToken[], ThemedToken[]>();
  const counted = (line: ThemedToken[]) => {
    let copy = copies.get(line);
    if (!copy) {
      copy = line.map((token, j) =>
        j > 0
          ? token
          : Object.defineProperty({ ...token }, "content", {
              enumerable: true,
              get() {
                tokenizers.lineRenders = (tokenizers.lineRenders ?? 0) + 1;
                return token.content;
              },
            }),
      );
      copies.set(line, copy);
    }
    return copy;
  };
  return {
    ...actual,
    createBlockTokenizer: (): HighlighterModule.BlockTokenizer => {
      tokenizers.created += 1;
      const tokenize = actual.createBlockTokenizer();
      return Object.assign(
        (h: HighlighterModule.Highlighter, text: string, lang: string) => {
          tokenizers.texts.push(text);
          if (lang === tokenizers.failing) {
            throw new SyntaxError("Invalid regular expression");
          }
          const lines = tokenize(h, text, lang);
          return tokenizers.lineRenders === null ? lines : lines.map(counted);
        },
        {
          subscribe: (onProgress: () => void) => tokenize.subscribe(onProgress),
          getVersion: () => tokenize.getVersion(),
        },
      );
    },
  };
});

afterEach(() => {
  cleanup();
  // Minuteries simulées encore en attente (fin de la fenêtre du budget de la
  // tâche, même après un échec) : jouées, pour que le test suivant ait son
  // propre budget.
  if (vi.isFakeTimers()) vi.runOnlyPendingTimers();
  resetLinks();
  vi.restoreAllMocks();
  vi.useRealTimers();
  openExternal.mockClear();
  clipboard.copyText.mockClear();
  clipboard.copyRich.mockClear();
  tokenizers.created = 0;
  tokenizers.texts = [];
  tokenizers.failing = null;
  tokenizers.lineRenders = null;
});

function renderMarkdown(content: string, isStreaming = false) {
  const view = render(
    <StreamingMarkdownContent content={content} isStreaming={isStreaming} />,
  );
  const show = (next: string, streaming: boolean) =>
    view.rerender(
      <StreamingMarkdownContent content={next} isStreaming={streaming} />,
    );
  return { ...view, show };
}

/** `text` en fragments de `size` caractères. */
function fragmentsOf(text: string, size: number): string[] {
  const parts: string[] = [];
  for (let i = 0; i < text.length; i += size) {
    parts.push(text.slice(i, i + size));
  }
  return parts;
}

/**
 * HTML rendu, comparable d'un rendu à l'autre. Le code est comparé en
 * texte : sa coloration a sa propre file. Ordre des attributs et style vidé :
 * sans effet (un élément mis à jour pendant le flux, un lien devenu
 * cliquable, garde l'ordre de ses ajouts).
 */
function normalizedHtml(root: Element): string {
  const copy = root.cloneNode(true) as Element;
  for (const code of copy.querySelectorAll("pre code")) {
    code.replaceChildren(code.textContent ?? "");
  }
  for (const element of copy.querySelectorAll("*")) {
    const attributes = [...element.attributes]
      .filter((a) => !(a.name === "style" && a.value === ""))
      .map((a) => [a.name, a.value])
      .sort(([a], [b]) => a.localeCompare(b));
    for (const name of element.getAttributeNames()) {
      element.removeAttribute(name);
    }
    for (const [name, value] of attributes) {
      element.setAttribute(name, value);
    }
  }
  return copy.innerHTML;
}

/**
 * Rendu de `reply` reçu en `parts` puis fin du flux (`streamed`), et rendu
 * direct du même texte (`direct`). `keep` : le rendu du flux reste monté
 * (`container`), le rendu direct est fait à part.
 */
function streamThenEnd(reply: string, parts: string[], { keep = false } = {}) {
  expect(parts.join("")).toBe(reply);
  const { container, show } = renderMarkdown(parts[0], true);
  let text = parts[0];
  for (const part of parts.slice(1)) {
    text += part;
    show(text, true);
  }
  show(reply, false);
  const streamed = normalizedHtml(container);
  if (!keep) cleanup();
  const other = render(<StreamingMarkdownContent content={reply} />);
  const direct = normalizedHtml(other.container);
  other.unmount();
  return { container, streamed, direct };
}

/** Jetons colorés d'un <pre> (clair ou sombre). */
function coloredSpans(container: Element, theme: "light" | "dark") {
  return [
    ...container.querySelectorAll<HTMLElement>(
      `pre[data-theme="${theme}"] code span[style]`,
    ),
  ];
}

/** Lignes en texte brut (un seul <span> sans couleur) d'un <pre>. */
function plainSpans(container: Element, theme: "light" | "dark") {
  return [
    ...container.querySelectorAll<HTMLElement>(
      `pre[data-theme="${theme}"] code span:not([style])`,
    ),
  ];
}

/**
 * Temps figé : le budget de la tâche n'est jamais entamé et vscode-textmate
 * ne coupe jamais une ligne, quelle que soit la vitesse de la machine (ou la
 * compilation des expressions, faite au premier usage) : un bloc est
 * entièrement coloré dès son rendu.
 */
function freezeTime() {
  vi.spyOn(Date, "now").mockReturnValue(1_000_000_000);
  vi.spyOn(performance, "now").mockReturnValue(1_000_000_000);
}

/** Le highlighter chargé, celui que rendent les blocs de code. */
function loadedHighlighter(): HighlighterModule.Highlighter {
  const { result, unmount } = renderHook(() => useHighlighter());
  unmount();
  return result.current!;
}

/**
 * Temps simulé, minuteries simulées (jouées à la demande) : chaque ligne
 * python tokenisée coûte `cost` ms (performance.now, que lit le budget du
 * tokenizer) ; Date.now figé, vscode-textmate ne coupe donc que les lignes
 * dont le coût dépasse la limite qu'il reçoit.
 */
async function simulateLineCost(cost: number) {
  // Minuteries réelles en attente servies d'abord (fin de la tâche d'un test
  // précédent) : le budget de ce test part de zéro.
  await new Promise((resolve) => setTimeout(resolve, 0));
  const grammar = loadedHighlighter().core.getLanguage("python");
  const tokenizeLine2 = grammar.tokenizeLine2.bind(grammar);
  // Loin du temps réel : la fenêtre du budget est rouverte.
  let now = 2_000_000_000;
  vi.spyOn(performance, "now").mockImplementation(() => now);
  vi.spyOn(Date, "now").mockReturnValue(1_000_000_000);
  vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
  const start = now;
  const lines = vi
    .spyOn(grammar, "tokenizeLine2")
    .mockImplementation((line, state, timeLimit = 0) => {
      if (timeLimit > 0 && cost > timeLimit) {
        now += timeLimit + 1;
        return {
          tokens: new Uint32Array([0, 0]),
          ruleStack: state!,
          stoppedEarly: true,
        };
      }
      now += cost;
      return tokenizeLine2(line, state, 0);
    });
  return { elapsed: () => now - start, lines };
}

const PYTHON = "```python\ndef greet(name):\n    return name\n```";

// Le surligneur se charge à part : avant, le code reste en texte brut.
describe("code blocks before the highlighter is loaded", () => {
  it("stay plain, then are colored once it is ready", async () => {
    freezeTime();
    const { container } = renderMarkdown(PYTHON);
    const light = container.querySelector('pre[data-theme="light"] code');
    expect(light?.textContent).toBe("def greet(name):\n    return name\n");
    expect(coloredSpans(container, "light")).toHaveLength(0);

    await act(() => preloadHighlighter());

    // Rendu de nouveau dès que le surligneur est prêt (temps figé : dans le
    // budget de la tâche).
    expect(coloredSpans(container, "light").length).toBeGreaterThan(3);
    expect(plainSpans(container, "light")).toHaveLength(0);
    expect(light?.textContent).toBe("def greet(name):\n    return name\n");
  });
});

describe("StreamingMarkdownContent (real Streamdown)", () => {
  beforeAll(async () => {
    await act(() => preloadHighlighter());
  });

  it("gives headings only the classes the official app generates", () => {
    const { container } = renderMarkdown(
      "# One\n\n## Two\n\n### Three\n\n#### Four\n\n##### Five\n\n###### Six",
    );
    const classes = [
      ...container.querySelectorAll("h1, h2, h3, h4, h5, h6"),
    ].map(
      (h) => `${h.tagName} ${h.className} ${h.getAttribute("data-streamdown")}`,
    );
    expect(classes).toEqual([
      "H1 mt-6 mb-2 font-semibold heading-1",
      "H2 mt-6 mb-2 font-semibold text-2xl heading-2",
      "H3 mt-6 mb-2 font-semibold heading-3",
      "H4 mt-6 mb-2 font-semibold text-lg heading-4",
      "H5 mt-6 mb-2 font-semibold text-base heading-5",
      "H6 mt-6 mb-2 font-semibold text-sm heading-6",
    ]);
  });

  it("renders lists, bold, quotes, rules and inline code like the official app", () => {
    const { container } = renderMarkdown(
      [
        "- a\n- b",
        "1. one",
        "> quote with **bold**",
        "---",
        "Text `code` and `two\nlines` here, H~2~O x^2^",
      ].join("\n\n"),
    );
    const html = container.innerHTML;
    // Ni marge ni puces de Streamdown : prose s'en charge.
    expect(container.querySelector("ul")?.outerHTML).toMatch(
      /^<ul data-streamdown="unordered-list">/,
    );
    expect(container.querySelector("ol")?.getAttribute("class")).toBeNull();
    expect(container.querySelector("li")?.className).toBe("py-1");
    expect(container.querySelector("blockquote")?.className).toBe(
      "my-4 italic",
    );
    expect(container.querySelector("hr")?.className).toBe("my-6");
    // Gras en <span>, comme Streamdown.
    const bold = container.querySelector('[data-streamdown="strong"]');
    expect(bold?.tagName).toBe("SPAN");
    expect(bold?.className).toBe("font-semibold");
    expect(container.querySelector("strong")).toBeNull();
    // Code en ligne, même coupé par un saut de ligne : jamais un bloc.
    const inline = [...container.querySelectorAll("p code")];
    expect(inline.map((c) => c.textContent)).toEqual(["code", "two lines"]);
    for (const code of inline) {
      expect(code.className).toBe("rounded px-1.5 py-0.5 font-mono text-sm");
      expect(code.getAttribute("data-streamdown")).toBe("inline-code");
    }
    expect(container.querySelectorAll("pre")).toHaveLength(0);
    expect(container.querySelector("del")?.textContent).toBe("2");
    expect(html).not.toMatch(/node=|ml-4|list-disc|border-l-4|text-3xl/);
  });

  it("wraps tables for horizontal scrolling, with alignment and no node attribute", () => {
    const { container } = renderMarkdown(
      "| Model | Size |\n|:--|--:|\n| qwen3:0.6b | 523 MB |",
    );
    const table = container.querySelector("table");
    expect(table?.parentElement?.className).toBe("overflow-x-auto max-w-full");
    expect(table?.className).toBe(
      "border-collapse w-full border border-neutral-200 dark:border-neutral-700 rounded-lg overflow-hidden",
    );
    expect(table?.hasAttribute("node")).toBe(false);
    expect(container.querySelector("thead")?.getAttribute("class")).toBeNull();
    expect(container.querySelector("tr")?.className).toBe("border-b");
    const th = container.querySelectorAll("th");
    expect(th[0].className).toBe(
      "whitespace-nowrap px-4 py-2 text-left font-semibold text-sm",
    );
    expect(th[1].style.textAlign).toBe("right");
    expect(container.querySelector("td")?.className).toBe("px-4 py-2 text-sm");
  });

  it("colors a code block on its first render, light and dark", () => {
    freezeTime();
    const { container } = renderMarkdown(PYTHON);

    const header = container.querySelector("[data-code-block-header]");
    expect(header?.className).toBe("flex select-none");
    expect(header?.firstElementChild?.textContent).toBe("python");
    const copy = header?.querySelector("button.copy-button");
    expect(copy?.textContent).toBe("Copy");
    expect(copy?.className).toContain("focus-visible:ring-inset");

    const light = coloredSpans(container, "light");
    const dark = coloredSpans(container, "dark");
    expect(light.length).toBeGreaterThan(3);
    expect(dark).toHaveLength(light.length);
    const keyword = light.find((s) => s.textContent === "def");
    // one-light #A626A4, one-dark #C678DD
    expect(keyword?.style.color).toBe("rgb(166, 38, 164)");
    expect(dark.find((s) => s.textContent === "def")?.style.color).toBe(
      "rgb(198, 120, 221)",
    );
    for (const theme of ["light", "dark"]) {
      expect(
        container.querySelector(`pre[data-theme="${theme}"] code`)?.textContent,
      ).toBe("def greet(name):\n    return name\n");
    }
    expect(container.querySelector('pre[data-theme="light"]')?.className).toBe(
      "dark:hidden m-0 bg-neutral-100 text-sm overflow-x-auto p-4",
    );
    expect(container.querySelector('pre[data-theme="dark"]')?.className).toBe(
      "hidden dark:block m-0 bg-neutral-800 text-sm overflow-x-auto p-4",
    );
  });

  it("keeps the language label as written and matches it case-insensitively", () => {
    freezeTime();
    const { container } = renderMarkdown("```Python\nx = 1\n```");
    expect(
      container.querySelector("[data-code-block-header]")?.firstElementChild
        ?.textContent,
    ).toBe("Python");
    expect(coloredSpans(container, "light").length).toBeGreaterThan(0);
  });

  it("shows a block without language plain, without a label", () => {
    const { container } = renderMarkdown(
      "```\nno   language\n    indented\n```",
    );
    const header = container.querySelector("[data-code-block-header]");
    expect(header?.children).toHaveLength(2); // bouton et annonce
    expect(header?.firstElementChild?.tagName).toBe("BUTTON");
    expect(coloredSpans(container, "light")).toHaveLength(0);
    expect(
      container.querySelector('pre[data-theme="light"] code')?.textContent,
    ).toBe("no   language\n    indented\n");
  });

  it("shows an unknown language plain, without logging errors", () => {
    const error = vi.spyOn(console, "error");
    const { container, show } = renderMarkdown('```ruby\nputs "a"', true);
    show('```ruby\nputs "a"\nputs "b"', true);
    show('```ruby\nputs "a"\nputs "b"\n```', false);

    expect(
      container.querySelector("[data-code-block-header]")?.firstElementChild
        ?.textContent,
    ).toBe("ruby");
    expect(coloredSpans(container, "light")).toHaveLength(0);
    expect(
      container.querySelector('pre[data-theme="dark"] code')?.textContent,
    ).toBe('puts "a"\nputs "b"\n');
    expect(error).not.toHaveBeenCalled();
    // Langage vérifié avant : Shiki n'est jamais appelé pour lui.
    expect(tokenizers.texts).toEqual([]);
  });

  it("copies a code block without its final newline", async () => {
    const { container } = renderMarkdown(PYTHON);
    const copy = container.querySelector<HTMLButtonElement>(
      "[data-code-block-header] button",
    )!;
    await act(async () => {
      fireEvent.click(copy);
    });
    expect(clipboard.copyText).toHaveBeenCalledWith(
      "def greet(name):\n    return name",
    );
    expect(copy.textContent).toBe("Copied");
  });

  it("shows raw HTML as text and never loads images", () => {
    const { container } = renderMarkdown(
      [
        "<script>window.__pwned = true</script>",
        '<iframe src="https://example.com"></iframe>',
        'Inline <img src="x" onerror="window.__pwned = true" /> tag',
        "![secret](https://attacker.example/pixel.png) and ![](https://attacker.example/empty.png)",
      ].join("\n\n"),
    );
    expect(container.querySelector("script, iframe, img")).toBeNull();
    expect(container.textContent).toContain(
      "<script>window.__pwned = true</script>",
    );
    expect(container.textContent).toContain("<iframe");
    expect(container.innerHTML).not.toContain("attacker.example");
    const spans = [...container.querySelectorAll("p > span")].map(
      (s) => s.textContent,
    );
    expect(spans).toEqual(["secret"]);
  });

  it("opens web links in the browser and leaves other links inert", async () => {
    const { container } = renderMarkdown(
      '[site](https://ollama.com/library "Model library") [docs](https://ollama.com/docs) [js](javascript:alert(1)) [empty]() [mail](mailto:a@b.c) [local](/c/1 "Local page") www.example.com',
    );
    const links = [...container.querySelectorAll("a[href]")];
    expect(links.map((a) => a.getAttribute("href"))).toEqual([
      "https://ollama.com/library",
      "https://ollama.com/docs",
      "http://www.example.com",
    ]);
    // Titre du Markdown, sinon l'adresse ouverte.
    expect(links.map((a) => a.getAttribute("title"))).toEqual([
      "Model library",
      "https://ollama.com/docs",
      "http://www.example.com/",
    ]);
    for (const link of container.querySelectorAll("a")) {
      expect(link.className).toBe("font-medium underline");
      expect(link.getAttribute("data-streamdown")).toBe("link");
      expect(link.hasAttribute("target")).toBe(false);
    }
    const inert = [...container.querySelectorAll("a:not([href])")];
    expect(inert.map((a) => a.textContent)).toEqual([
      "js",
      "empty",
      "mail",
      "local",
    ]);
    expect(inert.map((a) => a.getAttribute("title"))).toEqual([
      null,
      null,
      null,
      "Local page",
    ]);

    let allowed = true;
    await act(async () => {
      allowed = fireEvent.click(links[0]);
    });
    expect(allowed).toBe(false);
    expect(openExternal).toHaveBeenCalledWith("https://ollama.com/library");
  });

  it("renders \\(…\\), \\[…\\] and $$…$$ as math, but not single dollars", () => {
    const { container } = renderMarkdown(
      "Inline \\(a^2 + b^2\\) and $$c^2$$.\n\n\\[\nE = mc^2\n\\]\n\n$$\n\\sum_{i=1}^{n} i\n$$\n\nIt costs $5 and $10.",
    );
    expect(container.querySelectorAll(".katex")).toHaveLength(4);
    expect(container.querySelectorAll(".katex-display")).toHaveLength(2);
    expect(container.textContent).toContain("It costs $5 and $10.");
  });

  it("repairs the last block while streaming, then shows the text as written", () => {
    const { container, show } = renderMarkdown("**very important", true);
    expect(
      container.querySelector('[data-streamdown="strong"]')?.textContent,
    ).toBe("very important");

    show("**very important", false);
    expect(container.querySelector('[data-streamdown="strong"]')).toBeNull();
    expect(container.textContent).toBe("**very important");

    // Lien dont l'adresse n'est pas encore arrivée : inerte.
    show("See [the docs](https://ollama.com/do", true);
    const link = container.querySelector('a[data-streamdown="link"]');
    expect(link?.textContent).toBe("the docs");
    expect(link?.hasAttribute("href")).toBe(false);
    expect(container.textContent).toBe("See the docs");

    show("Then:\n\n$$\nx^2 +", true);
    expect(container.querySelector(".katex-display")).not.toBeNull();
    expect(container.textContent).not.toContain("$$");
  });

  it("never repairs a finished block, while streaming or after", () => {
    const finished = "Note* important: 2 * 3 = 6\n\nDu **gras\n\n";
    const { container, show } = renderMarkdown(`${finished}Now **bold`, true);
    const paragraphs = () =>
      [...container.querySelectorAll("p")].map((p) => p.textContent);
    expect(paragraphs()).toEqual([
      "Note* important: 2 * 3 = 6",
      "Du **gras",
      "Now bold",
    ]);

    show(`${finished}Now **bold`, false);
    expect(paragraphs()).toEqual([
      "Note* important: 2 * 3 = 6",
      "Du **gras",
      "Now **bold",
    ]);
  });

  it("ends a reply without rendering its finished blocks again", () => {
    const text =
      "# Title\n\nSee [the docs](https://ollama.com/docs).\n\n```python\nprint('hi')\n```\n\nThe **end";
    const { container, show } = renderMarkdown(text, true);
    const nodes = () => ({
      heading: container.querySelector("h1"),
      paragraph: container.querySelector("p"),
      link: container.querySelector("a"),
      light: container.querySelector('pre[data-theme="light"] code'),
      dark: container.querySelector('pre[data-theme="dark"] code'),
      copy: container.querySelector("button.copy-button"),
    });
    const before = nodes();
    expect(Object.values(before).every(Boolean)).toBe(true);

    show(text, false);
    const after = nodes();
    for (const key of Object.keys(before) as (keyof typeof before)[]) {
      expect(after[key], key).toBe(before[key]);
    }
    expect(tokenizers.created).toBe(1);
    // Seul le dernier bloc, réparé pendant le flux, change.
    expect([...container.querySelectorAll("p")].at(-1)?.textContent).toBe(
      "The **end",
    );

    // Bloc de code ouvert pendant le flux, fermé à la fin : mêmes éléments.
    const open = "Intro.\n\n```python\nprint('hi')\n";
    show(open, true);
    const code = container.querySelector('pre[data-theme="light"] code');
    show(`${open}\`\`\``, false);
    expect(container.querySelector('pre[data-theme="light"] code')).toBe(code);
  });

  it("keeps the selection, the focus and a copy button's state at the end of a reply", async () => {
    const text =
      "First paragraph to select.\n\nSee [the docs](https://ollama.com/docs).\n\n```bash\nollama run qwen3\n```\n\nStill **writing";
    const { container, show } = renderMarkdown(text, true);
    const paragraph = container.querySelector("p")!;
    const range = document.createRange();
    range.selectNodeContents(paragraph);
    const selection = window.getSelection()!;
    selection.removeAllRanges();
    selection.addRange(range);

    show(text, false);
    expect(selection.toString()).toBe("First paragraph to select.");
    expect(paragraph.contains(selection.anchorNode)).toBe(true);

    // Focus gardé (jsdom replie la sélection quand un lien prend le focus :
    // vérifiés à part).
    selection.removeAllRanges();
    show(text, true);
    const link = container.querySelector<HTMLAnchorElement>("a[href]")!;
    link.focus();
    show(text, false);
    expect(document.activeElement).toBe(link);

    // Bouton copier d'un bloc : focus et « Copied » gardés.
    show(text, true);
    const copy =
      container.querySelector<HTMLButtonElement>("button.copy-button")!;
    copy.focus();
    await act(async () => {
      fireEvent.click(copy);
    });
    expect(copy.textContent).toBe("Copied");
    show(text, false);
    expect(document.activeElement).toBe(copy);
    expect(copy.textContent).toBe("Copied");
  });

  it("renders the end of a stream like the same text outside a stream", () => {
    const reply = [
      "# Result",
      "Note* important: 2 * 3 = 6, *.txt files and snake_case.",
      '- **One**: first\n- Two with `code` and [a link](https://ollama.com "Ollama")',
      "```python\nitems = [\n    1,\n    2,\n]\n```",
      "| a | b |\n|---|---|\n| **1** | $$x^2$$ |",
      "$$\n\\frac{1}{2}\n$$",
      "Done: 2 * 3 = 6",
    ].join("\n\n");
    const { streamed, direct } = streamThenEnd(reply, fragmentsOf(reply, 3));
    expect(streamed).toBe(direct);
  });

  // Streamdown ne rend de nouveau ses paragraphes et ses sections que si
  // leur position dans le texte change : sans les nôtres, le rendu réparé
  // du flux restait à la fin.
  it("shows a link with its address at the end of a reply, even as long as the incomplete-link marker", () => {
    // « https://ollama.com/library » : 26 caractères, comme
    // « streamdown:incomplete-link ».
    const parts = [
      "More models: ",
      "[the ",
      "Ollama library",
      "](",
      "https",
      "://",
      "ollama",
      ".com",
      "/library",
      ")",
      "\n\n",
      "Anything",
      " else?",
    ];
    const reply = parts.join("");
    const { container, streamed, direct } = streamThenEnd(reply, parts, {
      keep: true,
    });
    const link = container.querySelector("a");
    expect(link?.getAttribute("href")).toBe("https://ollama.com/library");
    expect(link?.title).toBe("https://ollama.com/library");
    expect(streamed).toBe(direct);
  });

  it("shows a paragraph repaired while streaming as written at the end of a reply", () => {
    const reply = "Some **bold and `code` here\n\n~~strike~~ and ~~open";
    const { container, streamed, direct } = streamThenEnd(
      reply,
      fragmentsOf(reply, 5),
      { keep: true },
    );
    expect(
      [...container.querySelectorAll("p")].map((p) => p.textContent),
    ).toEqual(["Some **bold and code here", "strike and ~~open"]);
    expect(container.querySelector('[data-streamdown="strong"]')).toBeNull();
    expect(streamed).toBe(direct);
  });

  it("shows footnotes at the end of a reply, with their references and back links", () => {
    const reply =
      "Claim with a note[^1] and **bold** text.\n\nMore text *here*.\n\n[^1]: The **note** itself, with `code`.";
    const { container, streamed, direct } = streamThenEnd(
      reply,
      fragmentsOf(reply, 1),
      { keep: true },
    );
    const reference = container.querySelector("sup a");
    expect(reference?.textContent).toBe("1");
    expect(reference?.id).toBe("user-content-fnref-1");
    expect(reference?.getAttribute("aria-describedby")).toBe("footnote-label");
    expect(reference?.hasAttribute("data-footnote-ref")).toBe(true);
    const note = container.querySelector("section[data-footnotes] li");
    expect(note?.textContent?.trim()).toBe("The note itself, with code. ↩");
    const back = note?.querySelector("a");
    expect(back?.getAttribute("aria-label")).toBe("Back to reference 1");
    expect(back?.hasAttribute("data-footnote-backref")).toBe(true);
    // Lien vers une ancre : inerte.
    expect(back?.hasAttribute("href")).toBe(false);
    expect(streamed).toBe(direct);
  });

  it("drops footnotes without text, like Streamdown", () => {
    const { container } = renderMarkdown(
      "One[^1] and two[^2].\n\n[^1]: First note.\n[^2]:",
    );
    const notes = [
      ...container.querySelectorAll("section[data-footnotes] li"),
    ].map((li) => li.textContent?.trim());
    expect(notes).toEqual(["First note. ↩"]);
  });

  it("renders a paragraph made of an image alone without a <p>, like Streamdown", () => {
    const { container } = renderMarkdown(
      "![Ollama logo](https://example.com/logo.png)\n\nText ![a](https://example.com/a.png)",
    );
    const content = container.querySelector(".prose > div");
    expect(content?.firstElementChild?.outerHTML).toBe(
      "<span>Ollama logo</span>",
    );
    expect(container.querySelectorAll("p")).toHaveLength(1);
  });

  it("renders the end of a stream like the same text outside a stream, whatever its fragments", () => {
    const corpus: Record<string, string> = {
      "markdown.md": fullReply,
      "markdown-stream.md": streamReply,
      "markdown-long.md": longReply,
      footnotes:
        "Claim[^a] and [a link](https://ollama.com/library).\n\n- item[^b]\n- **bold\n\n[^a]: Note *a* with `code`.\n[^b]: Note b.",
      tables:
        "| Model | Size |\n|:--|--:|\n| **qwen3** | `523 MB` |\n| [lib](https://ollama.com/library) | ~~1 GB~~ |\n| 4 \\| pipe | $$x^2$$ |\n\nAfter **the table**.",
      lists:
        "- a **b**\n- c\n  - d `e`\n    1. f\n    2. g *h*\n\n  continued para\n\n- [ ] task\n- [x] done\n\n1) x\n2) y with [a link](https://ollama.com/library)",
      openHtml:
        "<details>\n<summary>Click</summary>\n\nHidden **bold** text.\n\nStill [inside](https://ollama.com/library)",
      mathWithBlankLine:
        "Formula:\n\n$$\na = b\n\nc = d\n$$\n\nAnd inline $$x$$ done, then **bold**.",
      setext:
        "Title **one**\n===\n\nSub *two*\n---\n\nPara with [link](https://ollama.com/library).\n\nAnother\n-\n\nEnd",
      crlf: "Line **one**\r\n\r\n- item\r\n- `two` and [link](https://ollama.com/library)\r\n\r\n```js\r\nlet x = 1;\r\n```\r\n\r\nEnd **here**",
    };
    for (const [name, reply] of Object.entries(corpus)) {
      // Longs : moins de fragments, de tailles sans rapport avec le texte.
      const sizes = reply.length > 1000 ? [53, 197] : [1, 4, 11];
      for (const size of sizes) {
        const { streamed, direct } = streamThenEnd(
          reply,
          fragmentsOf(reply, size),
        );
        expect(streamed, `${name}, fragments of ${size}`).toBe(direct);
      }
    }
  });

  it("shows an open code block exactly as received while streaming", async () => {
    for (const code of [
      "items = [\n    1,\n    2,",
      "echo $$",
      "x = a * b",
      "x = arr[i",
      "x = _private",
      "print(f'{a}`')",
    ]) {
      const { container, unmount } = renderMarkdown(
        `Run:\n\n\`\`\`python\n${code}`,
        true,
      );
      for (const theme of ["light", "dark"]) {
        expect(
          container.querySelector(`pre[data-theme="${theme}"] code`)
            ?.textContent,
        ).toBe(`${code}\n`);
      }
      expect(container.textContent).not.toContain("```");
      await act(async () => {
        fireEvent.click(container.querySelector("button.copy-button")!);
      });
      expect(clipboard.copyText).toHaveBeenLastCalledWith(code);
      unmount();
    }
  });

  it("only ever extends the code of an open block while streaming", () => {
    // Streamdown réparait l'intérieur du bloc (« $$ » fermé, « ** »...) : le
    // texte ne prolongeait plus le précédent et tout le bloc était tokenisé
    // de nouveau à chaque fragment.
    const code =
      'echo "pid: $$"\nfor f in *.txt; do\n  echo [$f] **done\ndone\n';
    const { show } = renderMarkdown("Run:\n\n```bash\n", true);
    for (let i = 1; i <= code.length; i++) {
      show(`Run:\n\n\`\`\`bash\n${code.slice(0, i)}`, true);
    }
    const texts = tokenizers.texts.filter((t) => t !== "");
    expect(texts.at(-1)).toBe(code);
    for (let i = 1; i < texts.length; i++) {
      // Sans le "\n" final ajouté par mdast-util-to-hast.
      expect(texts[i].startsWith(texts[i - 1].slice(0, -1))).toBe(true);
    }
  });

  it("keeps a code block's tokenizer while it streams and after, and gives it the final newline", async () => {
    const code = ["import os", "", "def main():", "    print(os.getcwd())"];
    const { container, show } = renderMarkdown("Here:\n\n```python\n", true);
    let text = "";
    for (const line of code) {
      for (const char of `${line}\n`) {
        text += char;
        show(`Here:\n\n\`\`\`python\n${text}`, true);
      }
    }
    // Un seul bloc monté pendant tout le flux : seules les nouvelles lignes
    // sont tokenisées à chaque fragment.
    expect(tokenizers.created).toBe(1);
    // Texte reçu sous Streamdown : terminé par "\n" (mdast-util-to-hast).
    expect(
      tokenizers.texts.filter((t) => t !== "").every((t) => t.endsWith("\n")),
    ).toBe(true);
    expect(tokenizers.texts.at(-1)).toBe(`${code.join("\n")}\n`);

    show(`Here:\n\n\`\`\`python\n${text}\`\`\`\n\nDone.`, false);
    // Pas de remontage à la fin du flux : le même tokenizer.
    expect(tokenizers.created).toBe(1);
    expect(tokenizers.texts.at(-1)).toBe(`${code.join("\n")}\n`);
    expect(
      container.querySelector('pre[data-theme="light"] code')?.textContent,
    ).toBe(`${code.join("\n")}\n`);
    // Lignes laissées en texte brut pendant le flux (faute de temps) :
    // colorées ensuite en tâche de fond par ce même tokenizer (temps réel :
    // marge pour une machine chargée).
    await waitFor(
      () => {
        expect(coloredSpans(container, "light").length).toBeGreaterThan(5);
        expect(plainSpans(container, "light")).toHaveLength(0);
      },
      { timeout: 3000 },
    );
  });

  it("renders a long block partly plain within the task budget, then colors the rest in idle time", async () => {
    const clock = await simulateLineCost(5);
    const lines = Array.from({ length: 30 }, (_, i) => `budget_${i} = ${i}`);
    const code = `${lines.join("\n")}\n`;
    const { container } = renderMarkdown(`\`\`\`python\n${code}\`\`\``);

    // Premier rendu : 40 ms de budget pour la tâche, soit 8 lignes à 5 ms ;
    // les 22 autres en texte brut, au texte exact.
    expect(clock.elapsed()).toBe(40);
    for (const theme of ["light", "dark"] as const) {
      expect(plainSpans(container, theme).map((s) => s.textContent)).toEqual(
        lines.slice(8),
      );
      expect(
        container.querySelector(`pre[data-theme="${theme}"] code`)?.textContent,
      ).toBe(code);
    }

    // Temps morts : tranches d'environ 8 ms (deux lignes), chacune suivie
    // d'un rendu du bloc qui ne tokenise rien de plus.
    let plain = 22;
    // Borné : sans coloration en tâche de fond, le test échoue au lieu de
    // tourner sans fin.
    for (let timers = 0; plain > 0 && timers < 40; timers++) {
      const before = clock.lines.mock.calls.length;
      act(() => {
        vi.advanceTimersToNextTimer();
      });
      const done = clock.lines.mock.calls.length - before;
      expect(done).toBeLessThanOrEqual(2);
      plain -= done;
      expect(plainSpans(container, "light")).toHaveLength(plain);
      expect(plainSpans(container, "dark")).toHaveLength(plain);
    }
    expect(clock.lines).toHaveBeenCalledTimes(30);
    act(() => {
      vi.runAllTimers();
    });
    expect(clock.lines).toHaveBeenCalledTimes(30);

    // Tout coloré, comme un rendu dans le budget.
    const light = coloredSpans(container, "light").map((s) => s.style.color);
    const dark = coloredSpans(container, "dark").map((s) => s.style.color);
    cleanup();
    vi.useRealTimers();
    vi.restoreAllMocks();
    freezeTime();
    const reference = renderMarkdown(`\`\`\`python\n${code}\`\`\``).container;
    expect(light).toEqual(
      coloredSpans(reference, "light").map((s) => s.style.color),
    );
    expect(dark).toEqual(
      coloredSpans(reference, "dark").map((s) => s.style.color),
    );
    expect(light.length).toBeGreaterThan(30);
  });

  it("only renders the last lines of a code block again at each fragment of a stream", () => {
    freezeTime();
    tokenizers.lineRenders = 0;
    const lines = Array.from(
      { length: 12 },
      (_, i) => `memo_${i} = compute(${i}, "x")`,
    );
    const { container, show } = renderMarkdown("Here:\n\n```python\n", true);
    let text = "";
    const perFragment: number[] = [];
    for (const line of lines) {
      for (const char of `${line}\n`) {
        text += char;
        const before = tokenizers.lineRenders;
        show(`Here:\n\n\`\`\`python\n${text}`, true);
        perFragment.push(tokenizers.lineRenders - before);
      }
    }
    // Au plus la ligne qui vient de se terminer et la nouvelle dernière
    // ligne, dans les deux thèmes : jamais tout le bloc.
    expect(Math.max(...perFragment)).toBe(4);
    expect(plainSpans(container, "light")).toHaveLength(0);
    expect(
      container.querySelector('pre[data-theme="light"] code')?.textContent,
    ).toBe(text);

    // Fin du flux : rien n'est rendu de nouveau.
    const before = tokenizers.lineRenders;
    show(`Here:\n\n\`\`\`python\n${text}\`\`\`\n\nDone.`, false);
    expect(tokenizers.lineRenders).toBe(before);
  });

  it("shows code plain when the highlighter fails, and reports it once", () => {
    tokenizers.failing = "python";
    const error = vi.spyOn(console, "error").mockImplementation(() => {});
    const text =
      "# Title\n\nSome **bold**.\n\n```python\nx = 1\n```\n\n```python\ny = [2]\n```";
    const { container, show } = renderMarkdown(text, true);
    show(`${text}\n\nMore.`, false);

    expect(
      [...container.querySelectorAll("pre code")].map((c) => c.textContent),
    ).toEqual(["x = 1\n", "x = 1\n", "y = [2]\n", "y = [2]\n"]);
    expect(coloredSpans(container, "light")).toHaveLength(0);
    expect(tokenizers.texts).toContain("y = [2]\n");
    // Le reste de la réponse est rendu, sans le repli en texte brut.
    expect(container.querySelector("h1")?.textContent).toBe("Title");
    expect(
      container.querySelector('[data-streamdown="strong"]')?.textContent,
    ).toBe("bold");
    expect(container.querySelector(".whitespace-pre-wrap")).toBeNull();
    expect(error).toHaveBeenCalledOnce();
    expect(error).toHaveBeenCalledWith(
      "Failed to highlight code:",
      expect.any(SyntaxError),
    );
  });
});
