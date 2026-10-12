import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
} from "@testing-library/react";
import { useState } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { Slider, type SliderOption } from "./Slider";

// Crans de la longueur de contexte (page Settings).
const OPTIONS: SliderOption[] = [
  { value: 0, label: "Default" },
  { value: 4096, label: "4k" },
  { value: 8192, label: "8k" },
  { value: 16384, label: "16k" },
  { value: 32768, label: "32k" },
  { value: 65536, label: "64k" },
  { value: 131072, label: "128k" },
  { value: 262144, label: "256k" },
];

// Zone des crans simulée : 700 px à partir de x = 100, un cran tous les 100 px.
const LEFT = 100;
const STEP = 100;
/** Colonne de chaque cran (w-12) : le premier centre est à 24 px du bord. */
const COLUMN = 48;
const x = (index: number) => LEFT + COLUMN / 2 + index * STEP;

type Capture = (pointerId: number) => void;
let setPointerCapture: ReturnType<typeof vi.fn<Capture>>;

beforeEach(() => {
  // jsdom n'implémente pas la capture du pointeur.
  setPointerCapture = vi.fn<Capture>();
  Element.prototype.setPointerCapture = setPointerCapture;
  Element.prototype.releasePointerCapture = vi.fn<Capture>();
  Element.prototype.hasPointerCapture = vi.fn(() => false);
  // Seule la zone des crans (parent des colonnes) a une taille.
  vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockImplementation(
    function (this: HTMLElement) {
      const isTrack =
        this === screen.queryByText("Default")?.parentElement?.parentElement;
      return DOMRect.fromRect(
        isTrack ? { x: LEFT, y: 0, width: 7 * STEP + COLUMN, height: 36 } : {},
      );
    },
  );
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  const proto = Element.prototype as Partial<Element>;
  delete proto.setPointerCapture;
  delete proto.releasePointerCapture;
  delete proto.hasPointerCapture;
});

/** Curseur contrôlé : la valeur suit onChange, comme sur la page Settings. */
function renderSlider(
  initial: number,
  { disabled = false, follow = true } = {},
) {
  const onChange = vi.fn();
  let setValue: (value: number) => void = () => {};
  function Harness() {
    const [value, set] = useState(initial);
    setValue = set;
    return (
      <>
        <span id="label">Context length</span>
        <span id="desc">How much the model remembers.</span>
        <Slider
          options={OPTIONS}
          value={value}
          onChange={(next) => {
            onChange(next);
            if (follow) set(next);
          }}
          disabled={disabled}
          labelledBy="label"
          describedBy="desc"
        />
      </>
    );
  }
  render(<Harness />);
  return {
    onChange,
    setValue: (value: number) => act(() => setValue(value)),
  };
}

const slider = () => screen.getByRole("slider");
/** Élément racine du curseur (porte l'opacité). */
const root = () => track().parentElement!.parentElement!;
const track = () => screen.getByText("Default").parentElement!.parentElement!;
/** Libellé du cran où se trouve la pastille. */
const knobStep = () =>
  slider().parentElement!.parentElement!.lastElementChild!.textContent;
const pointer = (index: number, pointerId = 1) => ({
  clientX: x(index),
  pointerId,
  button: 0,
});

describe("Slider", () => {
  it("shows every label and the knob on the chosen step only", () => {
    renderSlider(8192);
    expect(OPTIONS.map((o) => screen.getByText(o.label))).toHaveLength(8);
    expect(screen.getAllByRole("slider")).toHaveLength(1);
    expect(knobStep()).toBe("8k");
    expect(root().className).not.toContain("opacity-50");
  });

  it("keeps the official look", () => {
    renderSlider(8192);
    for (const name of [
      "h-4 w-4 rounded-full border border-neutral-400 bg-white",
      "dark:border-neutral-500",
    ]) {
      expect(slider().className).toContain(name);
    }
    expect(slider().parentElement!.className).toContain("h-5 w-5");
    const rail = track().previousElementSibling!;
    expect(rail.className).toContain(
      "absolute top-[9px] right-6 left-6 h-1 rounded-full bg-neutral-300 dark:bg-neutral-600",
    );
    expect(track().className).toContain("flex");
    expect(track().className).toContain("justify-between");
    expect(screen.getByText("4k").className).toBe(
      "text-xs whitespace-nowrap text-neutral-500 dark:text-neutral-400",
    );
  });

  it("calls onChange once with the new value on a step click", () => {
    const { onChange } = renderSlider(0);
    fireEvent.pointerDown(screen.getByText("16k"), pointer(3));
    fireEvent.pointerUp(track(), pointer(3));
    fireEvent.click(screen.getByText("16k"));
    expect(onChange).toHaveBeenCalledTimes(1);
    expect(onChange).toHaveBeenCalledWith(16384);
    expect(knobStep()).toBe("16k");
    expect(setPointerCapture).toHaveBeenCalledWith(1);
  });

  it("does not call onChange on a click on the chosen step", () => {
    const { onChange } = renderSlider(4096);
    fireEvent.pointerDown(screen.getByText("4k"), pointer(1));
    fireEvent.pointerUp(track(), pointer(1));
    fireEvent.click(screen.getByText("4k"));
    expect(onChange).not.toHaveBeenCalled();
  });

  it("picks the closest step from the step row's position", () => {
    const { onChange } = renderSlider(0);
    // 140 px après le 2e cran : plus près du 3e.
    fireEvent.pointerDown(track(), { ...pointer(1), clientX: x(1) + 60 });
    fireEvent.pointerUp(track(), pointer(2));
    expect(onChange).toHaveBeenLastCalledWith(8192);
    // Au-delà des bords : premier et dernier crans.
    fireEvent.pointerDown(track(), { ...pointer(0), clientX: x(7) + 500 });
    fireEvent.pointerUp(track(), pointer(7));
    expect(onChange).toHaveBeenLastCalledWith(262144);
    fireEvent.pointerDown(track(), { ...pointer(0), clientX: 0 });
    fireEvent.pointerUp(track(), pointer(0));
    expect(onChange).toHaveBeenLastCalledWith(0);
    expect(onChange).toHaveBeenCalledTimes(3);
  });

  it("measures from the centres of equal-width step columns", () => {
    const { onChange } = renderSlider(8192);
    // Colonnes de même largeur, « Default » compris.
    for (const label of ["Default", "4k", "256k"]) {
      expect(screen.getByText(label).parentElement?.className).toContain(
        "w-12",
      );
    }
    // 45 px à droite du centre de « Default » : encore lui (le milieu est à
    // 50 px), alors qu'un calcul depuis le bord de la zone donnerait 4k.
    fireEvent.pointerDown(track(), { ...pointer(0), clientX: x(0) + 45 });
    fireEvent.pointerUp(track(), pointer(0));
    expect(onChange).toHaveBeenLastCalledWith(0);
  });

  it("follows the pointer while dragging and calls onChange once on release", () => {
    const { onChange } = renderSlider(4096);
    fireEvent.pointerDown(screen.getByText("4k"), pointer(1));
    fireEvent.pointerMove(track(), pointer(2));
    expect(knobStep()).toBe("8k");
    fireEvent.pointerMove(track(), pointer(5));
    expect(knobStep()).toBe("64k");
    expect(slider().getAttribute("aria-valuenow")).toBe("5");
    // Un autre pointeur ne déplace rien.
    fireEvent.pointerMove(track(), pointer(7, 2));
    fireEvent.pointerUp(track(), pointer(7, 2));
    expect(knobStep()).toBe("64k");
    fireEvent.pointerMove(track(), pointer(3));
    expect(onChange).not.toHaveBeenCalled();
    fireEvent.pointerUp(track(), pointer(3));
    fireEvent.lostPointerCapture(track(), pointer(3));
    expect(onChange).toHaveBeenCalledTimes(1);
    expect(onChange).toHaveBeenCalledWith(16384);
    expect(knobStep()).toBe("16k");
    // Un déplacement après le relâchement ne fait rien.
    fireEvent.pointerMove(track(), pointer(6));
    expect(knobStep()).toBe("16k");
  });

  it("releases on the last position even before it is rendered", () => {
    const { onChange } = renderSlider(4096);
    fireEvent.pointerDown(track(), pointer(1));
    // Déplacement et relâchement dans le même lot, sans rendu entre les deux.
    act(() => {
      fireEvent.pointerMove(track(), pointer(6));
      fireEvent.pointerUp(track(), pointer(6));
    });
    expect(onChange).toHaveBeenCalledTimes(1);
    expect(onChange).toHaveBeenCalledWith(131072);
  });

  it("does not call onChange when a drag ends on the current value", () => {
    const { onChange } = renderSlider(4096);
    fireEvent.pointerDown(track(), pointer(1));
    fireEvent.pointerMove(track(), pointer(4));
    fireEvent.pointerMove(track(), pointer(1));
    fireEvent.pointerUp(track(), pointer(1));
    expect(onChange).not.toHaveBeenCalled();
  });

  it("shows the value prop until the parent updates it after a release", () => {
    const { onChange, setValue } = renderSlider(4096, { follow: false });
    fireEvent.pointerDown(track(), pointer(6));
    expect(knobStep()).toBe("128k");
    fireEvent.pointerUp(track(), pointer(6));
    expect(onChange).toHaveBeenCalledWith(131072);
    expect(knobStep()).toBe("4k");
    setValue(131072);
    expect(knobStep()).toBe("128k");
  });

  it.each(["pointerCancel", "lostPointerCapture"] as const)(
    "goes back to the value without onChange on %s",
    (kind) => {
      const { onChange } = renderSlider(8192);
      fireEvent.pointerDown(track(), pointer(5));
      fireEvent.pointerMove(track(), pointer(6));
      expect(knobStep()).toBe("128k");
      fireEvent[kind](track(), pointer(6));
      expect(knobStep()).toBe("8k");
      fireEvent.pointerUp(track(), pointer(6));
      expect(onChange).not.toHaveBeenCalled();
      expect(knobStep()).toBe("8k");
    },
  );

  it("ignores buttons other than the main one", () => {
    const { onChange } = renderSlider(0);
    fireEvent.pointerDown(track(), { ...pointer(4), button: 2 });
    fireEvent.pointerUp(track(), { ...pointer(4), button: 2 });
    expect(knobStep()).toBe("Default");
    expect(onChange).not.toHaveBeenCalled();
    expect(setPointerCapture).not.toHaveBeenCalled();
  });

  it("focuses the knob on a press, then on its new step", () => {
    renderSlider(0);
    fireEvent.pointerDown(track(), pointer(3));
    expect(document.activeElement).toBe(slider());
    fireEvent.pointerUp(track(), pointer(3));
    expect(knobStep()).toBe("16k");
    expect(document.activeElement).toBe(slider());
  });

  it.each([
    ["ArrowRight", 8192, 16384],
    ["ArrowUp", 8192, 16384],
    ["PageUp", 8192, 16384],
    ["ArrowLeft", 8192, 4096],
    ["ArrowDown", 8192, 4096],
    ["PageDown", 8192, 4096],
    ["Home", 8192, 0],
    ["End", 8192, 262144],
  ])("moves with %s", (key, from, to) => {
    const { onChange } = renderSlider(from);
    slider().focus();
    const notPrevented = fireEvent.keyDown(slider(), { key });
    expect(notPrevented).toBe(false);
    expect(onChange).toHaveBeenCalledTimes(1);
    expect(onChange).toHaveBeenCalledWith(to);
    // La pastille a changé de cran et garde le focus.
    expect(slider().getAttribute("aria-valuenow")).toBe(
      String(OPTIONS.findIndex((o) => o.value === to)),
    );
    expect(document.activeElement).toBe(slider());
  });

  it.each([
    ["ArrowLeft", 0],
    ["ArrowDown", 0],
    ["PageDown", 0],
    ["Home", 0],
    ["ArrowRight", 262144],
    ["ArrowUp", 262144],
    ["PageUp", 262144],
    ["End", 262144],
  ])("does not call onChange on %s at the bound", (key, value) => {
    const { onChange } = renderSlider(value);
    const notPrevented = fireEvent.keyDown(slider(), { key });
    expect(notPrevented).toBe(false);
    expect(onChange).not.toHaveBeenCalled();
  });

  it("ignores other keys and shortcuts with modifiers", () => {
    const { onChange } = renderSlider(8192);
    expect(fireEvent.keyDown(slider(), { key: "a" })).toBe(true);
    expect(fireEvent.keyDown(slider(), { key: "Tab" })).toBe(true);
    for (const modifier of ["ctrlKey", "altKey", "metaKey"]) {
      expect(
        fireEvent.keyDown(slider(), { key: "ArrowRight", [modifier]: true }),
      ).toBe(true);
    }
    expect(onChange).not.toHaveBeenCalled();
  });

  it("exposes the slider to assistive technologies", () => {
    renderSlider(16384);
    const knob = screen.getByRole("slider", { name: "Context length" });
    expect(knob.getAttribute("aria-valuemin")).toBe("0");
    expect(knob.getAttribute("aria-valuemax")).toBe("7");
    expect(knob.getAttribute("aria-valuenow")).toBe("3");
    expect(knob.getAttribute("aria-valuetext")).toBe("16k");
    expect(knob.getAttribute("aria-labelledby")).toBe("label");
    expect(knob.getAttribute("aria-describedby")).toBe("desc");
    expect(knob.getAttribute("aria-disabled")).toBe("false");
    expect(knob.tabIndex).toBe(0);
    expect(knob.className).toContain("focus-visible:ring-2");
  });

  it("does nothing when disabled", () => {
    const { onChange } = renderSlider(8192, { disabled: true });
    const knob = slider();
    expect(knob.getAttribute("aria-disabled")).toBe("true");
    expect(knob.hasAttribute("tabindex")).toBe(false);
    expect(root().className).toContain("opacity-50");
    fireEvent.pointerDown(track(), pointer(5));
    expect(knobStep()).toBe("8k");
    fireEvent.pointerUp(track(), pointer(5));
    expect(setPointerCapture).not.toHaveBeenCalled();
    for (const key of ["ArrowRight", "ArrowLeft", "Home", "End"]) {
      fireEvent.keyDown(knob, { key });
    }
    expect(onChange).not.toHaveBeenCalled();
    expect(knobStep()).toBe("8k");
  });

  it("handles a value matching no step", () => {
    const { onChange } = renderSlider(10000);
    // Pas de pastille : la zone des crans porte le rôle.
    const row = slider();
    expect(row).toBe(track());
    expect(row.querySelector(".rounded-full")).toBeNull();
    expect(row.tabIndex).toBe(0);
    expect(row.hasAttribute("aria-valuenow")).toBe(false);
    expect(row.getAttribute("aria-valuetext")).toBe("10000");
    expect(row.getAttribute("aria-labelledby")).toBe("label");
    // ← vers le cran inférieur, puis la pastille reprend le focus.
    row.focus();
    fireEvent.keyDown(row, { key: "ArrowLeft" });
    expect(onChange).toHaveBeenLastCalledWith(8192);
    expect(knobStep()).toBe("8k");
    expect(document.activeElement).toBe(slider());
  });

  it.each([
    ["ArrowRight", 16384],
    ["PageUp", 16384],
    ["ArrowLeft", 8192],
    ["ArrowDown", 8192],
    ["PageDown", 8192],
    ["Home", 0],
    ["End", 262144],
  ])("moves from a value matching no step with %s", (key, to) => {
    const { onChange } = renderSlider(10000);
    fireEvent.keyDown(slider(), { key });
    expect(onChange).toHaveBeenCalledTimes(1);
    expect(onChange).toHaveBeenCalledWith(to);
  });

  it("stays put beyond the steps with a value matching no step", () => {
    const { onChange } = renderSlider(300000);
    expect(fireEvent.keyDown(slider(), { key: "ArrowRight" })).toBe(false);
    expect(onChange).not.toHaveBeenCalled();
    fireEvent.keyDown(slider(), { key: "ArrowLeft" });
    expect(onChange).toHaveBeenCalledWith(262144);
  });

  it("selects a step with the pointer from a value matching no step", () => {
    const { onChange } = renderSlider(10000);
    fireEvent.pointerDown(track(), pointer(2));
    expect(knobStep()).toBe("8k");
    expect(document.activeElement).toBe(slider());
    fireEvent.pointerUp(track(), pointer(2));
    expect(onChange).toHaveBeenCalledTimes(1);
    expect(onChange).toHaveBeenCalledWith(8192);
  });

  it("keeps the focus when the value leaves the steps and comes back", () => {
    const { setValue } = renderSlider(0);
    slider().focus();
    setValue(3000);
    expect(document.activeElement).toBe(track());
    setValue(4096);
    expect(knobStep()).toBe("4k");
    expect(document.activeElement).toBe(slider());
  });

  it("does not take the focus back once the user has left", () => {
    const { setValue } = renderSlider(0);
    slider().focus();
    slider().blur();
    setValue(65536);
    expect(knobStep()).toBe("64k");
    expect(document.activeElement).toBe(document.body);
  });

  it("reflects a new value prop without calling onChange", () => {
    const { onChange, setValue } = renderSlider(0);
    expect(knobStep()).toBe("Default");
    setValue(65536);
    expect(knobStep()).toBe("64k");
    expect(slider().getAttribute("aria-valuetext")).toBe("64k");
    setValue(3000);
    expect(slider()).toBe(track());
    setValue(4096);
    expect(knobStep()).toBe("4k");
    expect(onChange).not.toHaveBeenCalled();
  });

  it("renders nothing without options", () => {
    const { container } = render(
      <Slider options={[]} value={0} onChange={vi.fn()} />,
    );
    expect(container.innerHTML).toBe("");
  });
});
