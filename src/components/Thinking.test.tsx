import {
  act,
  cleanup,
  fireEvent,
  isInaccessible,
  render,
  screen,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { Thinking } from "./Thinking";

// Corps du bloc : propriétés reçues par StreamingMarkdownContent, notées à
// chaque rendu.
type MarkdownProps = { content: string; isStreaming?: boolean; size?: string };
const markdown = vi.hoisted(() => [] as MarkdownProps[]);
vi.mock("@/components/StreamingMarkdownContent", async () => {
  const { createElement } = await import("react");
  return {
    StreamingMarkdownContent: (props: MarkdownProps) => {
      markdown.push(props);
      return createElement("div", { "data-testid": "markdown" }, props.content);
    },
  };
});

afterEach(() => {
  cleanup();
  markdown.length = 0;
  vi.unstubAllGlobals();
});

const start = new Date(1_000_000);
/** Instant situé `seconds` secondes après le début du raisonnement. */
const after = (seconds: number) => new Date(start.getTime() + seconds * 1000);

function header() {
  return screen.getByRole("button");
}

/** Corps du bloc, désigné par l'en-tête (aria-controls). */
function body() {
  const id = header().getAttribute("aria-controls");
  expect(id).toBeTruthy();
  return document.getElementById(id!)!;
}

function lastMarkdown() {
  return markdown.at(-1)!;
}

/** Aperçu replié pendant le raisonnement : 12rem, visible, fin montrée. */
function expectPreview() {
  expect(header().getAttribute("aria-expanded")).toBe("false");
  expect(body().style.maxHeight).toBe("12rem");
  expect(body().style.opacity).toBe("1");
  expect(body().style.visibility).toBe("");
  // Colonne inversée : le contenu part du bas et déborde par le haut.
  expect(body().className).toContain("flex-col-reverse");
  expect(body().className).toContain("overflow-hidden");
  expect(body().hasAttribute("data-folded")).toBe(false);
}

/** Replié et fini : hauteur et opacité à 0 en 300 ms, puis masqué. */
function expectFolded() {
  expect(header().getAttribute("aria-expanded")).toBe("false");
  expect(body().style.maxHeight).toBe("0px");
  expect(body().style.opacity).toBe("0");
  expect(body().style.visibility).toBe("hidden");
  expect(body().className).toContain(
    "transition-[max-height,opacity,visibility] duration-300 ease-in-out",
  );
  expect(isInaccessible(body())).toBe(true);
  // Marqué : la liste des messages le compte à sa hauteur finale (nulle)
  // pendant l'animation du repli.
  expect(body().hasAttribute("data-folded")).toBe(true);
}

function expectExpanded() {
  expect(header().getAttribute("aria-expanded")).toBe("true");
  expect(body().style.maxHeight).toBe("");
  expect(body().style.opacity).toBe("1");
  expect(body().style.visibility).toBe("");
  expect(isInaccessible(body())).toBe(false);
  expect(body().hasAttribute("data-folded")).toBe(false);
}

describe("Thinking", () => {
  it("titles: in progress, then the duration (under 2 s: a moment, else one decimal)", () => {
    const { rerender } = render(<Thinking thinking="Hmm" startTime={start} />);
    expect(header().textContent).toBe("Thinking...");

    const titles = [
      [0.4, "Thought for a moment"],
      [1.99, "Thought for a moment"],
      [2, "Thought for 2.0 seconds"],
      [12.34, "Thought for 12.3 seconds"],
      [75.06, "Thought for 75.1 seconds"],
    ] as const;
    for (const [seconds, title] of titles) {
      rerender(
        <Thinking thinking="Hmm" startTime={start} endTime={after(seconds)} />,
      );
      expect(header().textContent).toBe(title);
    }
  });

  it("while thinking, shows a preview of the end, rendered as streaming Markdown", () => {
    const { rerender } = render(
      <Thinking thinking="First line" startTime={start} />,
    );
    expectPreview();
    expect(lastMarkdown()).toEqual({
      content: "First line",
      isStreaming: true,
      size: "sm",
    });

    rerender(
      <Thinking thinking={"First line\n\nSecond line"} startTime={start} />,
    );
    expectPreview();
    expect(screen.getByTestId("markdown").textContent).toBe(
      "First line\n\nSecond line",
    );
    expect(lastMarkdown().isStreaming).toBe(true);
    // Texte du raisonnement en cours : plus foncé.
    expect(header().closest("h3")!.parentElement!.className).toContain(
      "text-neutral-800 dark:text-neutral-200",
    );
  });

  it("folds itself when the thinking ends, and stops streaming the Markdown", () => {
    const { rerender } = render(<Thinking thinking="Hmm" startTime={start} />);
    expectPreview();

    rerender(<Thinking thinking="Hmm" startTime={start} endTime={after(3)} />);
    expectFolded();
    expect(header().textContent).toBe("Thought for 3.0 seconds");
    expect(lastMarkdown().isStreaming).toBe(false);
    expect(header().closest("h3")!.parentElement!.className).toContain(
      "text-neutral-600 dark:text-neutral-400",
    );
  });

  it("stays open at the end when the user opened it while thinking", () => {
    const { rerender } = render(<Thinking thinking="Hmm" startTime={start} />);
    fireEvent.click(header());
    expectExpanded();
    expect(lastMarkdown().isStreaming).toBe(true);

    rerender(<Thinking thinking="Hmm" startTime={start} endTime={after(1)} />);
    expectExpanded();
    expect(header().textContent).toBe("Thought for a moment");
    expect(lastMarkdown().isStreaming).toBe(false);

    // Refermé par l'utilisateur : fermé, sans aperçu.
    fireEvent.click(header());
    expectFolded();
  });

  it("opened then closed again while thinking, it folds at the end", () => {
    const { rerender } = render(<Thinking thinking="Hmm" startTime={start} />);
    fireEvent.click(header());
    fireEvent.click(header());
    expectPreview();

    rerender(<Thinking thinking="Hmm" startTime={start} endTime={after(5)} />);
    expectFolded();
  });

  it("a finished block opens and closes on click", () => {
    render(<Thinking thinking="Hmm" startTime={start} endTime={after(4)} />);
    expectFolded();
    expect(lastMarkdown().isStreaming).toBe(false);

    fireEvent.click(header());
    expectExpanded();
    // Déplié : pas de hauteur limite, donc pas de défilement interne.
    expect(body().className).not.toContain("overflow-hidden");
    const [bulb, chevron] = header().querySelectorAll("svg");
    expect(bulb.getAttribute("class")).toContain("opacity-0");
    expect(chevron.getAttribute("class")).toContain("rotate-0 opacity-100");

    fireEvent.click(header());
    expectFolded();
  });

  it("without timestamps (old message), it is finished: folded, never stuck on Thinking...", () => {
    render(<Thinking thinking="Old thoughts" />);
    expect(header().textContent).toBe("Thought for a moment");
    expectFolded();
    expect(lastMarkdown()).toEqual({
      content: "Old thoughts",
      isStreaming: false,
      size: "sm",
    });
  });

  it("an end without a start is finished too", () => {
    render(<Thinking thinking="Hmm" endTime={after(3)} />);
    expect(header().textContent).toBe("Thought for a moment");
    expectFolded();
  });

  it("has a real header button: heading, aria-expanded, aria-controls, keyboard", async () => {
    render(<Thinking thinking="Hmm" startTime={start} endTime={after(4)} />);
    const button = screen.getByRole("button", {
      name: "Thought for 4.0 seconds",
      expanded: false,
    });
    expect(button.getAttribute("type")).toBe("button");
    expect(button.parentElement!.tagName).toBe("H3");
    expect(button.getAttribute("aria-controls")).toBe(body().id);
    // Icônes décoratives : le nom du bouton est le titre seul.
    for (const icon of button.querySelectorAll("svg")) {
      expect(icon.getAttribute("aria-hidden")).toBe("true");
    }
    // Anneau bleu comme les contrôles de la zone de saisie (contraste ≥ 3:1
    // sur blanc et sur le fond sombre, WCAG 1.4.11).
    expect(button.className).toContain(
      "focus-visible:ring-2 focus-visible:ring-accent-fg",
    );

    await userEvent.tab();
    expect(document.activeElement).toBe(button);
    await userEvent.keyboard("{Enter}");
    expectExpanded();
    await userEvent.keyboard(" ");
    expectFolded();
  });

  it("shows a fade at the top only while the collapsed preview overflows", () => {
    class FakeResizeObserver {
      static current: FakeResizeObserver | null = null;
      targets = new Set<Element>();
      constructor(private callback: ResizeObserverCallback) {
        FakeResizeObserver.current = this;
      }
      observe(target: Element) {
        this.targets.add(target);
      }
      unobserve(target: Element) {
        this.targets.delete(target);
      }
      disconnect() {
        this.targets.clear();
      }
      fire() {
        act(() => this.callback([], this as unknown as ResizeObserver));
      }
    }
    vi.stubGlobal("ResizeObserver", FakeResizeObserver);

    const { rerender, unmount } = render(
      <Thinking thinking="Short" startTime={start} />,
    );
    const observer = FakeResizeObserver.current!;
    const wrapper = body();
    const content = wrapper.firstElementChild as HTMLElement;
    // Zone et contenu observés tous les deux.
    expect(observer.targets).toEqual(new Set([wrapper, content]));
    const sizes = (contentHeight: number, wrapperHeight: number) => {
      Object.defineProperty(content, "offsetHeight", {
        configurable: true,
        value: contentHeight,
      });
      Object.defineProperty(wrapper, "clientHeight", {
        configurable: true,
        value: wrapperHeight,
      });
      observer.fire();
    };
    const fade = () => screen.queryByTestId("thinking-fade");

    sizes(100, 100);
    expect(fade()).toBeNull();

    rerender(<Thinking thinking={"Long\n".repeat(40)} startTime={start} />);
    sizes(400, 192);
    expect(fade()).not.toBeNull();
    expect(fade()!.className).toContain(
      "bg-gradient-to-b from-white to-transparent dark:from-neutral-900",
    );

    // Déplié : tout est visible, pas de fondu.
    fireEvent.click(header());
    sizes(400, 400);
    expect(fade()).toBeNull();

    unmount();
    expect(observer.targets.size).toBe(0);
  });
});
