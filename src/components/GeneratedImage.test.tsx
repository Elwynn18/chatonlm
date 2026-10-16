// Image créée par l'outil `generate_image` (propre à ChatonLM) : une image
// qui ne se charge pas (pièce jointe absente) laisse un court texte, sans
// « Save image… ».

import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { GeneratedImage } from "./GeneratedImage";

afterEach(cleanup);

const RESULT = {
  attachmentId: 42,
  width: 512,
  height: 512,
  model: "FLUX.2 klein 4B",
};

describe("GeneratedImage", () => {
  it("an image that can't be loaded gives way to a short text, without Save", () => {
    render(<GeneratedImage result={RESULT} prompt="A red kitten" />);
    const image = screen.getByRole("img", { name: "A red kitten" });
    expect(screen.getByRole("button", { name: "Save image…" })).toBeTruthy();
    expect(screen.queryByTestId("image-missing")).toBeNull();
    fireEvent.error(image);
    expect(screen.queryByRole("img")).toBeNull();
    expect(screen.getByTestId("image-missing").textContent).toBe(
      "The image is no longer available.",
    );
    expect(screen.queryByRole("button", { name: "Save image…" })).toBeNull();
    // Le modèle reste nommé.
    expect(screen.getByText("Made with FLUX.2 klein 4B")).toBeTruthy();
  });
});
