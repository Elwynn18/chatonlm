// Langues de l'interface, zone du chat et du cadre : aucun texte écrit en dur
// (pseudo-langue : tout texte traduit est marqué, src/test/i18n.ts) dans les
// écrans principaux, puis quelques textes en français et en espagnol
// (pluriel, date, tailles), et un changement de langue sans rechargement.

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import {
  act,
  cleanup,
  configure,
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";
import { afterEach, describe, expect, it, onTestFinished, vi } from "vitest";
import type { DatabaseProblem } from "@/bindings/DatabaseProblem";
import type { HubDownload } from "@/bindings/HubDownload";
import type { Lang } from "@/bindings/Lang";
import type { Message } from "@/bindings/Message";
import type { ServerState } from "@/bindings/ServerState";
import type { ServerStatus } from "@/bindings/ServerStatus";
import type { ToolCall } from "@/bindings/ToolCall";
import { App } from "@/App";
import { CommandBlock } from "@/components/CommandBlock";
import { ComposerFiles } from "@/components/ComposerFiles";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { DatabaseProblemScreen } from "@/components/DatabaseProblemScreen";
import { Downloading } from "@/components/Downloading";
import { DropOverlay } from "@/components/DropOverlay";
import { ErrorMessage } from "@/components/ErrorMessage";
import { ExternalLink } from "@/components/ExternalLink";
import { LinkErrorNotice } from "@/components/LinkErrorNotice";
import { LoadingModel } from "@/components/LoadingModel";
import { ServerGate } from "@/components/ServerGate";
import { ServerLostBanner } from "@/components/ServerLostBanner";
import { ServerStatusScreen } from "@/components/ServerStatusScreen";
import { Thinking } from "@/components/Thinking";
import { WebSearchErrorActions } from "@/components/WebSearchErrorActions";
import { setLanguage } from "@/i18n";
import { openLink, reportCopyFailure, resetLinks } from "@/lib/openLink";
import { fakeBackend, type StoredChat, thinkingModel } from "@/test/backend";
import { untranslatedTexts, usePseudoLanguage } from "@/test/i18n";
import { composer, renderApp } from "@/test/renderApp";
import { installSimulator } from "@/test/simulator";

configure({ asyncUtilTimeout: 5000 });
vi.setConfig({ testTimeout: 30_000 });

afterEach(() => {
  cleanup();
  resetLinks();
});

const MODEL = "qwen3:0.6b";
const OTHER_MODEL = "qwen3.5:0.8b";
const DAY = 24 * 60 * 60 * 1000;
/** Création de la conversation sans titre ni message (date affichée). */
const UNTITLED_AT = new Date(2025, 9, 3, 15, 0).getTime();
const GIB = 1024 ** 3;

function chat(
  id: string,
  title: string,
  ageMs: number,
  extra: Partial<StoredChat> = {},
): StoredChat {
  const at = Date.now() - ageMs;
  return { id, title, createdAt: at, updatedAt: at, messages: [], ...extra };
}

function download(model: string, fields: Partial<HubDownload>): HubDownload {
  return {
    model,
    repo: null,
    label: null,
    state: "downloading",
    completed: 0,
    total: 0,
    bytesPerSecond: null,
    error: null,
    startedAt: 1,
    finishedAt: null,
    ...fields,
  };
}

/**
 * App à `path` : modèle qui raisonne par niveaux et sait chercher sur le web,
 * historique avec une favorite, une conversation du jour, une plus ancienne
 * et une sans titre ni message, plus `extra` ; `downloads` téléchargements
 * en cours.
 */
function setup(
  path: string,
  { downloads = 0, extra = [] as StoredChat[], language = "en" as Lang } = {},
) {
  const simulator = installSimulator();
  const backend = fakeBackend(simulator, {
    details: [
      {
        ...thinkingModel(MODEL, {
          values: [false, "low", "high"],
          default: "low",
        }),
        capabilities: ["completion", "thinking", "tools", "vision"],
      },
    ],
  });
  for (const stored of [
    chat("fav", "Trip ideas", 3 * DAY, { favorite: true }),
    chat("today", "Recipes", 60_000),
    chat("older", "Weekend plan", 40 * DAY),
    {
      ...chat("untitled", "", 50 * DAY),
      createdAt: UNTITLED_AT,
      updatedAt: UNTITLED_AT,
    },
    ...extra,
  ]) {
    backend.chats.set(stored.id, stored);
  }
  for (let i = 0; i < downloads; i++) {
    backend.hub.downloads.push(
      download(`hf.co/a/m${i}:Q4_K_M`, { completed: GIB, total: 4 * GIB }),
    );
  }
  // Langue des réglages, que l'app applique en les lisant.
  backend.settings.language = language;
  backend.settings.effectiveLanguage = language;
  simulator.handle("delete_chat", () => null);
  const app = renderApp(path);
  return { simulator, backend, app };
}

/** Données de l'historique et des modèles (pas des textes de l'interface). */
const HISTORY = ["Trip ideas", "Recipes", "Weekend plan", MODEL, OTHER_MODEL];

/** Barre latérale ouverte (état gardé pour la session), historique lu. */
async function openSidebar() {
  const toggle = await screen.findByRole("button", { name: /sidebar/i });
  const label = toggle.getAttribute("aria-label") ?? "";
  if (!/hide|masquer|ocultar/i.test(label)) await userEvent.click(toggle);
  await waitFor(() =>
    expect(document.querySelector('[data-chat-id="older"]')).not.toBeNull(),
  );
}

function row(id: string) {
  const found = document.querySelector<HTMLElement>(`[data-chat-id="${id}"]`);
  if (!found) throw new Error(`no row ${id}`);
  return found;
}

function message(
  id: number,
  role: "user" | "assistant",
  content: string,
  extra: Partial<Message> = {},
): Message {
  return {
    id,
    role,
    content,
    thinking: "",
    model: role === "assistant" ? MODEL : null,
    createdAt: id,
    updatedAt: id,
    thinkingStartedAt: null,
    thinkingEndedAt: null,
    attachments: [],
    toolCalls: [],
    ...extra,
  };
}

function toolCall(
  name: string,
  args: Record<string, string>,
  result: unknown,
  error: string | null = null,
): ToolCall {
  return {
    name,
    arguments: JSON.stringify(args),
    result: result === null ? null : JSON.stringify(result),
    error,
  };
}

function status(
  state: ServerState,
  extra: Partial<ServerStatus> = {},
): ServerStatus {
  return {
    url: "http://localhost:11434",
    state,
    version: null,
    alternativeUrl: null,
    hostEnv: null,
    httpStatus: null,
    detail: null,
    service: null,
    builtin: false,
    builtinAvailable: false,
    explicitAddress: false,
    logPath: null,
    ...extra,
  };
}

describe("pseudo-language: no text written in the code", () => {
  it("new chat: history with favorites and dates, composer, downloads", async () => {
    usePseudoLanguage();
    setup("/c/new", { downloads: 2 });
    await openSidebar();
    await screen.findByTestId("download-indicator");
    // Bouton Think (niveaux) et recherche web : détails du modèle lus.
    await waitFor(() =>
      expect(
        document.querySelector('[aria-haspopup="menu"][aria-expanded]'),
      ).not.toBeNull(),
    );
    await screen.findByRole("button", { name: "⟦Web search⟧" });
    expect(untranslatedTexts(document.body, HISTORY)).toEqual([]);

    // Sélecteur de modèle ouvert, puis sans résultat.
    await userEvent.click(
      document.querySelector<HTMLElement>('[aria-haspopup="listbox"]')!,
    );
    await screen.findByRole("listbox");
    expect(untranslatedTexts(document.body, HISTORY)).toEqual([]);
    await userEvent.type(screen.getAllByRole("textbox").at(-1)!, "zzz");
    await waitFor(() =>
      expect(screen.queryAllByRole("option")).toHaveLength(0),
    );
    expect(untranslatedTexts(document.body, HISTORY)).toEqual([]);
    await userEvent.keyboard("{Escape}");

    // Menu des niveaux de réflexion.
    await userEvent.click(
      document.querySelector<HTMLElement>('[aria-haspopup="menu"]')!,
    );
    await screen.findByRole("menu");
    expect(untranslatedTexts(document.body, HISTORY)).toEqual([]);
    await userEvent.keyboard("{Escape}");
  });

  it("history: context menu, delete confirmation and rename field", async () => {
    usePseudoLanguage();
    setup("/c/today");
    await openSidebar();
    fireEvent.contextMenu(row("fav"));
    const menu = await screen.findByRole("menu");
    expect(within(menu).getAllByRole("menuitem")).toHaveLength(3);
    expect(untranslatedTexts(document.body, HISTORY)).toEqual([]);

    // Supprimer : confirmation dessinée dans la page.
    await userEvent.click(within(menu).getAllByRole("menuitem")[2]);
    await screen.findByRole("alertdialog");
    expect(untranslatedTexts(document.body, HISTORY)).toEqual([]);
    await userEvent.keyboard("{Escape}");

    // Renommer : champ du titre.
    fireEvent.contextMenu(row("today"));
    const again = await screen.findByRole("menu");
    await userEvent.click(within(again).getAllByRole("menuitem")[1]);
    await waitFor(() =>
      expect(row("today").querySelector("input")).not.toBeNull(),
    );
    expect(untranslatedTexts(document.body, HISTORY)).toEqual([]);
  });

  it("chat: attachments, thinking, tool calls and their results, code", async () => {
    usePseudoLanguage();
    const search = toolCall(
      "web_search",
      { query: "latest ollama" },
      { results: [{ title: "Ollama", url: "https://ollama.com" }] },
    );
    const fetch = toolCall(
      "web_fetch",
      { url: "https://ollama.com/blog" },
      { title: "Blog", content: "News" },
    );
    const other = toolCall("get_weather", { query: "Paris" }, null, "timeout");
    const unnamed = toolCall("web_search", { query: "" }, { results: [] });
    const blank = toolCall("lookup", {}, "plain");
    const code = "```python\nprint(42)\n```";
    setup("/c/work", {
      extra: [
        {
          ...chat("work", "Research", 1000),
          messages: [
            message(1, "user", "Find news", {
              attachments: [
                {
                  id: 7,
                  filename: "notes.txt",
                  kind: "text",
                  mime: "text/plain",
                  size: 10,
                },
                {
                  id: 8,
                  filename: "photo.png",
                  kind: "image",
                  mime: "image/png",
                  size: 10,
                },
              ],
            }),
            message(2, "assistant", "", {
              thinking: "Let me search",
              thinkingStartedAt: 1000,
              thinkingEndedAt: 4000,
              toolCalls: [search, fetch, other, unnamed, blank],
            }),
            message(3, "assistant", code),
          ],
        },
      ],
    });
    await waitFor(() =>
      expect(document.querySelector("[data-role='tool-calls']")).not.toBeNull(),
    );
    await waitFor(() =>
      expect(document.querySelector("[data-code-block-header]")).not.toBeNull(),
    );
    const results = [search, fetch, unnamed].map((call) =>
      JSON.stringify(JSON.parse(call.result!), null, 2),
    );
    expect(
      untranslatedTexts(document.body, [
        ...HISTORY,
        "Research",
        "Find news",
        "notes.txt",
        "photo.png",
        "Let me search",
        "latest ollama",
        "https://ollama.com/blog",
        "get_weather",
        "Paris",
        "timeout",
        "lookup",
        "plain",
        "python",
        "print(42)",
        ...results,
      ]),
    ).toEqual([]);
  });

  it("chat parts: error and its actions, download, loading, lost server", () => {
    usePseudoLanguage();
    const ERROR = "web search requires an Ollama account";
    render(
      <div>
        <ErrorMessage message={ERROR}>
          <WebSearchErrorActions onSignIn={vi.fn()} onRetry={vi.fn()} />
        </ErrorMessage>
        <ErrorMessage message={ERROR}>
          <WebSearchErrorActions
            onSignIn={vi.fn()}
            onRetry={vi.fn()}
            signedIn
          />
        </ErrorMessage>
        <Downloading completed={1.2 * GIB} total={4.7 * GIB} />
        <LoadingModel />
        <Thinking thinking="hmm" startTime={new Date(0)} />
        <Thinking
          thinking="hmm"
          startTime={new Date(0)}
          endTime={new Date(500)}
        />
        <ServerLostBanner onRetry={() => Promise.resolve()} />
        <ServerLostBanner onRetry={() => Promise.resolve()} starting />
        <ServerLostBanner onRetry={() => Promise.resolve()} failed />
        <CommandBlock command="ollama serve" />
        <ExternalLink href="https://ollama.com">ollama.com</ExternalLink>
        <DropOverlay />
        <ConfirmDialog message="Delete?" onCancel={vi.fn()} />
      </div>,
    );
    const allow = [
      ERROR,
      "hmm",
      "ollama serve",
      "https://ollama.com/",
      "ollama.com",
      "Delete?",
    ];
    expect(untranslatedTexts(document.body, allow)).toEqual([]);
    // Menu du lien.
    fireEvent.contextMenu(screen.getByText("ollama.com"));
    expect(screen.getAllByRole("menuitem")).toHaveLength(2);
    expect(untranslatedTexts(document.body, allow)).toEqual([]);
  });

  it("link notices: browser not opened, link not copied", async () => {
    usePseudoLanguage();
    mockIPC(() => {
      throw { kind: "system", message: "xdg-open failed" };
    });
    onTestFinished(() => clearMocks());
    render(<LinkErrorNotice />);
    await act(() => openLink("https://ollama.com/library"));
    await screen.findByRole("alert");
    const allow = ["xdg-open failed", "https://ollama.com/library"];
    expect(untranslatedTexts(document.body, allow)).toEqual([]);
    act(() => reportCopyFailure("https://ollama.com/library"));
    expect(screen.getByRole("alert").textContent).toContain(
      "https://ollama.com/library",
    );
    expect(untranslatedTexts(document.body, allow)).toEqual([]);
  });

  it("composer files: kept, staged, being read, refused, unreadable", () => {
    usePseudoLanguage();
    render(
      <ComposerFiles
        kept={[
          {
            id: 1,
            filename: "old.png",
            kind: "image",
            mime: "image/png",
            size: 1,
          },
        ]}
        files={[
          {
            id: "a",
            filename: "notes.md",
            kind: "text",
            mime: "text/markdown",
            size: 1,
          },
        ]}
        reading={[{ key: 1, filename: "big.pdf" }]}
        errors={[
          { filename: "app.exe", error: "File type not supported" },
          { filename: "lost.txt", error: null },
        ]}
        visionMissing
        onRemove={vi.fn()}
        onRemoveError={vi.fn()}
      />,
    );
    expect(
      untranslatedTexts(document.body, [
        "old.png",
        "notes.md",
        "big.pdf",
        "app.exe",
        "lost.txt",
        // Raison donnée par le moteur (traduite par lui).
        "File type not supported",
      ]),
    ).toEqual([]);
  });

  it("server screens: every state, other address, browser failure", async () => {
    usePseudoLanguage();
    const user = { user: true, enabled: false };
    const system = { user: false, enabled: false };
    const statuses = [
      status("notInstalled"),
      status("serviceStopped", { service: user }),
      status("serviceStopped", { service: system }),
      status("noService", { hostEnv: "127.0.0.1:11500" }),
      status("noService"),
      status("noService", { url: "http://10.0.0.2:11434" }),
      status("serviceFailed", { service: user }),
      status("serviceStarting", { service: system }),
      status("notResponding", { httpStatus: 502 }),
      status("notResponding", { service: system }),
      status("notResponding"),
      status("remoteUnreachable", { httpStatus: 502 }),
      status("remoteUnreachable"),
      status("connectionFailed", { url: "https://localhost:11434" }),
      status("connectionFailed", { detail: "tls: bad record" }),
      status("notOllama", { httpStatus: 404 }),
      status("notOllama"),
      status("accessDenied", { httpStatus: 403 }),
      status("running", { version: "0.35.1" }),
      status("noService", { alternativeUrl: "http://127.0.0.1:11500" }),
    ];
    const allow = [
      "http://localhost:11434",
      "https://localhost:11434",
      "http://10.0.0.2:11434",
      "http://127.0.0.1:11500",
      "0.35.1",
      "tls: bad record",
      "browser missing",
    ];
    for (const current of statuses) {
      const { unmount } = render(
        <ServerStatusScreen
          status={current}
          savedUrl=""
          isChecking={false}
          onRetry={vi.fn()}
          onSaveAddress={vi.fn()}
          onOpen={() => Promise.reject(new Error("browser missing"))}
        />,
      );
      // Commandes à copier : telles quelles, dans toutes les langues.
      const commands = [...document.querySelectorAll("code")].map(
        (code) => code.textContent ?? "",
      );
      const link = screen.queryByRole("link");
      if (link) {
        await userEvent.click(link);
        await screen.findByRole("alert");
      }
      expect(
        untranslatedTexts(document.body, [...allow, ...commands]),
        `${current.state}`,
      ).toEqual([]);
      unmount();
    }
    // Vérification en cours : « Checking… ».
    render(
      <ServerStatusScreen
        status={status("notInstalled")}
        savedUrl=""
        isChecking
        onRetry={vi.fn()}
        onSaveAddress={vi.fn()}
        onOpen={vi.fn()}
      />,
    );
    const commands = [...document.querySelectorAll("code")].map(
      (code) => code.textContent ?? "",
    );
    expect(untranslatedTexts(document.body, [...allow, ...commands])).toEqual(
      [],
    );
  });

  it("server gate: first check, then a failed check", async () => {
    usePseudoLanguage();
    let answer: () => Promise<unknown> = () => new Promise(() => {});
    mockIPC((command) => {
      if (command === "server_status") return answer();
      if (command === "get_settings") return { serverUrl: "" };
      throw new Error(`unexpected command ${command}`);
    });
    onTestFinished(() => clearMocks());
    const client = new QueryClient({
      defaultOptions: { queries: { networkMode: "always", retry: false } },
    });
    const { unmount } = render(
      <QueryClientProvider client={client}>
        <ServerGate>
          <p>app</p>
        </ServerGate>
      </QueryClientProvider>,
    );
    await screen.findByRole("status");
    expect(untranslatedTexts(document.body)).toEqual([]);
    unmount();

    answer = () => Promise.reject({ kind: "system", message: "no D-Bus" });
    render(
      <QueryClientProvider
        client={
          new QueryClient({
            defaultOptions: {
              queries: { networkMode: "always", retry: false },
            },
          })
        }
      >
        <ServerGate>
          <p>app</p>
        </ServerGate>
      </QueryClientProvider>,
    );
    await screen.findByRole("alert");
    expect(untranslatedTexts(document.body, ["no D-Bus"])).toEqual([]);
  });

  it("database problems: every kind, while checking", async () => {
    usePseudoLanguage();
    const kinds: DatabaseProblem["kind"][] = [
      "newer",
      "corrupt",
      "locked",
      "unavailable",
    ];
    const PATH = "/home/me/.local/share/chatonlm/chatonlm.db";
    const REASON = "ChatonLM can't create the folder /x";
    for (const kind of kinds) {
      const { unmount } = render(
        <DatabaseProblemScreen
          problem={{ kind, message: REASON, path: PATH }}
          onRecover={() => new Promise(() => {})}
          onRecovered={vi.fn()}
        />,
      );
      expect(untranslatedTexts(document.body, [PATH, REASON]), kind).toEqual(
        [],
      );
      const buttons = screen.queryAllByRole("button");
      if (buttons.length > 0) {
        await userEvent.click(buttons[0]);
        expect(untranslatedTexts(document.body, [PATH, REASON]), kind).toEqual(
          [],
        );
      }
      unmount();
    }
  });

  it("chat pages: loading, unknown chat, unreadable settings", async () => {
    usePseudoLanguage();
    const { simulator } = setup("/c/missing");
    await screen.findByText("⟦Chat not found⟧");
    expect(untranslatedTexts(document.body, HISTORY)).toEqual([]);
    simulator.handle("get_chat", () => {
      throw { kind: "database", message: "disk I/O error" };
    });
    cleanup();
    renderApp("/c/today");
    await screen.findByText("⟦Error loading chat⟧");
    expect(untranslatedTexts(document.body, HISTORY)).toEqual([]);
  });

  it("whole app: settings that can't be read", async () => {
    usePseudoLanguage();
    const simulator = installSimulator({
      handlers: { database_problem: () => null },
    });
    const backend = fakeBackend(simulator);
    backend.settingsErrors.get = {
      kind: "database",
      message: "disk I/O error",
    };
    vi.stubGlobal("matchMedia", (query: string) => ({
      matches: false,
      media: query,
      addEventListener: vi.fn(),
      removeEventListener: vi.fn(),
    }));
    onTestFinished(() => {
      vi.unstubAllGlobals();
    });
    const client = new QueryClient({
      defaultOptions: { queries: { networkMode: "always", retry: false } },
    });
    render(
      <QueryClientProvider client={client}>
        <App />
      </QueryClientProvider>,
    );
    await screen.findByRole("main");
    await waitFor(() =>
      expect(document.body.textContent).toContain("disk I/O error"),
    );
    expect(untranslatedTexts(document.body, ["disk I/O error"])).toEqual([]);
  });
});

describe("French and Spanish", () => {
  it("history in French: groups, date of an untitled chat, menu, plural", async () => {
    const { backend } = setup("/c/new", { downloads: 2, language: "fr" });
    await openSidebar();
    const titles = [...document.querySelectorAll("nav h3")].map(
      (title) => title.textContent,
    );
    expect(titles).toEqual(["Favoris", "Aujourd'hui", "Plus ancien"]);
    expect(row("untitled").textContent).toBe("03/10/2025 15:00:00");
    const indicator = await screen.findByTestId("download-indicator");
    expect(indicator.textContent).toBe("2 téléchargements · 25 %");
    expect(
      within(indicator).getByRole("progressbar", { name: "Téléchargements" }),
    ).toBeTruthy();

    fireEvent.contextMenu(row("fav"));
    const menu = await screen.findByRole("menu");
    expect(
      within(menu)
        .getAllByRole("menuitem")
        .map((item) => item.textContent),
    ).toEqual(["Retirer des favoris", "Renommer", "Supprimer"]);
    await userEvent.keyboard("{Escape}");

    // Un seul téléchargement : singulier.
    backend.hub.downloads.splice(1);
    await backend.hub.progress(backend.hub.downloads[0].model, 2 * GIB);
    await waitFor(() =>
      expect(indicator.textContent).toBe("1 téléchargement · 50 %"),
    );
  });

  it("download sizes in French: decimal comma, units, no grouping", () => {
    act(() => setLanguage("fr"));
    const { container } = render(
      <Downloading completed={1_261_646_644} total={5_046_586_573} />,
    );
    expect(screen.getByText("Téléchargement du modèle")).toBeTruthy();
    expect(container.querySelector(".text-sm")?.textContent).toBe(
      "1,2 Go / 4,7 Go (25 %)",
    );
    cleanup();
    render(<Downloading completed={999 * 1024} total={1000 * 1024} />);
    expect(screen.getByRole("progressbar").getAttribute("aria-valuetext")).toBe(
      "999 Ko / 1000 Ko (99 %)",
    );
  });

  it("Spanish: thinking time, dialog buttons, server advice", () => {
    act(() => setLanguage("es"));
    render(
      <div>
        <Thinking
          thinking="hmm"
          startTime={new Date(0)}
          endTime={new Date(3000)}
        />
        <ConfirmDialog message="?" onCancel={vi.fn()} />
        <ServerStatusScreen
          status={status("notInstalled")}
          savedUrl=""
          isChecking={false}
          onRetry={vi.fn()}
          onSaveAddress={vi.fn()}
          onOpen={vi.fn()}
        />
      </div>,
    );
    expect(screen.getByText("Pensó durante 3,0 segundos")).toBeTruthy();
    expect(
      within(screen.getByRole("alertdialog"))
        .getAllByRole("button")
        .map((button) => button.textContent),
    ).toEqual(["Cancelar", "Aceptar"]);
    expect(screen.getByText("Ollama no está instalado")).toBeTruthy();
    expect(screen.getByRole("button", { name: "Reintentar" })).toBeTruthy();
  });

  it("changing the language keeps the draft and translates what is shown", async () => {
    setup("/c/new");
    const field = await waitFor(() => composer());
    await userEvent.type(field, "Hola");
    expect(field.placeholder).toBe("Send a message");
    act(() => setLanguage("es"));
    // Même champ (pas de nouveau montage), brouillon gardé.
    expect(screen.getByLabelText("Mensaje")).toBe(field);
    expect(field.value).toBe("Hola");
    expect(field.placeholder).toBe("Escribe un mensaje");
    expect(screen.getByRole("link", { name: "Nuevo chat" })).toBeTruthy();
  });

  it("an unreadable file follows the language; the engine's reason is kept", () => {
    render(
      <ComposerFiles
        files={[]}
        reading={[]}
        errors={[
          { filename: "app.exe", error: "File type not supported" },
          { filename: "lost.txt", error: null },
        ]}
        visionMissing={false}
        onRemove={vi.fn()}
        onRemoveError={vi.fn()}
      />,
    );
    const reasons = () =>
      screen
        .getAllByTestId("composer-file-error")
        .map((chip) => chip.querySelector(".text-xs")?.textContent);
    expect(reasons()).toEqual([
      "• File type not supported",
      "• Error reading file",
    ]);
    act(() => setLanguage("fr"));
    expect(reasons()).toEqual([
      "• File type not supported",
      "• Erreur de lecture du fichier",
    ]);
    expect(
      screen.getByRole("button", { name: "Retirer lost.txt" }),
    ).toBeTruthy();
  });
});
