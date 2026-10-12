import { describe, expect, it } from "vitest";
import { cleanCopyHtml } from "./copyHtml";

/** Réponse telle que la rend le Markdown : bloc de code, gras, formule. */
const RENDERED = `
<div class="space-y-4" data-streamdown="root">
  <p>Use <span class="font-semibold" data-streamdown="strong">this</span> from <a href="https://ollama.com/" rel="noreferrer" class="underline" data-streamdown="link">ollama.com</a>:</p>
  <div class="relative my-6 rounded-2xl" data-code-block>
    <div class="flex select-none" data-code-block-header>
      <span class="font-mono text-[13px]">bash</span>
      <button type="button" class="copy-button ml-auto">Copy</button>
      <span class="sr-only" role="status" data-copy-status></span>
    </div>
    <pre class="dark:hidden m-0 p-4" data-theme="light"><code class="font-mono text-sm"><span style="color: #4078F2">ollama</span> run qwen3</code></pre>
    <pre class="hidden dark:block m-0 p-4" data-theme="dark"><code class="font-mono text-sm"><span style="color: #61AFEF">ollama</span> run qwen3</code></pre>
  </div>
  <p>Then <span class="katex"><span class="katex-mathml"><math><mi>x</mi></math></span><span class="katex-html" aria-hidden="true"><span class="base">x</span></span></span> é.</p>
  <button type="button" class="copy-button">Copy</button>
  <span class="sr-only" role="status" data-copy-status>Copied</span>
</div>`;

function rendered(): HTMLElement {
  const root = document.createElement("div");
  root.className = "prose";
  root.dataset.role = "assistant";
  root.innerHTML = RENDERED;
  return root;
}

function parse(html: string): HTMLElement {
  const container = document.createElement("div");
  container.innerHTML = html;
  return container;
}

describe("cleanCopyHtml", () => {
  it("keeps a single code block, without its header", () => {
    const html = parse(cleanCopyHtml(rendered()));
    const pres = html.querySelectorAll("pre");
    expect(pres).toHaveLength(1);
    expect(pres[0].textContent).toBe("ollama run qwen3");
    expect(html.textContent).not.toContain("bash");
    expect(html.textContent).not.toContain("Cop");
    expect(html.querySelector("button")).toBeNull();
    expect(html.querySelector("[role=status]")).toBeNull();
  });

  it("keeps the light code colors", () => {
    const html = parse(cleanCopyHtml(rendered()));
    const token = html.querySelector("pre span");
    expect(token?.getAttribute("style")).toBe("color: #4078F2");
  });

  it("turns Streamdown bold into <strong>", () => {
    const html = parse(cleanCopyHtml(rendered()));
    const strong = html.querySelector("strong");
    expect(strong?.textContent).toBe("this");
    expect(html.querySelector("span.font-semibold")).toBeNull();
  });

  it("keeps the MathML of formulas, not their visual copy", () => {
    const html = parse(cleanCopyHtml(rendered()));
    expect(html.querySelectorAll("math")).toHaveLength(1);
    expect(html.textContent).toContain("Then x é.");
  });

  it("drops every class and data attribute", () => {
    const html = cleanCopyHtml(rendered());
    expect(html).not.toMatch(/\sclass=/);
    expect(html).not.toMatch(/\sdata-/);
    // Les autres attributs restent.
    expect(html).toContain('<a href="https://ollama.com/" rel="noreferrer">');
  });

  it("starts with the encoding, and leaves the page untouched", () => {
    const root = rendered();
    const before = root.innerHTML;
    expect(cleanCopyHtml(root)).toMatch(/^<meta charset="utf-8">/);
    expect(root.innerHTML).toBe(before);
  });
});
