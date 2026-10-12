// Settings › General face au simulateur : ligne « Ollama » (lequel sert, sa
// version, passage à l'autre), adresse de l'intégré en lecture seule, état de
// l'intégré (démarrage, échec et journal), avis des modèles restés dans son
// dossier ; textes de chaque langue.

import {
  act,
  configure,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import type { Settings } from "@/bindings/Settings";
import { setLanguage } from "@/i18n";
import {
  BUILTIN_MODELS_DIR,
  BUILTIN_URL,
  type FakeBackend,
  fakeBackend,
  OLLAMA_LOG,
  SERVER_URL,
} from "@/test/backend";
import { untranslatedTexts, usePseudoLanguage } from "@/test/i18n";
import { renderApp } from "@/test/renderApp";
import { installSimulator } from "@/test/simulator";

configure({ asyncUtilTimeout: 5000 });
vi.setConfig({ testTimeout: 30_000 });

const GIB = 1024 ** 3;
const REMOTE = "http://192.168.1.2:11434";

function setup(
  prepare: (backend: FakeBackend) => void = () => {},
  settings: Partial<Settings> = {},
) {
  const simulator = installSimulator();
  const backend = fakeBackend(simulator);
  Object.assign(backend.settings, settings);
  backend.settings.effectiveServerUrl =
    backend.settings.serverUrl || SERVER_URL;
  prepare(backend);
  if (backend.runtime.kind === "builtin") {
    backend.settings.effectiveServerUrl = BUILTIN_URL;
  }
  const app = renderApp("/settings");
  return { simulator, backend, app };
}

/** L'intégré sert ; `source` : choisi (builtin) ou faute d'autre (auto). */
function builtinServes(source: "auto" | "builtin") {
  return (backend: FakeBackend) => {
    backend.runtime.source = source;
    backend.runtime.kind = "builtin";
  };
}

const ollamaRow = () => screen.findByText(/^(Built-in Ollama|Ollama )/);
const address = () => screen.getByLabelText<HTMLInputElement>("Server address");

describe("Ollama row", () => {
  it("says which Ollama serves, before the server address", async () => {
    setup();
    const row = await screen.findByText("Ollama 0.35.1 on this computer");
    expect(screen.getByText("Ollama")).toBeTruthy();
    expect(
      row.compareDocumentPosition(address()) & Node.DOCUMENT_POSITION_FOLLOWING,
    ).toBeTruthy();
    expect(
      screen.getByRole("button", { name: "Use the built-in Ollama" }),
    ).toBeTruthy();
    // L'adresse reste modifiable, avec l'aide habituelle.
    expect(address().readOnly).toBe(false);
  });

  it("names the address of an Ollama elsewhere", async () => {
    setup(() => {}, { serverUrl: REMOTE });
    expect(await screen.findByText(`Ollama 0.35.1 at ${REMOTE}`)).toBeTruthy();
  });

  it("follows a server address saved below", async () => {
    setup();
    await screen.findByText("Ollama 0.35.1 on this computer");
    await userEvent.type(address(), `${REMOTE}{Enter}`);
    expect(await screen.findByText(`Ollama 0.35.1 at ${REMOTE}`)).toBeTruthy();
  });

  it("offers nothing to switch to without a built-in Ollama", async () => {
    setup((backend) => {
      backend.runtime.builtinAvailable = false;
    });
    await screen.findByText("Ollama 0.35.1 on this computer");
    expect(screen.queryByRole("button", { name: /built-in/ })).toBeNull();
  });

  it("switches to the built-in Ollama: its version, its address read-only, Saved", async () => {
    const { simulator } = setup();
    await userEvent.click(
      await screen.findByRole("button", { name: "Use the built-in Ollama" }),
    );
    expect(await screen.findByText("Built-in Ollama 0.40.0")).toBeTruthy();
    expect(simulator.callsTo("set_ollama_source")).toEqual([
      { source: "builtin" },
    ]);
    expect(await screen.findByText("Saved")).toBeTruthy();
    await waitFor(() => expect(address().value).toBe(BUILTIN_URL));
    expect(address().readOnly).toBe(true);
    const description = screen.getByText(
      "Address of the built-in Ollama, chosen by ChatonLM. Only apps on this computer can reach it.",
    );
    expect(address().getAttribute("aria-describedby")).toBe(description.id);
    // Pas de bouton Save : rien à enregistrer.
    expect(screen.queryByRole("button", { name: "Save" })).toBeNull();
    // Et le retour.
    await userEvent.click(
      screen.getByRole("button", { name: "Use Ollama on this computer" }),
    );
    expect(
      await screen.findByText("Ollama 0.35.1 on this computer"),
    ).toBeTruthy();
    expect(simulator.callsTo("set_ollama_source")).toEqual([
      { source: "builtin" },
      { source: "auto" },
    ]);
    await waitFor(() => expect(address().readOnly).toBe(false));
  });

  it("goes back to the saved address, named on the button", async () => {
    setup(builtinServes("builtin"), { serverUrl: REMOTE });
    expect(
      await screen.findByRole("button", {
        name: `Use the Ollama at ${REMOTE}`,
      }),
    ).toBeTruthy();
  });

  it("goes back to the address of OLLAMA_HOST, not to this computer", async () => {
    setup((backend) => {
      builtinServes("builtin")(backend);
      // Rien dans les réglages : OLLAMA_HOST donne l'adresse de la machine.
      backend.runtime.machineUrl = REMOTE;
    });
    expect(
      await screen.findByRole("button", {
        name: `Use the Ollama at ${REMOTE}`,
      }),
    ).toBeTruthy();
    expect(
      screen.queryByRole("button", { name: "Use Ollama on this computer" }),
    ).toBeNull();
  });

  it("an address of this computer, even on another port, is this computer", async () => {
    setup(builtinServes("builtin"), { serverUrl: "http://127.0.0.1:11500" });
    expect(
      await screen.findByRole("button", {
        name: "Use Ollama on this computer",
      }),
    ).toBeTruthy();
  });

  it("says Switching… meanwhile, and why a switch failed", async () => {
    const { simulator, backend } = setup();
    let release: () => void = () => {};
    simulator.handle(
      "set_ollama_source",
      () => new Promise((resolve) => (release = () => resolve(null))),
    );
    await userEvent.click(
      await screen.findByRole("button", { name: "Use the built-in Ollama" }),
    );
    expect(
      screen.getByRole<HTMLButtonElement>("button", { name: "Switching…" })
        .disabled,
    ).toBe(true);
    await act(async () => release());
    await screen.findByRole("button", { name: "Use the built-in Ollama" });

    simulator.handle("set_ollama_source", () => {
      throw { kind: "database", message: "disk full" };
    });
    await userEvent.click(
      screen.getByRole("button", { name: "Use the built-in Ollama" }),
    );
    expect((await screen.findByRole("alert")).textContent).toBe(
      "Couldn't switch Ollama: disk full",
    );
    expect(backend.runtime.kind).toBe("machine");
  });

  it("built-in Ollama chosen for lack of another: said, no switch, address editable", async () => {
    setup(builtinServes("auto"));
    expect(await screen.findByText("Built-in Ollama 0.40.0")).toBeTruthy();
    expect(
      screen.getByText("No other Ollama was found on this computer."),
    ).toBeTruthy();
    expect(screen.queryByRole("button", { name: /^Use / })).toBeNull();
    expect(address().readOnly).toBe(false);
    // Jamais l'adresse privée de l'intégré (son port change à chaque
    // lancement) : celle qu'un champ vide donne, comme l'écran d'accueil.
    expect(address().value).toBe("");
    expect(address().placeholder).toBe("http://localhost:11434");
  });

  it("says when the built-in Ollama is starting, then its version", async () => {
    const { simulator, backend } = setup((backend) => {
      builtinServes("builtin")(backend);
      backend.serverState = "builtinStarting";
    });
    const starting = await screen.findByText("Starting Ollama…");
    expect(starting.getAttribute("role")).toBe("status");
    // Version de l'intégré connue avant qu'il réponde.
    expect(screen.getByText("Built-in Ollama 0.40.0")).toBeTruthy();
    // Toutes les lectures de la page faites (dossier de l'intégré compris) :
    // seule la relecture périodique peut voir qu'il est prêt.
    await waitFor(() =>
      expect(
        screen.getByLabelText<HTMLInputElement>("Model location").value,
      ).toBe(BUILTIN_MODELS_DIR),
    );
    const reads = simulator.callsTo("ollama_runtime").length;
    backend.serverState = "running";
    await waitFor(() =>
      expect(screen.queryByText("Starting Ollama…")).toBeNull(),
    );
    expect(simulator.callsTo("ollama_runtime").length).toBeGreaterThan(reads);
  });

  it("gives the log of the built-in Ollama when it failed, and restarts it", async () => {
    const { simulator, backend } = setup((backend) => {
      builtinServes("builtin")(backend);
      backend.serverState = "builtinFailed";
      backend.builtinState = "builtinStarting";
      backend.runtime.builtinFailure = "Ollama stopped 5 times in 2 minutes.";
    });
    const alert = await screen.findByRole("alert");
    expect([...alert.querySelectorAll("p")].map((p) => p.textContent)).toEqual([
      `The built-in Ollama failed to start. See its log: ${OLLAMA_LOG}`,
      "Ollama stopped 5 times in 2 minutes.",
    ]);
    // Le journal s'ouvre ; un échec est dit.
    await userEvent.click(screen.getByRole("button", { name: "Open log" }));
    expect(simulator.callsTo("open_builtin_log")).toHaveLength(1);
    backend.runtimeErrors.openLog = { kind: "system", message: "no viewer" };
    await userEvent.click(screen.getByRole("button", { name: "Open log" }));
    expect(
      await screen.findByText("Couldn't open the log: no viewer"),
    ).toBeTruthy();
    backend.runtimeErrors.restart = { kind: "system", message: "no binary" };
    await userEvent.click(screen.getByRole("button", { name: "Retry" }));
    expect(
      await screen.findByText("Couldn't start Ollama: no binary"),
    ).toBeTruthy();
    backend.runtimeErrors.restart = null;
    await userEvent.click(screen.getByRole("button", { name: "Retry" }));
    expect(await screen.findByText("Starting Ollama…")).toBeTruthy();
    expect(simulator.callsTo("restart_builtin_ollama")).toHaveLength(2);
    expect(screen.queryByRole("alert")).toBeNull();
  });
});

describe("Models left in the folder of the built-in Ollama", () => {
  const leftover = (count: number, bytes: number) => (backend: FakeBackend) => {
    backend.runtime.builtinModels = { count, bytes };
  };

  it("are pointed out under the model location: count, size, folder", async () => {
    setup(leftover(3, 4.5 * GIB));
    const notice = await screen.findByTestId("builtin-leftover");
    expect(notice.textContent).toContain(
      "The built-in Ollama still has 3 models (4.5 GB) in its folder. Ollama on this computer doesn't see them.",
    );
    expect(within(notice).getByText(BUILTIN_MODELS_DIR)).toBeTruthy();
    const field = screen.getByLabelText("Model location");
    expect(
      field.compareDocumentPosition(notice) & Node.DOCUMENT_POSITION_FOLLOWING,
    ).toBeTruthy();
  });

  it("one model: singular", async () => {
    setup(leftover(1, 700 * 1024 ** 2));
    expect(
      (await screen.findByTestId("builtin-leftover")).textContent,
    ).toContain(
      "The built-in Ollama still has 1 model (700 MB) in its folder.",
    );
  });

  it("opens the folder, and says when it can't", async () => {
    const { simulator, backend } = setup(leftover(2, GIB));
    const notice = await screen.findByTestId("builtin-leftover");
    await userEvent.click(
      within(notice).getByRole("button", { name: "Open folder" }),
    );
    expect(simulator.callsTo("open_builtin_folder")).toHaveLength(1);
    backend.runtimeErrors.openFolder = {
      kind: "system",
      message: "xdg-open is not installed",
    };
    await userEvent.click(
      within(notice).getByRole("button", { name: "Open folder" }),
    );
    expect((await within(notice).findByRole("alert")).textContent).toBe(
      "Couldn't open the folder: xdg-open is not installed",
    );
  });

  it("switches to the built-in Ollama from the notice, which then goes away", async () => {
    const { simulator } = setup(leftover(2, GIB));
    const notice = await screen.findByTestId("builtin-leftover");
    // Même libellé que la ligne Ollama, nom accessible distinct.
    const button = within(notice).getByRole("button", {
      name: "Use the built-in Ollama and its 2 models",
    });
    expect(button.textContent).toBe("Use the built-in Ollama");
    await userEvent.click(button);
    await waitFor(() =>
      expect(screen.queryByTestId("builtin-leftover")).toBeNull(),
    );
    expect(simulator.callsTo("set_ollama_source")).toEqual([
      { source: "builtin" },
    ]);
  });

  it("one switch at a time, from either button, each with its own name", async () => {
    const { simulator } = setup(leftover(2, GIB));
    const notice = await screen.findByTestId("builtin-leftover");
    // Aucun nom accessible en double parmi les boutons de bascule.
    const names = screen
      .getAllByRole("button")
      .map((button) => button.getAttribute("aria-label") ?? button.textContent)
      .filter((name) => name?.includes("built-in"));
    expect(names).toEqual([
      "Use the built-in Ollama",
      "Use the built-in Ollama and its 2 models",
    ]);
    let release: () => void = () => {};
    simulator.handle(
      "set_ollama_source",
      () => new Promise((resolve) => (release = () => resolve(null))),
    );
    await userEvent.click(
      screen.getByRole("button", { name: "Use the built-in Ollama" }),
    );
    expect(
      screen.getByRole<HTMLButtonElement>("button", { name: "Switching…" })
        .disabled,
    ).toBe(true);
    // L'autre bouton attend la fin de cette bascule.
    const other = within(notice).getByRole<HTMLButtonElement>("button", {
      name: "Use the built-in Ollama and its 2 models",
    });
    expect(other.disabled).toBe(true);
    await userEvent.click(other);
    expect(simulator.callsTo("set_ollama_source")).toHaveLength(1);
    await act(async () => release());
    await waitFor(() =>
      expect(
        screen.getByRole<HTMLButtonElement>("button", {
          name: "Use the built-in Ollama and its 2 models",
        }).disabled,
      ).toBe(false),
    );
    expect(simulator.callsTo("set_ollama_source")).toHaveLength(1);
  });

  it("nothing while the built-in Ollama serves, nor without models", async () => {
    setup((backend) => {
      leftover(2, GIB)(backend);
      builtinServes("builtin")(backend);
    });
    await screen.findByText("Built-in Ollama 0.40.0");
    expect(screen.queryByTestId("builtin-leftover")).toBeNull();
  });

  it("nothing with an empty folder", async () => {
    setup(leftover(0, 0));
    await ollamaRow();
    expect(screen.queryByTestId("builtin-leftover")).toBeNull();
  });
});

describe("Missing folder of the built-in Ollama", () => {
  it("says that the default folder is used instead", async () => {
    setup((backend) => {
      builtinServes("builtin")(backend);
      backend.runtime.builtinModelsDirMissing = true;
    });
    const notice = await screen.findByTestId("builtin-dir-missing");
    expect(notice.textContent).toBe(
      "The model location chosen for the built-in Ollama can't be found. It uses its default folder instead.",
    );
    // Sous l'emplacement des modèles.
    expect(
      screen.getByLabelText("Model location").compareDocumentPosition(notice) &
        Node.DOCUMENT_POSITION_FOLLOWING,
    ).toBeTruthy();
    usePseudoLanguage();
    await screen.findByText("⟦Ollama⟧");
    expect(untranslatedTexts(notice)).toEqual([]);
  });

  it("nothing when its folder is there", async () => {
    setup(builtinServes("builtin"));
    await screen.findByText("Built-in Ollama 0.40.0");
    expect(screen.queryByTestId("builtin-dir-missing")).toBeNull();
  });

  it("nothing while Ollama of this computer serves", async () => {
    setup((backend) => {
      backend.runtime.builtinModelsDirMissing = true;
    });
    await screen.findByText("Ollama 0.35.1 on this computer");
    expect(screen.queryByTestId("builtin-dir-missing")).toBeNull();
  });
});

describe("Ollama row in other languages", () => {
  function speak(backend: FakeBackend, lang: "fr" | "es") {
    Object.assign(backend.settings, {
      language: lang,
      effectiveLanguage: lang,
    });
    act(() => setLanguage(lang));
  }

  it("every text comes from the catalogs: machine, leftover, errors", async () => {
    const { backend } = setup((backend) => {
      backend.runtime.builtinModels = { count: 2, bytes: GIB };
      backend.runtimeErrors.setSource = { kind: "database", message: "locked" };
      backend.runtimeErrors.openFolder = { kind: "system", message: "no xdg" };
    });
    const notice = await screen.findByTestId("builtin-leftover");
    await userEvent.click(
      within(notice).getByRole("button", { name: "Open folder" }),
    );
    await userEvent.click(
      within(notice).getByRole("button", {
        name: "Use the built-in Ollama and its 2 models",
      }),
    );
    await waitFor(() =>
      expect(within(notice).getAllByRole("alert")).toHaveLength(2),
    );
    expect(backend.runtime.kind).toBe("machine");
    usePseudoLanguage();
    await screen.findByText("⟦Ollama⟧");
    expect(
      untranslatedTexts(screen.getByTestId("settings"), [
        SERVER_URL,
        BUILTIN_MODELS_DIR,
        "/var/lib/ollama",
        "locked",
        "no xdg",
      ]),
    ).toEqual([]);
  });

  it("every text comes from the catalogs: built-in, chosen, failed", async () => {
    setup((backend) => {
      builtinServes("builtin")(backend);
      backend.serverState = "builtinFailed";
    });
    await screen.findByRole("alert");
    usePseudoLanguage();
    await screen.findByText("⟦Ollama⟧");
    expect(
      untranslatedTexts(screen.getByTestId("settings"), [
        BUILTIN_URL,
        BUILTIN_MODELS_DIR,
        OLLAMA_LOG,
        "/var/lib/ollama",
      ]),
    ).toEqual([]);
  });

  it("every text comes from the catalogs: built-in for lack of another, starting", async () => {
    setup((backend) => {
      builtinServes("auto")(backend);
      backend.serverState = "builtinStarting";
    });
    await screen.findByText("Starting Ollama…");
    usePseudoLanguage();
    await screen.findByText("⟦Ollama⟧");
    expect(
      untranslatedTexts(screen.getByTestId("settings"), [
        BUILTIN_URL,
        BUILTIN_MODELS_DIR,
        "/var/lib/ollama",
        "http://localhost:11434",
      ]),
    ).toEqual([]);
  });

  it("speaks French and Spanish: which Ollama, plural and size", async () => {
    const { backend } = setup((backend) => {
      backend.runtime.builtinModels = { count: 3, bytes: 4.5 * GIB };
    });
    await screen.findByText("Ollama 0.35.1 on this computer");
    speak(backend, "fr");
    expect(screen.getByText("Ollama 0.35.1 sur cet ordinateur")).toBeTruthy();
    const notice = screen.getByTestId("builtin-leftover");
    expect(notice.textContent).toContain(
      "L'Ollama intégré garde encore 3 modèles (4,5\u00a0Go) dans son dossier.",
    );
    expect(
      within(notice).getByRole("button", { name: "Ouvrir le dossier" }),
    ).toBeTruthy();
    expect(
      within(notice).getByRole("button", {
        name: "Utiliser l'Ollama intégré et ses 3 modèles",
      }).textContent,
    ).toBe("Utiliser l'Ollama intégré");
    speak(backend, "es");
    expect(screen.getByText("Ollama 0.35.1 en este equipo")).toBeTruthy();
    expect(notice.textContent).toContain(
      "El Ollama integrado aún tiene 3 modelos (4,5\u00a0GB) en su carpeta.",
    );
    await userEvent.click(
      within(notice).getByRole("button", {
        name: "Usar el Ollama integrado y sus 3 modelos",
      }),
    );
    expect(await screen.findByText("Ollama integrado 0.40.0")).toBeTruthy();
    expect(
      screen.getByRole("button", { name: "Usar el Ollama de este equipo" }),
    ).toBeTruthy();
  });
});
