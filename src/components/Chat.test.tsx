import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import {
  act,
  cleanup,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type { Channel } from "@tauri-apps/api/core";
import { clearMocks, mockIPC, mockWindows } from "@tauri-apps/api/mocks";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { Chat as ChatData } from "@/bindings/Chat";
import type { ChatEvent } from "@/bindings/ChatEvent";
import type { Message } from "@/bindings/Message";
import type { SendRequest } from "@/bindings/SendRequest";
import { chatKey } from "@/hooks/useChats";
import * as streams from "@/lib/streams";
import { Chat } from "./Chat";

const navigate = vi.hoisted(() => vi.fn());
vi.mock("@tanstack/react-router", () => ({ useNavigate: () => navigate }));

afterEach(() => {
  cleanup();
  clearMocks();
  streams.resetStreams();
});

const settings = {
  serverUrl: "",
  effectiveServerUrl: "http://127.0.0.1:11434",
  contextLength: 0,
  theme: "system",
  selectedModel: "qwen3:0.6b",
  windowWidth: null,
  windowHeight: null,
};

function message(id: number, role: "user" | "assistant", content: string) {
  return {
    id,
    role,
    content,
    thinking: "",
    model: role === "assistant" ? "qwen3:0.6b" : null,
    createdAt: id,
    updatedAt: id,
    thinkingStartedAt: null,
    thinkingEndedAt: null,
    attachments: [],
    toolCalls: [],
  } satisfies Message;
}

type Send = (request: SendRequest, onEvent: Channel<ChatEvent>) => unknown;

function renderChat(
  chatId: string,
  send: Send,
  {
    cancel = vi.fn(),
    chat,
    handlers = {},
  }: {
    cancel?: (args: { streamId: string }) => unknown;
    chat?: ChatData;
    /** Autres commandes (model_details, pick_files, unstage...). */
    handlers?: Record<string, (args: Record<string, unknown>) => unknown>;
  } = {},
) {
  mockIPC(
    (command, args) => {
      const { request, onEvent } = (args ?? {}) as {
        request: SendRequest;
        onEvent: Channel<ChatEvent>;
      };
      switch (command) {
        case "get_settings":
        case "update_settings":
          return settings;
        case "list_models":
          return [];
        case "list_chats":
          return [];
        case "server_status":
          return { state: "running" };
        case "send_message":
          return send(request, onEvent);
        case "cancel_message":
          return cancel(args as { streamId: string });
        case "get_chat":
          // Relecture finale qui traîne : les flux restent dans le magasin.
          return new Promise(() => {});
      }
      const handler = handlers[command];
      if (handler) return handler((args ?? {}) as Record<string, unknown>);
      throw new Error(`unexpected command ${command}`);
    },
    { shouldMockEvents: true },
  );
  mockWindows("main");
  const client = new QueryClient({
    defaultOptions: { queries: { networkMode: "always", retry: false } },
  });
  if (chat) client.setQueryData(chatKey(chat.id), chat);
  render(
    <QueryClientProvider client={client}>
      <Chat chatId={chatId} />
    </QueryClientProvider>,
  );
}

async function type(text: string) {
  const textarea = screen.getByLabelText("Message");
  await waitFor(() =>
    expect(screen.getByTitle("Select model").textContent).toBe("qwen3:0.6b"),
  );
  await userEvent.type(textarea, `${text}{Enter}`);
}

/** Textes des messages affichés d'un rôle (sans le bouton « edit »). */
function texts(role: "user" | "assistant") {
  return [...document.querySelectorAll(`[data-role='${role}']`)].map(
    (element) => {
      const copy = element.cloneNode(true) as Element;
      for (const button of copy.querySelectorAll(".edit-button")) {
        button.remove();
      }
      return copy.textContent;
    },
  );
}

describe("Chat", () => {
  it("a new chat refused before its creation shows the error above the composer", async () => {
    renderChat("new", () => {
      throw { kind: "database", message: "database is locked" };
    });

    await type("Hello");

    const alert = await screen.findByRole("alert");
    expect(alert.textContent).toContain("database is locked");
    expect(within(screen.getByTestId("new-chat")).getByRole("alert")).toBe(
      alert,
    );
    const textarea = screen.getByLabelText("Message") as HTMLTextAreaElement;
    expect(textarea.value).toBe("");
    // Au-dessus de la zone de saisie.
    expect(
      alert.compareDocumentPosition(textarea) &
        Node.DOCUMENT_POSITION_FOLLOWING,
    ).toBeTruthy();
  });

  it("a second message is not hidden by the previous reply still being read", async () => {
    const sent: string[] = [];
    const cancel = vi.fn();
    renderChat(
      "c",
      (request, onEvent) => {
        sent.push(request.streamId);
        if (sent.length === 1) {
          onEvent.onmessage({
            type: "userMessage",
            message: message(3, "user", "A"),
          });
          onEvent.onmessage({
            type: "replyStarted",
            messageId: 4,
            createdAt: 4,
          });
          onEvent.onmessage({ type: "content", text: "Reply A" });
          onEvent.onmessage({ type: "done", messageId: 4 });
          return null;
        }
        // Deuxième envoi : la réponse n'a pas encore commencé.
        return new Promise(() => {});
      },
      {
        cancel,
        chat: {
          id: "c",
          title: "Hi",
          createdAt: 1,
          updatedAt: 2,
          messages: [
            message(1, "user", "Hi"),
            message(2, "assistant", "Hello"),
          ],
        },
      },
    );

    await type("A");
    await waitFor(() =>
      expect(texts("assistant")).toEqual(["Hello", "Reply A"]),
    );
    // Réponse A terminée, sa relecture en base pas encore revenue.
    expect(streams.streamsFor("c")).toHaveLength(1);
    await waitFor(() => screen.getByLabelText("Send message"));

    await type("B");

    await waitFor(() => expect(texts("user")).toEqual(["Hi", "A", "B"]));
    expect(texts("assistant")).toEqual(["Hello", "Reply A"]);
    await userEvent.click(screen.getByLabelText("Stop"));
    expect(cancel).toHaveBeenCalledWith({ streamId: sent[1] });
  });
});

describe("editing a message", () => {
  const NOTES = {
    id: 7,
    filename: "notes.txt",
    kind: "text",
    mime: "text/plain",
    size: 3,
  } as const;
  const RED = {
    id: 8,
    filename: "red.png",
    kind: "image",
    mime: "image/png",
    size: 4,
  } as const;
  const CHAT: ChatData = {
    id: "c",
    title: "",
    createdAt: 1,
    updatedAt: 4,
    messages: [
      message(1, "user", "First"),
      message(2, "assistant", "Ok"),
      { ...message(3, "user", "Hi"), attachments: [NOTES, RED] },
      message(4, "assistant", "Hello"),
    ],
  };

  /** Opacité de chaque message affiché (user ou assistant). */
  const opacities = () =>
    [...document.querySelectorAll("[data-message-index]")].map((wrapper) =>
      (wrapper.firstElementChild as HTMLElement).className.includes(
        "opacity-50",
      )
        ? 50
        : 100,
    );
  const editButtons = () => screen.getAllByRole("button", { name: "edit" });
  const keptChips = () =>
    screen.queryAllByTestId("composer-file").map((chip) => chip.textContent);

  it("fills the composer with the message, fading it and the following ones", async () => {
    renderChat("c", () => null, { chat: CHAT });
    await screen.findByText("Hello");
    // Messages de l'utilisateur enregistrés seulement.
    expect(editButtons()).toHaveLength(2);
    await userEvent.click(editButtons()[1]);

    const textarea = screen.getByLabelText("Message") as HTMLTextAreaElement;
    expect(textarea.value).toBe("Hi");
    expect(document.activeElement).toBe(textarea);
    expect(screen.getByText("Press ESC to cancel editing")).toBeTruthy();
    expect(keptChips()).toEqual(["notes.txt", "red.png"]);
    expect(
      screen
        .getAllByRole("img", { name: "red.png" })
        .map((i) => i.getAttribute("src")),
    ).toContain("chatonlm://localhost/attachment/8");
    expect(opacities()).toEqual([100, 100, 50, 50]);
    // Son propre bouton : estompé et désactivé ; celui d'avant, actif.
    expect(editButtons()[1].className).toContain("opacity-30");
    expect((editButtons()[1] as HTMLButtonElement).disabled).toBe(true);
    expect((editButtons()[0] as HTMLButtonElement).disabled).toBe(false);
  });

  it("Escape cancels the edit before stopping a reply", async () => {
    const cancel = vi.fn();
    renderChat("c", () => new Promise(() => {}), { chat: CHAT, cancel });
    await screen.findByText("Hello");
    await type("More");
    await waitFor(() => screen.getByLabelText("Stop"));
    // Message de plusieurs lignes : la zone grandit en édition.
    Object.defineProperty(screen.getByLabelText("Message"), "scrollHeight", {
      configurable: true,
      value: 96,
    });
    await userEvent.click(editButtons()[1]);
    expect(
      screen.getByLabelText<HTMLTextAreaElement>("Message").style.height,
    ).toBe("96px");
    expect(opacities()[2]).toBe(50);
    expect(keptChips()).toEqual(["notes.txt", "red.png"]);

    await userEvent.keyboard("{Escape}");
    expect(cancel).not.toHaveBeenCalled();
    const textarea = screen.getByLabelText("Message") as HTMLTextAreaElement;
    expect(textarea.value).toBe("");
    // Hauteur d'origine, comme après un envoi.
    expect(textarea.style.height).toBe("auto");
    expect(keptChips()).toEqual([]);
    expect(screen.queryByText("Press ESC to cancel editing")).toBeNull();
    expect(opacities().every((o) => o === 100)).toBe(true);
    await waitFor(() => expect(document.activeElement).toBe(textarea));

    await userEvent.keyboard("{Escape}");
    expect(cancel).toHaveBeenCalledTimes(1);
  });

  it("sends the edit: the message and the following ones are replaced", async () => {
    const requests: SendRequest[] = [];
    renderChat(
      "c",
      (request, onEvent) => {
        requests.push(request);
        onEvent.onmessage({
          type: "userMessage",
          message: {
            ...message(3, "user", "Hi again"),
            updatedAt: 10,
            attachments: [NOTES],
          },
        });
        onEvent.onmessage({
          type: "replyStarted",
          messageId: 5,
          createdAt: 11,
        });
        onEvent.onmessage({ type: "content", text: "New answer" });
        onEvent.onmessage({ type: "done", messageId: 5 });
        return null;
      },
      { chat: CHAT },
    );
    await screen.findByText("Hello");
    await userEvent.click(editButtons()[1]);
    await userEvent.click(
      screen.getByRole("button", { name: "Remove red.png" }),
    );
    expect(keptChips()).toEqual(["notes.txt"]);
    const textarea = screen.getByLabelText("Message");
    await userEvent.clear(textarea);
    await type("Hi again");

    await waitFor(() => expect(requests).toHaveLength(1));
    expect(requests[0]).toMatchObject({
      chatId: "c",
      content: "Hi again",
      editMessageId: 3,
      keptAttachmentIds: [7],
    });
    expect(requests[0].attachmentIds).toBeUndefined();
    await waitFor(() =>
      expect(texts("assistant")).toEqual(["Ok", "New answer"]),
    );
    expect(texts("user")).toEqual(["First", "notes.txtHi again"]);
    expect(screen.queryByText("Press ESC to cancel editing")).toBeNull();
    expect(opacities().every((o) => o === 100)).toBe(true);
    expect(keptChips()).toEqual([]);
  });

  it("hides the replaced messages as soon as the edit is sent", async () => {
    renderChat("c", () => new Promise(() => {}), { chat: CHAT });
    await screen.findByText("Hello");
    await userEvent.click(editButtons()[1]);
    await userEvent.clear(screen.getByLabelText("Message"));
    await type("Hi again");
    await waitFor(() =>
      expect(texts("user")).toEqual(["First", "notes.txtHi again"]),
    );
    expect(texts("assistant")).toEqual(["Ok"]);
    // Pièces jointes gardées : leurs images viennent déjà de la base.
    expect(
      screen
        .getAllByRole("img", { name: "red.png" })
        .map((i) => i.getAttribute("src")),
    ).toContain("chatonlm://localhost/attachment/8");
  });

  it("a refused edit shows the original messages again, with the error", async () => {
    renderChat(
      "c",
      () => Promise.reject({ kind: "invalid", message: "unknown message: 3" }),
      { chat: CHAT },
    );
    await screen.findByText("Hello");
    await userEvent.click(editButtons()[1]);
    await userEvent.clear(screen.getByLabelText("Message"));
    await type("Hi again");
    await waitFor(() =>
      expect(screen.getByRole("alert").textContent).toContain(
        "unknown message: 3",
      ),
    );
    expect(texts("user")).toEqual(["First", "notes.txtHi"]);
    expect(texts("assistant")).toEqual(["Ok", "Hello"]);
    expect(
      screen
        .getAllByRole("img", { name: "red.png" })
        .map((i) => i.getAttribute("src")),
    ).toEqual(["chatonlm://localhost/attachment/8"]);
  });

  it("cancelling an edit clears the error shown", async () => {
    renderChat(
      "c",
      () => Promise.reject({ kind: "invalid", message: "unknown message: 3" }),
      { chat: CHAT },
    );
    await screen.findByText("Hello");
    await userEvent.click(editButtons()[1]);
    await userEvent.clear(screen.getByLabelText("Message"));
    await type("Hi again");
    await screen.findByRole("alert");
    await userEvent.click(editButtons()[1]);
    expect(screen.getByRole("alert")).toBeTruthy();
    await userEvent.keyboard("{Escape}");
    await waitFor(() => expect(screen.queryByRole("alert")).toBeNull());
  });

  it("leaves out kept images for a model without vision", async () => {
    const requests: SendRequest[] = [];
    renderChat(
      "c",
      (request) => {
        requests.push(request);
        return new Promise(() => {});
      },
      {
        chat: CHAT,
        handlers: {
          model_details: () => ({
            model: "qwen3:0.6b",
            capabilities: ["completion"],
            thinking: null,
          }),
          get_think_pref: () => null,
        },
      },
    );
    await screen.findByText("Hello");
    await userEvent.click(editButtons()[1]);
    await waitFor(() =>
      expect(
        screen.getByText("This model does not support images"),
      ).toBeTruthy(),
    );
    await userEvent.clear(screen.getByLabelText("Message"));
    await type("Hi again");
    await waitFor(() => expect(requests).toHaveLength(1));
    expect(requests[0].keptAttachmentIds).toEqual([7]);
  });

  it("starting an edit releases the files added before", async () => {
    const unstaged: string[][] = [];
    renderChat("c", () => null, {
      chat: CHAT,
      handlers: {
        pick_files: () => ["/a/extra.md"],
        stage_paths: () => [
          {
            status: "staged",
            file: {
              id: "s1",
              filename: "extra.md",
              kind: "text",
              mime: "text/plain",
              size: 1,
            },
          },
        ],
        unstage: (args) => {
          unstaged.push(args.ids as string[]);
          return null;
        },
      },
    });
    await screen.findByText("Hello");
    await userEvent.click(
      screen.getByRole("button", { name: "Upload multiple files" }),
    );
    await waitFor(() => expect(keptChips()).toEqual(["extra.md"]));
    await userEvent.click(editButtons()[1]);
    expect(keptChips()).toEqual(["notes.txt", "red.png"]);
    expect(unstaged).toEqual([["s1"]]);
  });

  it("a file still being read when the edit starts never joins it", async () => {
    const unstaged: string[][] = [];
    let answer: (value: unknown) => void = () => {};
    renderChat("c", () => null, {
      chat: CHAT,
      handlers: {
        pick_files: () => ["/a/big.pdf"],
        stage_paths: () =>
          new Promise((resolve) => {
            answer = resolve;
          }),
        unstage: (args) => {
          unstaged.push(args.ids as string[]);
          return null;
        },
      },
    });
    await screen.findByText("Hello");
    await userEvent.click(
      screen.getByRole("button", { name: "Upload multiple files" }),
    );
    // Lecture en cours : sa pastille d'attente.
    await screen.findByText("big.pdf");
    await userEvent.click(editButtons()[1]);
    expect(screen.queryByText("big.pdf")).toBeNull();
    await act(async () =>
      answer([
        {
          status: "staged",
          file: {
            id: "s2",
            filename: "big.pdf",
            kind: "pdf",
            mime: "application/pdf",
            size: 1,
          },
        },
      ]),
    );
    await waitFor(() => expect(unstaged).toEqual([["s2"]]));
    expect(keptChips()).toEqual(["notes.txt", "red.png"]);
  });

  it("a read abandoned by Escape shows no error when it fails", async () => {
    let fail: (error: unknown) => void = () => {};
    const consoleError = vi
      .spyOn(console, "error")
      .mockImplementation(() => {});
    renderChat("c", () => null, {
      chat: CHAT,
      handlers: {
        pick_files: () => ["/a/big.pdf"],
        stage_paths: () =>
          new Promise((_, reject) => {
            fail = reject;
          }),
      },
    });
    await screen.findByText("Hello");
    await userEvent.click(editButtons()[1]);
    await userEvent.click(
      screen.getByRole("button", { name: "Upload multiple files" }),
    );
    await screen.findByText("big.pdf");
    await userEvent.keyboard("{Escape}");
    await act(async () => fail({ kind: "system", message: "gone" }));
    await act(() => new Promise((resolve) => setTimeout(resolve, 20)));
    expect(screen.queryByText("big.pdf")).toBeNull();
    expect(screen.queryByText(/Error reading file/)).toBeNull();
    consoleError.mockRestore();
  });
});
