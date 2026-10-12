// Dossier .ollama caché au service d'Ollama (ProtectHome=) face au
// simulateur : alerte sous Model location, « Fix » (en cours, réussi :
// alerte retirée et « Saved », annulé, chaque échec avec « Details »),
// commandes à copier sans pkexec ou sans agent, et liens « Fix in Settings ›
// General » sous les erreurs qu'il explique (carte du compte, téléchargement
// du ModelHub, erreur d'une conversation).

import {
  act,
  configure,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import type { HiddenHome } from "@/bindings/HiddenHome";
import type { HomeFixFailure } from "@/bindings/HomeFixFailure";
import type { HomeFixOutcome } from "@/bindings/HomeFixOutcome";
import type { HubDownload } from "@/bindings/HubDownload";
import { accountKey } from "@/hooks/useAccount";
import { setLanguage } from "@/i18n";
import { fakeBackend } from "@/test/backend";
import { untranslatedTexts, usePseudoLanguage } from "@/test/i18n";
import { renderApp } from "@/test/renderApp";
import { renderHub } from "@/test/renderHub";
import { installSimulator, sequence } from "@/test/simulator";

configure({ asyncUtilTimeout: 5000 });
vi.setConfig({ testTimeout: 30_000 });

const HIDDEN: HiddenHome = {
  dir: "/home/alice/.ollama",
  hiddenRoot: "/home",
  user: "alice",
  running: true,
  canElevate: true,
  commands:
    "sudo -u alice mkdir -p '/home/alice/.ollama'\nsudo systemctl daemon-reload",
};

const ALERT =
  "Ollama can't read its own files in /home/alice/.ollama: its systemd service hides /home (ProtectHome). Downloads from ollama.com and your Ollama account won't work until this is fixed.";

const KEY_ERROR =
  "pull model manifest: open /home/alice/.ollama/id_ed25519: permission denied";

function setup(hidden: HiddenHome | null = HIDDEN, path = "/settings") {
  const simulator = installSimulator();
  const backend = fakeBackend(simulator);
  backend.ollamaHome = hidden && { ...hidden };
  const app = renderApp(path);
  return { simulator, backend, app };
}

/** fix_ollama_home retenu : la réponse aux mains du test. */
function holdFix(simulator: ReturnType<typeof installSimulator>) {
  const held: {
    finish: (outcome: HomeFixOutcome) => void;
    reject: (error: unknown) => void;
  }[] = [];
  simulator.handle(
    "fix_ollama_home",
    () =>
      new Promise((resolve, reject) => {
        held.push({ finish: resolve, reject });
      }),
  );
  return held;
}

const box = () => screen.queryByTestId("ollama-home");

/** Alertes de la boîte, une fois le message de « Fix » affiché sous l'alerte. */
async function noticeAlerts() {
  await waitFor(() =>
    expect(within(box()!).getAllByRole("alert")).toHaveLength(2),
  );
  return within(box()!).getAllByRole("alert");
}
const fixButton = () =>
  screen.getByRole<HTMLButtonElement>("button", { name: /^Fix/ });

describe("Ollama's own folder hidden from its service", () => {
  it("nothing to say: no alert", async () => {
    const { simulator } = setup(null);
    await screen.findByLabelText("Model location");
    await waitFor(() =>
      expect(simulator.callsTo("get_ollama_home")).toHaveLength(1),
    );
    expect(box()).toBeNull();
  });

  it("the alert under Model location, with Fix", async () => {
    setup();
    const alert = await screen.findByTestId("ollama-home");
    expect(within(alert).getByRole("alert").textContent).toBe(ALERT);
    expect(fixButton().textContent).toBe("Fix");
    expect(
      within(alert).getByText(
        "Fix asks for an administrator password, then restarts Ollama.",
      ),
    ).toBeTruthy();
    // Sous le champ du dossier des modèles, avant la longueur de contexte.
    const field = screen.getByLabelText("Model location");
    const context = screen.getByRole("slider", { name: "Context length" });
    expect(
      field.compareDocumentPosition(alert) & Node.DOCUMENT_POSITION_FOLLOWING,
    ).toBeTruthy();
    expect(
      alert.compareDocumentPosition(context) & Node.DOCUMENT_POSITION_FOLLOWING,
    ).toBeTruthy();
  });

  it("other folder roots are named, and a stopped service is not restarted", async () => {
    setup({
      ...HIDDEN,
      dir: "/run/user/961/.ollama",
      hiddenRoot: "/run/user",
      user: "ollama",
      running: false,
    });
    const alert = await screen.findByTestId("ollama-home");
    expect(within(alert).getByRole("alert").textContent).toBe(
      "Ollama can't read its own files in /run/user/961/.ollama: its systemd service hides /run/user (ProtectHome). Downloads from ollama.com and your Ollama account won't work until this is fixed.",
    );
    expect(
      within(alert).getByText("Fix asks for an administrator password."),
    ).toBeTruthy();
  });

  it("Fix: Fixing… while it runs, then the alert goes and Saved shows", async () => {
    const { simulator, backend, app } = setup();
    const held = holdFix(simulator);
    await screen.findByTestId("ollama-home");
    // Requêtes du serveur en cache (pages quittées) : à relire ensuite.
    app.queryClient.setQueryData(["models"], []);
    app.queryClient.setQueryData(accountKey, { state: "signedOut" });
    const reads = simulator.callsTo("get_ollama_home").length;
    await userEvent.click(fixButton());
    await waitFor(() => expect(held).toHaveLength(1));
    expect(fixButton().textContent).toBe("Fixing…");
    expect(fixButton().disabled).toBe(true);
    // Un second clic ne relance rien.
    await userEvent.click(fixButton());
    expect(simulator.callsTo("fix_ollama_home")).toHaveLength(1);

    backend.ollamaHome = null;
    held[0].finish({ kind: "done", changed: true, restarted: true });
    await waitFor(() => expect(box()).toBeNull());
    expect(simulator.callsTo("get_ollama_home").length).toBeGreaterThan(reads);
    expect(await screen.findByText("Saved")).toBeTruthy();
    // Ollama a redémarré : modèles et compte à relire.
    expect(app.queryClient.getQueryState(["models"])?.isInvalidated).toBe(true);
    expect(app.queryClient.getQueryState(accountKey)?.isInvalidated).toBe(true);
  });

  it("nothing changed (fixed elsewhere meanwhile): no Saved", async () => {
    const { simulator, backend } = setup();
    const held = holdFix(simulator);
    await screen.findByTestId("ollama-home");
    await userEvent.click(fixButton());
    await waitFor(() => expect(held).toHaveLength(1));
    backend.ollamaHome = null;
    held[0].finish({ kind: "done", changed: false, restarted: false });
    await waitFor(() => expect(box()).toBeNull());
    expect(screen.queryByText("Saved")).toBeNull();
  });

  it("prompt closed: nothing changes, Fix again", async () => {
    const { simulator } = setup();
    const held = holdFix(simulator);
    await screen.findByTestId("ollama-home");
    const reads = simulator.callsTo("get_ollama_home").length;
    await userEvent.click(fixButton());
    await waitFor(() => expect(held).toHaveLength(1));
    held[0].finish({ kind: "cancelled" });
    await waitFor(() => expect(fixButton().textContent).toBe("Fix"));
    expect(fixButton().disabled).toBe(false);
    expect(simulator.callsTo("get_ollama_home")).toHaveLength(reads);
    expect(within(box()!).getAllByRole("alert")).toHaveLength(1);
    expect(screen.queryByText("Saved")).toBeNull();
  });

  const failures: [HomeFixFailure, string][] = [
    ["notAuthorized", "Authentication failed. Nothing was changed."],
    [
      "unitChanged",
      "The Ollama service changed. Reopen Settings and try again.",
    ],
    [
      "notYours",
      "/home/alice/.ollama is not a folder of the Ollama service (user alice). Nothing was changed.",
    ],
    [
      "cannotCreate",
      "The Ollama service (user alice) can't create /home/alice/.ollama. Nothing was changed.",
    ],
    [
      "configFailed",
      "The Ollama service could not be changed. Everything was put back as before.",
    ],
    [
      "overridden",
      "Another setting of the Ollama service hides /home again. Everything was put back as before.",
    ],
    [
      "startFailed",
      "Ollama did not restart with this change, so it was undone. See: journalctl -u ollama",
    ],
    [
      "restoreFailed",
      "ChatonLM could not undo every change. Check the service with: systemctl status ollama",
    ],
    ["unexpected", "The Ollama service could not be changed."],
  ];

  it.each(failures)(
    "failure %s: its reason, with Details",
    async (reason, text) => {
      const { simulator } = setup();
      const held = holdFix(simulator);
      await screen.findByTestId("ollama-home");
      await userEvent.click(fixButton());
      await waitFor(() => expect(held).toHaveLength(1));
      held[0].finish({
        kind: "failed",
        reason,
        restored: true,
        detail: "chatonlm: something went wrong",
      });
      const alerts = await noticeAlerts();
      expect(alerts[1].textContent).toContain(text);
      expect(within(alerts[1]).getByText("Details")).toBeTruthy();
      expect(
        within(alerts[1]).getByText("chatonlm: something went wrong"),
      ).toBeTruthy();
      // L'alerte reste, Fix aussi.
      expect(fixButton().textContent).toBe("Fix");
      expect(screen.queryByText("Saved")).toBeNull();
    },
  );

  it("no password prompt in this session: the commands to copy", async () => {
    const { simulator } = setup();
    const held = holdFix(simulator);
    await screen.findByTestId("ollama-home");
    await userEvent.click(fixButton());
    await waitFor(() => expect(held).toHaveLength(1));
    held[0].finish({
      kind: "failed",
      reason: "noAgent",
      restored: false,
      detail: "",
    });
    const alerts = await noticeAlerts();
    expect(alerts[1].textContent).toContain(
      "No password prompt is available in this session. Run these commands in a terminal:",
    );
    expect(alerts[1].querySelector("code")?.textContent).toBe(HIDDEN.commands);
    expect(within(alerts[1]).queryByText("Details")).toBeNull();
  });

  it("refused by the app (downloads, another change): its reason", async () => {
    const { simulator } = setup();
    const held = holdFix(simulator);
    await screen.findByTestId("ollama-home");
    await userEvent.click(fixButton());
    await waitFor(() => expect(held).toHaveLength(1));
    held[0].reject({
      kind: "invalid",
      message: "wait for the model downloads to finish, then try again",
    });
    const alerts = await noticeAlerts();
    expect(alerts[1].textContent).toBe(
      "Couldn't fix this: wait for the model downloads to finish, then try again",
    );
    expect(fixButton().disabled).toBe(false);
  });

  it("read again after a Model location change (its drop-in can show the folder)", async () => {
    const { simulator, backend } = setup();
    backend.folderChecks.set("/home/alice/LLM models", {});
    const changes: ((outcome: unknown) => void)[] = [];
    simulator.handle(
      "change_models_location",
      () => new Promise((resolve) => changes.push(resolve)),
    );
    await screen.findByTestId("ollama-home");
    backend.folderPicks.push("/home/alice/LLM models");
    const browse = screen.getByRole<HTMLButtonElement>("button", {
      name: "Browse",
    });
    await waitFor(() => expect(browse.disabled).toBe(false));
    await userEvent.click(browse);
    const confirm = await screen.findByRole("alertdialog");
    await userEvent.click(
      within(confirm).getByRole("button", { name: "Leave them" }),
    );
    await waitFor(() => expect(changes).toHaveLength(1));
    const reads = simulator.callsTo("get_ollama_home").length;
    backend.ollamaHome = null;
    changes[0]({
      kind: "done",
      restarted: true,
      renamed: 0,
      copied: 0,
      skipped: 0,
      cleanupFailed: false,
    });
    await waitFor(() => expect(box()).toBeNull());
    expect(simulator.callsTo("get_ollama_home").length).toBeGreaterThan(reads);
  });

  it("a service that runs as root: an explanation, neither Fix nor commands", async () => {
    const { simulator } = setup({
      ...HIDDEN,
      user: "root",
      commands: null,
    });
    const alert = await screen.findByTestId("ollama-home");
    expect(within(alert).getByRole("alert").textContent).toBe(ALERT);
    expect(
      within(alert).getByText(
        "Ollama's service runs as root: ChatonLM won't change it. Run the service as another user (User= in its unit), or change its ProtectHome= setting yourself.",
      ),
    ).toBeTruthy();
    expect(within(alert).queryByRole("button")).toBeNull();
    expect(alert.querySelector("code")).toBeNull();
    expect(simulator.callsTo("fix_ollama_home")).toHaveLength(0);
  });

  it("without pkexec: the commands to copy, no Fix", async () => {
    setup({ ...HIDDEN, canElevate: false });
    const alert = await screen.findByTestId("ollama-home");
    expect(within(alert).queryByRole("button", { name: "Fix" })).toBeNull();
    expect(
      within(alert).getByText(
        "Fixing this needs administrator rights, and pkexec is not installed. Run these commands in a terminal:",
      ),
    ).toBeTruthy();
    expect(alert.querySelector("code")?.textContent).toBe(HIDDEN.commands);
    expect(
      within(alert).getByRole("button", { name: "Copy command to clipboard" }),
    ).toBeTruthy();
  });
});

describe("Fix in Settings › General", () => {
  const link = () =>
    screen.queryByRole("link", { name: "Fix in Settings › General" });

  it("under the account card that cannot be read, when the folder is hidden", async () => {
    const simulator = installSimulator();
    const backend = fakeBackend(simulator);
    backend.ollamaHome = { ...HIDDEN };
    backend.accountErrors.status = {
      kind: "ollama",
      message: "account unavailable",
    };
    const app = renderApp("/settings?section=account");
    const account = await screen.findByTestId("ollama-account");
    await within(account).findByTestId("account-failure");
    await waitFor(() => expect(link()).toBeTruthy());
    await userEvent.click(link()!);
    // Onglet General, avec l'alerte et Fix.
    await screen.findByTestId("ollama-home");
    expect(app.router.state.location.search).toEqual({});
    expect(
      screen.getByTestId("settings-tab-general").getAttribute("aria-current"),
    ).toBe("page");
  });

  it("not when the folder is visible", async () => {
    const simulator = installSimulator();
    const backend = fakeBackend(simulator);
    backend.accountErrors.status = {
      kind: "ollama",
      message: "account unavailable",
    };
    renderApp("/settings?section=account");
    await screen.findByTestId("account-failure");
    await waitFor(() =>
      expect(simulator.callsTo("get_ollama_home").length).toBeGreaterThan(0),
    );
    expect(link()).toBeNull();
  });

  it("not for another reason: the folder is not even read", async () => {
    const simulator = installSimulator();
    const backend = fakeBackend(simulator);
    backend.ollamaHome = { ...HIDDEN };
    backend.accountErrors.status = { kind: "ollama", message: "boom" };
    renderApp("/settings?section=account");
    await screen.findByTestId("account-failure");
    expect(link()).toBeNull();
    expect(simulator.callsTo("get_ollama_home")).toHaveLength(0);
  });

  it("under a failed ModelHub download", async () => {
    const model = "llama3.2:1b";
    const failed: HubDownload = {
      model,
      repo: null,
      label: null,
      state: "failed",
      completed: 0,
      total: 0,
      bytesPerSecond: null,
      error: KEY_ERROR,
      startedAt: 1,
      finishedAt: 2,
    };
    const { app } = renderHub("/hub/downloads", ({ backend, hub }) => {
      backend.ollamaHome = { ...HIDDEN };
      hub.downloads.push(failed);
    });
    const row = await screen.findByTestId("download");
    expect(within(row).getByRole("alert").textContent).toBe(KEY_ERROR);
    const fix = await within(row).findByRole("link", {
      name: "Fix in Settings › General",
    });
    await userEvent.click(fix);
    await waitFor(() => expect(app.path()).toBe("/settings"));
    await screen.findByTestId("ollama-home");
  });

  it("under a download that failed for another reason: nothing", async () => {
    const { simulator } = renderHub("/hub/downloads", ({ backend, hub }) => {
      backend.ollamaHome = { ...HIDDEN };
      hub.downloads.push({
        model: "llama3.2:1b",
        repo: null,
        label: null,
        state: "failed",
        completed: 0,
        total: 0,
        bytesPerSecond: null,
        error: "Hugging Face didn't answer in time. Try again.",
        startedAt: 1,
        finishedAt: 2,
      });
    });
    await screen.findByTestId("download");
    expect(link()).toBeNull();
    expect(simulator.callsTo("get_ollama_home")).toHaveLength(0);
  });

  it("under the error of a chat whose model could not be downloaded", async () => {
    const simulator = installSimulator();
    const backend = fakeBackend(simulator, {
      selectedModel: "chatonlm-pull-offline",
    });
    backend.ollamaHome = { ...HIDDEN };
    const refused = sequence("download-offline");
    for (const event of refused.events) {
      if (event.type === "error") event.message = KEY_ERROR;
    }
    simulator.queue(refused);
    const app = renderApp("/c/new");
    const textarea = await screen.findByLabelText("Message");
    await waitFor(() =>
      expect(screen.getByTitle("Select model").textContent).toBe(
        "chatonlm-pull-offline",
      ),
    );
    await userEvent.type(textarea, `${refused.request.content}{Enter}`);
    const fix = await screen.findByRole("link", {
      name: "Fix in Settings › General",
    });
    await userEvent.click(fix);
    await waitFor(() => expect(app.path()).toBe("/settings"));
    await screen.findByTestId("ollama-home");
  });
});

describe("Ollama's own folder in other languages", () => {
  /** Données montrées telles quelles : dossiers, utilisateur, commandes. */
  const DATA = [HIDDEN.dir, HIDDEN.hiddenRoot, HIDDEN.commands!, "alice"];
  // Pseudo-langue : « ⟦Fix⟧ », trouvé par motif.
  const anyFix = () =>
    within(box()!).getByRole<HTMLButtonElement>("button", { name: /Fix/ });

  /**
   * Langue réglée et affichée (chaque lecture des réglages rend celle du
   * moteur).
   */
  function speak(backend: ReturnType<typeof fakeBackend>, lang: "fr" | "es") {
    Object.assign(backend.settings, {
      language: lang,
      effectiveLanguage: lang,
    });
    act(() => setLanguage(lang));
  }

  it.each<[string, Partial<HiddenHome>]>([
    ["Fix, then a restart", {}],
    ["Fix, stopped service", { running: false }],
    ["without pkexec", { canElevate: false }],
    ["service run as root", { user: "root", commands: null }],
  ])("writes the alert from the catalogs (%s)", async (_, fields) => {
    setup({ ...HIDDEN, ...fields });
    await screen.findByTestId("ollama-home");
    usePseudoLanguage();
    await waitFor(() =>
      expect(within(box()!).getAllByRole("alert")[0].textContent).toMatch(/^⟦/),
    );
    expect(untranslatedTexts(box()!, [...DATA, "root"])).toEqual([]);
  });

  it("writes a failure, its Details and an IPC error from the catalogs", async () => {
    const { simulator } = setup();
    const held = holdFix(simulator);
    await screen.findByTestId("ollama-home");
    usePseudoLanguage();
    await userEvent.click(anyFix());
    await waitFor(() => expect(held).toHaveLength(1));
    const detail = "chatonlm: something went wrong";
    act(() =>
      held[0].finish({
        kind: "failed",
        reason: "notYours",
        restored: true,
        detail,
      }),
    );
    const alerts = await noticeAlerts();
    expect(alerts[1].querySelector("summary")?.textContent).toBe("⟦Details⟧");
    expect(untranslatedTexts(box()!, [...DATA, detail])).toEqual([]);

    await userEvent.click(anyFix());
    await waitFor(() => expect(held).toHaveLength(2));
    act(() => held[1].reject({ kind: "system", message: "pkexec crashed" }));
    await within(box()!).findByText("⟦Couldn't fix this: pkexec crashed⟧");
    expect(untranslatedTexts(box()!, [...DATA, "pkexec crashed"])).toEqual([]);
  });

  it("writes the link under an account error from the catalogs", async () => {
    const simulator = installSimulator();
    const backend = fakeBackend(simulator);
    backend.ollamaHome = { ...HIDDEN };
    backend.accountErrors.status = {
      kind: "ollama",
      message: "account unavailable",
    };
    renderApp("/settings?section=account");
    await screen.findByTestId("ollama-home-link");
    usePseudoLanguage();
    await waitFor(() =>
      expect(screen.getByTestId("ollama-home-link").textContent).toBe(
        "⟦Fix in Settings › General⟧",
      ),
    );
  });

  it("speaks French and Spanish: the alert, Fix, a failure", async () => {
    const { simulator, backend } = setup();
    const held = holdFix(simulator);
    await screen.findByTestId("ollama-home");
    speak(backend, "fr");
    expect(within(box()!).getAllByRole("alert")[0].textContent).toBe(
      "Ollama ne peut pas lire ses propres fichiers dans /home/alice/.ollama : son service systemd cache /home (ProtectHome). Les téléchargements depuis ollama.com et votre compte Ollama ne fonctionneront pas tant que ce ne sera pas corrigé.",
    );
    expect(
      within(box()!).getByRole("button", { name: "Corriger" })
        .nextElementSibling?.textContent,
    ).toBe(
      "« Corriger » demande un mot de passe d'administrateur, puis redémarre Ollama.",
    );
    await userEvent.click(
      within(box()!).getByRole("button", { name: "Corriger" }),
    );
    await waitFor(() => expect(held).toHaveLength(1));
    act(() =>
      held[0].finish({
        kind: "failed",
        reason: "cannotCreate",
        restored: true,
        detail: "",
      }),
    );
    const alerts = await noticeAlerts();
    expect(alerts[1].textContent).toBe(
      "Le service d'Ollama (utilisateur alice) ne peut pas créer /home/alice/.ollama. Rien n'a été changé.",
    );
    // Message déjà montré : il suit la langue.
    speak(backend, "es");
    expect(alerts[1].textContent).toBe(
      "El servicio de Ollama (usuario alice) no puede crear /home/alice/.ollama. No se cambió nada.",
    );
    expect(
      within(box()!).getByRole("button", { name: "Corregir" }),
    ).toBeTruthy();
  });
});
