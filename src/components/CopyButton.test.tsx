import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
} from "@testing-library/react";
import { createRef } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { CopyButton } from "./CopyButton";

/** ClipboardItem réduit : garde les données reçues. */
class FakeClipboardItem {
  constructor(readonly items: Record<string, Blob>) {}
}

let clipboard: {
  write: ReturnType<typeof vi.fn>;
  writeText: ReturnType<typeof vi.fn>;
};

beforeEach(() => {
  vi.useFakeTimers();
  clipboard = {
    write: vi.fn().mockResolvedValue(undefined),
    writeText: vi.fn().mockResolvedValue(undefined),
  };
  Object.defineProperty(navigator, "clipboard", {
    value: clipboard,
    configurable: true,
  });
  vi.stubGlobal("ClipboardItem", FakeClipboardItem);
});

afterEach(() => {
  cleanup();
  vi.useRealTimers();
  vi.unstubAllGlobals();
  Reflect.deleteProperty(navigator, "clipboard");
  Reflect.deleteProperty(document, "execCommand");
});

/** Laisse la copie (promesses) se terminer. */
async function settle() {
  await act(async () => {
    await Promise.resolve();
  });
}

function status() {
  return screen.getByRole("status");
}

describe("CopyButton", () => {
  it("copies the text, shows Copied for 2 s, then Copy again", async () => {
    const onCopy = vi.fn();
    render(
      <CopyButton content="ollama run qwen3" showLabels onCopy={onCopy} />,
    );
    const button = screen.getByRole("button", { name: "Copy" });

    fireEvent.click(button);
    // Écriture partie dans le clic, avant toute attente.
    expect(clipboard.writeText).toHaveBeenCalledWith("ollama run qwen3");
    await settle();

    expect(button.textContent).toBe("Copied");
    expect(status().textContent).toBe("Copied");
    expect(onCopy).toHaveBeenCalledOnce();
    expect(clipboard.write).not.toHaveBeenCalled();

    act(() => vi.advanceTimersByTime(1999));
    expect(button.textContent).toBe("Copied");
    act(() => vi.advanceTimersByTime(1));
    expect(button.textContent).toBe("Copy");
    expect(status().textContent).toBe("");
  });

  it("shows and announces Copy failed when every way fails", async () => {
    clipboard.writeText.mockRejectedValue(new Error("not allowed"));
    // Absente de jsdom : refus de l'ancienne commande « copy ».
    const execCommand = vi.fn().mockReturnValue(false);
    Object.defineProperty(document, "execCommand", {
      value: execCommand,
      configurable: true,
    });
    const onCopy = vi.fn();
    render(<CopyButton content="x" showLabels onCopy={onCopy} />);
    const button = screen.getByRole("button");

    fireEvent.click(button);
    await settle();
    await settle();

    expect(button.textContent).toBe("Copy failed");
    expect(status().textContent).toBe("Copy failed");
    expect(execCommand).toHaveBeenCalledWith("copy");
    expect(onCopy).not.toHaveBeenCalled();

    act(() => vi.advanceTimersByTime(2000));
    expect(button.textContent).toBe("Copy");
  });

  it("changes the icon of the icon-only button on failure", async () => {
    clipboard.writeText.mockRejectedValue(new Error("not allowed"));
    render(<CopyButton content="x" size="md" title="Copy" />);
    const button = screen.getByRole("button", { name: "Copy" });
    const idle = button.innerHTML;

    // Sans execCommand (jsdom), le repli échoue aussi.
    fireEvent.click(button);
    await settle();
    await settle();

    expect(button.innerHTML).not.toBe(idle);
    expect(status().textContent).toBe("Copy failed");
    act(() => vi.advanceTimersByTime(2000));
    expect(button.innerHTML).toBe(idle);
  });

  it("clears its timer when unmounted", async () => {
    const { unmount } = render(<CopyButton content="x" showLabels />);
    fireEvent.click(screen.getByRole("button"));
    await settle();
    expect(vi.getTimerCount()).toBe(1);

    unmount();

    expect(vi.getTimerCount()).toBe(0);
  });

  it("copies cleaned HTML and the Markdown with copyRef", async () => {
    const ref = createRef<HTMLDivElement>();
    render(
      <>
        <div ref={ref} className="prose">
          <p>
            Use{" "}
            <span className="font-semibold" data-streamdown="strong">
              this
            </span>
          </p>
        </div>
        <CopyButton content="Use **this**" copyRef={ref} title="Copy" />
      </>,
    );

    fireEvent.click(screen.getByRole("button", { name: "Copy" }));
    expect(clipboard.write).toHaveBeenCalledOnce();
    await settle();

    const [items] = clipboard.write.mock.calls[0] as [FakeClipboardItem[]];
    const { "text/html": html, "text/plain": text } = items[0].items;
    expect(await html.text()).toBe(
      '<meta charset="utf-8"><p>Use <strong>this</strong></p>',
    );
    expect(await text.text()).toBe("Use **this**");
    expect(clipboard.writeText).not.toHaveBeenCalled();
    expect(status().textContent).toBe("Copied");
  });

  it("names the icon-only button after its title", () => {
    render(<CopyButton content="x" size="md" title="Copy" />);
    const button = screen.getByRole("button", { name: "Copy" });
    expect(button.getAttribute("aria-label")).toBe("Copy");
    expect(button.getAttribute("title")).toBe("Copy");
    expect(button.className).toMatch(/^h-7 w-7 /);
  });

  it("keeps the visible label as the name with labels", () => {
    render(<CopyButton content="x" showLabels className="copy-button" />);
    const button = screen.getByRole("button", { name: "Copy" });
    expect(button.hasAttribute("aria-label")).toBe(false);
    expect(button.getAttribute("title")).toBe("");
    expect(button.className).toBe(
      "text-xs px-4 py-2 z-10 cursor-pointer rounded-lg copy-button",
    );
  });
});
