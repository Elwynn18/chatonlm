import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const openExternal = vi.hoisted(() => vi.fn());
vi.mock("@/lib/ipc", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/lib/ipc")>()),
  openExternal,
}));

import { linkProblem, resetLinks } from "@/lib/openLink";
import { ExternalLink } from "./ExternalLink";
import { LinkErrorNotice } from "./LinkErrorNotice";

const LINK = "https://ollama.com/library";

beforeEach(() => {
  openExternal.mockReset();
  openExternal.mockResolvedValue(undefined);
});

afterEach(() => {
  cleanup();
  resetLinks();
  Reflect.deleteProperty(navigator, "clipboard");
});

function renderLink(href: string | undefined, title?: string) {
  const parentClick = vi.fn();
  const parentKey = vi.fn();
  render(
    // Le parent ne doit pas voir les clics du lien.
    <p onClick={parentClick} onKeyDown={parentKey}>
      See{" "}
      <ExternalLink
        href={href}
        title={title}
        className="underline"
        data-streamdown="link"
      >
        the library
      </ExternalLink>
    </p>,
  );
  return { parentClick, parentKey };
}

/** Clic d'un bouton auxiliaire (1 : milieu), tel que WebKit l'envoie. */
function auxClick(node: Element, button: number) {
  const event = new MouseEvent("auxclick", {
    button,
    bubbles: true,
    cancelable: true,
  });
  fireEvent(node, event);
  return event;
}

/** Laisse la promesse d'ouverture se terminer. */
async function settle() {
  await act(async () => {
    await Promise.resolve();
  });
}

describe("ExternalLink", () => {
  it("opens a web link on click, without navigating", async () => {
    const { parentClick } = renderLink(LINK);
    const link = screen.getByRole("link", { name: "the library" });
    expect(link.getAttribute("href")).toBe(LINK);
    expect(link.getAttribute("target")).toBeNull();
    expect(link.className).toBe("underline");
    expect(link.dataset.streamdown).toBe("link");

    const allowed = fireEvent.click(link);
    await settle();

    expect(allowed).toBe(false);
    expect(openExternal).toHaveBeenCalledWith(LINK);
    expect(parentClick).not.toHaveBeenCalled();
  });

  it("opens with Enter", async () => {
    renderLink(LINK);
    screen.getByRole("link").focus();
    await userEvent.keyboard("{Enter}");
    expect(openExternal).toHaveBeenCalledWith(LINK);
  });

  it("opens on middle click, and ignores other auxiliary buttons", async () => {
    renderLink(LINK);
    const link = screen.getByRole("link");

    const right = auxClick(link, 2);
    expect(right.defaultPrevented).toBe(true);
    expect(openExternal).not.toHaveBeenCalled();

    const middle = auxClick(link, 1);
    await settle();
    expect(middle.defaultPrevented).toBe(true);
    expect(openExternal).toHaveBeenCalledWith(LINK);
  });

  it("ignores a double click", async () => {
    renderLink(LINK);
    const link = screen.getByRole("link");
    fireEvent.click(link);
    fireEvent.click(link);
    await settle();
    expect(openExternal).toHaveBeenCalledOnce();
  });

  it("replaces the WebKit context menu with Open Link and Copy Link", async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, "clipboard", {
      value: { writeText },
      configurable: true,
    });
    renderLink(LINK);
    const link = screen.getByRole("link");

    const allowed = fireEvent.contextMenu(link, { clientX: 40, clientY: 50 });
    expect(allowed).toBe(false);
    const items = screen.getAllByRole("menuitem").map((i) => i.textContent);
    expect(items).toEqual(["Open Link", "Copy Link"]);
    expect(screen.getByRole("menu").style.left).toBe("40px");

    fireEvent.click(screen.getByRole("menuitem", { name: "Copy Link" }));
    await settle();
    expect(writeText).toHaveBeenCalledWith(LINK);
    expect(screen.queryByRole("menu")).toBeNull();
    expect(openExternal).not.toHaveBeenCalled();

    fireEvent.contextMenu(link);
    fireEvent.click(screen.getByRole("menuitem", { name: "Open Link" }));
    await settle();
    expect(openExternal).toHaveBeenCalledWith(LINK);
  });

  it("reports a failed Copy Link in the notice", async () => {
    Object.defineProperty(navigator, "clipboard", {
      value: { writeText: vi.fn().mockRejectedValue(new Error("denied")) },
      configurable: true,
    });
    // Absente de jsdom : refus de l'ancienne commande « copy ».
    Object.defineProperty(document, "execCommand", {
      value: vi.fn().mockReturnValue(false),
      configurable: true,
    });
    renderLink(LINK);

    fireEvent.contextMenu(screen.getByRole("link"));
    fireEvent.click(screen.getByRole("menuitem", { name: "Copy Link" }));
    await settle();
    await settle();

    expect(linkProblem()).toMatchObject({ kind: "copy", url: LINK });
    Reflect.deleteProperty(document, "execCommand");
  });

  it.each([
    ["http", "http://example.com/a?b=1#c", "http://example.com/a?b=1#c"],
    ["https", LINK, LINK],
    ["localhost", "http://localhost:11434", "http://localhost:11434/"],
  ])(
    "shows where a %s link goes when Markdown gives no title",
    async (_, href, url) => {
      renderLink(href);
      const link = screen.getByRole("link", { name: "the library" });
      // L'adresse ouverte, normalisée, et non le texte du lien.
      expect(link.title).toBe(url);
      expect(link.getAttribute("href")).toBe(href);

      fireEvent.click(link);
      await settle();
      expect(openExternal).toHaveBeenCalledWith(url);
    },
  );

  it("prefers the Markdown title of a link to its address", () => {
    renderLink(LINK, "Ollama library");
    expect(screen.getByRole("link").title).toBe("Ollama library");
  });

  it("treats an empty Markdown title as no title", () => {
    renderLink(LINK, "");
    expect(screen.getByRole("link").title).toBe(LINK);
  });

  it("shows the real destination when the text names another address", () => {
    render(
      <ExternalLink href="https://evil.example/x">
        https://ollama.com/download
      </ExternalLink>,
    );
    expect(screen.getByRole("link").title).toBe("https://evil.example/x");
  });

  it.each([
    ["an empty address", ""],
    ["no address", undefined],
    ["javascript:", "javascript:alert(1)"],
    ["mailto:", "mailto:someone@example.com"],
    ["a relative address", "/c/123"],
    ["an anchor", "#top"],
    ["an incomplete link while streaming", "streamdown:incomplete-link"],
  ])("leaves %s as an inert link, styled like the others", async (_, href) => {
    const { parentClick, parentKey } = renderLink(href);
    expect(screen.queryByRole("link")).toBeNull();
    const text = screen.getByText("the library");
    // Un <a>, pour les styles .prose de `a`, mais sans href : pas un lien.
    expect(text.tagName).toBe("A");
    expect(text.hasAttribute("href")).toBe(false);
    expect(text.hasAttribute("title")).toBe(false);
    expect(text.hasAttribute("rel")).toBe(false);
    expect(text.className).toBe("underline");
    expect(text.dataset.streamdown).toBe("link");
    // Hors de la tabulation : un <a> sans href ne prend pas le focus.
    expect(text.hasAttribute("tabindex")).toBe(false);
    text.focus();
    expect(document.activeElement).not.toBe(text);

    // Aucun gestionnaire : chaque événement suit son cours, jusqu'au parent.
    const click = fireEvent.click(text);
    const middle = auxClick(text, 1);
    const enter = fireEvent.keyDown(text, { key: "Enter" });
    const menu = fireEvent.contextMenu(text);
    await settle();

    expect([click, middle.defaultPrevented, enter, menu]).toEqual([
      true,
      false,
      true,
      true,
    ]);
    expect(parentClick).toHaveBeenCalledOnce();
    expect(parentKey).toHaveBeenCalledOnce();
    expect(screen.queryByRole("menu")).toBeNull();
    expect(openExternal).not.toHaveBeenCalled();
  });

  it("keeps the id, aria and data attributes of a web or inert link", () => {
    render(
      <>
        <ExternalLink
          href={LINK}
          id="user-content-fnref-1"
          aria-describedby="footnote-label"
          data-footnote-ref=""
        >
          web
        </ExternalLink>
        <ExternalLink
          href="#user-content-fnref-1"
          id="back-1"
          aria-label="Back to reference 1"
          data-footnote-backref=""
        >
          inert
        </ExternalLink>
      </>,
    );
    const web = screen.getByRole("link", { name: "web" });
    expect(web.getAttribute("href")).toBe(LINK);
    expect(web.id).toBe("user-content-fnref-1");
    expect(web.getAttribute("aria-describedby")).toBe("footnote-label");
    expect(web.hasAttribute("data-footnote-ref")).toBe(true);

    const inert = screen.getByText("inert");
    expect(inert.hasAttribute("href")).toBe(false);
    expect(inert.id).toBe("back-1");
    expect(inert.getAttribute("aria-label")).toBe("Back to reference 1");
    expect(inert.hasAttribute("data-footnote-backref")).toBe(true);
  });

  it("keeps a Markdown title on an inert link", () => {
    renderLink("mailto:someone@example.com", "Write to us");
    expect(screen.getByText("the library").title).toBe("Write to us");
  });

  it("gives the focus back to the link when its menu closes", async () => {
    Object.defineProperty(navigator, "clipboard", {
      value: { writeText: vi.fn().mockResolvedValue(undefined) },
      configurable: true,
    });
    renderLink(LINK);
    const link = screen.getByRole("link");

    // Touche Menu sur le lien focalisé, puis Échap.
    link.focus();
    fireEvent.contextMenu(link);
    expect(document.activeElement?.textContent).toBe("Open Link");
    fireEvent.keyDown(document.activeElement!, { key: "Escape" });
    expect(screen.queryByRole("menu")).toBeNull();
    expect(document.activeElement).toBe(link);

    // Même chose après un choix.
    fireEvent.contextMenu(link);
    fireEvent.click(screen.getByRole("menuitem", { name: "Copy Link" }));
    await settle();
    expect(screen.queryByRole("menu")).toBeNull();
    expect(document.activeElement).toBe(link);
  });

  it("shows the notice when the browser can't be opened", async () => {
    openExternal.mockRejectedValue({
      kind: "system",
      message: "xdg-open is not installed, so links can't be opened",
    });
    render(
      <>
        <ExternalLink href={LINK}>the library</ExternalLink>
        <LinkErrorNotice />
      </>,
    );

    fireEvent.click(screen.getByRole("link"));
    await settle();

    expect(screen.getByRole("alert").textContent).toContain(
      "Couldn't open the browser (xdg-open is not installed, so links can't be opened). Go to https://ollama.com/library",
    );
  });
});
