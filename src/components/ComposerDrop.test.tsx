// Fichiers déposés sur la fenêtre (dépôt natif de Tauri : chemins) et
// fichiers collés (octets), face au simulateur.

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { ChatForm } from "@/components/ChatForm";
import { setPlatform } from "@/lib/platform";
import { fakeBackend, MAX_FILE_SIZE } from "@/test/backend";
import { installSimulator, type RawBody } from "@/test/simulator";

// Démonté avant la fin du simulateur (désinscriptions, fichiers libérés).
afterEach(cleanup);

const MODEL = "qwen3:0.6b";
const POSITION = { x: 10, y: 20 };

/** Zone de saisie de la conversation « c », face au simulateur. */
function setup(
  files: Record<string, { size?: number } | { error: string }> = {},
) {
  const simulator = installSimulator();
  const backend = fakeBackend(simulator, {
    selectedModel: MODEL,
    files,
    // Images sans avertissement : seuls les noms dans les pastilles.
    details: [
      {
        model: MODEL,
        capabilities: ["completion", "vision"],
        thinking: null,
        alwaysThinks: false,
      },
    ],
  });
  const onSubmit = vi.fn();
  const client = new QueryClient({
    defaultOptions: { queries: { networkMode: "always", retry: false } },
  });
  const view = render(
    <QueryClientProvider client={client}>
      <ChatForm
        chatId="c"
        hasMessages
        isStreaming={false}
        isDisabled={false}
        onSubmit={onSubmit}
        onCancel={() => {}}
      />
    </QueryClientProvider>,
  );
  return { simulator, backend, view, onSubmit };
}

const chips = () =>
  screen.queryAllByTestId("composer-file").map((chip) => chip.textContent);
const errorChips = () =>
  screen
    .queryAllByTestId("composer-file-error")
    .map((chip) => chip.textContent);
const overlay = () => screen.queryByTestId("drop-overlay");

/** Composer prêt : il écoute les dépôts. */
async function listening(simulator: ReturnType<typeof installSimulator>) {
  await waitFor(() => expect(simulator.listening("tauri://drag-drop")).toBe(1));
}

describe("dropped files", () => {
  it("shows the official overlay while files are dragged over the window", async () => {
    const { simulator } = setup();
    await listening(simulator);
    expect(overlay()).toBeNull();
    await simulator.emitEvent("tauri://drag-enter", {
      paths: ["/a/notes.txt"],
      position: POSITION,
    });
    expect(overlay()?.textContent).toBe(
      "Drop files here or paste from clipboard to add them to your message",
    );
    // Toute la fenêtre, sans prendre les clics.
    expect(overlay()?.className).toContain("fixed inset-0");
    expect(overlay()?.className).toContain("pointer-events-none");
    await simulator.emitEvent("tauri://drag-over", { position: POSITION });
    expect(overlay()).not.toBeNull();
    await simulator.emitEvent("tauri://drag-leave", null);
    expect(overlay()).toBeNull();
  });

  it("stages the dropped paths, in order", async () => {
    const { simulator } = setup({
      "/a/notes.txt": { size: 3 },
      "/a/report.docx": { error: "File type not supported" },
    });
    await listening(simulator);
    await simulator.emitEvent("tauri://drag-enter", {
      paths: ["/a/notes.txt", "/a/report.docx"],
      position: POSITION,
    });
    await simulator.emitEvent("tauri://drag-drop", {
      paths: ["/a/notes.txt", "/a/report.docx"],
      position: POSITION,
    });
    expect(overlay()).toBeNull();
    await waitFor(() => expect(chips()).toEqual(["notes.txt"]));
    expect(errorChips()).toEqual(["report.docx• File type not supported"]);
    expect(simulator.callsTo("stage_paths")).toEqual([
      { paths: ["/a/notes.txt", "/a/report.docx"] },
    ]);
  });

  it("on Windows, stages dropped drive and network paths under their file names", async () => {
    setPlatform("windows");
    const photo = "C:\\Users\\me\\My photos\\cat.png";
    const notes = "\\\\server\\share\\notes.txt";
    const { simulator } = setup({ [photo]: { size: 3 }, [notes]: { size: 3 } });
    await listening(simulator);
    await simulator.emitEvent("tauri://drag-enter", {
      paths: [photo, notes, "https://example.com/a.png"],
      position: POSITION,
    });
    expect(overlay()).not.toBeNull();
    await simulator.emitEvent("tauri://drag-drop", {
      paths: [photo, notes, "https://example.com/a.png"],
      position: POSITION,
    });
    await waitFor(() => expect(chips()).toEqual(["cat.png", "notes.txt"]));
    expect(simulator.callsTo("stage_paths")).toEqual([
      { paths: [photo, notes] },
    ]);
  });

  it("on Linux, a Windows path is not a local file", async () => {
    const { simulator } = setup();
    await listening(simulator);
    await simulator.emitEvent("tauri://drag-enter", {
      paths: ["C:\\Users\\me\\cat.png"],
      position: POSITION,
    });
    expect(overlay()).toBeNull();
    await simulator.emitEvent("tauri://drag-drop", {
      paths: ["C:\\Users\\me\\cat.png"],
      position: POSITION,
    });
    expect(simulator.callsTo("stage_paths")).toEqual([]);
  });

  it("leaves text and link drags to the page (no paths: no overlay)", async () => {
    const { simulator } = setup();
    await listening(simulator);
    await simulator.emitEvent("tauri://drag-enter", {
      paths: [],
      position: POSITION,
    });
    expect(overlay()).toBeNull();
  });

  it("ignores links and web images dragged from a browser or a reply", async () => {
    const { simulator } = setup({ "/a/notes.txt": { size: 3 } });
    await listening(simulator);
    const web = ["https://example.com/photo.png", "https://example.com/"];
    await simulator.emitEvent("tauri://drag-enter", {
      paths: web,
      position: POSITION,
    });
    expect(overlay()).toBeNull();
    await simulator.emitEvent("tauri://drag-drop", {
      paths: web,
      position: POSITION,
    });
    await act(() => new Promise((resolve) => setTimeout(resolve, 20)));
    expect(simulator.callsTo("stage_paths")).toEqual([]);
    expect(screen.queryByTestId("composer-files")).toBeNull();
    // Avec un fichier local : seul le fichier est joint.
    await simulator.emitEvent("tauri://drag-drop", {
      paths: [web[0], "/a/notes.txt"],
      position: POSITION,
    });
    await waitFor(() => expect(chips()).toEqual(["notes.txt"]));
    expect(simulator.callsTo("stage_paths")).toEqual([
      { paths: ["/a/notes.txt"] },
    ]);
  });

  it("puts the overlay in the page body, above dialogs and notices", async () => {
    const { simulator } = setup();
    await listening(simulator);
    await simulator.emitEvent("tauri://drag-enter", {
      paths: ["/a/notes.txt"],
      position: POSITION,
    });
    // Hors du contexte d'empilement du composer (sticky z-20 d'une
    // conversation) : son z-[9999] vaut pour toute la page.
    expect(overlay()?.parentElement).toBe(document.body);
  });

  it("an added file clears the picker failure, like the official app", async () => {
    const error = vi.spyOn(console, "error").mockImplementation(() => {});
    const { simulator } = setup({ "/a/notes.txt": { size: 3 } });
    simulator.handle("pick_files", () => {
      throw new Error("no display");
    });
    await listening(simulator);
    fireEvent.click(
      screen.getByRole("button", { name: "Upload multiple files" }),
    );
    await waitFor(() =>
      expect(screen.getByRole("alert").textContent).toContain("no display"),
    );
    await simulator.emitEvent("tauri://drag-drop", {
      paths: ["/a/notes.txt"],
      position: POSITION,
    });
    await waitFor(() => expect(chips()).toEqual(["notes.txt"]));
    expect(screen.queryByRole("alert")).toBeNull();
    error.mockRestore();
  });

  it("stops listening even when it goes away before listening started", async () => {
    const { simulator, view } = setup();
    // Démonté avant la réponse de listen (StrictMode : à chaque montage).
    view.unmount();
    await act(() => new Promise((resolve) => setTimeout(resolve, 50)));
    for (const event of ["enter", "over", "drop", "leave"]) {
      expect(simulator.listening(`tauri://drag-${event}`)).toBe(0);
    }
  });

  it("stops listening when the composer goes away", async () => {
    const { simulator, view } = setup({ "/a/notes.txt": {} });
    await listening(simulator);
    view.unmount();
    expect(simulator.listening("tauri://drag-drop")).toBe(0);
  });
});

/** Presse-papiers d'un collage (jsdom n'a pas de DataTransfer). */
function clipboard({
  text = "",
  files = [] as File[],
  items = files.map((file) => ({ kind: "file", getAsFile: () => file })),
}: {
  text?: string;
  files?: File[];
  items?: { kind: string; getAsFile: () => File | null }[];
}) {
  return {
    types: [
      ...(text ? ["text/plain"] : []),
      ...(files.length ? ["Files"] : []),
    ],
    getData: (type: string) => (type === "text/plain" ? text : ""),
    items,
    files,
  };
}

/** Corps et en-têtes des appels de stage_bytes. */
const pasted = (simulator: ReturnType<typeof installSimulator>) =>
  simulator.callsTo("stage_bytes") as unknown as RawBody[];

describe("pasted files", () => {
  it("stages a pasted screenshot under the name image.png", async () => {
    const { simulator } = setup();
    await listening(simulator);
    const shot = new File([new Uint8Array([1, 2, 3])], "", {
      type: "image/png",
    });
    const event = createPaste(clipboard({ files: [shot] }));
    await waitFor(() => expect(chips()).toEqual(["image.png"]));
    expect(event.defaultPrevented).toBe(true);
    expect(pasted(simulator)).toEqual([
      {
        bytes: [1, 2, 3],
        headers: { "x-filename": "image.png", "x-size": "3" },
      },
    ]);
  });

  it("pastes the text when the clipboard also has text, like the official app", async () => {
    const { simulator } = setup();
    await listening(simulator);
    const file = new File(["x"], "notes.txt", { type: "text/plain" });
    const event = createPaste(
      clipboard({ text: "file:///a/notes.txt", files: [file] }),
    );
    await act(() => new Promise((resolve) => setTimeout(resolve, 20)));
    expect(event.defaultPrevented).toBe(false);
    expect(pasted(simulator)).toEqual([]);
    expect(simulator.callsTo("stage_clipboard_image")).toEqual([]);
  });

  it("falls back on clipboardData.files when the items give no file", async () => {
    const { simulator } = setup();
    await listening(simulator);
    const file = new File(["hello"], "notes.txt", { type: "text/plain" });
    createPaste(
      clipboard({
        files: [file],
        items: [{ kind: "file", getAsFile: () => null }],
      }),
    );
    await waitFor(() => expect(chips()).toEqual(["notes.txt"]));
  });

  it("refuses a pasted file that is too large without reading it", async () => {
    const { simulator } = setup();
    await listening(simulator);
    const arrayBuffer = vi.fn();
    const huge = {
      name: "big.txt",
      type: "text/plain",
      size: MAX_FILE_SIZE + 1,
      arrayBuffer,
    } as unknown as File;
    createPaste(clipboard({ files: [huge] }));
    await waitFor(() =>
      expect(errorChips()).toEqual(["big.txt• File too large"]),
    );
    expect(arrayBuffer).not.toHaveBeenCalled();
    expect(pasted(simulator)[0].bytes).toEqual([]);
  });

  it("shows a pasted file that cannot be read as unreadable, keeping the others", async () => {
    const error = vi.spyOn(console, "error").mockImplementation(() => {});
    const { simulator } = setup();
    await listening(simulator);
    const broken = {
      name: "gone.txt",
      type: "text/plain",
      size: 4,
      arrayBuffer: () => Promise.reject(new Error("gone")),
    } as unknown as File;
    const fine = new File(["ok"], "fine.md", { type: "text/markdown" });
    createPaste(clipboard({ files: [broken, fine] }));
    await waitFor(() => expect(chips()).toEqual(["fine.md"]));
    expect(errorChips()).toEqual(["gone.txt• Error reading file"]);
    error.mockRestore();
  });
});

/** Colle dans la zone de saisie ; renvoie l'évènement (defaultPrevented). */
function createPaste(clipboardData: ReturnType<typeof clipboard>) {
  const textarea = screen.getByLabelText("Message");
  const event = new Event("paste", { bubbles: true, cancelable: true });
  Object.defineProperty(event, "clipboardData", { value: clipboardData });
  fireEvent(textarea, event);
  return event;
}

// WebKitGTK ne donne à la page ni type ni fichier pour une image collée
// (capture d'écran) : le backend la lit dans le presse-papiers GTK.
describe("pasted screenshots read by the backend", () => {
  it("a paste the page cannot read stages the clipboard image", async () => {
    const { simulator, backend } = setup();
    backend.clipboardImage = { size: 42 };
    await listening(simulator);
    const event = createPaste(clipboard({}));
    await waitFor(() => expect(chips()).toEqual(["image.png"]));
    expect(simulator.callsTo("stage_clipboard_image")).toHaveLength(1);
    // Rien à empêcher : la page n'avait rien à coller.
    expect(event.defaultPrevented).toBe(false);
  });

  it("nothing happens when the clipboard has no image", async () => {
    const { simulator } = setup();
    await listening(simulator);
    createPaste(clipboard({}));
    await waitFor(() =>
      expect(simulator.callsTo("stage_clipboard_image")).toHaveLength(1),
    );
    await act(() => new Promise((resolve) => setTimeout(resolve, 20)));
    expect(screen.queryByTestId("composer-files")).toBeNull();
  });

  it("text or files pasted never read the clipboard image", async () => {
    const { simulator, backend } = setup();
    backend.clipboardImage = { size: 42 };
    await listening(simulator);
    createPaste(clipboard({ text: "hello" }));
    const file = new File(["x"], "notes.txt", { type: "text/plain" });
    createPaste(clipboard({ files: [file] }));
    await waitFor(() => expect(chips()).toEqual(["notes.txt"]));
    expect(simulator.callsTo("stage_clipboard_image")).toEqual([]);
  });

  it("sending waits for the clipboard image being read", async () => {
    const { simulator, onSubmit } = setup();
    let answer: (result: unknown) => void = () => {};
    simulator.handle(
      "stage_clipboard_image",
      () => new Promise((resolve) => (answer = resolve)),
    );
    await listening(simulator);
    const textarea = screen.getByLabelText("Message");
    createPaste(clipboard({}));
    await waitFor(() =>
      expect(simulator.callsTo("stage_clipboard_image")).toHaveLength(1),
    );
    fireEvent.change(textarea, { target: { value: "Look" } });
    fireEvent.keyDown(textarea, { key: "Enter" });
    expect(onSubmit).not.toHaveBeenCalled();
    const send = screen.getByRole("button", { name: "Send message" });
    expect((send as HTMLButtonElement).disabled).toBe(true);

    await act(async () =>
      answer({
        status: "staged",
        file: {
          id: "staged-9",
          filename: "image.png",
          kind: "image",
          mime: "image/png",
          size: 42,
        },
      }),
    );
    await waitFor(() => expect(chips()).toEqual(["image.png"]));
    fireEvent.keyDown(textarea, { key: "Enter" });
    expect(onSubmit).toHaveBeenCalledWith(
      "Look",
      MODEL,
      undefined,
      [expect.objectContaining({ id: "staged-9", filename: "image.png" })],
      [],
      false,
    );
  });
});
