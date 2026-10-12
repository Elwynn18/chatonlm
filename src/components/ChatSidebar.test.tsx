// Barre latérale de l'historique, dans l'app (vraies routes) face au
// simulateur : affichée aussi sur le ModelHub et les réglages, rubrique
// « Favorites », boutons étoile et poubelle de chaque ligne, menu contextuel.

import {
  act,
  configure,
  fireEvent,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import { onlineManager } from "@tanstack/react-query";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, onTestFinished, vi } from "vitest";
import { listChats } from "@/lib/ipc";
import { fakeBackend, type StoredChat } from "@/test/backend";
import { renderApp } from "@/test/renderApp";
import { installSimulator } from "@/test/simulator";

configure({ asyncUtilTimeout: 5000 });
vi.setConfig({ testTimeout: 30_000 });

const DAY = 24 * 60 * 60 * 1000;
const CONFIRM = "Are you sure you want to remove this chat?";

function stored(id: string, ageMs: number, favorite = false): StoredChat {
  const at = Date.now() - ageMs;
  return {
    id,
    title: `Chat ${id}`,
    createdAt: at,
    updatedAt: at,
    messages: [],
    favorite,
  };
}

/**
 * App à `path`, historique : « today » et « older » (40 jours), plus les
 * conversations données ; delete_chat les retire.
 */
function setup(path: string, extra: StoredChat[] = []) {
  const simulator = installSimulator();
  const backend = fakeBackend(simulator);
  for (const chat of [
    stored("today", 60_000),
    stored("older", 40 * DAY),
    ...extra,
  ]) {
    backend.chats.set(chat.id, chat);
  }
  simulator.handle("delete_chat", (args) => {
    backend.chats.delete(args.id as string);
    return null;
  });
  const app = renderApp(path);
  return { simulator, backend, app };
}

/** Barre latérale ouverte (son état dure toute la session), historique lu. */
async function openHistory() {
  const toggle = await screen.findByRole("button", {
    name: /^(Show|Hide) sidebar$/,
  });
  if (toggle.getAttribute("aria-label") === "Show sidebar") {
    await userEvent.click(toggle);
  }
  await waitFor(() => expect(row("today")).toBeTruthy());
}

function row(id: string) {
  return document.querySelector<HTMLElement>(`[data-chat-id="${id}"]`);
}

function rowOf(id: string) {
  const found = row(id);
  if (!found) throw new Error(`no row for ${id}`);
  return found;
}

function star(id: string) {
  return within(rowOf(id)).getByTestId("chat-favorite");
}

function trash(id: string) {
  return within(rowOf(id)).getByTestId("chat-delete");
}

/** Rubriques de l'historique et leurs conversations, dans l'ordre. */
function history() {
  return [...document.querySelectorAll("nav h3")].map((heading) => [
    heading.textContent,
    [...heading.parentElement!.querySelectorAll("[data-chat-id]")].map(
      (element) => element.getAttribute("data-chat-id"),
    ),
  ]);
}

const classes = (element: Element) => element.className.split(/\s+/);

/** Laisse au rendu le temps de se faire (effets compris). */
async function settle() {
  await act(() => new Promise((resolve) => setTimeout(resolve, 50)));
}

/**
 * Focus clavier (`:focus-visible`) ou non pour l'élément actif. jsdom le
 * devine mal au moment du clic (faux pour Entrée sur un bouton, vrai pour un
 * clic venu d'un champ), alors que WebKit le fixe à la prise du focus : le
 * test le dit lui-même.
 */
function focusVisible(visible: boolean) {
  const matches = Element.prototype.matches;
  const spy = vi
    .spyOn(Element.prototype, "matches")
    .mockImplementation(function (this: Element, selector: string) {
      return selector === ":focus-visible"
        ? visible && this === document.activeElement
        : matches.call(this, selector);
    });
  onTestFinished(() => spy.mockRestore());
}

/** set_chat_favorite suspendu, une réponse par conversation. */
function suspendFavorites(
  simulator: ReturnType<typeof setup>["simulator"],
  backend: ReturnType<typeof setup>["backend"],
) {
  const answers = new Map<string, ReturnType<typeof deferred<null>>>();
  simulator.handle("set_chat_favorite", (args) => {
    const id = args.id as string;
    const answer = deferred<null>();
    answers.set(id, answer);
    return answer.promise.then(() => {
      backend.chats.get(id)!.favorite = args.favorite as boolean;
      return null;
    });
  });
  return (id: string) => {
    const answer = answers.get(id);
    if (!answer) throw new Error(`no set_chat_favorite for ${id}`);
    return answer;
  };
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<T>((ok, ko) => {
    resolve = ok;
    reject = ko;
  });
  return { promise, resolve, reject };
}

describe("history on every page", () => {
  it("shows the chats on ModelHub and Settings, their entry highlighted, no chat highlighted", async () => {
    const { app } = setup("/hub");
    await openHistory();
    // Barre recréée à chaque page : relue à chaque fois.
    const link = (name: string) =>
      within(rowOf("today").closest("nav")!).getByRole("link", { name });
    // Entrée courante : aplat roux ; conversation ouverte : sable.
    const highlighted = (element: Element) =>
      classes(element).some((name) =>
        ["bg-accent", "bg-neutral-200"].includes(name),
      );

    expect(history()).toEqual([
      ["Today", ["today"]],
      ["Older", ["older"]],
    ]);
    expect(highlighted(link("ModelHub"))).toBe(true);
    expect(highlighted(link("Chat"))).toBe(false);
    expect(highlighted(rowOf("today"))).toBe(false);

    await act(() => app.router.navigate({ to: "/settings" }));
    await screen.findByTestId("settings");
    await waitFor(() => expect(row("today")).toBeTruthy());
    expect(highlighted(link("Settings"))).toBe(true);
    expect(highlighted(link("ModelHub"))).toBe(false);
    expect(highlighted(rowOf("today"))).toBe(false);

    // Dans le chat, la conversation ouverte l'est.
    await act(() =>
      app.router.navigate({
        to: "/c/$chatId",
        params: { chatId: "today" },
      }),
    );
    await waitFor(() => expect(highlighted(rowOf("today"))).toBe(true));
    expect(highlighted(link("Chat"))).toBe(true);
    expect(highlighted(link("Settings"))).toBe(false);
  });

  it("a chat clicked in the settings opens; one deleted there leaves the page as is", async () => {
    const { app, simulator } = setup("/settings");
    await openHistory();

    await userEvent.click(trash("older"));
    await userEvent.click(screen.getByRole("button", { name: "OK" }));
    await waitFor(() => expect(row("older")).toBeNull());
    expect(simulator.callsTo("delete_chat")).toEqual([{ id: "older" }]);
    expect(app.path()).toBe("/settings");

    await userEvent.click(within(rowOf("today")).getByRole("link"));
    await waitFor(() => expect(app.path()).toBe("/c/today"));
  });
});

describe("favorites", () => {
  it("are listed first, under Favorites, and only there", async () => {
    setup("/c/new", [stored("old-favorite", 30 * DAY, true)]);
    await openHistory();
    expect(history()).toEqual([
      ["Favorites", ["old-favorite"]],
      ["Today", ["today"]],
      ["Older", ["older"]],
    ]);
  });

  it("the star moves a chat to Favorites at once, then saves it", async () => {
    const { simulator, backend, app } = setup("/c/new");
    await openHistory();
    const answer = deferred<null>();
    simulator.handle("set_chat_favorite", (args) => {
      backend.chats.get(args.id as string)!.favorite = args.favorite as boolean;
      return answer.promise;
    });
    const before = backend.chats.get("older")!.updatedAt;

    // Nom fixe, état dans aria-pressed, conversation en description.
    expect(
      within(rowOf("older")).getByRole("button", {
        name: "Favorite",
        description: "Chat older",
        pressed: false,
      }),
    ).toBe(star("older"));
    expect(star("older").title).toBe("Add to favorites");
    await userEvent.click(star("older"));

    // Avant la réponse du moteur.
    await waitFor(() =>
      expect(history()).toEqual([
        ["Favorites", ["older"]],
        ["Today", ["today"]],
      ]),
    );
    expect(
      within(rowOf("older")).getByRole("button", {
        name: "Favorite",
        description: "Chat older",
        pressed: true,
      }),
    ).toBe(star("older"));
    expect(star("older").title).toBe("Remove from favorites");
    expect(simulator.callsTo("set_chat_favorite")).toEqual([
      { id: "older", favorite: true },
    ]);
    // Le bouton est hors du lien : pas de navigation.
    expect(star("older").closest("a")).toBeNull();
    expect(app.path()).toBe("/c/new");

    const reads = simulator.callsTo("list_chats").length;
    await act(async () => answer.resolve(null));
    // Relu après la réponse, toujours favorite ; date d'activité inchangée.
    await waitFor(() =>
      expect(simulator.callsTo("list_chats").length).toBeGreaterThan(reads),
    );
    expect(history()[0]).toEqual(["Favorites", ["older"]]);
    expect(backend.chats.get("older")!.updatedAt).toBe(before);

    // Retirée des favoris : retour dans sa rubrique.
    simulator.handle("set_chat_favorite", (args) => {
      backend.chats.get(args.id as string)!.favorite = args.favorite as boolean;
      return null;
    });
    await userEvent.click(star("older"));
    await waitFor(() =>
      expect(history()).toEqual([
        ["Today", ["today"]],
        ["Older", ["older"]],
      ]),
    );
    expect(simulator.callsTo("set_chat_favorite").at(-1)).toEqual({
      id: "older",
      favorite: false,
    });
    expect(star("older").getAttribute("aria-pressed")).toBe("false");
  });

  it("a failure puts the chat back at once and is logged", async () => {
    const { simulator } = setup("/c/new");
    await openHistory();
    const error = vi.spyOn(console, "error").mockImplementation(() => {});
    const answer = deferred<null>();
    simulator.handle("set_chat_favorite", () => answer.promise);
    // Relecture de l'historique suspendue : seule l'annulation peut remettre
    // la conversation à sa place.
    const reread = deferred<never>();
    simulator.handle("list_chats", () => reread.promise);

    await userEvent.click(star("today"));
    await waitFor(() => expect(history()[0]).toEqual(["Favorites", ["today"]]));

    await act(async () =>
      answer.reject({ kind: "notFound", message: "chat not found: today" }),
    );
    await waitFor(() =>
      expect(history()).toEqual([
        ["Today", ["today"]],
        ["Older", ["older"]],
      ]),
    );
    expect(star("today").getAttribute("aria-pressed")).toBe("false");
    expect(error).toHaveBeenCalledWith(
      "Failed to update favorite:",
      "today",
      expect.objectContaining({ kind: "notFound" }),
    );
    reread.reject({ kind: "internal", message: "end of test" });
    error.mockRestore();
  });

  it("are saved without a network connection (local engine)", async () => {
    const { simulator } = setup("/c/new");
    await openHistory();
    onlineManager.setOnline(false);
    try {
      await userEvent.click(star("today"));
      await waitFor(() =>
        expect(simulator.callsTo("set_chat_favorite")).toEqual([
          { id: "today", favorite: true },
        ]),
      );
    } finally {
      onlineManager.setOnline(true);
    }
  });

  it("the keyboard focus follows the chat to its new group", async () => {
    setup("/c/new");
    await openHistory();
    const user = userEvent.setup();
    focusVisible(true);
    act(() => star("today").focus());

    await user.keyboard("{Enter}");
    await waitFor(() => expect(history()[0]).toEqual(["Favorites", ["today"]]));
    await waitFor(() => expect(document.activeElement).toBe(star("today")));

    // Depuis le menu ouvert sur le lien : le focus revient au lien.
    const link = within(rowOf("today")).getByRole("link");
    act(() => link.focus());
    fireEvent.contextMenu(link);
    await user.click(
      await screen.findByRole("menuitem", { name: "Remove from favorites" }),
    );
    await waitFor(() => expect(history()[0]).toEqual(["Today", ["today"]]));
    await waitFor(() =>
      expect(document.activeElement).toBe(
        within(rowOf("today")).getByRole("link"),
      ),
    );
  });

  it("a mouse click leaves no focus on the moved row", async () => {
    setup("/c/new");
    await openHistory();
    const user = userEvent.setup();
    // Comme WebKitGTK : le bouton ou le lien cliqué prend le focus, sans
    // anneau.
    focusVisible(false);

    await user.click(star("today"));
    await waitFor(() => expect(history()[0]).toEqual(["Favorites", ["today"]]));
    await settle();
    expect(document.activeElement).not.toBe(star("today"));

    // Menu ouvert à la souris sur le lien cliqué avant.
    const link = within(rowOf("today")).getByRole("link");
    act(() => link.focus());
    fireEvent.contextMenu(link);
    await user.click(
      await screen.findByRole("menuitem", { name: "Remove from favorites" }),
    );
    await waitFor(() => expect(history()[0]).toEqual(["Today", ["today"]]));
    await settle();
    expect(document.activeElement).not.toBe(
      within(rowOf("today")).getByRole("link"),
    );
  });

  it("the second click of a double click is ignored", async () => {
    const { simulator } = setup("/c/new");
    await openHistory();

    // Il tombe sur l'étoile d'une autre ligne, venue sous le pointeur.
    fireEvent.click(star("older"), { detail: 2 });
    await settle();
    expect(simulator.callsTo("set_chat_favorite")).toEqual([]);
    expect(history()[0]).toEqual(["Today", ["today"]]);

    fireEvent.click(star("older"), { detail: 1 });
    await waitFor(() => expect(history()[0]).toEqual(["Favorites", ["older"]]));
  });

  it("two toggles: the history is read again once both are saved", async () => {
    const { simulator, backend } = setup("/c/new");
    await openHistory();
    const answer = suspendFavorites(simulator, backend);
    const reads = () => simulator.callsTo("list_chats").length;

    await userEvent.click(star("today"));
    await userEvent.click(star("older"));
    await waitFor(() =>
      expect(history()).toEqual([["Favorites", ["today", "older"]]]),
    );
    const before = reads();

    await act(async () => answer("today").resolve(null));
    await settle();
    // La relecture effacerait un instant la seconde étoile, pas encore
    // enregistrée.
    expect(reads()).toBe(before);
    expect(history()).toEqual([["Favorites", ["today", "older"]]]);

    await act(async () => answer("older").resolve(null));
    await waitFor(() => expect(reads()).toBeGreaterThan(before));
    expect(history()).toEqual([["Favorites", ["today", "older"]]]);
  });

  it("a failure puts back only its own chat", async () => {
    const { simulator, backend } = setup("/c/new");
    await openHistory();
    const error = vi.spyOn(console, "error").mockImplementation(() => {});
    onTestFinished(() => error.mockRestore());
    const answer = suspendFavorites(simulator, backend);

    await userEvent.click(star("today"));
    await userEvent.click(star("older"));
    await waitFor(() =>
      expect(history()).toEqual([["Favorites", ["today", "older"]]]),
    );

    await act(async () =>
      answer("today").reject({ kind: "internal", message: "disk full" }),
    );
    await waitFor(() =>
      expect(history()).toEqual([
        ["Favorites", ["older"]],
        ["Today", ["today"]],
      ]),
    );
    expect(star("older").getAttribute("aria-pressed")).toBe("true");
  });

  it("a reading of the history already under way does not undo the star", async () => {
    const { simulator, app } = setup("/c/new");
    await openHistory();
    const stale = await listChats();
    const reread = deferred<void>();
    simulator.handle("list_chats", async () => {
      await reread.promise;
      return stale;
    });
    simulator.handle("set_chat_favorite", () => new Promise(() => {}));
    const reads = simulator.callsTo("list_chats").length;

    act(() => {
      void app.queryClient.invalidateQueries({ queryKey: ["chats"] });
    });
    await waitFor(() =>
      expect(simulator.callsTo("list_chats").length).toBe(reads + 1),
    );
    await userEvent.click(star("today"));
    await waitFor(() => expect(history()[0]).toEqual(["Favorites", ["today"]]));

    // Réponse d'avant l'étoile : abandonnée.
    await act(async () => reread.resolve());
    await settle();
    expect(history()[0]).toEqual(["Favorites", ["today"]]);
    expect(star("today").getAttribute("aria-pressed")).toBe("true");
  });

  it("are set from the context menu too", async () => {
    const { simulator } = setup("/c/new");
    await openHistory();

    fireEvent.contextMenu(rowOf("today"));
    expect(
      screen.getAllByRole("menuitem").map((item) => item.textContent),
    ).toEqual(["Add to favorites", "Rename", "Delete"]);
    await userEvent.click(
      screen.getByRole("menuitem", { name: "Add to favorites" }),
    );
    await waitFor(() => expect(history()[0]).toEqual(["Favorites", ["today"]]));
    expect(simulator.callsTo("set_chat_favorite")).toEqual([
      { id: "today", favorite: true },
    ]);

    fireEvent.contextMenu(rowOf("today"));
    await userEvent.click(
      screen.getByRole("menuitem", { name: "Remove from favorites" }),
    );
    await waitFor(() => expect(history()[0]).toEqual(["Today", ["today"]]));
    expect(simulator.callsTo("set_chat_favorite").at(-1)).toEqual({
      id: "today",
      favorite: false,
    });
  });
});

describe("row buttons", () => {
  it("are shown on hover or keyboard focus of the row; a favorite's star always", async () => {
    setup("/c/new", [stored("favorite", 2 * DAY, true)]);
    await openHistory();
    const revealed = [
      "opacity-0",
      "group-hover/chat:opacity-100",
      "group-has-[:focus-visible]/chat:opacity-100",
    ];

    expect(classes(rowOf("today"))).toContain("group/chat");
    for (const button of [star("today"), trash("today"), trash("favorite")]) {
      expect(classes(button)).toEqual(expect.arrayContaining(revealed));
    }
    // Étoile pleine d'une favorite : toujours là.
    expect(classes(star("favorite"))).not.toContain("opacity-0");
    expect(classes(star("favorite"))).toContain("text-accent-fg");
    expect(classes(trash("today"))).toContain("text-red-600");
    // Le titre s'arrête avant les boutons quand ils sont montrés.
    const link = (id: string) => within(rowOf(id)).getByRole("link");
    expect(classes(link("today"))).toEqual(
      expect.arrayContaining([
        "pr-2",
        "group-hover/chat:pr-13",
        "group-has-[:focus-visible]/chat:pr-13",
      ]),
    );
    expect(classes(link("favorite"))).toContain("pr-13");
    expect(classes(link("favorite"))).not.toContain("pr-2");
    expect(
      within(rowOf("today")).getByRole("button", {
        name: "Delete chat",
        description: "Chat today",
      }),
    ).toBe(trash("today"));
    expect(trash("today").title).toBe("Delete chat");
    // Un clic à côté des icônes traverse leur conteneur jusqu'au lien.
    expect(classes(star("today").parentElement!)).toContain(
      "pointer-events-none",
    );
    for (const button of [star("today"), trash("today")]) {
      expect(classes(button)).toContain("pointer-events-auto");
    }
  });

  it("are hidden while the row is renamed", async () => {
    setup("/c/new");
    await openHistory();

    fireEvent.contextMenu(rowOf("today"));
    await userEvent.click(screen.getByRole("menuitem", { name: "Rename" }));
    await screen.findByLabelText("Chat title");

    expect(within(rowOf("today")).queryByTestId("chat-favorite")).toBeNull();
    expect(within(rowOf("today")).queryByTestId("chat-delete")).toBeNull();
    expect(star("older")).toBeTruthy();
    expect(trash("older")).toBeTruthy();
  });

  it("the trash asks the same confirmation as the menu, then deletes", async () => {
    const { simulator, app } = setup("/c/new");
    await openHistory();

    await userEvent.click(trash("today"));
    expect(screen.getByRole("alertdialog").textContent).toContain(CONFIRM);
    expect(app.path()).toBe("/c/new");
    expect(trash("today").closest("a")).toBeNull();
    await userEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(screen.queryByRole("alertdialog")).toBeNull();
    expect(simulator.callsTo("delete_chat")).toEqual([]);
    expect(row("today")).toBeTruthy();

    // Même boîte par le menu contextuel.
    fireEvent.contextMenu(rowOf("today"));
    await userEvent.click(screen.getByRole("menuitem", { name: "Delete" }));
    expect(screen.getByRole("alertdialog").textContent).toContain(CONFIRM);
    await userEvent.click(screen.getByRole("button", { name: "Cancel" }));

    await userEvent.click(trash("today"));
    await userEvent.click(screen.getByRole("button", { name: "OK" }));
    await waitFor(() => expect(row("today")).toBeNull());
    expect(simulator.callsTo("delete_chat")).toEqual([{ id: "today" }]);
    expect(app.path()).toBe("/c/new");
  });

  it("deletes and renames without a network connection (local engine)", async () => {
    const { simulator } = setup("/c/new");
    simulator.handle("rename_chat", () => null);
    await openHistory();
    const user = userEvent.setup();
    onlineManager.setOnline(false);
    try {
      await user.click(trash("older"));
      await user.click(screen.getByRole("button", { name: "OK" }));
      await waitFor(() =>
        expect(simulator.callsTo("delete_chat")).toEqual([{ id: "older" }]),
      );

      fireEvent.contextMenu(rowOf("today"));
      await user.click(screen.getByRole("menuitem", { name: "Rename" }));
      const title = await screen.findByLabelText("Chat title");
      await user.clear(title);
      await user.type(title, "Renamed{Enter}");
      await waitFor(() =>
        expect(simulator.callsTo("rename_chat")).toEqual([
          { id: "today", title: "Renamed" },
        ]),
      );
    } finally {
      onlineManager.setOnline(true);
    }
  });

  it("deleting the open chat with the trash goes to a new chat", async () => {
    const { app } = setup("/c/today");
    await openHistory();

    await userEvent.click(trash("today"));
    await userEvent.click(screen.getByRole("button", { name: "OK" }));

    await waitFor(() => expect(app.path()).toBe("/c/new"));
    await waitFor(() => expect(row("today")).toBeNull());
  });
});
