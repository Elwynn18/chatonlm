// Couche d'hôte (host.ts) : les composants partagés du rendu des réponses
// passent par l'hôte du contexte (panneau VS Code), l'app par défaut.
import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
} from "@testing-library/react";
import { useRef } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";

const openExternal = vi.hoisted(() => vi.fn(() => Promise.resolve()));
vi.mock("@/lib/ipc", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/lib/ipc")>()),
  openExternal,
}));

import { CopyButton } from "@/components/CopyButton";
import { ExternalLink } from "@/components/ExternalLink";
import { StreamingMarkdownContent } from "@/components/StreamingMarkdownContent";
import { appHost } from "@/lib/appHost";
import { copyRich, copyText } from "@/lib/clipboard";
import { type Host, HostContext, useHost } from "@/lib/host";
import { openLink, reportCopyFailure, resetLinks } from "@/lib/openLink";

const LINK = "https://chatonlm.com/";

function fakeHost(overrides: Partial<Host> = {}): Host {
  return {
    openLink: vi.fn(() => Promise.resolve()),
    copyText: vi.fn(() => Promise.resolve()),
    copyRich: vi.fn(() => Promise.resolve()),
    copyLinkFailed: vi.fn(),
    ...overrides,
  };
}

afterEach(() => {
  cleanup();
  resetLinks();
  openExternal.mockClear();
});

async function settle() {
  await act(async () => {
    await Promise.resolve();
  });
}

function RichCopy({ text }: { text: string }) {
  const ref = useRef<HTMLDivElement>(null);
  return (
    <>
      <div ref={ref}>
        <strong>{text}</strong>
      </div>
      <CopyButton content={text} copyRef={ref} title="copy-rich" />
    </>
  );
}

describe("host layer", () => {
  it("defaults to the app: Tauri for links, the webview clipboard", () => {
    function Probe() {
      return <span>{useHost() === appHost ? "app" : "other"}</span>;
    }
    render(<Probe />);
    expect(screen.getByText("app")).toBeTruthy();
    expect(appHost).toEqual({
      openLink,
      copyText,
      copyRich,
      copyLinkFailed: reportCopyFailure,
    });
  });

  it("opens links of a reply through the host of the context", async () => {
    const host = fakeHost();
    render(
      <HostContext value={host}>
        <StreamingMarkdownContent content={`See [the site](${LINK}).`} />
      </HostContext>,
    );
    fireEvent.click(screen.getByRole("link", { name: "the site" }));
    await settle();
    expect(host.openLink).toHaveBeenCalledWith(LINK);
    expect(openExternal).not.toHaveBeenCalled();
  });

  it("copies a link address through the host, and reports a failure to it", async () => {
    const host = fakeHost({
      copyText: vi.fn(() => Promise.reject(new Error("refused"))),
    });
    render(
      <HostContext value={host}>
        <ExternalLink href={LINK}>site</ExternalLink>
      </HostContext>,
    );
    fireEvent.contextMenu(screen.getByRole("link"), {
      clientX: 10,
      clientY: 10,
    });
    fireEvent.click(screen.getByRole("menuitem", { name: "Copy Link" }));
    await settle();
    expect(host.copyText).toHaveBeenCalledWith(LINK);
    expect(host.copyLinkFailed).toHaveBeenCalledWith(LINK);

    fireEvent.contextMenu(screen.getByRole("link"), {
      clientX: 10,
      clientY: 10,
    });
    fireEvent.click(screen.getByRole("menuitem", { name: "Open Link" }));
    await settle();
    expect(host.openLink).toHaveBeenCalledWith(LINK);
  });

  it("copies text and rich replies through the host", async () => {
    const host = fakeHost();
    render(
      <HostContext value={host}>
        <CopyButton content="plain" title="copy-plain" />
        <RichCopy text="**rich**" />
      </HostContext>,
    );
    fireEvent.click(screen.getByRole("button", { name: "copy-plain" }));
    await settle();
    expect(host.copyText).toHaveBeenCalledWith("plain");
    expect(screen.getAllByRole("status")[0].textContent).toBe("Copied");

    fireEvent.click(screen.getByRole("button", { name: "copy-rich" }));
    await settle();
    expect(host.copyRich).toHaveBeenCalledWith(
      expect.stringContaining("<strong>**rich**</strong>"),
      "**rich**",
    );
  });

  it("shows a failed copy reported by the host", async () => {
    const host = fakeHost({
      copyText: vi.fn(() => Promise.reject(new Error("no clipboard"))),
    });
    render(
      <HostContext value={host}>
        <CopyButton content="plain" title="copy-plain" />
      </HostContext>,
    );
    fireEvent.click(screen.getByRole("button", { name: "copy-plain" }));
    await settle();
    expect(screen.getByRole("status").textContent).toBe("Copy failed");
  });
});
