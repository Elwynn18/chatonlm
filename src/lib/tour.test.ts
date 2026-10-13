import { afterEach, describe, expect, it } from "vitest";
import { BUBBLE_GAP, bubblePlace, grown, sameBox, targetBox } from "@/lib/tour";

const VIEWPORT = { width: 1200, height: 800 };
const BUBBLE = { width: 320, height: 200 };

/** Élément de la page dont le rectangle est donné. */
function element(id: string, rect: Partial<DOMRect>) {
  const node = document.createElement("div");
  node.id = id;
  node.getBoundingClientRect = () =>
    ({
      left: 0,
      top: 0,
      right: 0,
      bottom: 0,
      width: 0,
      height: 0,
      ...rect,
    }) as DOMRect;
  document.body.append(node);
  return node;
}

afterEach(() => {
  document.body.replaceChildren();
});

describe("targetBox", () => {
  it("is the rectangle of the element shown", () => {
    element("a", {
      left: 10,
      top: 20,
      right: 110,
      bottom: 60,
      width: 100,
      height: 40,
    });
    expect(targetBox(["#a"])).toEqual({
      left: 10,
      top: 20,
      width: 100,
      height: 40,
    });
  });

  it("encloses several elements, ignoring the absent and the hidden ones", () => {
    element("a", {
      left: 10,
      top: 20,
      right: 110,
      bottom: 60,
      width: 100,
      height: 40,
    });
    element("b", {
      left: 10,
      top: 64,
      right: 150,
      bottom: 104,
      width: 140,
      height: 40,
    });
    element("hidden", {});
    expect(targetBox(["#a", "#missing", "#hidden", "#b"])).toEqual({
      left: 10,
      top: 20,
      width: 140,
      height: 84,
    });
  });

  it("is null when nothing is shown", () => {
    element("hidden", {});
    expect(targetBox(["#missing", "#hidden"])).toBeNull();
    expect(targetBox([])).toBeNull();
  });
});

describe("bubblePlace", () => {
  it("goes to the right of a sidebar entry, level with it", () => {
    const spot = { left: 10, top: 120, width: 170, height: 40 };
    expect(bubblePlace(spot, BUBBLE, VIEWPORT)).toEqual({
      left: 180 + BUBBLE_GAP,
      top: 120,
    });
  });

  it("stays inside the window next to an entry near the bottom", () => {
    const spot = { left: 10, top: 760, width: 170, height: 36 };
    expect(bubblePlace(spot, BUBBLE, VIEWPORT)).toEqual({
      left: 180 + BUBBLE_GAP,
      top: VIEWPORT.height - BUBBLE.height - BUBBLE_GAP,
    });
  });

  it("goes under a wide area, centred on it, when there is room", () => {
    const spot = { left: 300, top: 100, width: 780, height: 100 };
    expect(bubblePlace(spot, BUBBLE, VIEWPORT)).toEqual({
      left: 300 + (780 - 320) / 2,
      top: 200 + BUBBLE_GAP,
    });
  });

  it("goes above a wide area at the bottom of the window", () => {
    const spot = { left: 300, top: 650, width: 780, height: 120 };
    expect(bubblePlace(spot, BUBBLE, VIEWPORT)).toEqual({
      left: 300 + (780 - 320) / 2,
      top: 650 - BUBBLE_GAP - BUBBLE.height,
    });
  });

  it("covers the bottom of an area that leaves no room around it", () => {
    const spot = { left: 0, top: 50, width: 1200, height: 740 };
    expect(bubblePlace(spot, BUBBLE, VIEWPORT)).toEqual({
      left: (1200 - 320) / 2,
      top: VIEWPORT.height - BUBBLE.height - BUBBLE_GAP,
    });
  });

  it("never leaves the window sideways", () => {
    const narrow = { width: 500, height: 800 };
    const spot = { left: 380, top: 100, width: 110, height: 40 };
    const place = bubblePlace(spot, BUBBLE, narrow);
    expect(place.left).toBe(narrow.width - BUBBLE.width - BUBBLE_GAP);
    expect(place.top).toBe(140 + BUBBLE_GAP);
  });
});

describe("grown and sameBox", () => {
  it("adds the margin on every side", () => {
    expect(grown({ left: 10, top: 20, width: 100, height: 40 }, 6)).toEqual({
      left: 4,
      top: 14,
      width: 112,
      height: 52,
    });
  });

  it("compares to the half pixel", () => {
    const box = { left: 10, top: 20, width: 100, height: 40 };
    expect(sameBox(box, { ...box, left: 10.3 })).toBe(true);
    expect(sameBox(box, { ...box, width: 101 })).toBe(false);
    expect(sameBox(null, null)).toBe(true);
    expect(sameBox(box, null)).toBe(false);
  });
});
