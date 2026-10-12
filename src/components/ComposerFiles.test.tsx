// Pièces jointes du composer : bouton « + » (sélecteur natif du backend),
// pastilles, erreurs officielles, lectures en cours, avertissement vision et
// envoi des fichiers préparés.

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { clearMocks, mockIPC, mockWindows } from "@tauri-apps/api/mocks";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { ModelDetails } from "@/bindings/ModelDetails";
import type { StagedFile } from "@/bindings/StagedFile";
import type { StageResult } from "@/bindings/StageResult";
import { ChatForm } from "./ChatForm";

afterEach(() => {
  cleanup();
  clearMocks();
});

const MODEL = "qwen3.5:0.8b";

/** Fichier préparé pour un chemin, comme le backend (identifiant inventé). */
function stagedFor(path: string): StagedFile {
  const filename = path.split("/").pop() ?? path;
  const image = /\.(png|jpe?g|webp)$/i.test(filename);
  return {
    id: `id-${filename}`,
    filename,
    kind: image ? "image" : "text",
    mime: image ? "image/png" : "text/plain",
    size: 10,
  };
}

/** Réponses de pick_files déjà données (tous composers confondus). */
const answered = { count: 0, last: [] as string[] };

const staged = (path: string): StageResult => ({
  status: "staged",
  file: stagedFor(path),
});

/**
 * Zone de saisie de la conversation « c » avec le modèle MODEL. `picks` :
 * réponses successives de pick_files (une erreur la fait échouer) ; `stage` :
 * réponse de stage_paths ; `capabilities` : capacités du modèle (null :
 * détails illisibles).
 */
function renderForm({
  picks = [] as (string[] | Error | Promise<string[]>)[],
  stage = (paths: string[]): StageResult[] | Promise<StageResult[]> =>
    paths.map(staged),
  capabilities = ["completion", "vision"] as string[] | null,
} = {}) {
  const unstaged: string[][] = [];
  const stagedPaths: string[][] = [];
  let pickCalls = 0;
  mockIPC(
    (command, args) => {
      switch (command) {
        case "get_settings":
          return { selectedModel: MODEL, serverUrl: "" };
        case "list_models":
          return [];
        case "model_details": {
          if (capabilities === null) {
            throw { kind: "notFound", message: `model '${MODEL}' not found` };
          }
          return {
            model: MODEL,
            capabilities,
            thinking: null,
            alwaysThinks: false,
          } satisfies ModelDetails;
        }
        case "get_think_pref":
          return null;
        case "pick_files": {
          pickCalls++;
          const next = picks.shift() ?? [];
          const answer = (paths: string[]) => {
            answered.count++;
            answered.last = paths;
          };
          if (next instanceof Error) {
            answer([]);
            throw next;
          }
          if (next instanceof Promise) {
            return next.then((paths) => {
              answer(paths);
              return paths;
            });
          }
          answer(next);
          return next;
        }
        case "stage_paths": {
          const { paths } = args as { paths: string[] };
          stagedPaths.push(paths);
          return stage(paths);
        }
        case "unstage":
          unstaged.push((args as { ids: string[] }).ids);
          return null;
      }
      throw new Error(`unexpected command ${command}`);
    },
    { shouldMockEvents: true },
  );
  mockWindows("main");
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
  return {
    onSubmit,
    unstaged,
    stagedPaths,
    pickCalls: () => pickCalls,
    unmount: view.unmount,
    textarea: screen.getByLabelText("Message") as HTMLTextAreaElement,
  };
}

const uploadButton = () =>
  screen.getByRole("button", { name: "Upload multiple files" });
const sendButton = () =>
  screen.getByRole("button", { name: "Send message" }) as HTMLButtonElement;
const chips = () =>
  screen.queryAllByTestId("composer-file").map((chip) => chip.textContent);
const errorChips = () =>
  screen
    .queryAllByTestId("composer-file-error")
    .map((chip) => chip.textContent);

/** Le modèle est choisi et ses détails lus (avertissement vision à jour). */
async function ready() {
  await waitFor(() =>
    expect(screen.getByTitle("Select model").textContent).toBe(MODEL),
  );
  await act(() => new Promise((resolve) => setTimeout(resolve, 20)));
}

/**
 * Clique sur « + » et attend la réponse du sélecteur, puis une pastille
 * (préparée ou en erreur) pour chaque fichier choisi, sans lecture en cours.
 */
async function pick() {
  const before = answered.count;
  await userEvent.click(uploadButton());
  await waitFor(() => expect(answered.count).toBe(before + 1));
  const names = answered.last.map((path) => path.split("/").pop() ?? path);
  await waitFor(() => {
    const shown = [
      ...screen.queryAllByTestId("composer-file"),
      ...screen.queryAllByTestId("composer-file-error"),
    ].map((chip) => chip.textContent ?? "");
    for (const name of names) {
      expect(shown.some((text) => text.startsWith(name))).toBe(true);
    }
    expect(screen.queryAllByTestId("composer-file-reading")).toHaveLength(0);
  });
}

describe("ComposerFiles", () => {
  it("adds the picked files as chips, in order, with their thumbnails", async () => {
    const { stagedPaths } = renderForm({
      picks: [["/home/u/notes.txt", "/home/u/red.png"]],
    });
    await ready();
    expect(uploadButton().getAttribute("title")).toBe("Upload multiple files");
    await pick();
    expect(stagedPaths).toEqual([["/home/u/notes.txt", "/home/u/red.png"]]);
    expect(chips()).toEqual(["notes.txt", "red.png"]);
    const image = screen.getByRole("img", { name: "red.png" });
    expect(image.getAttribute("src")).toBe(
      "chatonlm://localhost/staged/id-red.png",
    );
    // Un fichier texte : icône de document, pas de vignette.
    expect(screen.queryByRole("img", { name: "notes.txt" })).toBeNull();
  });

  it("does nothing when the picker is cancelled", async () => {
    const { stagedPaths } = renderForm({ picks: [[]] });
    await ready();
    await pick();
    expect(stagedPaths).toEqual([]);
    expect(screen.queryByTestId("composer-files")).toBeNull();
  });

  it("shows refused files with the official text, and they block sending (Enter too)", async () => {
    const { onSubmit, textarea } = renderForm({
      picks: [["/a/notes.txt", "/a/report.docx"]],
      stage: (paths) => [
        staged(paths[0]),
        {
          status: "rejected",
          filename: "report.docx",
          error: "File type not supported",
        },
      ],
    });
    await ready();
    await pick();
    expect(errorChips()).toEqual(["report.docx• File type not supported"]);
    await userEvent.type(textarea, "Hello{Enter}");
    expect(onSubmit).not.toHaveBeenCalled();
    expect(sendButton().disabled).toBe(true);

    // Erreur retirée : pas de retrait côté backend (rien n'y est préparé).
    await userEvent.click(
      screen.getByRole("button", { name: "Remove report.docx" }),
    );
    expect(errorChips()).toEqual([]);
    expect(sendButton().disabled).toBe(false);
    await userEvent.type(textarea, "{Enter}");
    expect(onSubmit).toHaveBeenCalledWith(
      "Hello",
      MODEL,
      undefined,
      [stagedFor("/a/notes.txt")],
      [],
      false,
    );
  });

  it("removes a file from the composer and from the backend", async () => {
    const { unstaged, onSubmit, textarea } = renderForm({
      picks: [["/a/notes.txt", "/a/red.png"]],
    });
    await ready();
    await pick();
    await userEvent.click(
      screen.getByRole("button", { name: "Remove notes.txt" }),
    );
    expect(chips()).toEqual(["red.png"]);
    expect(unstaged).toEqual([["id-notes.txt"]]);
    await userEvent.type(textarea, "Hello{Enter}");
    expect(onSubmit).toHaveBeenCalledWith(
      "Hello",
      MODEL,
      undefined,
      [stagedFor("/a/red.png")],
      [],
      false,
    );
  });

  it("waits for the files still being read before sending", async () => {
    let finish: (results: StageResult[]) => void = () => {};
    const { onSubmit, textarea } = renderForm({
      picks: [["/a/big.pdf"]],
      stage: () => new Promise((resolve) => (finish = resolve)),
    });
    await ready();
    await userEvent.click(uploadButton());
    const reading = await screen.findByTestId("composer-file-reading");
    expect(reading.textContent).toBe("big.pdf");
    expect(reading.getAttribute("aria-busy")).toBe("true");
    await userEvent.type(textarea, "Hello{Enter}");
    expect(onSubmit).not.toHaveBeenCalled();
    expect(sendButton().disabled).toBe(true);

    await act(async () => finish([staged("/a/big.pdf")]));
    expect(screen.queryByTestId("composer-file-reading")).toBeNull();
    expect(chips()).toEqual(["big.pdf"]);
    await userEvent.type(textarea, "{Enter}");
    expect(onSubmit).toHaveBeenCalledWith(
      "Hello",
      MODEL,
      undefined,
      [stagedFor("/a/big.pdf")],
      [],
      false,
    );
  });

  it("sends the files in order, then empties the composer without releasing them", async () => {
    const { onSubmit, unstaged, textarea, unmount } = renderForm({
      picks: [["/a/one.txt"], ["/a/two.png", "/a/three.md"]],
    });
    await ready();
    await pick();
    await pick();
    expect(chips()).toEqual(["one.txt", "two.png", "three.md"]);
    await userEvent.type(textarea, "Hello{Enter}");
    expect(onSubmit).toHaveBeenCalledWith(
      "Hello",
      MODEL,
      undefined,
      ["/a/one.txt", "/a/two.png", "/a/three.md"].map(stagedFor),
      [],
      false,
    );
    expect(screen.queryByTestId("composer-files")).toBeNull();
    // Partis avec le message : le backend les a pris, rien à libérer.
    unmount();
    expect(unstaged).toEqual([]);
  });

  it("releases the remaining files when the composer goes away", async () => {
    const { unstaged, unmount } = renderForm({
      picks: [["/a/one.txt", "/a/two.png"]],
    });
    await ready();
    await pick();
    unmount();
    expect(unstaged).toEqual([["id-one.txt", "id-two.png"]]);
  });

  it("releases files whose reading ends after the composer went away", async () => {
    let finish: (results: StageResult[]) => void = () => {};
    const { unstaged, unmount } = renderForm({
      picks: [["/a/big.pdf", "/a/bad.exe"]],
      stage: () => new Promise((resolve) => (finish = resolve)),
    });
    await ready();
    await userEvent.click(uploadButton());
    await screen.findAllByTestId("composer-file-reading");
    unmount();
    expect(unstaged).toEqual([]);
    await act(async () =>
      finish([
        staged("/a/big.pdf"),
        {
          status: "rejected",
          filename: "bad.exe",
          error: "File type not supported",
        },
      ]),
    );
    expect(unstaged).toEqual([["id-big.pdf"]]);
  });

  it("shows every file as unreadable when the backend fails", async () => {
    const error = vi.spyOn(console, "error").mockImplementation(() => {});
    renderForm({
      picks: [["/a/one.txt", "/a/two.txt"]],
      stage: () => Promise.reject(new Error("ipc down")),
    });
    await ready();
    await pick();
    await waitFor(() =>
      expect(errorChips()).toEqual([
        "one.txt• Error reading file",
        "two.txt• Error reading file",
      ]),
    );
    expect(error).toHaveBeenCalled();
    error.mockRestore();
  });

  it("opens one picker at a time (it is not modal on Linux)", async () => {
    let choose: (paths: string[]) => void = () => {};
    const { pickCalls } = renderForm({
      picks: [
        new Promise<string[]>((resolve) => (choose = resolve)),
        ["/a/two.txt"],
      ],
    });
    await ready();
    await userEvent.click(uploadButton());
    await userEvent.click(uploadButton());
    expect(pickCalls()).toBe(1);
    await act(async () => choose(["/a/one.txt"]));
    await waitFor(() => expect(chips()).toEqual(["one.txt"]));
    // Sélecteur refermé : « + » en rouvre un.
    await pick();
    expect(pickCalls()).toBe(2);
    expect(chips()).toEqual(["one.txt", "two.txt"]);
  });

  it("one picker for all chats: its files go to the composer shown when it closes", async () => {
    let choose: (paths: string[]) => void = () => {};
    const first = renderForm({
      picks: [new Promise<string[]>((resolve) => (choose = resolve))],
    });
    await ready();
    await userEvent.click(uploadButton());
    expect(first.pickCalls()).toBe(1);
    // Autre conversation, sélecteur encore ouvert : pas de second sélecteur.
    first.unmount();
    const second = renderForm({ picks: [["/a/other.txt"]] });
    await ready();
    await userEvent.click(uploadButton());
    expect(second.pickCalls()).toBe(0);
    // Fermé : les fichiers choisis vont au composer affiché.
    await act(async () => choose(["/a/one.txt"]));
    await waitFor(() => expect(chips()).toEqual(["one.txt"]));
    expect(second.stagedPaths).toEqual([["/a/one.txt"]]);
    expect(first.stagedPaths).toEqual([]);
  });

  it("shows the picker failure above the composer, cleared by the next pick", async () => {
    const error = vi.spyOn(console, "error").mockImplementation(() => {});
    renderForm({
      picks: [new Error("no display"), ["/a/notes.txt"]],
    });
    await ready();
    await pick();
    const alert = await screen.findByRole("alert");
    expect(alert.textContent).toContain("no display");
    await pick();
    expect(screen.queryByRole("alert")).toBeNull();
    expect(chips()).toEqual(["notes.txt"]);
    error.mockRestore();
  });
});

describe("ComposerFiles: vision", () => {
  const WARNING = "This model does not support images";

  it("warns about images for a model without vision, and leaves them out", async () => {
    const { onSubmit, unstaged, textarea } = renderForm({
      picks: [["/a/notes.txt", "/a/red.png"]],
      capabilities: ["completion"],
    });
    await ready();
    await pick();
    const image = screen.getAllByTestId("composer-file")[1];
    expect(within(image).getByText(WARNING)).toBeTruthy();
    expect(image.className).toContain("bg-red-50");
    expect(screen.getAllByText(WARNING)).toHaveLength(1);
    // L'avertissement ne bloque pas l'envoi : l'image est retirée, libérée.
    await userEvent.type(textarea, "Hello{Enter}");
    expect(onSubmit).toHaveBeenCalledWith(
      "Hello",
      MODEL,
      undefined,
      [stagedFor("/a/notes.txt")],
      [],
      false,
    );
    expect(unstaged).toEqual([["id-red.png"]]);
  });

  it("keeps images for a vision model", async () => {
    const { onSubmit, unstaged, textarea } = renderForm({
      picks: [["/a/red.png"]],
      capabilities: ["completion", "vision"],
    });
    await ready();
    await pick();
    expect(screen.queryByText(WARNING)).toBeNull();
    await userEvent.type(textarea, "Hello{Enter}");
    expect(onSubmit).toHaveBeenCalledWith(
      "Hello",
      MODEL,
      undefined,
      [stagedFor("/a/red.png")],
      [],
      false,
    );
    expect(unstaged).toEqual([]);
  });

  for (const [label, capabilities] of [
    ["details that cannot be read (model not downloaded yet)", null],
    ["a server too old to announce capabilities", []],
  ] as const) {
    it(`neither warns nor leaves images out with ${label}`, async () => {
      const { onSubmit, textarea } = renderForm({
        picks: [["/a/red.png"]],
        capabilities: capabilities === null ? null : [...capabilities],
      });
      await ready();
      await pick();
      expect(screen.queryByText(WARNING)).toBeNull();
      await userEvent.type(textarea, "Hello{Enter}");
      expect(onSubmit).toHaveBeenCalledWith(
        "Hello",
        MODEL,
        undefined,
        [stagedFor("/a/red.png")],
        [],
        false,
      );
    });
  }
});

describe("ImageThumbnail in the composer", () => {
  it("falls back to a picture icon when the image cannot be loaded", async () => {
    renderForm({ picks: [["/a/red.png"]] });
    await ready();
    await pick();
    fireEvent.error(screen.getByRole("img", { name: "red.png" }));
    const fallback = screen.getByRole("img", { name: "red.png" });
    expect(fallback.tagName).toBe("DIV");
  });
});
