import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { Downloading } from "./Downloading";

afterEach(() => cleanup());

const KB = 1024;
const MB = 1024 ** 2;
const GB = 1024 ** 3;
const TB = 1024 ** 4;

/** Ligne de progression affichée pour ces tailles. */
function progressText(completed: number, total: number) {
  const { container } = render(
    <Downloading completed={completed} total={total} />,
  );
  const text = container.querySelector(".text-sm")?.textContent ?? null;
  cleanup();
  return text;
}

describe("Downloading", () => {
  it("shows the title, the progress in the unit of the total, and a bar", () => {
    const { container } = render(
      <Downloading completed={1_261_646_644} total={5_046_586_573} />,
    );
    expect(screen.getByText("Downloading model")).toBeTruthy();
    expect(screen.getByText("1.2 GB / 4.7 GB (25%)")).toBeTruthy();
    const bar = container.querySelector<HTMLElement>(".bg-accent");
    expect(bar?.style.width).toBe(`${(1_261_646_644 / 5_046_586_573) * 100}%`);
    // Icône de téléchargement à gauche du titre, même largeur que l'officielle.
    expect(container.querySelector("svg.absolute.w-3\\.5")).not.toBeNull();
  });

  it("formats the sizes in base 1024, with one decimal in GB and TB only", () => {
    expect(progressText(0, 0)).toBe("0 B / 0 B (0%)");
    expect(progressText(500, 1000)).toBe("500 B / 1000 B (50%)");
    expect(progressText(1023, 1023)).toBe("1023 B / 1023 B (100%)");
    expect(progressText(KB, 1536)).toBe("1 KB / 2 KB (66%)");
    expect(progressText(100 * MB, 487 * MB)).toBe("100 MB / 487 MB (20%)");
    expect(progressText(0, GB - 1)).toBe("0 MB / 1024 MB (0%)");
    expect(progressText(0, GB)).toBe("0.0 GB / 1.0 GB (0%)");
    expect(progressText(TB, 2.25 * TB)).toBe("1.0 TB / 2.3 TB (44%)");
    // Au-delà, l'unité reste TB.
    expect(progressText(0, 3 * 1024 * TB)).toBe("0.0 TB / 3072.0 TB (0%)");
  });

  it("shows the completed part in the unit of the total", () => {
    expect(progressText(512 * KB, GB)).toBe("0.0 GB / 1.0 GB (0%)");
    expect(progressText(300 * KB, 2 * MB)).toBe("0 MB / 2 MB (14%)");
    expect(progressText(999 * KB, 1000 * KB)).toBe("999 KB / 1000 KB (99%)");
    expect(progressText(1023, 2 * KB)).toBe("1 KB / 2 KB (49%)");
  });

  it("rounds the percentage down, and fills the bar", () => {
    expect(progressText(5_046_586_573, 5_046_586_573)).toBe(
      "4.7 GB / 4.7 GB (100%)",
    );
    expect(progressText(1_261_646_643, 5_046_586_573)).toBe(
      "1.2 GB / 4.7 GB (24%)",
    );
    const { container } = render(<Downloading completed={0} total={0} />);
    const bar = container.querySelector<HTMLElement>(".bg-accent");
    expect(bar?.style.width).toBe("0%");
  });

  it("is a progressbar for assistive technologies", () => {
    render(<Downloading completed={1.5 * GB} total={4 * GB} />);
    const bar = screen.getByRole("progressbar", { name: "Downloading model" });
    expect(bar.getAttribute("aria-valuemin")).toBe("0");
    expect(bar.getAttribute("aria-valuemax")).toBe("100");
    expect(bar.getAttribute("aria-valuenow")).toBe("37");
    expect(bar.getAttribute("aria-valuetext")).toBe("1.5 GB / 4.0 GB (37%)");
  });
});
