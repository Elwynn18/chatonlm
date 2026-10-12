import { afterEach, describe, expect, it } from "vitest";
import { allowsNativeMenu } from "./contextMenu";

function element(html: string) {
  const host = document.createElement("div");
  host.innerHTML = html;
  document.body.append(host);
  return host.firstElementChild as Element;
}

afterEach(() => {
  document.body.innerHTML = "";
});

describe("allowsNativeMenu", () => {
  it("allows the marked zones and anything inside them", () => {
    const zone = element(
      "<main class='allow-context-menu'><p><strong>text</strong></p></main>",
    );
    expect(allowsNativeMenu(zone)).toBe(true);
    expect(allowsNativeMenu(zone.querySelector("strong"))).toBe(true);
  });

  it("allows editable fields anywhere", () => {
    for (const html of [
      "<input />",
      "<textarea></textarea>",
      "<div contenteditable='true'><span>x</span></div>",
    ]) {
      const field = element(html);
      expect(allowsNativeMenu(field.firstElementChild ?? field), html).toBe(
        true,
      );
    }
  });

  it("refuses everything else", () => {
    for (const html of [
      "<div><p>text</p></div>",
      "<button>x</button>",
      "<div contenteditable='false'>x</div>",
      "<nav class='context-menu'>x</nav>",
    ]) {
      expect(allowsNativeMenu(element(html)), html).toBe(false);
    }
    expect(allowsNativeMenu(document.body)).toBe(false);
    expect(allowsNativeMenu(null)).toBe(false);
    expect(allowsNativeMenu(window)).toBe(false);
  });
});
