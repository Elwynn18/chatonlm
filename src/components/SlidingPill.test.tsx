// Pastille qui glisse : rien au premier affichage ; trajet de l'ancien
// élément actif au nouveau, aplat du nouveau masqué le temps du trajet, puis
// pastille effacée ; place gardée quand la barre est remontée (autre page) ;
// rien sous mouvement réduit.

import { act, cleanup, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { SlidingPill } from "@/components/SlidingPill";
import { PILL_DURATION, resetSlidingPills } from "@/lib/slidingPill";

/** Places des éléments (jsdom ne calcule aucune mise en page). */
const LEFT: Record<string, number> = { a: 0, b: 120, c: 260 };

function Bar({ active }: { active: string }) {
  return (
    <div>
      <SlidingPill name="bar" active={active} />
      {Object.keys(LEFT).map((key) => (
        <a key={key} data-pill={key} data-testid={key} href="#">
          {key}
        </a>
      ))}
    </div>
  );
}

const pill = () => screen.getByTestId("sliding-pill-bar");

function mockMotion(reduce: boolean) {
  window.matchMedia = vi.fn().mockReturnValue({
    matches: reduce,
    addEventListener: () => {},
    removeEventListener: () => {},
  }) as unknown as typeof window.matchMedia;
}

describe("SlidingPill", () => {
  const original = window.matchMedia;
  const PROPERTIES = [
    "offsetLeft",
    "offsetTop",
    "offsetWidth",
    "offsetHeight",
  ] as const;
  const descriptors = PROPERTIES.map((property) =>
    Object.getOwnPropertyDescriptor(HTMLElement.prototype, property),
  );
  beforeEach(() => {
    vi.useFakeTimers();
    resetSlidingPills();
    mockMotion(false);
    for (const [property, value] of [
      ["offsetLeft", (el: HTMLElement) => LEFT[el.dataset.pill ?? ""] ?? 0],
      ["offsetTop", () => 4],
      ["offsetWidth", (el: HTMLElement) => (el.dataset.pill ? 90 : 0)],
      ["offsetHeight", () => 28],
    ] as const) {
      Object.defineProperty(HTMLElement.prototype, property, {
        configurable: true,
        get(this: HTMLElement) {
          return value(this);
        },
      });
    }
  });
  afterEach(() => {
    cleanup();
    vi.useRealTimers();
    window.matchMedia = original;
    PROPERTIES.forEach((property, index) => {
      const descriptor = descriptors[index];
      if (descriptor) {
        Object.defineProperty(HTMLElement.prototype, property, descriptor);
      }
    });
  });

  it("slides from the previous item to the new one, then fades out", () => {
    const { rerender } = render(<Bar active="a" />);
    // Premier affichage : aucun trajet.
    expect(pill().style.opacity).toBe("");
    expect(screen.getByTestId("a").dataset.pillMoving).toBeUndefined();

    rerender(<Bar active="c" />);
    expect(screen.getByTestId("c").dataset.pillMoving).toBe("");
    expect(pill().style.opacity).toBe("1");
    expect(pill().style.transform).toBe("translate(260px, 4px)");
    expect(pill().style.width).toBe("90px");
    expect(pill().style.transition).toContain(`transform ${PILL_DURATION}ms`);

    act(() => vi.advanceTimersByTime(PILL_DURATION + 40));
    expect(screen.getByTestId("c").dataset.pillMoving).toBeUndefined();
    expect(pill().style.opacity).toBe("0");
  });

  it("keeps the place of the active item when the bar is mounted again", () => {
    const first = render(<Bar active="b" />);
    first.unmount();
    render(<Bar active="a" />);
    // Part de b (gardé), va vers a.
    expect(screen.getByTestId("a").dataset.pillMoving).toBe("");
    expect(pill().style.transform).toBe("translate(0px, 4px)");
  });

  it("does not move under reduced motion", () => {
    mockMotion(true);
    const { rerender } = render(<Bar active="a" />);
    rerender(<Bar active="b" />);
    expect(screen.getByTestId("b").dataset.pillMoving).toBeUndefined();
    expect(pill().style.opacity).toBe("");
  });
});
