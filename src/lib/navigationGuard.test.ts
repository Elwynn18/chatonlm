import { afterEach, describe, expect, it } from "vitest";
import { shouldBlockKey, shouldBlockMouse } from "./navigationGuard";

/** Touche reçue par `target` (un élément ajouté à la page). */
function keyOn(target: Element, init: KeyboardEventInit) {
  const event = new KeyboardEvent("keydown", { ...init, bubbles: true });
  let seen: KeyboardEvent | null = null;
  const listener = (e: Event) => (seen = e as KeyboardEvent);
  document.addEventListener("keydown", listener);
  target.dispatchEvent(event);
  document.removeEventListener("keydown", listener);
  return seen as unknown as KeyboardEvent;
}

function element(html: string) {
  const host = document.createElement("div");
  host.innerHTML = html;
  document.body.append(host);
  return host.firstElementChild as Element;
}

afterEach(() => {
  document.body.innerHTML = "";
});

describe("shouldBlockKey", () => {
  it("blocks F5 and Ctrl+R everywhere, also in a field", () => {
    const field = element("<input />");
    for (const target of [document.body, field]) {
      for (const init of [
        { key: "F5" },
        { key: "r", ctrlKey: true },
        { key: "R", ctrlKey: true, shiftKey: true },
        { key: "r", metaKey: true },
      ]) {
        expect(shouldBlockKey(keyOn(target, init)), JSON.stringify(init)).toBe(
          true,
        );
      }
    }
  });

  it("blocks Backspace and history arrows outside editable fields", () => {
    const button = element("<button>x</button>");
    for (const init of [
      { key: "Backspace" },
      { key: "ArrowLeft", altKey: true },
      { key: "ArrowRight", altKey: true },
      { key: "ArrowLeft", ctrlKey: true },
      { key: "ArrowRight", metaKey: true },
    ]) {
      expect(shouldBlockKey(keyOn(button, init)), JSON.stringify(init)).toBe(
        true,
      );
    }
  });

  it("leaves them to editable fields, contenteditable children included", () => {
    const targets = [
      element("<input />"),
      element("<textarea></textarea>"),
      element("<select><option>a</option></select>"),
      element("<div contenteditable='true'><span>x</span></div>")
        .firstElementChild as Element,
    ];
    for (const target of targets) {
      for (const init of [
        { key: "Backspace" },
        { key: "ArrowLeft", altKey: true },
        { key: "ArrowLeft", ctrlKey: true },
      ]) {
        expect(shouldBlockKey(keyOn(target, init)), target.tagName).toBe(false);
      }
    }
    // contenteditable="false" n'est pas éditable.
    const frozen = element("<div contenteditable='false'>x</div>");
    expect(shouldBlockKey(keyOn(frozen, { key: "Backspace" }))).toBe(true);
  });

  it("lets other keys through", () => {
    for (const init of [
      { key: "r" },
      { key: "ArrowLeft" },
      { key: "ArrowRight", shiftKey: true },
      { key: "Delete" },
      { key: "F6" },
    ]) {
      expect(
        shouldBlockKey(keyOn(document.body, init)),
        JSON.stringify(init),
      ).toBe(false);
    }
  });
});

describe("shouldBlockMouse", () => {
  it("blocks the back and forward buttons only", () => {
    const button = (b: number) => new MouseEvent("mousedown", { button: b });
    expect(shouldBlockMouse(button(3))).toBe(true);
    expect(shouldBlockMouse(button(4))).toBe(true);
    for (const b of [0, 1, 2]) expect(shouldBlockMouse(button(b))).toBe(false);
  });
});
