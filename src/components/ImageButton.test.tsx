import { cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { untranslatedTexts, usePseudoLanguage } from "@/test/i18n";
import { ImageButton } from "./ImageButton";

afterEach(cleanup);

describe("ImageButton", () => {
  it("is a toggle: pressed and in the accent colour when active", async () => {
    const onToggle = vi.fn();
    const { rerender } = render(
      <ImageButton isVisible isActive={false} onToggle={onToggle} />,
    );
    const button = screen.getByRole("button", { name: "Image" });
    expect(button.getAttribute("aria-pressed")).toBe("false");
    expect(button.title).toBe("Enable image mode");
    expect(button.className).not.toContain("bg-accent");
    await userEvent.click(button);
    expect(onToggle).toHaveBeenCalledTimes(1);

    rerender(<ImageButton isVisible isActive onToggle={onToggle} />);
    expect(button.getAttribute("aria-pressed")).toBe("true");
    expect(button.title).toBe("Disable image mode");
    expect(button.className).toContain("bg-accent text-neutral-900");
  });

  it("is hidden when not visible", () => {
    render(
      <ImageButton isVisible={false} isActive={false} onToggle={() => {}} />,
    );
    expect(screen.queryByRole("button")).toBeNull();
  });

  it("without the processor instructions: greyed out, inert, says why", async () => {
    const onToggle = vi.fn();
    render(
      <ImageButton
        isVisible
        missingCpuFeatures={["AVX2", "FMA"]}
        isActive={false}
        onToggle={onToggle}
      />,
    );
    const button = screen.getByRole("button", { name: "Image" });
    const reason =
      "This computer can't create images: its processor lacks instructions the image engine needs (AVX2, FMA).";
    expect(button.getAttribute("aria-disabled")).toBe("true");
    expect(button.title).toBe(reason);
    expect(button.getAttribute("aria-description")).toBe(reason);
    expect(button.hasAttribute("aria-pressed")).toBe(false);
    await userEvent.click(button);
    expect(onToggle).not.toHaveBeenCalled();
  });

  it("has every text translated", () => {
    usePseudoLanguage();
    const { container } = render(
      <>
        <ImageButton isVisible isActive onToggle={() => {}} />
        <ImageButton
          isVisible
          missingCpuFeatures={["AVX2"]}
          isActive={false}
          onToggle={() => {}}
        />
      </>,
    );
    expect(untranslatedTexts(container, ["AVX2"])).toEqual([]);
  });
});
