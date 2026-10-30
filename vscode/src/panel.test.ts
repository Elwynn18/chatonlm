// @vitest-environment node
import type * as vscode from "vscode";
import { describe, expect, it, vi } from "vitest";
import {
  ChatPanel,
  contentSecurityPolicy,
  panelLocale,
  panelTheme,
} from "./panel";
import { ColorThemeKind, Uri } from "./test/vscode";

function fakeView() {
  let receive: ((m: unknown) => void) | null = null;
  let dispose: (() => void) | null = null;
  let visibility: (() => void) | null = null;
  const posted: unknown[] = [];
  const view = {
    show: vi.fn(),
    visible: true,
    onDidDispose: (cb: () => void) => void (dispose = cb),
    onDidChangeVisibility: (cb: () => void) => void (visibility = cb),
    webview: {
      options: {} as vscode.WebviewOptions,
      html: "",
      cspSource: "https://file+.vscode-resource.vscode-cdn.net",
      asWebviewUri: (uri: Uri) => ({
        toString: () =>
          `https://file+.vscode-resource.vscode-cdn.net${uri.path}`,
      }),
      onDidReceiveMessage: (cb: (m: unknown) => void) => void (receive = cb),
      postMessage: async (m: unknown) => void posted.push(m),
    },
  };
  return {
    view: view as unknown as vscode.WebviewView,
    raw: view,
    posted,
    receive: (m: unknown) => receive?.(m),
    dispose: () => dispose?.(),
    setVisible: (visible: boolean) => {
      view.visible = visible;
      visibility?.();
    },
  };
}

function panel(onMessage = vi.fn(), onVisible = vi.fn()) {
  return new ChatPanel({
    extensionUri: Uri.file("/ext") as unknown as vscode.Uri,
    onMessage,
    onVisible,
    locale: () => "es",
    theme: () => "dark",
  });
}

describe("panel", () => {
  it("loads the built page from dist/webview under a strict policy", () => {
    const { view, raw } = fakeView();
    panel().resolveWebviewView(view);
    expect(raw.webview.options.enableScripts).toBe(true);
    expect(raw.webview.options.localResourceRoots?.map((u) => u.path)).toEqual([
      "/ext/dist/webview",
    ]);
    const html = raw.webview.html;
    const nonce = /nonce="([^"]+)"/.exec(html)?.[1];
    expect(nonce).toMatch(/^[\w-]{16,}$/);
    const csp = /Content-Security-Policy" content="([^"]+)"/.exec(html)?.[1];
    expect(csp).toBe(
      contentSecurityPolicy(
        "https://file+.vscode-resource.vscode-cdn.net",
        nonce!,
      ),
    );
    expect(csp).toContain("default-src 'none'");
    expect(csp).toContain(`script-src 'nonce-${nonce}'`);
    expect(csp).not.toMatch(
      /unsafe-eval|script-src[^;]*unsafe-inline|https?:\/\/(?!file\+)/,
    );
    expect(html).toContain(
      'src="https://file+.vscode-resource.vscode-cdn.net/ext/dist/webview/index.js"',
    );
    expect(html).toContain(
      'href="https://file+.vscode-resource.vscode-cdn.net/ext/dist/webview/index.css"',
    );
    expect(html).toMatch(/<html lang="es" class="dark">/);
    // Nouveau nonce à chaque chargement.
    const again = fakeView();
    panel().resolveWebviewView(again.view);
    expect(/nonce="([^"]+)"/.exec(again.raw.webview.html)?.[1]).not.toBe(nonce);
  });

  it("posts to the page only once it is ready, and relays its messages", () => {
    const onMessage = vi.fn();
    const p = panel(onMessage);
    const { view, posted, receive, dispose } = fakeView();
    p.resolveWebviewView(view);
    p.post({ type: "theme", theme: "light" });
    expect(posted).toEqual([]);
    receive({ type: "ready" });
    expect(onMessage).toHaveBeenCalledWith({ type: "ready" });
    p.post({ type: "theme", theme: "light" });
    expect(posted).toEqual([{ type: "theme", theme: "light" }]);
    receive({ type: "send", text: "x" });
    expect(onMessage).toHaveBeenLastCalledWith({ type: "send", text: "x" });
    dispose();
    p.post({ type: "theme", theme: "dark" });
    expect(posted).toHaveLength(1);
  });

  it("replays a send's messages after init when the page was not ready", () => {
    // Action lancée panneau jamais ouvert : la page met un moment à dire
    // « ready » ; la réponse doit quand même s'afficher en direct.
    const onMessage = vi.fn();
    const p = panel(onMessage);
    const { view, posted, receive } = fakeView();
    const event = {
      type: "event",
      requestId: 7,
      event: { type: "content", text: "Hi" },
    } as const;
    const receipt = {
      type: "receipt",
      requestId: 7,
      receipt: {} as never,
    } as const;
    p.post(receipt);
    p.resolveWebviewView(view);
    p.post({ type: "state", state: {} as never });
    p.post(event);
    receive({ type: "ready" });
    expect(posted).toEqual([]);
    const init = { type: "init", locale: "en", theme: "light" } as never;
    p.post(init);
    // Après init seulement, dans l'ordre ; l'ancien état n'est pas rejoué.
    expect(posted).toEqual([init, receipt, event]);
    p.post(init);
    expect(posted).toHaveLength(4);
  });

  it("says when the view shows again", () => {
    const onVisible = vi.fn();
    const p = panel(vi.fn(), onVisible);
    const { view, receive, setVisible } = fakeView();
    p.resolveWebviewView(view);
    receive({ type: "ready" });
    setVisible(false);
    expect(onVisible).not.toHaveBeenCalled();
    setVisible(true);
    expect(onVisible).toHaveBeenCalledTimes(1);
  });

  it("brings VS Code's language down to en, es or fr", () => {
    expect(panelLocale("fr")).toBe("fr");
    expect(panelLocale("es-ES")).toBe("es");
    expect(panelLocale("pt-br")).toBe("en");
    expect(panelLocale("en")).toBe("en");
  });

  it("follows VS Code's theme", () => {
    expect(panelTheme(ColorThemeKind.Dark as never)).toBe("dark");
    expect(panelTheme(ColorThemeKind.HighContrast as never)).toBe("dark");
    expect(panelTheme(ColorThemeKind.Light as never)).toBe("light");
    expect(panelTheme(ColorThemeKind.HighContrastLight as never)).toBe("light");
  });
});
