import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
} from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const openExternal = vi.hoisted(() => vi.fn());
vi.mock("@/lib/ipc", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/lib/ipc")>()),
  openExternal,
}));

import { openLink, reportCopyFailure, resetLinks } from "@/lib/openLink";
import { LinkErrorNotice } from "./LinkErrorNotice";

const LINK = "https://ollama.com/download";
const DETAIL =
  "no application is set to open web links (xdg-open exited with 3)";

beforeEach(() => {
  vi.useFakeTimers({ now: 100_000 });
  openExternal.mockReset();
  openExternal.mockRejectedValue({ kind: "system", message: DETAIL });
});

afterEach(() => {
  cleanup();
  vi.useRealTimers();
  resetLinks();
  Reflect.deleteProperty(navigator, "clipboard");
});

/** Échec d'ouverture de `url`, puis affichage. */
async function failToOpen(url = LINK) {
  await act(() => openLink(url));
}

/** Laisse une copie (promesses) se terminer. */
async function settle() {
  await act(async () => {
    await Promise.resolve();
  });
}

describe("LinkErrorNotice", () => {
  it("shows nothing while links open fine", async () => {
    openExternal.mockResolvedValue(undefined);
    const { container } = render(<LinkErrorNotice />);
    await act(() => openLink(LINK));
    expect(container.innerHTML).toBe("");
  });

  it("gives the reason and a selectable address", async () => {
    render(<LinkErrorNotice />);
    await failToOpen();

    const alert = screen.getByRole("alert");
    expect(alert.textContent).toContain(
      `Couldn't open the browser (${DETAIL}). Go to ${LINK}`,
    );
    expect(screen.getByText(LINK).className).toContain("select-text");
    expect(screen.getByRole("button", { name: "Copy link" })).toBeTruthy();
  });

  it("closes by itself after 8 s", async () => {
    render(<LinkErrorNotice />);
    await failToOpen();

    act(() => vi.advanceTimersByTime(7999));
    expect(screen.queryByRole("alert")).not.toBeNull();
    act(() => vi.advanceTimersByTime(1));
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("stays while the pointer or the focus is on it", async () => {
    render(<LinkErrorNotice />);
    await failToOpen();
    const alert = screen.getByRole("alert");

    fireEvent.mouseEnter(alert);
    act(() => vi.advanceTimersByTime(20_000));
    expect(screen.queryByRole("alert")).not.toBeNull();

    // Le focus garde l'avis après le départ du pointeur.
    fireEvent.focus(screen.getByRole("button", { name: "Copy link" }));
    fireEvent.mouseLeave(alert);
    act(() => vi.advanceTimersByTime(20_000));
    expect(screen.queryByRole("alert")).not.toBeNull();

    // Délai complet à partir du départ du focus.
    fireEvent.blur(screen.getByRole("button", { name: "Copy link" }));
    act(() => vi.advanceTimersByTime(7999));
    expect(screen.queryByRole("alert")).not.toBeNull();
    act(() => vi.advanceTimersByTime(1));
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("closes with its button or Escape", async () => {
    render(<LinkErrorNotice />);
    await failToOpen();
    fireEvent.click(screen.getByRole("button", { name: "Dismiss" }));
    expect(screen.queryByRole("alert")).toBeNull();

    vi.setSystemTime(200_000);
    await failToOpen();
    const escape = fireEvent.keyDown(screen.getByRole("alert"), {
      key: "Escape",
    });
    expect(escape).toBe(false);
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("sits at the top of the content area, clear of the composer", async () => {
    render(<LinkErrorNotice />);
    await failToOpen();

    const alert = screen.getByRole("alert");
    const band = alert.parentElement!;
    // Placé par le <main> de SidebarLayout (relative), sous sa barre du haut.
    const bandClasses = band.className.split(" ");
    expect(bandClasses).toEqual(
      expect.arrayContaining(["absolute", "inset-x-0", "top-14", "z-50"]),
    );
    expect(band.className).not.toMatch(/\b(fixed|bottom-\S+)/);
    // La bande laisse passer le pointeur ; seul l'avis le prend.
    expect(bandClasses).toContain("pointer-events-none");
    expect(alert.className.split(" ")).toContain("pointer-events-auto");
  });

  describe("gives the focus back when dismissed", () => {
    /** Zone de saisie focalisée, puis focus passé à l'avis (Tab, clic). */
    async function focusFromComposer() {
      render(
        <>
          <textarea aria-label="Message" />
          <button type="button">Elsewhere</button>
          <LinkErrorNotice />
        </>,
      );
      await failToOpen();
      const composer = screen.getByLabelText("Message");
      composer.focus();
      return composer;
    }

    it("with its button", async () => {
      const composer = await focusFromComposer();
      const dismiss = screen.getByRole("button", { name: "Dismiss" });
      dismiss.focus();
      fireEvent.click(dismiss);
      expect(screen.queryByRole("alert")).toBeNull();
      expect(document.activeElement).toBe(composer);
    });

    it("with Escape, after moving between its buttons", async () => {
      const composer = await focusFromComposer();
      screen.getByRole("button", { name: "Copy link" }).focus();
      const dismiss = screen.getByRole("button", { name: "Dismiss" });
      dismiss.focus();
      fireEvent.keyDown(dismiss, { key: "Escape" });
      expect(screen.queryByRole("alert")).toBeNull();
      expect(document.activeElement).toBe(composer);
    });

    it("but steals nothing when it no longer has the focus", async () => {
      await focusFromComposer();
      const dismiss = screen.getByRole("button", { name: "Dismiss" });
      dismiss.focus();
      const elsewhere = screen.getByRole("button", { name: "Elsewhere" });
      elsewhere.focus();
      // Clic sans prise de focus (WebKit sur certains boutons).
      fireEvent.click(dismiss);
      expect(screen.queryByRole("alert")).toBeNull();
      expect(document.activeElement).toBe(elsewhere);
    });

    it("and leaves it on the page when nothing had it before", async () => {
      render(<LinkErrorNotice />);
      await failToOpen();
      const dismiss = screen.getByRole("button", { name: "Dismiss" });
      dismiss.focus();
      fireEvent.click(dismiss);
      expect(document.activeElement).toBe(document.body);
    });
  });

  it("copies the address", async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, "clipboard", {
      value: { writeText },
      configurable: true,
    });
    render(<LinkErrorNotice />);
    await failToOpen();

    const copy = screen.getByRole("button", { name: "Copy link" });
    fireEvent.click(copy);
    expect(writeText).toHaveBeenCalledWith(LINK);
    await settle();
    expect(copy.textContent).toBe("Copied");
  });

  it("selects the address when it can't be copied", async () => {
    Object.defineProperty(navigator, "clipboard", {
      value: { writeText: vi.fn().mockRejectedValue(new Error("denied")) },
      configurable: true,
    });
    render(<LinkErrorNotice />);
    await failToOpen();

    const copy = screen.getByRole("button", { name: "Copy link" });
    fireEvent.click(copy);
    await settle();
    await settle();

    expect(copy.textContent).toBe("Copy failed");
    expect(window.getSelection()?.toString()).toBe(LINK);
  });

  it("is replaced by a newer failure, with a fresh delay", async () => {
    render(<LinkErrorNotice />);
    await failToOpen();
    act(() => vi.advanceTimersByTime(5000));

    act(() => reportCopyFailure("https://ollama.com/library"));
    expect(screen.getByRole("alert").textContent).toContain(
      "Couldn't copy the link: https://ollama.com/library",
    );
    act(() => vi.advanceTimersByTime(5000));
    expect(screen.queryByRole("alert")).not.toBeNull();
    act(() => vi.advanceTimersByTime(3000));
    expect(screen.queryByRole("alert")).toBeNull();
  });
});
