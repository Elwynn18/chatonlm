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

import { linkProblem, resetLinks } from "@/lib/openLink";
import { ErrorMessage } from "./ErrorMessage";

beforeEach(() => {
  openExternal.mockReset();
  openExternal.mockResolvedValue(undefined);
});

afterEach(() => {
  cleanup();
  resetLinks();
});

async function settle() {
  await act(async () => {
    await Promise.resolve();
  });
}

describe("ErrorMessage", () => {
  it("turns public addresses into links, keeping the text", () => {
    render(
      <ErrorMessage message="connection failed. Try visiting https://ollama.com or check https://ollama.com/doc for help" />,
    );
    const alert = screen.getByRole("alert");
    expect(alert.textContent).toBe(
      "Errorconnection failed. Try visiting https://ollama.com or check https://ollama.com/doc for help",
    );
    const links = screen.getAllByRole("link");
    expect(links.map((link) => link.getAttribute("href"))).toEqual([
      "https://ollama.com",
      "https://ollama.com/doc",
    ]);
    expect(links[0].className).toBe("break-all underline");
    // Infobulle d'ExternalLink : l'adresse ouverte.
    expect(links.map((link) => link.title)).toEqual([
      "https://ollama.com/",
      "https://ollama.com/doc",
    ]);
  });

  it("leaves local addresses as text, like the official app", () => {
    render(
      <ErrorMessage message="Ollama at http://localhost:11434 or http://192.168.1.2:11434 failed" />,
    );
    expect(screen.queryByRole("link")).toBeNull();
  });

  it("opens links through the backend, with the shared link behavior", async () => {
    render(<ErrorMessage message="See https://ollama.com/download" />);
    const link = screen.getByRole("link");

    expect(fireEvent.click(link)).toBe(false);
    await settle();
    expect(openExternal).toHaveBeenCalledWith("https://ollama.com/download");

    // Menu maison au lieu de celui de WebKit.
    expect(fireEvent.contextMenu(link)).toBe(false);
    expect(screen.getByRole("menuitem", { name: "Copy Link" })).toBeTruthy();
  });

  it("no longer swallows a failure to open the browser", async () => {
    openExternal.mockRejectedValue({
      kind: "system",
      message: "xdg-open exited with 4",
    });
    render(<ErrorMessage message="See https://ollama.com/download" />);

    fireEvent.click(screen.getByRole("link"));
    await settle();

    expect(linkProblem()).toMatchObject({
      kind: "open",
      url: "https://ollama.com/download",
      detail: "xdg-open exited with 4",
    });
  });
});
