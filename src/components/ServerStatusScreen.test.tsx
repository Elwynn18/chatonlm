import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { ServerState } from "@/bindings/ServerState";
import type { ServerStatus } from "@/bindings/ServerStatus";
import { setLanguage, translator } from "@/i18n";
import type { OllamaRuntime } from "@/bindings/OllamaRuntime";
import type { OllamaSource } from "@/bindings/OllamaSource";
import type { ServiceStart } from "@/bindings/ServiceStart";
import { setPlatform } from "@/lib/platform";
import { adviceFor } from "@/lib/serverAdvice";
import { machineRuntime } from "@/test/backend";
import { untranslatedTexts, usePseudoLanguage } from "@/test/i18n";
import { ServerStatusScreen } from "./ServerStatusScreen";

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

const URL_LOCAL = "http://localhost:11434";

/** Textes de l'interface en anglais (langue source). */
const en = translator("en");

function status(state: ServerState, extra: Partial<ServerStatus> = {}) {
  return {
    url: URL_LOCAL,
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
  } satisfies ServerStatus;
}

const systemService = { user: false, enabled: true };

function renderScreen(value: ServerStatus, savedUrl = "") {
  const props = {
    onRetry: vi.fn(),
    onSaveAddress: vi.fn<(url: string) => Promise<void>>(() =>
      Promise.resolve(),
    ),
    onOpen: vi.fn<(url: string) => Promise<void>>(() => Promise.resolve()),
  };
  render(
    <ServerStatusScreen
      status={value}
      savedUrl={savedUrl}
      isChecking={false}
      {...props}
    />,
  );
  return props;
}

const commands = () =>
  [...document.querySelectorAll("code")].map((code) => code.textContent);

describe("advice for each state", () => {
  // Titre, commandes proposées et lien de téléchargement, en littéraux.
  const cases: [string, ServerStatus, string, string[], boolean][] = [
    [
      "not installed",
      status("notInstalled"),
      "Ollama is not installed",
      ["curl -fsSL https://ollama.com/install.sh | sh"],
      true,
    ],
    [
      "stopped, enabled service",
      status("serviceStopped", { service: systemService }),
      "Ollama is not running",
      ["sudo systemctl start ollama"],
      false,
    ],
    [
      "stopped, disabled service",
      status("serviceStopped", { service: { user: false, enabled: false } }),
      "Ollama is not running",
      ["sudo systemctl start ollama", "sudo systemctl enable ollama"],
      false,
    ],
    [
      "stopped user service",
      status("serviceStopped", { service: { user: true, enabled: false } }),
      "Ollama is not running",
      ["systemctl --user start ollama", "systemctl --user enable ollama"],
      false,
    ],
    [
      "installed without service",
      status("noService"),
      "Ollama is not running",
      ["ollama serve"],
      false,
    ],
    [
      "other port",
      status("noService", {
        url: "http://127.0.0.1:11435",
        hostEnv: "127.0.0.1:11435",
      }),
      "Ollama is not running at this address",
      ["OLLAMA_HOST=127.0.0.1:11435 ollama serve"],
      false,
    ],
    [
      "address that ollama serve can't listen on",
      status("noService", { url: "https://localhost:8443" }),
      "Ollama is not running at this address",
      [],
      false,
    ],
    [
      "failed service",
      status("serviceFailed", { service: systemService }),
      "Ollama failed to start",
      ["systemctl status ollama", "journalctl -e -u ollama"],
      false,
    ],
    [
      "starting service",
      status("serviceStarting", { service: systemService }),
      "Ollama is starting",
      ["journalctl -e -u ollama"],
      false,
    ],
    [
      "no answer, with a service",
      status("notResponding", { service: systemService }),
      "Ollama is not responding",
      ["journalctl -e -u ollama"],
      false,
    ],
    [
      "no answer, without a service",
      status("notResponding"),
      "Ollama is not responding",
      [],
      false,
    ],
    [
      "remote server",
      status("remoteUnreachable", { url: "http://192.168.1.2:11434" }),
      "Can't reach the Ollama server",
      [],
      false,
    ],
    [
      "something else listens",
      status("connectionFailed", { detail: "connection failed: tls" }),
      "Can't connect to Ollama",
      [],
      false,
    ],
    [
      "not Ollama",
      status("notOllama", { httpStatus: 404 }),
      "This is not an Ollama server",
      [],
      false,
    ],
    [
      "access refused",
      status("accessDenied", { httpStatus: 403 }),
      "Access denied",
      [],
      false,
    ],
  ];

  it.each(cases)("%s", (_name, value, title, expected, download) => {
    renderScreen(value);
    expect(screen.getByRole("heading").textContent).toBe(title);
    expect(commands()).toEqual(expected);
    expect(
      screen.queryByRole("link", { name: "Download from ollama.com" }) !== null,
    ).toBe(download);
  });

  it("descriptions name the cause", () => {
    expect(
      adviceFor(en, status("notResponding", { httpStatus: 502 })).description,
    ).toBe(
      "http://localhost:11434 responded with HTTP 502: the Ollama server behind it is not responding.",
    );
    expect(
      adviceFor(
        en,
        status("connectionFailed", { url: "https://localhost:11434" }),
      ).description,
    ).toContain("Ollama serves plain HTTP");
    expect(
      adviceFor(en, status("notOllama", { httpStatus: 404 })).description,
    ).toBe(
      "http://localhost:11434 responded with HTTP 404, but it isn't an Ollama server. Check the server address below.",
    );
    expect(
      adviceFor(
        en,
        status("remoteUnreachable", { url: "http://192.168.1.2:11434" }),
      ).description,
    ).toContain("OLLAMA_HOST=0.0.0.0");
  });

  it("shows the technical detail of a failure", () => {
    renderScreen(
      status("connectionFailed", { detail: "connection failed: tls" }),
    );
    expect(screen.getByText("connection failed: tls")).toBeTruthy();
  });
});

describe("ServerStatusScreen", () => {
  it("opens ollama.com/download, and shows the address if that fails", async () => {
    const props = renderScreen(status("notInstalled"));
    const link = screen.getByRole("link", { name: "Download from ollama.com" });
    expect(link.getAttribute("href")).toBe("https://ollama.com/download");
    await userEvent.click(link);
    expect(props.onOpen).toHaveBeenCalledWith("https://ollama.com/download");

    // Pas de menu de WebKit (« Download Linked File »).
    expect(fireEvent.contextMenu(link)).toBe(false);

    props.onOpen.mockRejectedValueOnce(new Error("xdg-open exited with 3"));
    await userEvent.click(link);
    expect((await screen.findByRole("alert")).textContent).toBe(
      "Couldn't open the browser (xdg-open exited with 3). Go to https://ollama.com/download",
    );
  });

  it("retry button checks again", async () => {
    const { onRetry } = renderScreen(status("serviceStopped"));
    await userEvent.click(screen.getByRole("button", { name: "Retry" }));
    expect(onRetry).toHaveBeenCalledOnce();
  });

  it("says Checking… only while a requested check runs", () => {
    render(
      <ServerStatusScreen
        status={status("serviceStopped")}
        savedUrl=""
        isChecking
        onRetry={vi.fn()}
        onSaveAddress={vi.fn()}
        onOpen={vi.fn()}
      />,
    );
    const button = screen.getByRole("button", { name: "Checking…" });
    expect((button as HTMLButtonElement).disabled).toBe(true);
  });

  it("offers an address where Ollama answers, and shows a failure", async () => {
    const { onSaveAddress } = renderScreen(
      status("noService", { alternativeUrl: "http://127.0.0.1:11434" }),
    );
    expect(
      screen.getByText("Ollama is running at http://127.0.0.1:11434."),
    ).toBeTruthy();
    onSaveAddress.mockRejectedValueOnce(new Error("database is locked"));
    await userEvent.click(screen.getByText("Use this address"));
    expect(onSaveAddress).toHaveBeenCalledWith("http://127.0.0.1:11434");
    expect((await screen.findByRole("alert")).textContent).toBe(
      "database is locked",
    );
  });

  it("address field: saved value, effective one as placeholder", async () => {
    const { onSaveAddress } = renderScreen(status("notResponding"));
    const input = screen.getByLabelText("Server address") as HTMLInputElement;
    expect(input.value).toBe("");
    expect(input.placeholder).toBe(URL_LOCAL);
    // Rien à enregistrer tant que la valeur n'a pas changé.
    const save = screen.getByRole("button", { name: "Save" });
    expect((save as HTMLButtonElement).disabled).toBe(true);
    await userEvent.type(input, "{Enter}");
    expect(onSaveAddress).not.toHaveBeenCalled();
  });

  it("saves the typed address and shows backend errors", async () => {
    const { onSaveAddress } = renderScreen(
      status("notResponding"),
      "http://192.168.1.2:11434",
    );
    onSaveAddress.mockRejectedValueOnce(new Error("invalid server address"));
    const input = screen.getByLabelText("Server address");
    expect((input as HTMLInputElement).value).toBe("http://192.168.1.2:11434");
    await userEvent.clear(input);
    await userEvent.type(input, "ftp://example.com{Enter}");
    expect(onSaveAddress).toHaveBeenCalledWith("ftp://example.com");
    const alert = await screen.findByRole("alert");
    expect(alert.textContent).toBe("invalid server address");
    expect(input.getAttribute("aria-invalid")).toBe("true");
    expect(input.getAttribute("aria-describedby")).toBe(alert.id);
  });

  it("copies a command, with a fallback, and says when it fails", async () => {
    const user = userEvent.setup();
    const writeText = vi.spyOn(navigator.clipboard, "writeText");
    renderScreen(status("serviceStopped", { service: systemService }));
    const copy = screen.getByRole("button", {
      name: "Copy command to clipboard",
    });
    await user.click(copy);
    expect(writeText).toHaveBeenCalledWith("sudo systemctl start ollama");
    // Libellé visible dans le bouton, annonce dans une zone sœur.
    await vi.waitFor(() => expect(copy.textContent).toBe("Copied"));
    const live = copy.nextElementSibling as HTMLElement;
    expect(live.getAttribute("role")).toBe("status");
    expect(live.textContent).toBe("Copied");
    expect(copy.querySelector("[aria-live]")).toBeNull();

    // API refusée, puis ancienne commande refusée aussi.
    writeText.mockRejectedValueOnce(new Error("not allowed"));
    const execCommand = document.execCommand;
    document.execCommand = vi.fn(() => false);
    try {
      await user.click(copy);
      await vi.waitFor(() => expect(copy.textContent).toBe("Copy failed"));
      expect(copy.nextElementSibling?.textContent).toBe("Copy failed");
      expect(window.getSelection()?.toString()).toBe(
        "sudo systemctl start ollama",
      );
    } finally {
      document.execCommand = execCommand;
    }
  });
});

describe("Start Ollama and the built-in Ollama", () => {
  const BUILTIN_URL = "http://127.0.0.1:47811";
  const LOG = "/home/user/.local/share/chatonlm/logs/ollama.log";

  /** État d'un paquet qui livre l'Ollama intégré. */
  function runtimeStatus(
    state: ServerState,
    extra: Partial<ServerStatus> = {},
  ): ServerStatus {
    return status(state, { builtinAvailable: true, ...extra });
  }

  /** L'intégré sert (choisi faute d'autre, par défaut) et démarre. */
  function runtime(fields: Partial<OllamaRuntime> = {}): OllamaRuntime {
    return machineRuntime({
      kind: "builtin",
      url: BUILTIN_URL,
      servedVersion: null,
      builtinState: "starting",
      logPath: LOG,
      ...fields,
    });
  }

  function renderWith(
    value: ServerStatus,
    {
      savedUrl = "",
      current = null,
    }: { savedUrl?: string; current?: OllamaRuntime | null } = {},
  ) {
    const props = {
      onRetry: vi.fn(),
      onSaveAddress: vi.fn<(url: string) => Promise<void>>(() =>
        Promise.resolve(),
      ),
      onOpen: vi.fn<(url: string) => Promise<void>>(() => Promise.resolve()),
      onStart: vi.fn<() => Promise<ServiceStart>>(() =>
        Promise.resolve({ kind: "started" }),
      ),
      onOpenLog: vi.fn<() => Promise<void>>(() => Promise.resolve()),
      onSetSource: vi.fn<(source: OllamaSource) => Promise<void>>(() =>
        Promise.resolve(),
      ),
    };
    render(
      <ServerStatusScreen
        status={value}
        savedUrl={savedUrl}
        isChecking={false}
        runtime={current}
        {...props}
      />,
    );
    return props;
  }

  /**
   * Focus rendu à <body>, comme le fait un moteur web pour un bouton
   * désactivé (jsdom le laisse au bouton).
   */
  function loseFocus() {
    act(() => {
      const other = document.createElement("input");
      document.body.append(other);
      other.focus();
      other.remove();
    });
    expect(document.activeElement).toBe(document.body);
  }

  /** Comme renderWith, avec de quoi rendre de nouveau l'écran. */
  function renderWithRerender(value: ServerStatus) {
    const props = {
      onRetry: vi.fn(),
      onSaveAddress: vi.fn<(url: string) => Promise<void>>(() =>
        Promise.resolve(),
      ),
      onOpen: vi.fn<(url: string) => Promise<void>>(() => Promise.resolve()),
      onStart: vi.fn<() => Promise<ServiceStart>>(() =>
        Promise.resolve({ kind: "started" }),
      ),
      onOpenLog: vi.fn<() => Promise<void>>(() => Promise.resolve()),
      onSetSource: vi.fn<(source: OllamaSource) => Promise<void>>(() =>
        Promise.resolve(),
      ),
    };
    const { rerender } = render(
      <ServerStatusScreen
        status={value}
        savedUrl=""
        isChecking={false}
        {...props}
      />,
    );
    return { props, rerender };
  }

  /** Promesse tenue par le test. */
  function held<T = void>() {
    let release: (value: T) => void = () => {};
    let reject: (error: unknown) => void = () => {};
    const promise = new Promise<T>((resolve, fail) => {
      release = resolve;
      reject = fail;
    });
    return { promise, release, reject };
  }

  it("starts a stopped service, says Starting… meanwhile, and shows why it failed", async () => {
    const props = renderWith(
      runtimeStatus("serviceStopped", {
        service: systemService,
        builtinAvailable: false,
      }),
    );
    expect(
      screen.getByText(
        "Ollama is installed, but its service is stopped. Click Start Ollama, or start it in a terminal:",
      ),
    ).toBeTruthy();
    // La commande reste proposée, Retry aussi.
    expect(commands()).toEqual(["sudo systemctl start ollama"]);
    expect(screen.getByRole("button", { name: "Retry" })).toBeTruthy();

    const first = held<ServiceStart>();
    props.onStart.mockReturnValueOnce(first.promise);
    const start = screen.getByRole("button", { name: "Start Ollama" });
    await userEvent.click(start);
    expect(props.onStart).toHaveBeenCalledOnce();
    expect(start.textContent).toBe("Starting…");
    expect((start as HTMLButtonElement).disabled).toBe(true);
    await act(async () => first.reject(new Error("D-Bus is not running")));
    expect((await screen.findByRole("alert")).textContent).toBe(
      "Couldn't start Ollama: D-Bus is not running",
    );
    // Chaque issue de systemd, dite ; un nouvel essai efface la précédente.
    const outcomes: [ServiceStart, string | null][] = [
      [
        { kind: "notAuthorized" },
        "Authentication failed. Ollama was not started.",
      ],
      [
        { kind: "noService" },
        "No Ollama service was found. Start Ollama in a terminal with ollama serve.",
      ],
      [
        { kind: "failed", detail: "Unit ollama.service is masked." },
        "Couldn't start Ollama: Unit ollama.service is masked.",
      ],
      [{ kind: "started" }, null],
    ];
    for (const [outcome, text] of outcomes) {
      props.onStart.mockResolvedValueOnce(outcome);
      await userEvent.click(start);
      await waitFor(() => expect(start.textContent).toBe("Start Ollama"));
      expect(screen.queryByRole("alert")?.textContent ?? null).toBe(text);
    }
    expect(props.onStart).toHaveBeenCalledTimes(5);
  });

  it("on Windows, Start Ollama opens Ollama's app: no service, no systemctl", async () => {
    setPlatform("windows");
    const props = renderWith(
      runtimeStatus("serviceStopped", {
        service: { user: true, enabled: true },
        builtinAvailable: false,
      }),
    );
    expect(
      screen.getByText(
        "Ollama is installed, but it isn't running. Click Start Ollama, or open Ollama from the Start menu.",
      ),
    ).toBeTruthy();
    expect(commands()).toEqual([]);
    const start = screen.getByRole("button", { name: "Start Ollama" });
    props.onStart.mockResolvedValueOnce({ kind: "noService" });
    await userEvent.click(start);
    expect((await screen.findByRole("alert")).textContent).toBe(
      "Ollama's app was not found. Start Ollama in a terminal with ollama serve.",
    );
    props.onStart.mockResolvedValueOnce({
      kind: "failed",
      detail: "Ollama is being updated. Try again in a moment.",
    });
    await userEvent.click(start);
    await waitFor(() =>
      expect(screen.getByRole("alert").textContent).toBe(
        "Couldn't start Ollama: Ollama is being updated. Try again in a moment.",
      ),
    );
  });

  it("offers Start Ollama only for a stopped service", () => {
    renderWith(runtimeStatus("serviceStopped", { builtinAvailable: false }));
    expect(screen.queryByRole("button", { name: "Start Ollama" })).toBeNull();
    cleanup();
    renderWith(
      runtimeStatus("serviceFailed", {
        service: systemService,
        builtinAvailable: false,
      }),
    );
    expect(screen.queryByRole("button", { name: "Start Ollama" })).toBeNull();
  });

  it("offers the built-in Ollama instead of an installed Ollama that is stopped", async () => {
    const props = renderWith(
      runtimeStatus("serviceStopped", { service: systemService }),
    );
    const link = screen.getByRole("button", {
      name: "Use the built-in Ollama instead",
    });
    const first = held();
    props.onSetSource.mockReturnValueOnce(first.promise);
    await userEvent.click(link);
    expect(props.onSetSource).toHaveBeenCalledWith("builtin");
    expect(
      screen.getByRole<HTMLButtonElement>("button", { name: "Switching…" })
        .disabled,
    ).toBe(true);
    await act(async () => first.reject(new Error("database is locked")));
    expect((await screen.findByRole("alert")).textContent).toBe(
      "Couldn't switch Ollama: database is locked",
    );
  });

  it.each([
    ["installed without service", runtimeStatus("noService"), "instead"],
    [
      "failed service",
      runtimeStatus("serviceFailed", { service: systemService }),
      "instead",
    ],
    [
      "something else at the default address",
      runtimeStatus("notOllama", { httpStatus: 404 }),
      "instead",
    ],
    [
      "remote address",
      runtimeStatus("remoteUnreachable", {
        url: "http://192.168.1.2:11434",
        explicitAddress: true,
      }),
      "use",
    ],
    [
      "address set, nothing listening",
      runtimeStatus("noService", {
        url: "http://127.0.0.1:11500",
        hostEnv: "127.0.0.1:11500",
        explicitAddress: true,
      }),
      "use",
    ],
    [
      "address set, service stopped",
      runtimeStatus("serviceStopped", {
        service: systemService,
        explicitAddress: true,
      }),
      "use",
    ],
  ])("%s: link to the built-in Ollama", (_name, value, kind) => {
    renderWith(value);
    expect(
      screen.queryByRole("button", {
        name: "Use the built-in Ollama instead",
      }) !== null,
    ).toBe(kind === "instead");
    expect(
      screen.queryByRole("button", { name: "Use the built-in Ollama" }) !==
        null,
    ).toBe(kind === "use");
  });

  it("no link without a built-in Ollama, nor while it serves", () => {
    renderWith(runtimeStatus("serviceStopped", { builtinAvailable: false }));
    expect(screen.queryByRole("button", { name: /built-in/ })).toBeNull();
    cleanup();
    renderWith(runtimeStatus("builtinFailed", { builtin: true }), {
      current: runtime(),
    });
    expect(screen.queryByRole("button", { name: /built-in/ })).toBeNull();
  });

  it("while the built-in Ollama starts: no Retry, the address of Ollama on this computer", () => {
    renderWith(
      runtimeStatus("builtinStarting", { builtin: true, url: BUILTIN_URL }),
      { current: runtime() },
    );
    expect(screen.getByRole("heading").textContent).toBe("Starting Ollama…");
    expect(screen.getByRole("status").textContent).toContain(
      "ChatonLM is starting its built-in Ollama.",
    );
    expect(screen.queryByRole("button", { name: "Retry" })).toBeNull();
    expect(commands()).toEqual([]);
    // Choix automatique : une adresse peut encore être saisie, et l'adresse
    // privée de l'intégré n'y est pas montrée.
    const input = screen.getByLabelText<HTMLInputElement>("Server address");
    expect(input.placeholder).toBe("http://localhost:11434");
    expect(screen.queryByText(BUILTIN_URL)).toBeNull();
  });

  it("shows the log of the built-in Ollama when it failed, with Retry", async () => {
    const props = renderWith(
      runtimeStatus("builtinFailed", {
        builtin: true,
        url: BUILTIN_URL,
        logPath: LOG,
      }),
      { current: runtime() },
    );
    expect(screen.getByRole("heading").textContent).toBe(
      "The built-in Ollama failed to start",
    );
    expect(screen.getByText(LOG)).toBeTruthy();
    await userEvent.click(screen.getByRole("button", { name: "Retry" }));
    expect(props.onRetry).toHaveBeenCalledOnce();
    // Le journal s'ouvre ; un échec est dit.
    await userEvent.click(screen.getByRole("button", { name: "Open log" }));
    expect(props.onOpenLog).toHaveBeenCalledOnce();
    props.onOpenLog.mockRejectedValueOnce(new Error("xdg-open exited with 4"));
    await userEvent.click(screen.getByRole("button", { name: "Open log" }));
    expect((await screen.findByRole("alert")).textContent).toBe(
      "Couldn't open the log: xdg-open exited with 4",
    );
    // Choisi automatiquement (rien d'autre trouvé) : pas de retour proposé.
    expect(screen.queryByRole("button", { name: /^Use / })).toBeNull();
  });

  it("speaks French and Spanish", () => {
    act(() => setLanguage("fr"));
    renderWith(runtimeStatus("serviceStopped", { service: systemService }));
    expect(
      screen.getByRole("button", { name: "Démarrer Ollama" }),
    ).toBeTruthy();
    expect(
      screen.getByRole("button", { name: "Utiliser plutôt l'Ollama intégré" }),
    ).toBeTruthy();
    cleanup();
    act(() => setLanguage("es"));
    renderWith(
      runtimeStatus("builtinFailed", {
        builtin: true,
        url: BUILTIN_URL,
        logPath: LOG,
      }),
      { current: runtime({ source: "builtin" }) },
    );
    expect(screen.getByRole("heading").textContent).toBe(
      "El Ollama integrado no pudo iniciarse",
    );
    expect(
      screen.getByRole("button", { name: "Usar el Ollama de este equipo" }),
    ).toBeTruthy();
    expect(screen.getByRole("button", { name: "Abrir registro" })).toBeTruthy();
  });

  it("writes every new text from the catalogs, errors included", async () => {
    usePseudoLanguage();
    const allow = [URL_LOCAL, BUILTIN_URL, LOG, "denied", "locked", "no xdg"];
    const cases: [ServerStatus, OllamaRuntime | null][] = [
      [runtimeStatus("serviceStopped", { service: systemService }), null],
      [runtimeStatus("remoteUnreachable"), null],
      [runtimeStatus("builtinStarting", { builtin: true }), runtime()],
      [
        runtimeStatus("builtinFailed", { builtin: true, logPath: LOG }),
        runtime({ source: "builtin" }),
      ],
    ];
    for (const [value, current] of cases) {
      const props = renderWith(value, { current });
      props.onStart.mockRejectedValue(new Error("denied"));
      props.onOpenLog.mockRejectedValue(new Error("no xdg"));
      props.onSetSource.mockRejectedValue(new Error("locked"));
      // Chaque bouton cliqué, pour faire paraître les erreurs.
      const buttons = screen.queryAllByRole("button", {
        name: /Start|Use|Open/,
      });
      for (const button of buttons) await userEvent.click(button);
      if (buttons.length > 0) {
        expect(await screen.findAllByRole("alert")).toHaveLength(
          buttons.length,
        );
      }
      const commands = [...document.querySelectorAll("code")].map(
        (code) => code.textContent ?? "",
      );
      expect(
        untranslatedTexts(document.body, [...allow, ...commands]),
        value.state,
      ).toEqual([]);
      cleanup();
    }
  });

  it("goes back from the chosen built-in Ollama, without the address field", async () => {
    const props = renderWith(
      runtimeStatus("builtinFailed", { builtin: true, url: BUILTIN_URL }),
      { current: runtime({ source: "builtin" }) },
    );
    expect(screen.queryByLabelText("Server address")).toBeNull();
    await userEvent.click(
      screen.getByRole("button", { name: "Use Ollama on this computer" }),
    );
    expect(props.onSetSource).toHaveBeenCalledWith("auto");
    cleanup();
    // Adresse de la machine donnée par OLLAMA_HOST (rien d'enregistré) : le
    // lien la nomme, ce n'est pas cet ordinateur.
    renderWith(
      runtimeStatus("builtinStarting", { builtin: true, url: BUILTIN_URL }),
      {
        current: runtime({
          source: "builtin",
          machineUrl: "http://192.168.1.2:11434",
          machineLocal: false,
        }),
      },
    );
    expect(
      screen.getByRole("button", {
        name: "Use the Ollama at http://192.168.1.2:11434",
      }),
    ).toBeTruthy();
  });

  /** Rendu de nouveau avec un autre état, mêmes props. */
  function rerenderWith(
    rerender: ReturnType<typeof render>["rerender"],
    props: ReturnType<typeof renderWith>,
    value: ServerStatus,
    current: OllamaRuntime | null = null,
  ) {
    rerender(
      <ServerStatusScreen
        status={value}
        savedUrl=""
        isChecking={false}
        runtime={current}
        {...props}
      />,
    );
  }

  it("moves the focus to the title after a switch, back to the link after a failure", async () => {
    const { props, rerender } = renderWithRerender(
      runtimeStatus("serviceStopped", { service: systemService }),
    );
    const link = screen.getByRole("button", {
      name: "Use the built-in Ollama instead",
    });
    const failed = held();
    props.onSetSource.mockReturnValueOnce(failed.promise);
    await userEvent.click(link);
    // Désactivé pendant la bascule : le moteur web peut lui retirer le focus.
    loseFocus();
    await act(async () => failed.reject(new Error("database is locked")));
    await screen.findByRole("alert");
    expect(document.activeElement).toBe(link);

    const switched = held();
    props.onSetSource.mockReturnValueOnce(switched.promise);
    await userEvent.click(link);
    // La bascule faite, l'intégré démarre : le lien a disparu.
    rerenderWith(
      rerender,
      props,
      runtimeStatus("builtinStarting", { builtin: true, url: BUILTIN_URL }),
      runtime({ source: "builtin" }),
    );
    await act(async () => switched.release());
    const title = screen.getByRole("heading", { name: "Starting Ollama…" });
    expect(document.activeElement).toBe(title);
  });

  it("moves the focus to the title once Ollama is started", async () => {
    const { props, rerender } = renderWithRerender(
      runtimeStatus("serviceStopped", {
        service: systemService,
        builtinAvailable: false,
      }),
    );
    const start = screen.getByRole("button", { name: "Start Ollama" });
    const refused = held<ServiceStart>();
    props.onStart.mockReturnValueOnce(refused.promise);
    await userEvent.click(start);
    loseFocus();
    await act(async () => refused.release({ kind: "notAuthorized" }));
    await screen.findByRole("alert");
    expect(document.activeElement).toBe(start);

    const started = held<ServiceStart>();
    props.onStart.mockReturnValueOnce(started.promise);
    await userEvent.click(start);
    rerenderWith(
      rerender,
      props,
      runtimeStatus("serviceStarting", {
        service: systemService,
        builtinAvailable: false,
      }),
    );
    await act(async () => started.release({ kind: "started" }));
    expect(document.activeElement).toBe(
      screen.getByRole("heading", { name: "Ollama is starting" }),
    );
  });
});
