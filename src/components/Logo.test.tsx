// Le logo de l'app reprend les tracés du dessin source assets/logo.svg (celui
// des icônes) : tête, rayures et yeux, et la couleur de la tête.
import { render } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import source from "../../assets/logo.svg?raw";
import { Logo } from "@/components/Logo";

const SOURCE = new DOMParser().parseFromString(source, "image/svg+xml");

const attributes = (element: Element | null, names: string[]) =>
  names.map((name) => element?.getAttribute(name));

describe("Logo", () => {
  it("draws the head of the source drawing, in its colour", () => {
    const { container } = render(<Logo />);
    const head = container.querySelector("svg > path");
    expect(attributes(head, ["d", "fill"])).toEqual(
      attributes(SOURCE.querySelector("#head"), ["d", "fill"]),
    );
    expect(head?.getAttribute("d")).toMatch(/^M256 182C/);
  });

  it("cuts the stripes and the eyes of the source drawing out of the head", () => {
    const { container } = render(<Logo />);
    const mask = container.querySelector("mask");
    expect(container.querySelector("svg > path")?.getAttribute("mask")).toBe(
      `url(#${mask?.id})`,
    );
    const stripes = ["d", "stroke-width", "stroke-linecap"];
    expect(attributes(mask?.querySelector("path") ?? null, stripes)).toEqual(
      attributes(SOURCE.querySelector("#stripes"), stripes),
    );
    const eye = ["cx", "cy", "rx", "ry"];
    const eyes = [...(mask?.querySelectorAll("ellipse") ?? [])];
    const sourceEyes = [...SOURCE.querySelectorAll("ellipse")];
    expect(eyes).toHaveLength(2);
    expect(eyes.map((element) => attributes(element, eye))).toEqual(
      sourceEyes.map((element) => attributes(element, eye)),
    );
    // Percés : noirs dans le masque, sur un fond blanc qui garde la tête.
    expect(mask?.querySelector("rect")?.getAttribute("fill")).toBe("#fff");
    expect(eyes.map((element) => element.getAttribute("fill"))).toEqual([
      "#000",
      "#000",
    ]);
    expect(mask?.querySelector("path")?.getAttribute("stroke")).toBe("#000");
  });

  it("gives each logo its own mask", () => {
    const { container } = render(
      <>
        <Logo />
        <Logo size={32} />
      </>,
    );
    const [first, second] = [...container.querySelectorAll("mask")];
    expect(first.id).not.toBe(second.id);
    expect(container.querySelectorAll("svg")[1].getAttribute("width")).toBe(
      "32",
    );
  });
});
