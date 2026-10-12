// Raccourci Ctrl+, de bout en bout : l'app (vraies routes, routeur en
// mémoire) face au simulateur, depuis une nouvelle conversation. Aussi
// Ctrl+N, Ctrl+Q, zoom, garde de navigation, menus contextuels et pages
// demandées par le moteur (événement « navigate »).

import {
  act,
  configure,
  fireEvent,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { fakeBackend } from "@/test/backend";
import { composer, renderApp } from "@/test/renderApp";
import { zoomLevel, zoomReset } from "@/lib/zoom";
import { installSimulator, sequence } from "@/test/simulator";

// Rendu complet de l'app : sous la charge de toute la suite, une étape peut
// dépasser l'attente par défaut d'1 s.
configure({ asyncUtilTimeout: 5000 });
vi.setConfig({ testTimeout: 30_000 });

/** Nouvelle conversation affichée, zone de saisie prête. */
async function openNewChat() {
  const simulator = installSimulator();
  fakeBackend(simulator);
  const app = renderApp("/c/new");
  await waitFor(() => expect(composer()).toBeTruthy());
  return app;
}

describe("Ctrl+, opens the settings", () => {
  it("from the chat page", async () => {
    const app = await openNewChat();
    const user = userEvent.setup();
    // Focus hors de tout champ.
    act(() => (document.activeElement as HTMLElement | null)?.blur());
    expect(document.activeElement).toBe(document.body);

    await user.keyboard("{Control>},{/Control}");

    await waitFor(() => expect(app.path()).toBe("/settings"));
    expect(await screen.findByTestId("settings")).toBeTruthy();
  });

  it("from the message field", async () => {
    const app = await openNewChat();
    const user = userEvent.setup();
    await user.click(composer());
    expect(document.activeElement).toBe(composer());

    await user.keyboard("{Control>},{/Control}");

    await waitFor(() => expect(app.path()).toBe("/settings"));
  });

  it("prevents the default action", async () => {
    const app = await openNewChat();

    const notPrevented = fireEvent.keyDown(composer(), {
      key: ",",
      code: "Comma",
      ctrlKey: true,
    });

    expect(notPrevented).toBe(false);
    await waitFor(() => expect(app.path()).toBe("/settings"));
  });

  it("not with Ctrl+Shift+, or without Ctrl", async () => {
    const app = await openNewChat();
    const field = composer();

    const notPrevented = [
      fireEvent.keyDown(field, { key: ",", ctrlKey: true, shiftKey: true }),
      fireEvent.keyDown(document.body, {
        key: ",",
        ctrlKey: true,
        shiftKey: true,
      }),
      fireEvent.keyDown(document.body, { key: "," }),
    ];
    // Laisse à une navigation éventuelle le temps de se faire.
    await act(() => new Promise((resolve) => setTimeout(resolve, 50)));

    expect(notPrevented).toEqual([true, true, true]);
    expect(app.path()).toBe("/c/new");
  });

  it("not while a menu or a dialog is open", async () => {
    const app = await openNewChat();
    const menu = document.createElement("div");
    menu.setAttribute("role", "menu");
    document.body.append(menu);
    try {
      fireEvent.keyDown(document.body, { key: ",", ctrlKey: true });
      await act(() => new Promise((resolve) => setTimeout(resolve, 50)));
      expect(app.path()).toBe("/c/new");
    } finally {
      menu.remove();
    }
  });

  it("saves a chat title being typed before leaving", async () => {
    const simulator = installSimulator();
    const backend = fakeBackend(simulator);
    backend.record(sequence("new-chat-thinking"));
    const renamed: unknown[] = [];
    simulator.handle("rename_chat", (args) => {
      renamed.push(args);
      return null;
    });
    const app = renderApp("/c/new");
    const user = userEvent.setup();
    await waitFor(() => expect(composer()).toBeTruthy());
    const show = screen.queryByLabelText("Show sidebar");
    if (show) await user.click(show);
    const link = await waitFor(() => {
      const found = [...document.querySelectorAll("a[href^='/c/']")].find(
        (a) => a.getAttribute("href") !== "/c/new",
      );
      expect(found).toBeTruthy();
      return found as HTMLElement;
    });
    fireEvent.contextMenu(link);
    await user.click(await screen.findByRole("menuitem", { name: "Rename" }));
    const title = await screen.findByLabelText("Chat title");
    await user.clear(title);
    await user.type(title, "Renamed");

    await user.keyboard("{Control>},{/Control}");

    await waitFor(() => expect(app.path()).toBe("/settings"));
    expect(renamed).toEqual([{ id: expect.any(String), title: "Renamed" }]);
  });
});

describe("Ctrl+N, zoom and navigation guard", () => {
  it("Ctrl+N opens a new chat, from the settings too", async () => {
    const app = await openNewChat();
    const user = userEvent.setup();
    await user.keyboard("{Control>},{/Control}");
    await waitFor(() => expect(app.path()).toBe("/settings"));
    const notPrevented = fireEvent.keyDown(document.body, {
      key: "n",
      ctrlKey: true,
    });
    expect(notPrevented).toBe(false);
    await waitFor(() => expect(app.path()).toBe("/c/new"));
  });

  it("Ctrl+= and Ctrl+- zoom, Ctrl+0 resets", async () => {
    await openNewChat();
    try {
      fireEvent.keyDown(composer(), { key: "=", ctrlKey: true });
      expect(zoomLevel()).toBe(1.1);
      fireEvent.keyDown(document.body, { key: "-", ctrlKey: true });
      fireEvent.keyDown(document.body, { key: "-", ctrlKey: true });
      expect(zoomLevel()).toBe(0.9);
      const notPrevented = fireEvent.keyDown(document.body, {
        key: "0",
        ctrlKey: true,
      });
      expect(notPrevented).toBe(false);
      expect(zoomLevel()).toBe(1);
    } finally {
      await zoomReset();
    }
  });

  it("blocks reloads, Backspace outside a field and the mouse back button", async () => {
    const app = await openNewChat();
    const blocked = [
      fireEvent.keyDown(document.body, { key: "F5" }),
      fireEvent.keyDown(composer(), { key: "r", ctrlKey: true }),
      fireEvent.keyDown(document.body, { key: "Backspace" }),
      fireEvent.keyDown(document.body, { key: "ArrowLeft", altKey: true }),
      fireEvent.mouseDown(document.body, { button: 3 }),
      fireEvent.mouseUp(document.body, { button: 4 }),
    ];
    expect(blocked).toEqual([false, false, false, false, false, false]);
    // Dans la zone de saisie, Retour arrière efface.
    expect(fireEvent.keyDown(composer(), { key: "Backspace" })).toBe(true);
    expect(fireEvent.mouseDown(document.body, { button: 0 })).toBe(true);
    expect(app.path()).toBe("/c/new");
  });
});

describe("corrections of a reloaded page", () => {
  it("are stopped once when the app loads", async () => {
    const simulator = installSimulator();
    const backend = fakeBackend(simulator);
    renderApp("/c/new");
    await waitFor(() => expect(composer()).toBeTruthy());
    await waitFor(() => expect(backend.proofread.cancelAllCalls).toBe(1));
  });
});

describe("undo and redo in a text field", () => {
  it("runs the native editing commands (WebKitGTK does not bind the keys)", async () => {
    await openNewChat();
    const execCommand = vi.fn(() => true);
    Object.defineProperty(document, "execCommand", {
      configurable: true,
      value: execCommand,
    });
    try {
      expect(fireEvent.keyDown(composer(), { key: "z", ctrlKey: true })).toBe(
        false,
      );
      expect(
        fireEvent.keyDown(composer(), {
          key: "Z",
          ctrlKey: true,
          shiftKey: true,
        }),
      ).toBe(false);
      expect(fireEvent.keyDown(composer(), { key: "y", ctrlKey: true })).toBe(
        false,
      );
      expect(execCommand.mock.calls).toEqual([["undo"], ["redo"], ["redo"]]);
      // Hors d'un champ de texte, ou touche déjà prise : rien.
      expect(
        fireEvent.keyDown(document.body, { key: "z", ctrlKey: true }),
      ).toBe(true);
      composer().addEventListener(
        "keydown",
        (event) => event.preventDefault(),
        {
          once: true,
        },
      );
      fireEvent.keyDown(composer(), { key: "z", ctrlKey: true });
      expect(execCommand).toHaveBeenCalledTimes(3);
    } finally {
      Reflect.deleteProperty(document, "execCommand");
    }
  });
});

describe("context menu zones", () => {
  it("keeps the WebKit menu in the chat and the composer only", async () => {
    await openNewChat();
    // Zone de saisie : menu permis.
    expect(fireEvent.contextMenu(composer())).toBe(true);
    // Ailleurs (fond de page, bouton) : aucun menu.
    expect(fireEvent.contextMenu(document.body)).toBe(false);
    // Bouton de la barre latérale (ouverte ou non selon les tests d'avant).
    const toggle = screen.getByLabelText(/^(Show|Hide) sidebar$/);
    expect(fireEvent.contextMenu(toggle)).toBe(false);
  });

  it("in the settings, only in the address field", async () => {
    const app = await openNewChat();
    await act(() => app.router.navigate({ to: "/settings" }));
    const field = await screen.findByLabelText("Server address");
    expect(fireEvent.contextMenu(field)).toBe(true);
    expect(fireEvent.contextMenu(screen.getByText("Context length"))).toBe(
      false,
    );
  });
});

describe("Ctrl+Q quits", () => {
  it("from the chat and from a field, with Caps Lock too", async () => {
    const simulator = installSimulator();
    fakeBackend(simulator);
    renderApp("/c/new");
    await waitFor(() => expect(composer()).toBeTruthy());
    const notPrevented = fireEvent.keyDown(composer(), {
      key: "q",
      ctrlKey: true,
    });
    expect(notPrevented).toBe(false);
    fireEvent.keyDown(document.body, { key: "Q", ctrlKey: true });
    await waitFor(() => expect(simulator.callsTo("quit_app")).toHaveLength(2));
  });

  it("not with Shift, Alt, without Ctrl, nor repeated", async () => {
    const simulator = installSimulator();
    fakeBackend(simulator);
    renderApp("/c/new");
    await waitFor(() => expect(composer()).toBeTruthy());
    const notPrevented = [
      fireEvent.keyDown(document.body, {
        key: "Q",
        ctrlKey: true,
        shiftKey: true,
      }),
      fireEvent.keyDown(document.body, {
        key: "q",
        ctrlKey: true,
        altKey: true,
      }),
      fireEvent.keyDown(document.body, { key: "q" }),
      fireEvent.keyDown(document.body, {
        key: "q",
        ctrlKey: true,
        repeat: true,
      }),
    ];
    await act(() => new Promise((resolve) => setTimeout(resolve, 50)));
    expect(notPrevented).toEqual([true, true, true, true]);
    expect(simulator.callsTo("quit_app")).toEqual([]);
  });

  it("a failure to quit is logged, nothing else", async () => {
    const simulator = installSimulator();
    fakeBackend(simulator);
    simulator.handle("quit_app", () => {
      throw { kind: "internal", message: "busy" };
    });
    const logged = vi.spyOn(console, "error").mockImplementation(() => {});
    try {
      renderApp("/c/new");
      await waitFor(() => expect(composer()).toBeTruthy());
      fireEvent.keyDown(document.body, { key: "q", ctrlKey: true });
      await waitFor(() =>
        expect(logged).toHaveBeenCalledWith("Failed to quit:", {
          kind: "internal",
          message: "busy",
        }),
      );
    } finally {
      logged.mockRestore();
    }
  });
});

describe("pages asked by the backend (navigate event)", () => {
  async function chat() {
    const simulator = installSimulator();
    fakeBackend(simulator);
    const app = renderApp("/c/new");
    await waitFor(() => expect(composer()).toBeTruthy());
    await waitFor(() => expect(simulator.listening("navigate")).toBe(1));
    return { simulator, app };
  }

  it("opens the dictation settings, its card focused", async () => {
    const { simulator, app } = await chat();
    act(() => (document.activeElement as HTMLElement | null)?.blur());
    await simulator.emitEvent("navigate", "/settings?section=dictation");
    await waitFor(() => expect(app.path()).toBe("/settings"));
    expect(app.router.state.location.search).toEqual({ section: "dictation" });
    const card = await screen.findByTestId("dictation");
    await waitFor(() =>
      expect(document.activeElement).toBe(
        within(card).getByRole("heading", { name: "Dictation" }),
      ),
    );
  });

  it("opens a page of the ModelHub", async () => {
    const { simulator, app } = await chat();
    await simulator.emitEvent("navigate", "/hub/transcription");
    await waitFor(() => expect(app.path()).toBe("/hub/transcription"));
  });

  it("not while a menu or a dialog is open", async () => {
    const { simulator, app } = await chat();
    const dialog = document.createElement("div");
    dialog.setAttribute("aria-modal", "true");
    document.body.append(dialog);
    try {
      await simulator.emitEvent("navigate", "/settings?section=dictation");
      await act(() => new Promise((resolve) => setTimeout(resolve, 50)));
      expect(app.path()).toBe("/c/new");
    } finally {
      dialog.remove();
    }
  });

  it("the page already shown is replaced, not stacked in the history", async () => {
    const { simulator, app } = await chat();
    await simulator.emitEvent("navigate", "/hub/transcription");
    await waitFor(() => expect(app.path()).toBe("/hub/transcription"));
    const length = app.router.history.length;
    await simulator.emitEvent("navigate", "/hub/transcription");
    await waitFor(() =>
      expect(app.router.state.location.state.navigateRequest).toBeGreaterThan(
        1,
      ),
    );
    expect(app.router.history.length).toBe(length);
  });

  it.each([
    "/c/new",
    "/",
    "https://example.com/settings",
    "//example.com/settings",
    "javascript:alert(1)",
    "/settings\\..\\x",
  ])("refuses %s", async (path) => {
    const { simulator, app } = await chat();
    await app.router.navigate({ to: "/hub/installed" });
    await waitFor(() => expect(app.path()).toBe("/hub/installed"));
    const logged = vi.spyOn(console, "error").mockImplementation(() => {});
    try {
      await simulator.emitEvent("navigate", path);
      await act(() => new Promise((resolve) => setTimeout(resolve, 50)));
      expect(app.path()).toBe("/hub/installed");
      expect(logged).toHaveBeenCalledWith("Refused navigation to", path);
    } finally {
      logged.mockRestore();
    }
  });
});
