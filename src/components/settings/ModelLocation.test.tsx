// Settings > Model location face au simulateur : ligne officielle, cas du
// rapport 36 § 8 (service système, arrêté, lancement à la main, service de
// l'utilisateur, conteneur, autre machine, inconnu, sans pkexec, sans agent),
// confirmation à trois boutons, progression, « Saved », chaque erreur avec
// « Details », commandes à copier, Reset sans effet sur le dossier.

import {
  act,
  configure,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import type { FolderCheck } from "@/bindings/FolderCheck";
import type { ModelsLocation } from "@/bindings/ModelsLocation";
import type { MoveFailure } from "@/bindings/MoveFailure";
import type { MoveOutcome } from "@/bindings/MoveOutcome";
import { healthKey } from "@/hooks/useServerHealth";
import { setLanguage } from "@/i18n";
import { setPlatform } from "@/lib/platform";
import {
  BUILTIN_MODELS_DIR,
  BUILTIN_URL,
  fakeBackend,
  SYSTEM_SERVICE,
} from "@/test/backend";
import { untranslatedTexts, usePseudoLanguage } from "@/test/i18n";
import { renderApp } from "@/test/renderApp";
import {
  type Args,
  type FakeChannel,
  installSimulator,
} from "@/test/simulator";

configure({ asyncUtilTimeout: 5000 });
vi.setConfig({ testTimeout: 30_000 });

const NEW = "/data/Models";
const GIB = 1024 ** 3;

function setup(
  location: ModelsLocation | Partial<typeof SYSTEM_SERVICE> = {},
  check: Partial<FolderCheck> = {},
) {
  const simulator = installSimulator();
  const backend = fakeBackend(simulator);
  // Emplacement complet, ou retouche du service système par défaut.
  backend.modelsLocation =
    "kind" in location
      ? (location as ModelsLocation)
      : { ...SYSTEM_SERVICE, ...location };
  backend.folderChecks.set(NEW, check);
  const app = renderApp("/settings");
  return { simulator, backend, app };
}

/** change_models_location retenu : canal et réponse aux mains du test. */
function holdChange(simulator: ReturnType<typeof installSimulator>) {
  const held: {
    args: Args;
    channel: FakeChannel;
    finish: (outcome: MoveOutcome) => void;
    reject: (error: unknown) => void;
  }[] = [];
  simulator.handle(
    "change_models_location",
    (args) =>
      new Promise((resolve, reject) => {
        held.push({
          args,
          channel: simulator.channel(args.onEvent),
          finish: resolve,
          reject,
        });
      }),
  );
  return held;
}

const field = () => screen.getByLabelText<HTMLInputElement>("Model location");
const browseButton = () =>
  screen.getByRole<HTMLButtonElement>("button", { name: "Browse" });
const dialog = () => screen.queryByRole("alertdialog");

/** Browse, avec `path` choisi dans le sélecteur. */
async function browse(
  backend: ReturnType<typeof fakeBackend>,
  path: string | null = NEW,
) {
  backend.folderPicks.push(path);
  await waitFor(() => expect(browseButton().disabled).toBe(false));
  await userEvent.click(browseButton());
}

const done = (overrides: Partial<MoveOutcome> = {}): MoveOutcome =>
  ({
    kind: "done",
    restarted: true,
    renamed: 0,
    copied: 5,
    skipped: 0,
    cleanupFailed: false,
    ...overrides,
  }) as MoveOutcome;

const failure = (reason: MoveFailure, detail = ""): MoveOutcome => ({
  kind: "failed",
  reason,
  restored: true,
  detail,
});

/**
 * Langue réglée et affichée (chaque lecture des réglages rend celle du
 * moteur).
 */
function speak(backend: ReturnType<typeof fakeBackend>, lang: "fr" | "es") {
  Object.assign(backend.settings, { language: lang, effectiveLanguage: lang });
  act(() => setLanguage(lang));
}

describe("Model location", () => {
  it("is the official row, between the server address and the context length", async () => {
    setup();
    await waitFor(() => expect(field().value).toBe("/var/lib/ollama"));
    expect(field().readOnly).toBe(true);
    expect(screen.getByText("Location where models are stored.")).toBeTruthy();
    expect(field().getAttribute("aria-describedby")).toBe(
      screen.getByText("Location where models are stored.").id,
    );
    expect(browseButton().disabled).toBe(false);
    const address = screen.getByLabelText("Server address");
    const context = screen.getByRole("slider", { name: "Context length" });
    expect(
      address.compareDocumentPosition(field()) &
        Node.DOCUMENT_POSITION_FOLLOWING,
    ).toBeTruthy();
    expect(
      field().compareDocumentPosition(context) &
        Node.DOCUMENT_POSITION_FOLLOWING,
    ).toBeTruthy();
  });

  it("does nothing when the folder picker is cancelled", async () => {
    const { simulator, backend } = setup();
    await browse(backend, null);
    await waitFor(() =>
      expect(simulator.callsTo("pick_models_folder")).toHaveLength(1),
    );
    expect(simulator.callsTo("check_models_folder")).toHaveLength(0);
    expect(dialog()).toBeNull();
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("moves the models after confirmation, with progress, then shows Saved", async () => {
    const { simulator, backend, app } = setup(
      {},
      {
        models: { count: 3, bytes: 4.5 * GIB },
        sameDrive: false,
        repliesRunning: true,
      },
    );
    app.queryClient.setQueryData(["models"], []);
    app.queryClient.setQueryData(healthKey, null);
    const held = holdChange(simulator);
    await browse(backend);
    const confirm = await screen.findByRole("alertdialog", {
      name: "Move models to the new location?",
    });
    expect(simulator.callsTo("check_models_folder")).toEqual([{ path: NEW }]);
    expect(
      [...confirm.querySelectorAll("p")].map((p) => p.textContent),
    ).toEqual([
      "Ollama will restart and store new models in /data/Models.",
      "Your 3 models (4.5 GB) are in /var/lib/ollama.",
      "Copying them to another drive can take several minutes.",
      "Replies in progress will stop.",
    ]);
    expect(
      within(confirm)
        .getAllByRole("button")
        .map((button) => button.textContent),
    ).toEqual(["Cancel", "Leave them", "Move models"]);
    expect(document.activeElement).toBe(
      within(confirm).getByRole("button", { name: "Move models" }),
    );

    await userEvent.click(
      within(confirm).getByRole("button", { name: "Move models" }),
    );
    await waitFor(() => expect(held).toHaveLength(1));
    expect(held[0].args.path).toBe(NEW);
    expect(held[0].args.moveModels).toBe(true);
    expect(dialog()).toBeNull();
    expect(browseButton().disabled).toBe(true);
    expect(screen.getByText("Moving models…")).toBeTruthy();

    const { channel } = held[0];
    await channel.send({ type: "step", step: "prepare" });
    await channel.send({ type: "step", step: "stop" });
    await channel.send({ type: "step", step: "move" });
    await channel.send({ type: "total", bytes: 4.5 * GIB });
    await channel.send({ type: "progress", done: 1.5 * GIB });
    // Octets affichés, mais seule l'étape est annoncée (aria-live).
    const step = screen.getByText("Moving models…");
    expect(step.getAttribute("aria-live")).toBe("polite");
    expect(step.parentElement?.textContent).toBe(
      "Moving models… 1.5 GB of 4.5 GB",
    );
    expect(screen.getByText("1.5 GB of 4.5 GB").closest("[aria-live]")).toBe(
      null,
    );
    const bar = screen.getByRole("progressbar", { name: "Moving models" });
    expect(bar.getAttribute("aria-valuenow")).toBe("33");
    expect(bar.getAttribute("aria-valuetext")).toBe(
      "Moving models… 1.5 GB of 4.5 GB",
    );
    expect(bar.closest("[aria-live]")).toBe(null);
    expect(
      [...document.querySelectorAll("[aria-live]")].map(
        (live) => live.textContent,
      ),
    ).toEqual(["Moving models…"]);
    await channel.send({ type: "progress", done: 4.5 * GIB });
    await channel.send({ type: "step", step: "config" });
    await channel.send({ type: "step", step: "start" });
    expect(
      screen.getByText("Restarting Ollama…").getAttribute("aria-live"),
    ).toBe("polite");
    expect(screen.queryByText("1.5 GB of 4.5 GB")).toBeNull();
    expect(screen.queryByText("Saved")).toBeNull();

    // Le moteur relit ensuite le nouveau dossier.
    backend.modelsLocation = { ...SYSTEM_SERVICE, dir: NEW };
    act(() => held[0].finish(done()));
    await screen.findByText("Saved");
    await waitFor(() => expect(field().value).toBe(NEW));
    expect(screen.queryByRole("progressbar")).toBeNull();
    expect(screen.queryByRole("alert")).toBeNull();
    expect(browseButton().disabled).toBe(false);
    // Modèles et santé du serveur relus.
    expect(app.queryClient.getQueryState(["models"])?.isInvalidated).toBe(true);
    expect(app.queryClient.getQueryState(healthKey)?.isInvalidated).toBe(true);
  });

  it("can leave the models where they are", async () => {
    const { simulator, backend } = setup();
    const held = holdChange(simulator);
    await browse(backend);
    await userEvent.click(
      await screen.findByRole("button", { name: "Leave them" }),
    );
    await waitFor(() => expect(held).toHaveLength(1));
    expect(held[0].args.moveModels).toBe(false);
    expect(screen.getByText("Changing location…")).toBeTruthy();
    await held[0].channel.send({ type: "step", step: "start" });
    expect(screen.getByText("Restarting Ollama…")).toBeTruthy();
    act(() => held[0].finish(done({ copied: 0 })));
    await screen.findByText("Saved");
  });

  it("Cancel closes the confirmation without asking for a password", async () => {
    const { simulator, backend } = setup();
    await browse(backend);
    await userEvent.click(
      within(await screen.findByRole("alertdialog")).getByRole("button", {
        name: "Cancel",
      }),
    );
    expect(dialog()).toBeNull();
    expect(simulator.callsTo("change_models_location")).toHaveLength(0);
    expect(browseButton().disabled).toBe(false);
    expect(field().value).toBe("/var/lib/ollama");
  });

  it("only offers to change the location when there is no model to move", async () => {
    const { simulator, backend } = setup(
      {},
      { models: { count: 0, bytes: 0 } },
    );
    const held = holdChange(simulator);
    await browse(backend);
    const confirm = await screen.findByRole("alertdialog", {
      name: "Change the model location?",
    });
    expect(confirm.textContent).not.toContain("Your");
    expect(
      within(confirm)
        .getAllByRole("button")
        .map((button) => button.textContent),
    ).toEqual(["Cancel", "Change location"]);
    await userEvent.click(
      within(confirm).getByRole("button", { name: "Change location" }),
    );
    await waitFor(() => expect(held).toHaveLength(1));
    expect(held[0].args.moveModels).toBe(false);
  });

  it("changes a stopped service without restarting it", async () => {
    const { simulator, backend } = setup(
      { running: false },
      { models: null, repliesRunning: true },
    );
    const held = holdChange(simulator);
    await browse(backend);
    const confirm = await screen.findByRole("alertdialog", {
      name: "Move models to the new location?",
    });
    expect(
      [...confirm.querySelectorAll("p")].map((p) => p.textContent),
    ).toEqual([
      "Ollama will store new models in /data/Models.",
      "Your models are in /var/lib/ollama.",
      "Used the next time Ollama starts.",
    ]);
    await userEvent.click(
      within(confirm).getByRole("button", { name: "Move models" }),
    );
    await waitFor(() => expect(held).toHaveLength(1));
    await held[0].channel.send({ type: "step", step: "start" });
    expect(screen.queryByText("Restarting Ollama…")).toBeNull();
    act(() => held[0].finish(done({ restarted: false })));
    expect(
      await screen.findByText(
        "Saved. Ollama will use this folder the next time it starts.",
      ),
    ).toBeTruthy();
  });

  it("says when the old copies could not be deleted", async () => {
    const { simulator, backend } = setup();
    const held = holdChange(simulator);
    await browse(backend);
    await userEvent.click(
      await screen.findByRole("button", { name: "Move models" }),
    );
    await waitFor(() => expect(held).toHaveLength(1));
    act(() => held[0].finish(done({ cleanupFailed: true })));
    await screen.findByText("Saved");
    expect(
      screen.getByText(
        "The models were copied, but the old copies in /var/lib/ollama could not be deleted.",
      ),
    ).toBeTruthy();
  });

  it("a dismissed password prompt changes nothing", async () => {
    const { simulator, backend } = setup();
    const held = holdChange(simulator);
    await waitFor(() => expect(field().value).toBe("/var/lib/ollama"));
    const reads = simulator.callsTo("get_models_location").length;
    await browse(backend);
    await userEvent.click(
      await screen.findByRole("button", { name: "Move models" }),
    );
    await waitFor(() => expect(held).toHaveLength(1));
    act(() => held[0].finish({ kind: "cancelled" }));
    await waitFor(() => expect(browseButton().disabled).toBe(false));
    expect(screen.queryByRole("alert")).toBeNull();
    expect(screen.queryByText("Saved")).toBeNull();
    expect(screen.queryByText("Moving models…")).toBeNull();
    expect(field().value).toBe("/var/lib/ollama");
    expect(simulator.callsTo("get_models_location")).toHaveLength(reads);
  });

  it("explains each failure, with the details of the script", async () => {
    const { simulator, backend, app } = setup(
      {},
      {
        commands: "sudo mkdir -p '/data/Models'\nsudo systemctl daemon-reload",
      },
    );
    const held = holdChange(simulator);
    const cases: [MoveFailure, string][] = [
      [
        { kind: "notAuthorized" },
        "Authentication failed. The model location was not changed.",
      ],
      [
        { kind: "refused" },
        "This folder can't hold models. Choose a folder outside the system folders.",
      ],
      [
        { kind: "notEmpty" },
        "Choose an empty folder, or a folder that already contains Ollama models.",
      ],
      [
        { kind: "noSpace", need: 5 * GIB, free: GIB },
        "Not enough free space: 5.0 GB needed, 1.0 GB available.",
      ],
      [
        { kind: "cannotWrite" },
        "The Ollama service (user ollama) can't write to this folder. Its drive may not support Linux permissions, or a parent folder is private.",
      ],
      [
        { kind: "environmentFile", file: "/etc/ollama.env" },
        "OLLAMA_MODELS is set in /etc/ollama.env. Change it there.",
      ],
      [
        { kind: "unitChanged" },
        "The Ollama service changed. Reopen Settings and try again.",
      ],
      [
        { kind: "stopFailed" },
        "Ollama could not be stopped. Nothing was changed.",
      ],
      [
        { kind: "moveFailed" },
        "The model location could not be changed. Everything was put back as before.",
      ],
      [
        { kind: "configFailed" },
        "The model location could not be changed. Everything was put back as before.",
      ],
      [
        { kind: "overridden" },
        "Another setting of the Ollama service chooses the model location. Everything was put back as before.",
      ],
      [
        { kind: "startFailed" },
        "Ollama did not start with the new location, so the previous one was restored. See: journalctl -u ollama",
      ],
      [
        { kind: "restoreFailed" },
        "ChatonLM could not undo every change. Check the service with: systemctl status ollama",
      ],
      [{ kind: "unexpected" }, "The model location could not be changed."],
    ];
    for (const [index, [reason, text]] of cases.entries()) {
      await browse(backend);
      await userEvent.click(
        await screen.findByRole("button", { name: "Move models" }),
      );
      await waitFor(() => expect(held).toHaveLength(index + 1));
      const detail = `chatonlm: ${reason.kind} failed`;
      act(() => held[index].finish(failure(reason, detail)));
      const alert = await screen.findByRole("alert");
      expect(alert.querySelector("p")?.textContent, reason.kind).toBe(text);
      expect(within(alert).getByText("Details").tagName).toBe("SUMMARY");
      expect(alert.querySelector("pre")?.textContent).toBe(detail);
      expect(screen.queryByText("Saved")).toBeNull();
    }
    // Échec : le dossier est relu (le service a pu changer).
    expect(
      app.queryClient.getQueryState([
        "modelsLocation",
        "http://127.0.0.1:11434",
      ])?.dataUpdateCount,
    ).toBeGreaterThan(1);
  });

  it("gives the commands to run when no password prompt is available", async () => {
    const commands =
      "sudo mkdir -p '/data/Models' && sudo chown ollama:ollama '/data/Models'\nsudo systemctl daemon-reload";
    const { simulator, backend } = setup({}, { commands });
    const held = holdChange(simulator);
    await browse(backend);
    await userEvent.click(
      await screen.findByRole("button", { name: "Move models" }),
    );
    await waitFor(() => expect(held).toHaveLength(1));
    act(() =>
      held[0].finish(
        failure(
          { kind: "noAgent" },
          "Error executing command as another user: No authentication agent found.",
        ),
      ),
    );
    const alert = await screen.findByRole("alert");
    expect(alert.querySelector("p")?.textContent).toBe(
      "No password prompt is available in this session. Run these commands in a terminal:",
    );
    expect(alert.querySelector("code")?.textContent).toBe(commands);
    // userEvent.setup() installe le presse-papiers simulé.
    const user = userEvent.setup();
    const writeText = vi.spyOn(navigator.clipboard, "writeText");
    await user.click(
      within(alert).getByRole("button", { name: "Copy command to clipboard" }),
    );
    expect(writeText).toHaveBeenCalledWith(commands);
    writeText.mockRestore();
  });

  it("refuses an unsuitable folder before any prompt", async () => {
    const { simulator, backend } = setup();
    const problems: [FolderCheck["problem"], string][] = [
      [
        "system",
        "This folder can't hold models. Choose a folder outside the system folders.",
      ],
      [
        "badCharacters",
        'Choose a folder whose name has no " \\ : $ or ` characters.',
      ],
      [
        "notEmpty",
        "Choose an empty folder, or a folder that already contains Ollama models.",
      ],
      ["inside", "Choose a folder outside the current model location."],
      ["missing", "This folder can't be found."],
      ["notYours", "Choose a folder you created, or a new folder."],
    ];
    for (const [problem, text] of problems) {
      backend.folderChecks.set(NEW, { problem });
      await browse(backend);
      expect((await screen.findByRole("alert")).textContent).toBe(text);
      expect(dialog()).toBeNull();
    }
    // Le dossier actuel : rien à faire, rien à dire.
    backend.folderChecks.set(NEW, { problem: "same" });
    await browse(backend);
    await waitFor(() =>
      expect(simulator.callsTo("check_models_folder")).toHaveLength(7),
    );
    await waitFor(() => expect(browseButton().disabled).toBe(false));
    expect(screen.queryByRole("alert")).toBeNull();
    expect(dialog()).toBeNull();
  });

  it("waits for model downloads to finish", async () => {
    const { simulator, backend } = setup({}, { downloadsRunning: true });
    await browse(backend);
    expect((await screen.findByRole("alert")).textContent).toBe(
      "Wait for the model downloads to finish, then try again.",
    );
    expect(dialog()).toBeNull();
    expect(simulator.callsTo("change_models_location")).toHaveLength(0);
  });

  it("shows why the app refused the change", async () => {
    const { simulator, backend } = setup();
    const held = holdChange(simulator);
    await browse(backend);
    await userEvent.click(
      await screen.findByRole("button", { name: "Move models" }),
    );
    await waitFor(() => expect(held).toHaveLength(1));
    act(() =>
      held[0].reject({
        kind: "invalid",
        message: "the model location is already being changed",
      }),
    );
    expect((await screen.findByRole("alert")).textContent).toBe(
      "Couldn't save this setting: the model location is already being changed",
    );
    expect(browseButton().disabled).toBe(false);
  });

  it("gives the command for Ollama started from a terminal", async () => {
    const command =
      "OLLAMA_HOST=127.0.0.1:11435 OLLAMA_MODELS='/data/Models' ollama serve";
    const { simulator, backend } = setup(
      {
        kind: "manual",
        dir: "/home/alice/.ollama/models",
        hostEnv: "127.0.0.1:11435",
      },
      { commands: command },
    );
    await waitFor(() =>
      expect(field().value).toBe("/home/alice/.ollama/models"),
    );
    await browse(backend);
    expect(
      await screen.findByText(
        "Ollama was started from a terminal. Stop it, then start it again with:",
      ),
    ).toBeTruthy();
    expect(document.querySelector("code")?.textContent).toBe(command);
    expect(screen.getByRole("button", { name: "Copy command to clipboard" }));
    expect(dialog()).toBeNull();
    expect(screen.queryByRole("alert")).toBeNull();
    expect(simulator.callsTo("change_models_location")).toHaveLength(0);
  });

  it("gives the commands for a service of the user account", async () => {
    const commands =
      "mkdir -p ~/.config/systemd/user/ollama.service.d\nprintf '%s\\n' '[Service]' 'Environment=\"OLLAMA_MODELS=/data/Models\"' > ~/.config/systemd/user/ollama.service.d/zz-chatonlm-models.conf\nsystemctl --user daemon-reload && systemctl --user restart ollama.service";
    const { backend } = setup({ kind: "userService", dir: null }, { commands });
    await waitFor(() => expect(browseButton().disabled).toBe(false));
    expect(field().value).toBe("");
    await browse(backend);
    expect(
      await screen.findByText(
        "Ollama runs as a service of your user account. Run:",
      ),
    ).toBeTruthy();
    expect(document.querySelector("code")?.textContent).toBe(commands);
  });

  it("gives a start command when the launch is unknown", async () => {
    const { backend } = setup(
      { kind: "unknown", running: true },
      { commands: "OLLAMA_MODELS='/data/Models' ollama serve" },
    );
    await waitFor(() => expect(field().value).toBe("Unknown"));
    await browse(backend);
    expect(
      await screen.findByText(
        "ChatonLM can't tell how Ollama was started. Start it with:",
      ),
    ).toBeTruthy();
    expect(document.querySelector("code")?.textContent).toBe(
      "OLLAMA_MODELS='/data/Models' ollama serve",
    );
  });

  it("without pkexec, gives the commands to run with sudo", async () => {
    const commands = "sudo mkdir -p '/data/Models'";
    const { simulator, backend } = setup({ canElevate: false }, { commands });
    await browse(backend);
    expect(
      await screen.findByText(
        "Changing the Ollama service needs administrator rights, and pkexec is not installed. Run these commands in a terminal:",
      ),
    ).toBeTruthy();
    expect(document.querySelector("code")?.textContent).toBe(commands);
    expect(dialog()).toBeNull();
    expect(simulator.callsTo("change_models_location")).toHaveLength(0);
  });

  it("gives the commands to run for a service that runs as root", async () => {
    const commands =
      "sudo mkdir -p '/data/Models' && sudo chown root: '/data/Models'\nsudo systemctl daemon-reload && sudo systemctl restart ollama.service";
    const { simulator, backend } = setup(
      { user: "root", group: "" },
      { commands },
    );
    await browse(backend);
    expect(
      await screen.findByText(
        "The Ollama service runs as root. Change its model location in a terminal:",
      ),
    ).toBeTruthy();
    expect(document.querySelector("code")?.textContent).toBe(commands);
    expect(screen.getByRole("button", { name: "Copy command to clipboard" }));
    expect(dialog()).toBeNull();
    expect(screen.queryByRole("alert")).toBeNull();
    expect(simulator.callsTo("change_models_location")).toHaveLength(0);
  });

  it.each<[string, ModelsLocation, string, string]>([
    [
      "container",
      { kind: "container" },
      "Unknown",
      "Ollama seems to run in a container. Change the models volume where the container is created, for example:",
    ],
    [
      "remote",
      { kind: "remote", host: "192.168.1.20" },
      "On 192.168.1.20",
      "Ollama runs on 192.168.1.20. Change the model location on that computer.",
    ],
    [
      "nothing running",
      { kind: "unknown", running: false },
      "Unknown",
      "Start Ollama to see where models are stored.",
    ],
    [
      "environment file",
      { ...SYSTEM_SERVICE, envFile: "/etc/ollama.env" },
      "/var/lib/ollama",
      "OLLAMA_MODELS is set in /etc/ollama.env. Change it there.",
    ],
  ])(
    "says what to do where nothing can be changed from here (%s)",
    async (_, location, value, text) => {
      setup(location);
      await waitFor(() => expect(field().value).toBe(value));
      expect(screen.getByText(text)).toBeTruthy();
      expect(browseButton().disabled).toBe(true);
      expect(screen.queryByRole("alert")).toBeNull();
      if (location.kind === "container") {
        expect(document.querySelector("code")?.textContent).toBe(
          "docker run -d -v /data/Models:/root/.ollama -p 11434:11434 --name ollama ollama/ollama",
        );
      }
    },
  );

  it("on Windows, shows the folder of Ollama's app with guidance only", async () => {
    setPlatform("windows");
    const dir = "C:\\Users\\me\\.ollama\\models";
    const { simulator } = setup({ kind: "desktopApp", dir });
    await waitFor(() => expect(field().value).toBe(dir));
    expect(
      screen.getByText(
        "Ollama's app chooses this folder; the one shown is its usual location. To change it, open Ollama's settings (Model location), or set the OLLAMA_MODELS variable of your user account, then quit Ollama from the system tray and start it again.",
      ),
    ).toBeTruthy();
    expect(browseButton().disabled).toBe(true);
    expect(screen.queryByRole("alert")).toBeNull();
    expect(document.querySelector("code")).toBeNull();
    expect(simulator.callsTo("pick_models_folder")).toHaveLength(0);
  });

  it("Reset to defaults leaves the model location alone", async () => {
    const { simulator } = setup();
    await waitFor(() => expect(field().value).toBe("/var/lib/ollama"));
    await userEvent.click(
      screen.getByRole("button", { name: "Reset to defaults" }),
    );
    await screen.findByText("Saved");
    expect(simulator.callsTo("update_settings")).toEqual([
      {
        patch: {
          language: "system",
          serverUrl: "",
          contextLength: 0,
          theme: "system",
          autoRead: false,
        },
      },
    ]);
    expect(simulator.callsTo("pick_models_folder")).toHaveLength(0);
    expect(simulator.callsTo("change_models_location")).toHaveLength(0);
    expect(field().value).toBe("/var/lib/ollama");
  });

  it("follows the server address", async () => {
    const { simulator, backend } = setup();
    await waitFor(() => expect(field().value).toBe("/var/lib/ollama"));
    backend.modelsLocation = { kind: "remote", host: "192.168.1.20" };
    const address = screen.getByLabelText<HTMLInputElement>("Server address");
    await userEvent.type(address, "http://192.168.1.20:11434{Enter}");
    await waitFor(() => expect(field().value).toBe("On 192.168.1.20"));
    expect(
      simulator.callsTo("get_models_location").length,
    ).toBeGreaterThanOrEqual(2);
  });
});

describe("Model location in other languages", () => {
  /** Page General entière (ligne Model location comprise). */
  const page = () => screen.getByTestId("settings");
  /** Données montrées telles quelles : chemins, hôtes, commandes, adresse. */
  const DATA = [
    "/var/lib/ollama",
    NEW,
    "192.168.1.20",
    "/etc/ollama.env",
    "http://127.0.0.1:11434",
  ];
  // Pseudo-langue : libellés entourés de « ⟦ ⟧ », trouvés par motif.
  const anyField = () =>
    screen.getByLabelText<HTMLInputElement>(/Model location/);
  const anyBrowse = () =>
    screen.getByRole<HTMLButtonElement>("button", { name: /Browse/ });
  async function browseAny(backend: ReturnType<typeof fakeBackend>) {
    backend.folderPicks.push(NEW);
    await waitFor(() => expect(anyBrowse().disabled).toBe(false));
    await userEvent.click(anyBrowse());
  }
  /** Dernier bouton de la confirmation (Move models, ou Change location). */
  async function confirmLast() {
    const confirm = await screen.findByRole("alertdialog");
    await userEvent.click(within(confirm).getAllByRole("button").at(-1)!);
  }

  it.each<[string, ModelsLocation]>([
    ["detected", SYSTEM_SERVICE],
    ["remote", { kind: "remote", host: "192.168.1.20" }],
    ["container", { kind: "container" }],
    ["nothing running", { kind: "unknown", running: false }],
    ["environment file", { ...SYSTEM_SERVICE, envFile: "/etc/ollama.env" }],
  ])("writes every text of the row from the catalogs (%s)", async (_, at) => {
    setup(at);
    await waitFor(() => expect(field().value).not.toBe(""));
    usePseudoLanguage();
    await waitFor(() => expect(anyBrowse().textContent).toBe("⟦Browse⟧"));
    const command = document.querySelector("code")?.textContent ?? "";
    expect(untranslatedTexts(page(), [...DATA, command])).toEqual([]);
    // Valeur du champ : « On … » et « Unknown » traduits aussi.
    if (at.kind !== "systemService") expect(anyField().value).toMatch(/^⟦/);
  });

  it("writes the instructions of a manual launch from the catalogs", async () => {
    const command = "OLLAMA_MODELS='/data/Models' ollama serve";
    const { backend } = setup(
      { kind: "manual", dir: "/home/alice/.ollama/models", hostEnv: null },
      { commands: command },
    );
    await waitFor(() =>
      expect(field().value).toBe("/home/alice/.ollama/models"),
    );
    await browse(backend);
    await screen.findByText(
      "Ollama was started from a terminal. Stop it, then start it again with:",
    );
    // Message déjà montré : il change de langue avec la page.
    usePseudoLanguage();
    await screen.findByText(
      "⟦Ollama was started from a terminal. Stop it, then start it again with:⟧",
    );
    expect(untranslatedTexts(page(), [...DATA, command])).toEqual([]);
  });

  it("writes the confirmation and the progress of a move from the catalogs", async () => {
    const { simulator, backend } = setup(
      {},
      {
        models: { count: 3, bytes: 4.5 * GIB },
        sameDrive: false,
        repliesRunning: true,
      },
    );
    const held = holdChange(simulator);
    usePseudoLanguage();
    await browseAny(backend);
    const confirm = await screen.findByRole("alertdialog");
    expect(untranslatedTexts(confirm, DATA)).toEqual([]);
    await confirmLast();
    await waitFor(() => expect(held).toHaveLength(1));
    await held[0].channel.send({ type: "step", step: "move" });
    await held[0].channel.send({ type: "total", bytes: 4.5 * GIB });
    await held[0].channel.send({ type: "progress", done: 1.5 * GIB });
    expect(screen.getByRole("progressbar")).toBeTruthy();
    expect(untranslatedTexts(page(), DATA)).toEqual([]);
  });

  it("writes a failure and its Details from the catalogs", async () => {
    const { simulator, backend } = setup();
    const held = holdChange(simulator);
    usePseudoLanguage();
    await browseAny(backend);
    await confirmLast();
    await waitFor(() => expect(held).toHaveLength(1));
    const detail = "chatonlm: systemctl failed";
    act(() =>
      held[0].finish(
        failure({ kind: "noSpace", need: 5 * GIB, free: GIB }, detail),
      ),
    );
    const alert = await screen.findByRole("alert");
    expect(alert.querySelector("summary")?.textContent).toBe("⟦Details⟧");
    expect(untranslatedTexts(page(), [...DATA, detail])).toEqual([]);
  });

  it("speaks French and Spanish: plural, sizes and progress", async () => {
    const { simulator, backend } = setup(
      {},
      { models: { count: 3, bytes: 4.5 * GIB } },
    );
    const held = holdChange(simulator);
    speak(backend, "fr");
    await waitFor(() =>
      expect(
        screen.getByLabelText<HTMLInputElement>("Emplacement des modèles")
          .value,
      ).toBe("/var/lib/ollama"),
    );
    backend.folderPicks.push(NEW);
    await userEvent.click(screen.getByRole("button", { name: "Parcourir" }));
    let confirm = await screen.findByRole("alertdialog", {
      name: "Déplacer les modèles vers le nouvel emplacement ?",
    });
    expect(
      [...confirm.querySelectorAll("p")].map((p) => p.textContent),
    ).toEqual([
      "Ollama va redémarrer et stocker les nouveaux modèles dans /data/Models.",
      "Vos 3 modèles (4,5 Go) se trouvent dans /var/lib/ollama.",
    ]);
    await userEvent.click(
      within(confirm).getByRole("button", { name: "Déplacer les modèles" }),
    );
    await waitFor(() => expect(held).toHaveLength(1));
    await held[0].channel.send({ type: "step", step: "move" });
    await held[0].channel.send({ type: "total", bytes: 4.5 * GIB });
    await held[0].channel.send({ type: "progress", done: 1.5 * GIB });
    expect(
      screen
        .getByRole("progressbar", { name: "Déplacement des modèles" })
        .getAttribute("aria-valuetext"),
    ).toBe("Déplacement des modèles… 1,5 Go sur 4,5 Go");
    // Espagnol pendant le déplacement : l'étape suit aussitôt.
    speak(backend, "es");
    expect(screen.getByText("Moviendo los modelos…")).toBeTruthy();
    act(() =>
      held[0].finish(failure({ kind: "noSpace", need: 5 * GIB, free: GIB })),
    );
    expect(
      (await screen.findByRole("alert")).querySelector("p")?.textContent,
    ).toBe(
      "No hay suficiente espacio libre: se necesitan 5,0 GB y hay 1,0 GB disponibles.",
    );

    // Un seul modèle : la forme « one ».
    backend.folderChecks.set(NEW, {
      models: { count: 1, bytes: 850 * 1024 ** 2 },
    });
    backend.folderPicks.push(NEW);
    await userEvent.click(screen.getByRole("button", { name: "Examinar" }));
    confirm = await screen.findByRole("alertdialog", {
      name: "¿Mover los modelos a la nueva ubicación?",
    });
    expect(confirm.querySelectorAll("p")[1].textContent).toBe(
      "Tu modelo (850 MB) está en /var/lib/ollama.",
    );
  });
});

describe("Model location of the built-in Ollama", () => {
  /** L'intégré sert (choisi), son dossier vérifié comme ancien dossier. */
  function setupBuiltin(check: Partial<FolderCheck> = {}) {
    const simulator = installSimulator();
    const backend = fakeBackend(simulator);
    backend.runtime.source = "builtin";
    backend.runtime.kind = "builtin";
    backend.settings.effectiveServerUrl = BUILTIN_URL;
    backend.folderChecks.set(NEW, {
      old: BUILTIN_MODELS_DIR,
      commands: null,
      ...check,
    });
    const app = renderApp("/settings");
    return { simulator, backend, app };
  }

  /** change_builtin_models_dir retenu : canal et réponse aux mains du test. */
  function holdBuiltinChange(simulator: ReturnType<typeof installSimulator>) {
    const held: {
      args: Args;
      channel: FakeChannel;
      finish: (outcome: MoveOutcome) => void;
    }[] = [];
    simulator.handle(
      "change_builtin_models_dir",
      (args) =>
        new Promise((resolve) => {
          held.push({
            args,
            channel: simulator.channel(args.onEvent),
            finish: resolve,
          });
        }),
    );
    return held;
  }

  it("shows its folder; Browse moves the models without a password, then restarts it", async () => {
    const { simulator, backend, app } = setupBuiltin({
      models: { count: 2, bytes: 3 * GIB },
      sameDrive: false,
    });
    app.queryClient.setQueryData(["models"], []);
    const held = holdBuiltinChange(simulator);
    await waitFor(() => expect(field().value).toBe(BUILTIN_MODELS_DIR));
    // Pas de commande à copier ni d'avis du service système.
    expect(document.querySelector("code")).toBeNull();
    await browse(backend);
    const confirm = await screen.findByRole("alertdialog", {
      name: "Move models to the new location?",
    });
    expect(
      [...confirm.querySelectorAll("p")].map((p) => p.textContent),
    ).toEqual([
      "Ollama will restart and store new models in /data/Models.",
      `Your 2 models (3.0 GB) are in ${BUILTIN_MODELS_DIR}.`,
      "Copying them to another drive can take several minutes.",
    ]);
    await userEvent.click(
      within(confirm).getByRole("button", { name: "Move models" }),
    );
    await waitFor(() => expect(held).toHaveLength(1));
    expect(held[0].args.path).toBe(NEW);
    expect(held[0].args.moveModels).toBe(true);
    expect(simulator.callsTo("change_models_location")).toHaveLength(0);
    const { channel } = held[0];
    await channel.send({ type: "step", step: "stop" });
    await channel.send({ type: "step", step: "move" });
    await channel.send({ type: "total", bytes: 3 * GIB });
    await channel.send({ type: "progress", done: GIB });
    expect(
      screen
        .getByRole("progressbar", { name: "Moving models" })
        .getAttribute("aria-valuenow"),
    ).toBe("33");
    await channel.send({ type: "step", step: "start" });
    expect(screen.getByText("Restarting Ollama…")).toBeTruthy();

    const runtimeReads = simulator.callsTo("ollama_runtime").length;
    backend.runtime.builtinModelsDir = NEW;
    act(() => held[0].finish(done()));
    await screen.findByText("Saved");
    await waitFor(() => expect(field().value).toBe(NEW));
    // L'intégré a redémarré : Ollama qui sert, modèles et santé relus.
    expect(simulator.callsTo("ollama_runtime").length).toBeGreaterThan(
      runtimeReads,
    );
    expect(app.queryClient.getQueryState(["models"])?.isInvalidated).toBe(true);
    expect(browseButton().disabled).toBe(false);
  });

  it.each([
    // Dossier vide (ou absent) : ce que le moteur lit sur le disque.
    ["an empty folder", { count: 0, bytes: 0 }],
    // Sans nombre, jamais « Your models are in… » : rien n'est inconnu ici.
    ["no count", null],
    // Restes d'un téléchargement sans modèle complet.
    ["only partial downloads", { count: 0, bytes: 3 * 1024 ** 2 }],
  ])("changes the folder without models to move: %s", async (_name, models) => {
    const { simulator, backend } = setupBuiltin({ models, sameDrive: false });
    const held = holdBuiltinChange(simulator);
    await browse(backend);
    const confirm = await screen.findByRole("alertdialog", {
      name: "Change the model location?",
    });
    expect(
      [...confirm.querySelectorAll("p")].map((p) => p.textContent),
    ).toEqual(["Ollama will restart and store new models in /data/Models."]);
    expect(
      within(confirm).queryByRole("button", { name: "Move models" }),
    ).toBeNull();
    await userEvent.click(
      within(confirm).getByRole("button", { name: "Change location" }),
    );
    await waitFor(() => expect(held).toHaveLength(1));
    expect(held[0].args.moveModels).toBe(false);
    act(() => held[0].finish(done({ copied: 0 })));
    await screen.findByText("Saved");
  });

  it("on Windows, words path characters and write failures without Linux notions", async () => {
    setPlatform("windows");
    const { simulator, backend } = setupBuiltin({ problem: "badCharacters" });
    await browse(backend);
    expect((await screen.findByRole("alert")).textContent).toBe(
      "This folder's name has characters ChatonLM can't use. Choose another folder.",
    );
    const held = holdBuiltinChange(simulator);
    backend.folderChecks.set(NEW, { old: BUILTIN_MODELS_DIR, commands: null });
    await browse(backend);
    await userEvent.click(
      await screen.findByRole("button", { name: "Move models" }),
    );
    await waitFor(() => expect(held).toHaveLength(1));
    act(() => held[0].finish(failure({ kind: "cannotWrite" })));
    await waitFor(() =>
      expect(screen.getByRole("alert").querySelector("p")?.textContent).toBe(
        "ChatonLM can't write to this folder.",
      ),
    );
  });

  it("waits for model downloads, and refuses an unsuitable folder", async () => {
    const { simulator, backend } = setupBuiltin({ downloadsRunning: true });
    await browse(backend);
    expect((await screen.findByRole("alert")).textContent).toBe(
      "Wait for the model downloads to finish, then try again.",
    );
    backend.folderChecks.set(NEW, { problem: "notYours" });
    await browse(backend);
    await waitFor(() =>
      expect(screen.getByRole("alert").textContent).toBe(
        "Choose a folder you created, or a new folder.",
      ),
    );
    expect(dialog()).toBeNull();
    expect(simulator.callsTo("change_builtin_models_dir")).toHaveLength(0);
  });

  it("explains each failure in its own words", async () => {
    const { simulator, backend } = setupBuiltin();
    const held = holdBuiltinChange(simulator);
    const cases: [MoveFailure, string][] = [
      [
        { kind: "cannotWrite" },
        "ChatonLM can't write to this folder. Its drive may not support Linux permissions.",
      ],
      [
        { kind: "startFailed" },
        "The built-in Ollama did not start with the new location, so the previous one was restored.",
      ],
      [{ kind: "restoreFailed" }, "ChatonLM could not undo every change."],
      [
        { kind: "noSpace", need: 5 * GIB, free: GIB },
        "Not enough free space: 5.0 GB needed, 1.0 GB available.",
      ],
      [
        { kind: "moveFailed" },
        "The model location could not be changed. Everything was put back as before.",
      ],
      [{ kind: "notAuthorized" }, "The model location could not be changed."],
      [
        { kind: "modelConflict", model: "qwen3:0.6b" },
        "The new folder already has another version of qwen3:0.6b. Nothing was moved.",
      ],
      [
        { kind: "symlink", path: `${BUILTIN_MODELS_DIR}/blobs/sha256-ddd` },
        `The model folder has a link or a special file that ChatonLM doesn't move: ${BUILTIN_MODELS_DIR}/blobs/sha256-ddd. Nothing was moved.`,
      ],
    ];
    for (const [index, [reason, text]] of cases.entries()) {
      await browse(backend);
      await userEvent.click(
        await screen.findByRole("button", { name: "Move models" }),
      );
      await waitFor(() => expect(held).toHaveLength(index + 1));
      act(() => held[index].finish(failure(reason, "ollama: exit 1")));
      const alert = await screen.findByRole("alert");
      expect(alert.querySelector("p")?.textContent, reason.kind).toBe(text);
      expect(alert.querySelector("pre")?.textContent).toBe("ollama: exit 1");
    }
  });

  it("writes its confirmation and failures from the catalogs", async () => {
    const { simulator, backend } = setupBuiltin();
    const held = holdBuiltinChange(simulator);
    await browse(backend);
    await userEvent.click(
      await screen.findByRole("button", { name: "Move models" }),
    );
    await waitFor(() => expect(held).toHaveLength(1));
    act(() => held[0].finish(failure({ kind: "startFailed" }, "exit 1")));
    await screen.findByRole("alert");
    usePseudoLanguage();
    backend.folderPicks.push(NEW);
    const anyBrowse = screen.getByRole<HTMLButtonElement>("button", {
      name: /Browse/,
    });
    await waitFor(() => expect(anyBrowse.textContent).toBe("⟦Browse⟧"));
    await userEvent.click(anyBrowse);
    await screen.findByRole("alertdialog");
    expect(
      untranslatedTexts(document.body, [
        NEW,
        BUILTIN_MODELS_DIR,
        BUILTIN_URL,
        "exit 1",
      ]),
    ).toEqual([]);
  });
});
