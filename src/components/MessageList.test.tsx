import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
  within,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { Message } from "@/bindings/Message";
import type { ToolCall } from "@/bindings/ToolCall";
import { setLanguage } from "@/i18n";
import type { DownloadProgress } from "@/lib/streams";
import { MessageList } from "./MessageList";

const clipboard = vi.hoisted(() => ({
  copyText: vi.fn<(text: string) => Promise<void>>(() => Promise.resolve()),
  copyRich: vi.fn<(html: string, text: string) => Promise<void>>(() =>
    Promise.resolve(),
  ),
}));
vi.mock("@/lib/clipboard", () => clipboard);

// Rendus des messages : chaque élément JSX créé avec un message en propriété
// et une fonction pour type (UserMessage, AssistantMessage) est noté. Un
// message mémoïsé qui ne change pas ne recrée pas les siens.
const created = vi.hoisted(() => [] as unknown[]);
vi.mock("react/jsx-dev-runtime", async (importOriginal) => {
  const actual = await importOriginal<typeof import("react/jsx-dev-runtime")>();
  return {
    ...actual,
    jsxDEV: (
      type: unknown,
      props: { message?: unknown },
      ...rest: unknown[]
    ) => {
      if (typeof type === "function" && props?.message) {
        created.push(props.message);
      }
      return (actual.jsxDEV as (...args: unknown[]) => unknown)(
        type,
        props,
        ...rest,
      );
    },
  };
});

afterEach(() => {
  cleanup();
  created.length = 0;
  clipboard.copyRich.mockClear();
  vi.useRealTimers();
});

function message(id: number, role: "user" | "assistant", content: string) {
  return {
    id,
    role,
    content,
    thinking: "",
    model: role === "assistant" ? "m" : null,
    createdAt: id,
    updatedAt: id,
    thinkingStartedAt: null,
    thinkingEndedAt: null,
    attachments: [],
    toolCalls: [],
  } satisfies Message;
}

function list(
  messages: Message[],
  isStreaming = true,
  download: DownloadProgress | null = null,
  loading = false,
) {
  return (
    <MessageList
      messages={messages}
      spacerHeight={0}
      isStreaming={isStreaming}
      download={download}
      loading={loading}
      error={null}
    />
  );
}

const GB = 1024 ** 3;

function progress(completed: number, total: number, done = false) {
  return { completed, total, done };
}

/** Bloc de téléchargement affiché, ou null. */
function downloadBlock() {
  return document.querySelector<HTMLElement>("[data-testid=downloading]");
}

/** Boutons copier des réponses (ceux des blocs de code ont un libellé). */
function messageCopyButtons() {
  return screen
    .queryAllByRole("button", { name: "Copy" })
    .filter((button) => button.title === "Copy");
}

describe("MessageList", () => {
  it("during a reply, only the message being written renders again", () => {
    const saved = [message(1, "user", "Hi"), message(2, "assistant", "Hello")];
    const sent = message(3, "user", "Again");
    const { rerender } = render(
      list([...saved, sent, message(4, "assistant", "He")]),
    );
    expect(created).toHaveLength(4);
    created.length = 0;

    const reply = message(4, "assistant", "Hello again");
    rerender(list([...saved, sent, reply]));

    expect(screen.getByText("Hello again")).toBeTruthy();
    expect(created).toEqual([reply]);
  });

  it("renders replies as Markdown in a prose container, without a shared id", () => {
    const { container } = render(
      list(
        [
          message(1, "user", "Hi"),
          message(2, "assistant", "# Title\n\nSome **bold** text."),
          message(3, "user", "More"),
          message(4, "assistant", "Second *reply*"),
        ],
        false,
      ),
    );
    const replies = container.querySelectorAll("[data-role=assistant]");
    expect(replies).toHaveLength(2);
    for (const reply of replies) {
      expect(reply.className).toBe(
        "assistant-message-content prose dark:prose-invert max-w-full break-words",
      );
      expect(reply.hasAttribute("id")).toBe(false);
    }
    expect(replies[0].querySelector("h1")?.textContent).toBe("Title");
    expect(
      replies[0].querySelector('[data-streamdown="strong"]')?.textContent,
    ).toBe("bold");
    expect(replies[1].querySelector("em")?.textContent).toBe("reply");
  });

  it("shows the copy button of the reply being written only once it is done", () => {
    const history = [
      message(1, "user", "Hi"),
      message(2, "assistant", "Hello"),
      message(3, "user", "Again"),
    ];
    const { rerender } = render(
      list([...history, message(4, "assistant", "Hel")]),
    );
    // Seule la réponse déjà finie a le sien.
    expect(messageCopyButtons()).toHaveLength(1);

    rerender(list([...history, message(4, "assistant", "Hello again")], false));
    expect(messageCopyButtons()).toHaveLength(2);
  });

  it("shows neither content nor copy button for a blank reply", () => {
    const { container } = render(
      list([message(1, "user", "Hi"), message(2, "assistant", "  \n ")], false),
    );
    expect(container.querySelector("[data-role=assistant]")).toBeNull();
    expect(messageCopyButtons()).toHaveLength(0);
  });

  it("copies a reply as cleaned HTML and as Markdown", async () => {
    const markdown = "Use **this**:\n\n```bash\nollama run qwen3\n```";
    render(
      list(
        [message(1, "user", "Hi"), message(2, "assistant", markdown)],
        false,
      ),
    );

    const [button] = messageCopyButtons();
    expect(button.className).toContain(
      "copy-button z-10 text-neutral-500 dark:text-neutral-400",
    );
    await act(async () => {
      fireEvent.click(button);
    });

    expect(clipboard.copyRich).toHaveBeenCalledOnce();
    const [html, text] = clipboard.copyRich.mock.calls[0];
    expect(text).toBe(markdown);
    expect(html).toMatch(/^<meta charset="utf-8">/);
    expect(html).toContain("<strong>this</strong>");
    expect(html.match(/<pre/g)).toHaveLength(1);
    expect(html).not.toMatch(/class=|Copy|bash</);
  });

  it("shows the thinking above the reply, with its duration from the saved timestamps", () => {
    const reply = {
      ...message(2, "assistant", "The answer"),
      thinking: "Let me think.",
      thinkingStartedAt: 1_000,
      thinkingEndedAt: 13_340,
    };
    const { container } = render(
      list([message(1, "user", "Hi"), reply], false),
    );
    const header = screen.getByRole("button", {
      name: "Thought for 12.3 seconds",
      expanded: false,
    });
    const content = container.querySelector("[data-role=assistant]")!;
    expect(content.textContent).toBe("The answer");
    expect(
      header.compareDocumentPosition(content) &
        Node.DOCUMENT_POSITION_FOLLOWING,
    ).toBeTruthy();
    // Le raisonnement reste hors du contenu copié.
    expect(content.textContent).not.toContain("Let me think.");
    const body = document.getElementById(
      header.getAttribute("aria-controls")!,
    )!;
    expect(body.textContent).toBe("Let me think.");
    expect(body.style.maxHeight).toBe("0px");
  });

  it("a reply being written shows its thinking live, then folds it when the answer starts", () => {
    const sent = message(1, "user", "Hi");
    const thinking = {
      ...message(2, "assistant", ""),
      thinking: "Some **bold",
      thinkingStartedAt: 1_000,
    };
    const { container, rerender } = render(list([sent, thinking]));
    const header = screen.getByRole("button", { name: "Thinking..." });
    const body = document.getElementById(
      header.getAttribute("aria-controls")!,
    )!;
    expect(body.style.maxHeight).toBe("12rem");
    expect(body.style.opacity).toBe("1");
    // Markdown du raisonnement en cours réparé (isStreaming), pas de contenu.
    expect(body.querySelector('[data-streamdown="strong"]')?.textContent).toBe(
      "bold",
    );
    expect(container.querySelector("[data-role=assistant]")).toBeNull();
    // Déplié par l'utilisateur pendant le raisonnement...
    fireEvent.click(header);

    rerender(
      list([sent, { ...thinking, content: "Hello", thinkingEndedAt: 3_500 }]),
    );
    // ...même bloc, qui reste ouvert ; titre et Markdown de fin.
    expect(header.isConnected).toBe(true);
    expect(header.textContent).toBe("Thought for 2.5 seconds");
    expect(header.getAttribute("aria-expanded")).toBe("true");
    expect(body.querySelector('[data-streamdown="strong"]')).toBeNull();
    expect(body.textContent).toBe("Some **bold");
    expect(container.querySelector("[data-role=assistant]")?.textContent).toBe(
      "Hello",
    );
  });

  it("without a click, the thinking folds itself once the answer starts", () => {
    const sent = message(1, "user", "Hi");
    const thinking = {
      ...message(2, "assistant", ""),
      thinking: "Hmm",
      thinkingStartedAt: 1_000,
    };
    const { rerender } = render(list([sent, thinking]));
    rerender(
      list([sent, { ...thinking, content: "Hello", thinkingEndedAt: 1_500 }]),
    );
    const header = screen.getByRole("button", {
      name: "Thought for a moment",
      expanded: false,
    });
    const body = document.getElementById(
      header.getAttribute("aria-controls")!,
    )!;
    expect(body.style.maxHeight).toBe("0px");
    expect(body.style.opacity).toBe("0");
  });

  it("a reply stopped while thinking is shown finished, not stuck on Thinking...", () => {
    const stopped = {
      ...message(2, "assistant", ""),
      thinking: "Hmm, let me",
      thinkingStartedAt: 1_000,
    };
    render(list([message(1, "user", "Hi"), stopped], false));
    expect(screen.queryByText("Thinking...")).toBeNull();
    const header = screen.getByRole("button", {
      name: "Thought for a moment",
      expanded: false,
    });
    const body = document.getElementById(
      header.getAttribute("aria-controls")!,
    )!;
    expect(body.style.maxHeight).toBe("0px");
    // Ni contenu, ni bouton copier.
    expect(messageCopyButtons()).toHaveLength(0);
  });

  it("only the last reply of a streaming list can be thinking", () => {
    const open = {
      ...message(2, "assistant", ""),
      thinking: "Old",
      thinkingStartedAt: 1_000,
    };
    render(list([message(1, "user", "Hi"), open, message(3, "user", "Again")]));
    expect(screen.queryByText("Thinking...")).toBeNull();
    screen.getByRole("button", { name: "Thought for a moment" });
  });

  it("shows no thinking block for blank or missing thinking", () => {
    render(
      list(
        [
          message(1, "user", "Hi"),
          { ...message(2, "assistant", "Hello"), thinking: " \n\n " },
          message(3, "user", "Again"),
          message(4, "assistant", "Hi again"),
        ],
        false,
      ),
    );
    expect(screen.queryByRole("button", { name: /^Thought|^Thinking/ })).toBe(
      null,
    );
  });

  it("shows images above the user's bubble and other files inside it", () => {
    const sent = {
      ...message(1, "user", "Describe these files."),
      attachments: [
        {
          id: 7,
          filename: "notes.txt",
          kind: "text",
          mime: "text/plain",
          size: 1,
        },
        {
          id: 8,
          filename: "red.png",
          kind: "image",
          mime: "image/png",
          size: 1,
        },
        {
          id: 9,
          filename: "a.pdf",
          kind: "text",
          mime: "application/pdf",
          size: 1,
        },
      ],
    } satisfies Message;
    render(list([sent], false));
    const bubble = document.querySelector("[data-role=user]")!;
    const image = screen.getByRole("img", { name: "red.png" });
    expect(image.getAttribute("src")).toBe("chatonlm://localhost/attachment/8");
    expect(image.className).toContain("h-16 w-16");
    // Au-dessus de la bulle, pas dedans.
    expect(bubble.contains(image)).toBe(false);
    expect(
      image.compareDocumentPosition(bubble) & Node.DOCUMENT_POSITION_FOLLOWING,
    ).toBeTruthy();
    // Les autres fichiers : pastilles dans la bulle, avant le texte.
    expect(bubble.textContent).toBe("notes.txta.pdfDescribe these files.");
    expect(screen.queryByRole("img", { name: "notes.txt" })).toBeNull();
  });

  it("shows empty squares for the images of a message not saved yet", () => {
    const pending = {
      ...message(-1, "user", "Look"),
      attachments: [
        {
          id: -1,
          filename: "red.png",
          kind: "image",
          mime: "image/png",
          size: 1,
        },
      ],
    } satisfies Message;
    render(list([pending], true));
    expect(screen.queryByRole("img")).toBeNull();
    const square = screen.getByTestId("thumbnail-placeholder");
    expect(square.className).toContain("h-16 w-16");
  });

  it("offers edit on saved user messages only, not on the one being sent", () => {
    const onEdit = vi.fn();
    render(
      <MessageList
        messages={[
          message(1, "user", "Saved"),
          message(2, "assistant", "Reply"),
          message(-1, "user", "Sending"),
        ]}
        spacerHeight={0}
        isStreaming
        error={null}
        onEditMessage={onEdit}
      />,
    );
    const buttons = screen.getAllByRole("button", { name: "edit" });
    expect(buttons).toHaveLength(1);
    fireEvent.click(buttons[0]);
    expect(onEdit).toHaveBeenCalledWith(
      expect.objectContaining({ id: 1, content: "Saved" }),
    );
  });

  it("keeps the user's text as typed: indentation, and Markdown as plain text", () => {
    const typed = "  indented\n# not a title *nor bold*";
    render(list([message(1, "user", typed)], false));
    const bubble = screen.getByText(
      (_, element) =>
        element?.textContent === typed && element.children.length === 0,
    );
    expect(bubble.className).toContain("whitespace-pre-wrap");
    expect(bubble.closest("[data-role=user]")).not.toBeNull();
  });

  it("shows the user's ``` code blocks and `inline code` like the replies'", () => {
    render(
      list(
        [
          message(
            1,
            "user",
            "Why does `x` fail?\n```python\nprint(x)\n```\nThanks",
          ),
        ],
        false,
      ),
    );
    const bubble = document.querySelector("[data-role=user]")!;
    const block = bubble.querySelector("[data-user-code]")!;
    expect(
      block.querySelector("[data-code-block-header]")?.textContent,
    ).toContain("python");
    expect(block.querySelector("pre code")?.textContent).toBe("print(x)\n");
    // Avec son bouton Copier.
    expect(within(block as HTMLElement).getByRole("button")).toBeTruthy();
    expect(bubble.querySelector("code.font-mono")?.textContent).toBe("x");
    expect(bubble.textContent).not.toContain("```");
    expect(bubble.textContent).toContain("Thanks");
  });
  describe("while the model is downloaded", () => {
    it("shows the download block with its progress", () => {
      render(
        list([message(1, "user", "Hi")], true, progress(1.2 * GB, 4.7 * GB)),
      );
      const block = downloadBlock();
      expect(block?.textContent).toBe("Downloading model1.2 GB / 4.7 GB (25%)");
      // Sous le message envoyé.
      expect(
        screen.getByText("Hi").compareDocumentPosition(block!) &
          Node.DOCUMENT_POSITION_FOLLOWING,
      ).toBeTruthy();
    });

    it.each([0, 1000, 1024])(
      "shows nothing for a total of %i bytes, not even a stray 0",
      (total) => {
        const { container } = render(
          list([message(1, "user", "Hi")], true, progress(0, total)),
        );
        expect(downloadBlock()).toBeNull();
        expect(screen.queryByText("Downloading model")).toBeNull();
        expect(
          container.querySelector("[data-role=message-list]")?.textContent,
        ).toBe("Hi");
      },
    );

    it("shows the block from 1025 bytes on", () => {
      render(list([message(1, "user", "Hi")], true, progress(0, 1025)));
      expect(downloadBlock()?.textContent).toContain("0 KB / 1 KB (0%)");
    });

    it("slides the block up and fades it out once done", () => {
      const sent = [message(1, "user", "Hi")];
      const { rerender } = render(list(sent, true, progress(GB, 4 * GB)));
      const block = downloadBlock()!;
      expect(block.className.split(/\s+/).filter(Boolean)).toEqual([
        "transition-all",
        "ease-out",
        "duration-300",
        "opacity-100",
        "translate-y-0",
      ]);

      rerender(list(sent, true, progress(4 * GB, 4 * GB, true)));
      expect(downloadBlock()).toBe(block);
      expect(block.className.split(/\s+/).filter(Boolean)).toEqual([
        "transition-all",
        "ease-out",
        "duration-300",
        "opacity-0",
        "-translate-y-8",
      ]);
      expect(block.textContent).toContain("4.0 GB / 4.0 GB (100%)");
    });

    it("does not animate without messages", () => {
      render(list([], true, progress(GB, 4 * GB)));
      expect(downloadBlock()?.className).toContain("duration-0");
      expect(downloadBlock()?.className).not.toContain("duration-300");
    });

    it("hides the three dots during the download, then shows them again 750 ms after it", () => {
      vi.useFakeTimers();
      const sent = [message(1, "user", "Hi")];
      const dots = () => screen.queryByTestId("waiting");
      const { rerender } = render(list(sent));
      act(() => vi.advanceTimersByTime(750));
      expect(dots()).not.toBeNull();

      // Téléchargement commencé (taille quelconque) : plus de points.
      rerender(list(sent, true, progress(0, 1000)));
      expect(dots()).toBeNull();
      rerender(list(sent, true, progress(GB, 4 * GB)));
      act(() => vi.advanceTimersByTime(5000));
      expect(dots()).toBeNull();

      // Fini : les points reviennent après 750 ms, jusqu'à la réponse.
      rerender(list(sent, true, progress(4 * GB, 4 * GB, true)));
      expect(dots()).toBeNull();
      act(() => vi.advanceTimersByTime(749));
      expect(dots()).toBeNull();
      act(() => vi.advanceTimersByTime(1));
      expect(dots()).not.toBeNull();
      rerender(
        list(
          [...sent, message(2, "assistant", "Hello")],
          true,
          progress(4 * GB, 4 * GB, true),
        ),
      );
      expect(dots()).toBeNull();
    });

    it("never shows the dots while a download starts right away", () => {
      vi.useFakeTimers();
      const sent = [message(1, "user", "Hi")];
      render(list(sent, true, progress(0, 4 * GB)));
      act(() => vi.advanceTimersByTime(5000));
      expect(screen.queryByTestId("waiting")).toBeNull();
    });
  });

  describe("while the model is loaded into memory", () => {
    const sent = [message(1, "user", "Hi")];
    const dots = () => screen.queryByTestId("waiting");
    const line = () => screen.queryByTestId("loading-model");
    const region = () => screen.queryByTestId("loading-status");

    it("shows Loading model... in place of the dots after 750 ms, then the dots once loaded", () => {
      vi.useFakeTimers();
      const { rerender } = render(list(sent, true, null, true));
      // Région d'annonce déjà là, vide : seul son contenu changera.
      const status = screen.getByRole("status");
      expect(status).toBe(region());
      expect(status.textContent).toBe("");
      act(() => vi.advanceTimersByTime(749));
      expect(line()).toBeNull();
      expect(dots()).toBeNull();
      act(() => vi.advanceTimersByTime(1));
      const shown = line()!;
      expect(status.contains(shown)).toBe(true);
      expect(screen.getByRole("status")).toBe(status);
      expect(status.textContent).toBe("Loading model...");
      expect(shown.textContent).toBe("Loading model...");
      expect(dots()).toBeNull();
      // Couleurs de « Thinking... » en cours, icône décorative.
      expect(shown.className).toContain("text-neutral-800");
      expect(shown.className).toContain("dark:text-neutral-200");
      expect(shown.querySelector("svg")?.getAttribute("aria-hidden")).toBe(
        "true",
      );
      act(() => vi.advanceTimersByTime(10_000));
      expect(line()).not.toBeNull();

      // Chargé : les points aussitôt, sans nouvelle attente ; la région
      // reste, vide.
      rerender(list(sent, true, null, false));
      expect(line()).toBeNull();
      expect(dots()).not.toBeNull();
      expect(region()).toBe(status);
      expect(status.textContent).toBe("");
      // La réponse commence : plus rien.
      rerender(list([...sent, message(2, "assistant", "Hello")]));
      expect(line()).toBeNull();
      expect(dots()).toBeNull();
      expect(region()).toBe(status);
      // Réponse finie : plus de région.
      rerender(list([...sent, message(2, "assistant", "Hello")], false));
      expect(region()).toBeNull();
    });

    it("never flashes for a load shorter than 750 ms", () => {
      vi.useFakeTimers();
      let seen = false;
      const observer = new MutationObserver(() => {
        if (line()) seen = true;
      });
      observer.observe(document.body, { childList: true, subtree: true });
      const { rerender } = render(list(sent, true, null, true));
      act(() => vi.advanceTimersByTime(500));
      rerender(list(sent, true, null, false));
      act(() => vi.advanceTimersByTime(249));
      expect(dots()).toBeNull();
      // Les points, 750 ms après le départ de l'attente comme sans chargement.
      act(() => vi.advanceTimersByTime(1));
      expect(dots()).not.toBeNull();
      observer.disconnect();
      expect(seen).toBe(false);
      expect(line()).toBeNull();
    });

    it("waits 750 ms after a download before showing it", () => {
      vi.useFakeTimers();
      const done = progress(4 * GB, 4 * GB, true);
      const { rerender } = render(list(sent, true, progress(GB, 4 * GB)));
      act(() => vi.advanceTimersByTime(5000));
      rerender(list(sent, true, done, true));
      act(() => vi.advanceTimersByTime(749));
      expect(line()).toBeNull();
      act(() => vi.advanceTimersByTime(1));
      expect(line()).not.toBeNull();
      expect(dots()).toBeNull();
    });

    it("shows nothing once the reply is no longer streaming", () => {
      vi.useFakeTimers();
      render(list(sent, false, null, true));
      act(() => vi.advanceTimersByTime(5000));
      expect(line()).toBeNull();
      expect(dots()).toBeNull();
      expect(region()).toBeNull();
    });
  });

  describe("web search", () => {
    function call(
      name: string,
      args: unknown,
      done: Partial<Pick<ToolCall, "result" | "error">> = {},
    ): ToolCall {
      return {
        name,
        arguments: typeof args === "string" ? args : JSON.stringify(args),
        result: null,
        error: null,
        ...done,
      };
    }

    /** Réponse sans texte qui appelle des outils. */
    function toolTurn(id: number, toolCalls: ToolCall[]): Message {
      return { ...message(id, "assistant", ""), toolCalls };
    }

    const RESULTS = { results: [{ title: "T", url: "https://e.com" }] };
    const sent = message(1, "user", "Search");

    const callTexts = () =>
      [...document.querySelectorAll("[data-role=tool-calls] > div")].map(
        (line) => line.textContent,
      );
    const resultBlocks = () => [
      ...document.querySelectorAll<HTMLElement>("[data-role=tool]"),
    ];

    it("shows each call with the official texts", () => {
      render(
        list([
          sent,
          toolTurn(2, [
            call("web_search", { query: "ollama release" }),
            call("web_fetch", { url: "https://ollama.com/blog" }),
            call("get_weather", { city: "Paris", path: "/tmp/x" }),
            call("find", { path: "/a", url: "https://u.io" }),
            call("other", { count: 3 }),
            call("broken", "{not json"),
            call("web_search", { query: 42 }),
          ]),
        ]),
      );
      expect(callTexts()).toEqual([
        "Searching for ollama release…",
        "Fetching for https://ollama.com/blog…",
        "Calling get_weather: /tmp/x…",
        // Clés préférées dans l'ordre officiel.
        "Calling find: https://u.io…",
        "Calling other…",
        "Calling broken…",
        "Searching for …",
      ]);
      // Terme en gras, nom de l'outil en police fixe.
      const lines = document.querySelectorAll("[data-role=tool-calls] > div");
      expect(lines[0].querySelector(".font-medium")?.textContent).toBe(
        "ollama release",
      );
      expect(lines[2].querySelector(".font-mono")?.textContent).toBe(
        "get_weather",
      );
    });

    it("a reply without text but with calls has no text area nor copy button", () => {
      render(
        list(
          [
            sent,
            toolTurn(2, [call("web_search", { query: "q" }, { result: "{}" })]),
            message(3, "assistant", "Found it"),
          ],
          false,
        ),
      );
      expect(document.querySelectorAll("[data-role=assistant]")).toHaveLength(
        1,
      );
      // Seule la réponse finale a son bouton.
      expect(messageCopyButtons()).toHaveLength(1);
    });

    it("a reply with text and calls has no copy button, like the official", () => {
      render(
        list(
          [
            sent,
            {
              ...message(2, "assistant", "Let me look that up."),
              toolCalls: [call("web_search", { query: "q" }, { result: "{}" })],
            },
          ],
          false,
        ),
      );
      expect(screen.getByText("Let me look that up.")).toBeTruthy();
      expect(messageCopyButtons()).toHaveLength(0);
    });

    it("shows a folded block per finished call, titled by its own call", () => {
      render(
        list([
          sent,
          toolTurn(2, [
            call(
              "web_search",
              { query: "first" },
              { result: JSON.stringify(RESULTS) },
            ),
            call("web_search", { query: "second" }, { result: "{}" }),
            call("web_search", { query: "third" }),
          ]),
        ]),
      );
      // Le troisième appel n'a pas encore de résultat : pas de bloc.
      expect(resultBlocks().map((block) => block.textContent)).toEqual([
        expect.stringContaining("Search results for first"),
        expect.stringContaining("Search results for second"),
      ]);
      const [first] = resultBlocks();
      const header = first.querySelector("button")!;
      const body = document.getElementById(
        header.getAttribute("aria-controls")!,
      )!;
      expect(header.getAttribute("aria-expanded")).toBe("false");
      expect(body.style.maxHeight).toBe("0px");
      expect(body.style.visibility).toBe("hidden");
      expect(body.hasAttribute("data-folded")).toBe(true);

      fireEvent.click(header);
      expect(header.getAttribute("aria-expanded")).toBe("true");
      expect(body.style.maxHeight).toBe("20rem");
      expect(body.style.visibility).toBe("");
      expect(body.hasAttribute("data-folded")).toBe(false);
      // JSON indenté.
      expect(body.querySelector("pre")?.textContent).toBe(
        JSON.stringify(RESULTS, null, 2),
      );
      fireEvent.click(header);
      expect(body.style.maxHeight).toBe("0px");
    });

    it("titles the other results like the official", () => {
      render(
        list(
          [
            sent,
            toolTurn(2, [
              call("web_search", {}, { result: '{"query":"from result"}' }),
              call("web_search", { query: " " }, { result: "{}" }),
              call(
                "web_fetch",
                { url: "https://ollama.com/blog" },
                { result: '{"title":"Blog"}' },
              ),
              call("web_fetch", {}, { result: "{}" }),
              call("get_weather", { city: "Paris" }, { result: "sunny" }),
            ]),
          ],
          false,
        ),
      );
      const titles = resultBlocks().map(
        (block) => block.querySelector("button")?.textContent,
      );
      expect(titles).toEqual([
        "Search results for from result",
        "Web search results",
        "Fetch results for https://ollama.com/blog",
        "Web fetch results",
        "Raw tool result",
      ]);
      // Un résultat qui n'est pas du JSON est montré tel quel.
      expect(resultBlocks()[4].querySelector("pre")?.textContent).toBe("sunny");
    });

    it("shows a failed call as its error", () => {
      render(
        list([
          sent,
          toolTurn(2, [
            call("web_search", { query: "q" }, { error: "unauthorized" }),
          ]),
        ]),
      );
      const [block] = resultBlocks();
      expect(block.querySelector("pre")?.textContent).toBe(
        "Error: unauthorized",
      );
      expect(block.querySelector("button")).toBeNull();
    });

    it("shows the dots between the last result and the next reply", () => {
      vi.useFakeTimers();
      const dots = () => screen.queryByTestId("waiting");
      const pending = toolTurn(2, [
        call("web_search", { query: "a" }, { result: "{}" }),
        call("web_search", { query: "b" }),
      ]);
      const { rerender } = render(list([sent, pending]));
      // Un appel encore en cours : sa ligne « Searching for… » suffit.
      act(() => vi.advanceTimersByTime(2000));
      expect(dots()).toBeNull();

      const done = toolTurn(2, [
        call("web_search", { query: "a" }, { result: "{}" }),
        call("web_search", { query: "b" }, { error: "boom" }),
      ]);
      rerender(list([sent, done]));
      act(() => vi.advanceTimersByTime(749));
      expect(dots()).toBeNull();
      act(() => vi.advanceTimersByTime(1));
      expect(dots()).not.toBeNull();

      // Réponse suivante : plus de points ; fin de l'envoi non plus.
      rerender(list([sent, done, message(3, "assistant", "Hi")]));
      expect(dots()).toBeNull();
      rerender(list([sent, done], false));
      act(() => vi.advanceTimersByTime(2000));
      expect(dots()).toBeNull();
    });
  });

  describe("groups of steps", () => {
    function call(
      name: string,
      args: unknown,
      done: Partial<Pick<ToolCall, "result" | "error">> = {},
    ): ToolCall {
      return {
        name,
        arguments: JSON.stringify(args),
        result: null,
        error: null,
        ...done,
      };
    }

    /** Réponse de l'assistant : raisonnement (fini si `ended`) et appels. */
    function reply(
      id: number,
      {
        content = "",
        thinking = "",
        ended = true,
        toolCalls = [],
      }: {
        content?: string;
        thinking?: string;
        ended?: boolean;
        toolCalls?: ToolCall[];
      },
    ): Message {
      return {
        ...message(id, "assistant", content),
        thinking,
        thinkingStartedAt: thinking ? 1_000 : null,
        thinkingEndedAt: thinking && ended ? 4_000 : null,
        toolCalls,
      };
    }

    const sent = message(1, "user", "Search");
    // Tour en cours : réflexion + recherche, puis récupération avec un texte
    // écrit entre deux appels, puis réflexion en cours (4 étapes).
    const searched = reply(2, {
      thinking: "Plan",
      toolCalls: [call("web_search", { query: "kittens" }, { result: "{}" })],
    });
    const fetched = reply(3, {
      content: "Let me read it.",
      toolCalls: [
        call("web_fetch", { url: "https://e.com" }, { error: "timeout" }),
      ],
    });
    const thinkingNow = reply(4, { thinking: "Nearly", ended: false });
    const answered = reply(4, { thinking: "Nearly", content: "The answer" });
    const working = [sent, searched, fetched, thinkingNow];
    const done = [sent, searched, fetched, answered];

    const toggle = () =>
      document.querySelector<HTMLButtonElement>("[data-role=steps-toggle]");
    /** Visible : aucun ancêtre masqué. */
    const shown = (element: Element) => element.closest("[hidden]") === null;
    const callLines = () => [
      ...document.querySelectorAll("[data-role=tool-calls] > div"),
    ];
    const resultBlocks = () => [
      ...document.querySelectorAll<HTMLElement>("[data-role=tool]"),
    ];
    /** En-têtes de raisonnement accessibles (pas ceux d'un groupe replié). */
    const thinkingHeaders = () =>
      screen.queryAllByRole("button", { name: /^(Thought|Thinking)/ });
    const texts = () => [
      ...document.querySelectorAll<HTMLElement>("[data-role=assistant]"),
    ];
    /** Lignes d'appel et résultats visibles, raisonnements, texte intermédiaire. */
    const stepsShown = () => [
      ...[...callLines(), ...resultBlocks()].map(shown),
      thinkingHeaders().length,
      shown(texts()[0]),
    ];

    it("groups the steps of a turn under one button, unfolded while the model works", () => {
      render(list(working));
      const button = toggle()!;
      expect(button.tagName).toBe("BUTTON");
      expect(button.type).toBe("button");
      expect(button.textContent).toBe("4 steps");
      expect(button.getAttribute("aria-expanded")).toBe("true");
      expect(
        screen.getByRole("button", { name: "4 steps", expanded: true }),
      ).toBe(button);
      // Un seul groupe, au-dessus de la première étape.
      expect(
        document.querySelectorAll("[data-role=steps-toggle]"),
      ).toHaveLength(1);
      expect(
        button.compareDocumentPosition(thinkingHeaders()[0]) &
          Node.DOCUMENT_POSITION_FOLLOWING,
      ).toBeTruthy();
      // Tout se voit pendant le travail, chaque étape avec son propre repli.
      expect(callLines()).toHaveLength(2);
      expect(resultBlocks()).toHaveLength(2);
      expect(stepsShown()).toEqual([true, true, true, true, 2, true]);
      expect(resultBlocks()[0].querySelector("button")?.textContent).toBe(
        "Search results for kittens",
      );
    });

    it("stays unfolded between the results and the next reply", () => {
      render(list([sent, searched, fetched]));
      expect(toggle()?.getAttribute("aria-expanded")).toBe("true");
      expect(callLines().every(shown)).toBe(true);
    });

    it("folds them all as soon as the final text starts, keeping it in view", () => {
      const { rerender } = render(list(working));
      const button = toggle()!;
      rerender(list(done));
      // Même bouton, replié, toujours là.
      expect(toggle()).toBe(button);
      expect(button.getAttribute("aria-expanded")).toBe("false");
      expect(button.textContent).toBe("4 steps");
      expect(shown(button)).toBe(true);
      // Étapes masquées (et hors de l'arbre d'accessibilité), texte écrit
      // entre deux appels compris ; le texte final reste.
      expect(stepsShown()).toEqual([false, false, false, false, 0, false]);
      expect(texts().map((text) => [text.textContent, shown(text)])).toEqual([
        ["Let me read it.", false],
        ["The answer", true],
      ]);
      // Un message fait seulement d'étapes ne laisse aucune place.
      const middle = document.querySelector('[data-message-index="2"]')!;
      expect([...middle.children].every((child) => !shown(child))).toBe(true);
      // Fin de la réponse : toujours replié.
      rerender(list(done, false));
      expect(button.getAttribute("aria-expanded")).toBe("false");
      expect(callLines().some(shown)).toBe(false);
    });

    it("folds the text written before the first call with the steps", () => {
      render(
        list(
          [
            sent,
            reply(2, {
              content: "Looking it up.",
              toolCalls: [
                call("web_search", { query: "a" }, { result: "{}" }),
                call("web_search", { query: "b" }, { result: "{}" }),
              ],
            }),
            reply(3, { content: "Done" }),
          ],
          false,
        ),
      );
      expect(shown(toggle()!)).toBe(true);
      expect(texts().map((text) => [text.textContent, shown(text)])).toEqual([
        ["Looking it up.", false],
        ["Done", true],
      ]);
    });

    it("shows a finished turn folded, even without a final text", () => {
      render(
        list(
          [
            sent,
            searched,
            message(5, "user", "Again"),
            reply(6, {
              thinking: "Hmm",
              toolCalls: [call("web_search", { query: "b" })],
            }),
          ],
          false,
        ),
      );
      const buttons = screen.getAllByRole("button", {
        name: "2 steps",
        expanded: false,
      });
      expect(buttons).toHaveLength(2);
      expect(callLines().some(shown)).toBe(false);
      expect(thinkingHeaders()).toHaveLength(0);
    });

    it("unfolds and folds on click, the user's choice winning over the automatic state", () => {
      const { rerender, unmount } = render(list(working));
      const button = toggle()!;
      // Replié pendant le travail : il le reste quand d'autres étapes
      // arrivent.
      fireEvent.click(button);
      expect(button.getAttribute("aria-expanded")).toBe("false");
      expect(callLines().some(shown)).toBe(false);
      const more = reply(4, {
        thinking: "Nearly",
        toolCalls: [call("web_search", { query: "c" })],
      });
      rerender(list([sent, searched, fetched, more]));
      expect(button.textContent).toBe("5 steps");
      expect(button.getAttribute("aria-expanded")).toBe("false");
      expect(callLines().some(shown)).toBe(false);

      // Déplié par l'utilisateur : il le reste au texte final et à la fin.
      fireEvent.click(button);
      expect(button.getAttribute("aria-expanded")).toBe("true");
      rerender(list(done));
      rerender(list(done, false));
      expect(button.getAttribute("aria-expanded")).toBe("true");
      expect(stepsShown()).toEqual([true, true, true, true, 2, true]);
      // Chaque étape garde son propre repli.
      const result = resultBlocks()[0].querySelector("button")!;
      expect(result.getAttribute("aria-expanded")).toBe("false");
      fireEvent.click(result);
      expect(result.getAttribute("aria-expanded")).toBe("true");
      fireEvent.click(button);
      fireEvent.click(button);
      expect(result.getAttribute("aria-expanded")).toBe("true");

      // Choix gardé pour ce tour tant que la page vit (conversation rouverte).
      unmount();
      render(list(done, false));
      expect(toggle()?.getAttribute("aria-expanded")).toBe("true");
    });

    it("toggles from the keyboard, with nothing of a folded group reachable", async () => {
      render(list(done, false));
      await userEvent.tab();
      expect(document.activeElement).toBe(toggle());
      await userEvent.keyboard("{Enter}");
      expect(toggle()?.getAttribute("aria-expanded")).toBe("true");
      expect(callLines().every(shown)).toBe(true);
      await userEvent.keyboard(" ");
      expect(toggle()?.getAttribute("aria-expanded")).toBe("false");
      // Tab suivant : hors des étapes masquées.
      await userEvent.tab();
      expect(shown(document.activeElement!)).toBe(true);
      expect(document.activeElement).not.toBe(toggle());
    });

    it("shows a single step as before, without a group", () => {
      const { rerender } = render(
        list([sent, reply(2, { thinking: "Plan", content: "Hi" })], false),
      );
      expect(toggle()).toBeNull();
      expect(thinkingHeaders()).toHaveLength(1);
      rerender(
        list([
          sent,
          reply(2, {
            toolCalls: [call("web_search", { query: "q" }, { result: "{}" })],
          }),
          reply(3, { content: "Found it" }),
        ]),
      );
      expect(toggle()).toBeNull();
      expect(callLines().every(shown)).toBe(true);
      expect(resultBlocks().every(shown)).toBe(true);
      // Un raisonnement vide n'est pas une étape.
      rerender(
        list(
          [
            sent,
            reply(2, {
              thinking: "  ",
              toolCalls: [call("web_search", { query: "q" }, { result: "{}" })],
            }),
            reply(3, { content: "Found it" }),
          ],
          false,
        ),
      );
      expect(toggle()).toBeNull();
    });

    it("keeps a created image out of the group", () => {
      render(
        list(
          [
            sent,
            reply(2, {
              thinking: "Plan",
              toolCalls: [
                call("web_search", { query: "q" }, { result: "{}" }),
                // Arrêtée (réponse finie sans résultat).
                call("generate_image", { prompt: "cat" }),
              ],
            }),
            reply(3, { content: "Sorry" }),
          ],
          false,
        ),
      );
      expect(toggle()?.textContent).toBe("2 steps");
      expect(toggle()?.getAttribute("aria-expanded")).toBe("false");
      const image = document.querySelector("[data-role=image-stopped]")!;
      expect(shown(image)).toBe(true);
      expect(callLines().map(shown)).toEqual([false, true]);
    });

    it("stays unfolded while an Easy Link bot's reply runs", () => {
      render(
        <MessageList
          messages={[sent, searched, fetched]}
          spacerHeight={0}
          isStreaming={false}
          remoteReplyId={3}
          error={null}
        />,
      );
      expect(toggle()?.getAttribute("aria-expanded")).toBe("true");
    });

    it("counts the steps in the language of the interface", () => {
      setLanguage("fr");
      render(list(done, false));
      expect(toggle()?.textContent).toBe("4 étapes");
      cleanup();
      setLanguage("es");
      render(list(done, false));
      expect(toggle()?.textContent).toBe("4 pasos");
    });
  });

  it("shows actions under the error, only with an error", () => {
    const { rerender } = render(
      <MessageList
        messages={[message(1, "user", "Hi")]}
        spacerHeight={0}
        isStreaming={false}
        error="Web search requires an Ollama account"
        errorActions={<button type="button">Act</button>}
      />,
    );
    const alert = screen.getByRole("alert");
    expect(alert.textContent).toContain(
      "Web search requires an Ollama account",
    );
    expect(
      screen.getByRole("button", { name: "Act" }).closest("[role=alert]"),
    ).toBe(alert);
    rerender(
      <MessageList
        messages={[message(1, "user", "Hi")]}
        spacerHeight={0}
        isStreaming={false}
        error={null}
        errorActions={<button type="button">Act</button>}
      />,
    );
    expect(screen.queryByRole("button", { name: "Act" })).toBeNull();
  });
});
