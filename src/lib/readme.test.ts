import { describe, expect, it } from "vitest";
import { type ReadmeNode, readmeText, readmeTree } from "./readme";

const REPO = "unsloth/Qwen3.5-9B-GGUF";

/** Éléments de l'arbre, en profondeur. */
function elements(nodes: readonly ReadmeNode[]): ReadmeNode[] {
  return nodes.flatMap((node) =>
    node.kind === "element" ? [node, ...elements(node.children)] : [node],
  );
}

function tags(nodes: readonly ReadmeNode[]): string[] {
  return elements(nodes).flatMap((node) =>
    node.kind === "element" ? [node.tag] : [],
  );
}

function links(nodes: readonly ReadmeNode[]) {
  return elements(nodes).flatMap((node) =>
    node.kind === "element" && node.tag === "a"
      ? [{ text: readmeText(node.children), href: node.href }]
      : [],
  );
}

function images(nodes: readonly ReadmeNode[]) {
  return elements(nodes).filter((node) => node.kind === "image");
}

describe("readmeTree", () => {
  it("renders Markdown into allowed elements", () => {
    const tree = readmeTree(
      "# Title\n\nSome **bold** and *em* text.\n\n- one\n- two\n\n| A | B |\n|---|---|\n| 1 | 2 |\n\n```sh\nollama run x\n```\n",
      REPO,
    );
    expect(tags(tree)).toEqual(
      expect.arrayContaining([
        "h1",
        "p",
        "strong",
        "em",
        "ul",
        "li",
        "table",
        "thead",
        "tbody",
        "tr",
        "th",
        "td",
        "pre",
        "code",
      ]),
    );
    expect(readmeText(tree)).toContain("ollama run x");
  });

  it("keeps the HTML of model cards (div align, details, sup)", () => {
    const tree = readmeTree(
      '<div align="center"><b>Unsloth</b></div>\n\n<details><summary>More</summary>\n\nHidden <sup>1</sup>\n\n</details>',
      REPO,
    );
    const div = elements(tree).find(
      (node) => node.kind === "element" && node.tag === "div",
    );
    expect(div).toMatchObject({ kind: "element", tag: "div", align: "center" });
    expect(tags(tree)).toEqual(
      expect.arrayContaining(["details", "summary", "sup", "b"]),
    );
  });

  it("shows unknown and dangerous tags as text, never as elements", () => {
    const tree = readmeTree(
      'Before <think>reasoning</think> after.\n\n<script>alert("x")</script>\n\n<iframe src="https://evil.example"></iframe>\n\n<style>body{display:none}</style>',
      REPO,
    );
    const text = readmeText(tree);
    expect(text).toContain("<think>reasoning</think>");
    expect(text).toContain('<script>alert("x")</script>');
    expect(text).toContain('<iframe src="https://evil.example"></iframe>');
    for (const tag of tags(tree)) {
      expect(["think", "script", "iframe", "style"]).not.toContain(tag);
    }
  });

  it("drops every attribute but href and align (no event handler)", () => {
    const tree = readmeTree(
      '<p onclick="steal()" style="color:red" class="x" align="right">Hi</p>',
      REPO,
    );
    expect(tree).toEqual([
      {
        kind: "element",
        tag: "p",
        align: "right",
        children: [{ kind: "text", text: "Hi" }],
      },
    ]);
  });

  it("replaces images by link chips and removes shields.io badges", () => {
    const tree = readmeTree(
      '![Benchmark chart](https://github.com/org/repo/raw/main/chart.png)\n\n<img src="x" onerror="alert(1)">\n\n[![License](https://img.shields.io/badge/license-apache-blue)](https://www.apache.org/licenses/LICENSE-2.0)\n\n<img src="images/arch%20diagram.png">',
      REPO,
    );
    expect(images(tree)).toEqual([
      {
        kind: "image",
        label: "Benchmark chart",
        host: "github.com",
        href: "https://github.com/org/repo/raw/main/chart.png",
      },
      {
        kind: "image",
        label: "x",
        host: "huggingface.co",
        href: `https://huggingface.co/${REPO}/blob/main/x`,
      },
      {
        kind: "image",
        label: "arch diagram.png",
        host: "huggingface.co",
        href: `https://huggingface.co/${REPO}/blob/main/images/arch%20diagram.png`,
      },
    ]);
    // Le lien du badge, vide sans son image, disparaît.
    expect(links(tree)).toEqual([]);
    expect(readmeText(tree)).not.toContain("onerror");
  });

  it("an image inside a link is a plain chip: the link keeps its destination", () => {
    const tree = readmeTree(
      '<a href="https://discord.gg/unsloth"><img src="https://github.com/unslothai/unsloth/raw/main/images/Discord%20button.png" width="145"></a>\n\n[![Docs](https://docs.unsloth.ai/logo.png)](https://docs.unsloth.ai)',
      REPO,
    );
    expect(links(tree)).toEqual([
      { text: "Image: Discord button.png", href: "https://discord.gg/unsloth" },
      { text: "Image: Docs", href: "https://docs.unsloth.ai/" },
    ]);
    expect(images(tree)).toEqual([
      {
        kind: "image",
        label: "Discord button.png",
        host: "github.com",
        href: null,
      },
      { kind: "image", label: "Docs", host: "docs.unsloth.ai", href: null },
    ]);
  });

  it("opens http(s) links only; relative links point to the repository files", () => {
    const tree = readmeTree(
      "[site](https://qwen.ai) [license](LICENSE) [card](./docs/card.md) [org](/Qwen) [bad](javascript:alert(1)) [mail](mailto:a@b.c) [top](#usage)",
      REPO,
    );
    expect(links(tree)).toEqual([
      { text: "site", href: "https://qwen.ai/" },
      {
        text: "license",
        href: `https://huggingface.co/${REPO}/blob/main/LICENSE`,
      },
      {
        text: "card",
        href: `https://huggingface.co/${REPO}/blob/main/docs/card.md`,
      },
      { text: "org", href: "https://huggingface.co/Qwen" },
      { text: "bad", href: undefined },
      { text: "mail", href: undefined },
      { text: "top", href: undefined },
    ]);
  });

  it("an HTML javascript: link stays inert", () => {
    const tree = readmeTree(
      '<a href="javascript:alert(1)" onclick="x()">click</a> <a href=" JAVASCRIPT:alert(2)">two</a>',
      REPO,
    );
    expect(links(tree)).toEqual([
      { text: "click", href: undefined },
      { text: "two", href: undefined },
    ]);
  });

  it("drops layout whitespace inside tables (React would refuse it)", () => {
    const tree = readmeTree("<table>\n<tr>\n<td>a</td>\n</tr>\n</table>", REPO);
    const rows = elements(tree).filter(
      (node) =>
        node.kind === "element" && ["table", "tbody", "tr"].includes(node.tag),
    );
    for (const row of rows) {
      if (row.kind !== "element") continue;
      for (const child of row.children) expect(child.kind).not.toBe("text");
    }
  });
});
