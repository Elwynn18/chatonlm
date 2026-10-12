// Carte « Updates » de Settings › General face au simulateur : versions
// installées, état des mises à jour tenu à jour par « update-status »,
// « Check for updates » (une vérification à la fois), Download quand
// Auto-download est coupé, progression, « Restart to update » (qui reste
// « Restarting… » une fois acceptée), installation en cours, erreurs du
// moteur et commandes refusées avec Retry (une seule fois, et seulement
// dans l'état qu'elles supposent), échec gardé sous l'annonce (lastError),
// mot de passe d'un .deb dit à l'avance, mise à jour non prise en charge
// (trois raisons, dans les états que le moteur produit), annonces après une
// action de l'utilisateur seulement, carte montrée depuis la zone de
// notification (card=updates), réglage Auto-download (textes de
// l'officiel, « Saved », Reset sans effet), textes des catalogues en trois
// langues.

import {
  act,
  configure,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { UpdateErrorInfo } from "@/bindings/UpdateErrorInfo";
import type { UpdateInstaller } from "@/bindings/UpdateInstaller";
import type { UpdateState } from "@/bindings/UpdateState";
import type { UpdateStatus } from "@/bindings/UpdateStatus";
import { setLanguage } from "@/i18n";
import { type FakeBackend, fakeBackend } from "@/test/backend";
import { untranslatedTexts, usePseudoLanguage } from "@/test/i18n";
import { renderApp } from "@/test/renderApp";
import { installSimulator, type Simulator } from "@/test/simulator";
import { validateSettingsSearch } from "@/routes/settings";

configure({ asyncUtilTimeout: 5000 });
vi.setConfig({ testTimeout: 30_000 });

const MIB = 1024 * 1024;
const NOTES = "Faster startup.\nFixes the dictation shortcut.";
const AVAILABLE: UpdateState = {
  state: "available",
  version: "0.9.1",
  notes: NOTES,
  date: "2026-10-07T12:00:00Z",
};
const DOWNLOADING: UpdateState = {
  state: "downloading",
  version: "0.9.1",
  received: 4.5 * MIB,
  total: 18 * MIB,
};
const READY: UpdateState = { state: "ready", version: "0.9.1", notes: NOTES };
const INSTALLING: UpdateState = { state: "installing", version: "0.9.1" };
const UNREACHABLE: UpdateErrorInfo = {
  code: "install",
  detail: "ChatonLM could not reach the update server.",
};
const OFFLINE: UpdateErrorInfo = {
  code: "check",
  detail: "error sending request: connection refused",
};

function setup({
  state,
  autoDownload = true,
  checkedAt = null,
  ollama = null,
  installer = "appImage",
  lastError = null,
  path = "/settings",
}: {
  state?: UpdateState;
  autoDownload?: boolean;
  checkedAt?: number | null;
  ollama?: string | null;
  installer?: UpdateInstaller | null;
  lastError?: UpdateErrorInfo | null;
  path?: string;
} = {}) {
  const simulator = installSimulator();
  const backend = fakeBackend(simulator);
  backend.updates = {
    state: state ?? { state: "upToDate" },
    autoDownload,
    checkedAt,
    installer,
    lastError,
  };
  backend.versions = { chatonlm: "0.9.0", ollama };
  renderApp(path);
  return { simulator, backend };
}

/** Réponse de commande tenue jusqu'à `release()`. */
function held() {
  let release!: () => void;
  const answer = new Promise<null>((resolve) => {
    release = () => resolve(null);
  });
  return { answer, release };
}

/** Le moteur passe à un nouvel état : relu par update_status, et émis. */
async function moveTo(
  simulator: Simulator,
  backend: FakeBackend,
  state: UpdateState,
  fields: Partial<UpdateStatus> = {},
) {
  backend.updates = { ...backend.updates, ...fields, state };
  await simulator.emitEvent("update-status", backend.updates);
}

/** Carte affichée, écouteur de « update-status » inscrit. */
async function card(simulator: Simulator) {
  const found = await screen.findByTestId("updates");
  await waitFor(() =>
    expect(simulator.listening("update-status")).toBeGreaterThan(0),
  );
  return found;
}

const headline = () => screen.getByTestId("update-state");
const checkButton = () =>
  within(screen.getByTestId("updates")).getByRole("button", {
    name: "Check for updates",
  });
const autoSwitch = () =>
  screen.getByRole("switch", { name: "Auto-download updates" });

/** Date et heure courtes écrites par Intl, comme la carte. */
function dateTime(lang: string, time: number) {
  return new Intl.DateTimeFormat(lang, {
    dateStyle: "medium",
    timeStyle: "short",
  }).format(time);
}

describe("Updates card", () => {
  it("shows the installed versions and when ChatonLM was last checked", async () => {
    const checkedAt = Date.UTC(2026, 9, 7, 12, 30);
    const { simulator } = setup({ checkedAt, ollama: "0.12.3" });
    const updates = await card(simulator);
    expect(
      within(updates).getByRole("heading", { name: "Updates" }),
    ).toBeTruthy();
    await within(updates).findByText("ChatonLM 0.9.0");
    expect(within(updates).getByText("Built-in Ollama 0.12.3")).toBeTruthy();
    expect(headline().textContent).toBe("ChatonLM is up to date");
    expect(
      within(updates).getByText(`Last checked: ${dateTime("en", checkedAt)}`),
    ).toBeTruthy();
    // Carte à part, sous celle de General et au-dessus de Reset to defaults.
    const theme = screen.getByRole("radiogroup", { name: "Theme" });
    const reset = screen.getByRole("button", { name: "Reset to defaults" });
    expect(
      theme.compareDocumentPosition(updates) & Node.DOCUMENT_POSITION_FOLLOWING,
    ).toBeTruthy();
    expect(updates.contains(theme)).toBe(false);
    expect(
      updates.compareDocumentPosition(reset) & Node.DOCUMENT_POSITION_FOLLOWING,
    ).toBeTruthy();
    expect(simulator.callsTo("app_versions")).toHaveLength(1);
  });

  it("leaves out the built-in Ollama when the app has none", async () => {
    const { simulator } = setup();
    const updates = await card(simulator);
    await within(updates).findByText("ChatonLM 0.9.0");
    expect(within(updates).queryByText(/Ollama/)).toBeNull();
    expect(screen.queryByTestId("ollama-version")).toBeNull();
  });

  it("checks on demand, one check at a time, and announces the result", async () => {
    const { simulator, backend } = setup({
      state: { state: "idle" },
    });
    await card(simulator);
    expect(headline().textContent).toBe("Not checked yet");
    expect(
      screen.getByText(
        "ChatonLM checks for updates when it starts, then every hour.",
      ),
    ).toBeTruthy();
    // Pas d'annonce des vérifications de fond.
    expect(headline().getAttribute("aria-live")).toBeNull();
    const state = headline();
    // Comme le moteur : la commande rend la main la vérification finie.
    const check = held();
    simulator.handle("check_for_updates", () => check.answer);

    await userEvent.click(checkButton());
    expect(simulator.callsTo("check_for_updates")).toEqual([{}]);
    await moveTo(simulator, backend, { state: "checking" });
    await waitFor(() =>
      expect(headline().textContent).toBe("Checking for updates…"),
    );
    expect(checkButton().getAttribute("aria-disabled")).toBe("true");
    await userEvent.click(checkButton());
    expect(simulator.callsTo("check_for_updates")).toHaveLength(1);

    const checkedAt = Date.UTC(2026, 9, 7, 13, 0);
    await moveTo(simulator, backend, { state: "upToDate" }, { checkedAt });
    await act(async () => check.release());
    await waitFor(() =>
      expect(headline().textContent).toBe("ChatonLM is up to date"),
    );
    expect(
      screen.getByText(`Last checked: ${dateTime("en", checkedAt)}`),
    ).toBeTruthy();
    // Même région, devenue polie après le clic : le résultat est annoncé.
    expect(headline()).toBe(state);
    expect(state.getAttribute("aria-live")).toBe("polite");
    expect(checkButton().getAttribute("aria-disabled")).toBe("false");
    // Le bouton a gardé le focus pendant la vérification.
    expect(document.activeElement).toBe(checkButton());
  });

  it("stops announcing once the check the user asked for is over", async () => {
    const { simulator, backend } = setup();
    await card(simulator);
    const check = held();
    simulator.handle("check_for_updates", () => check.answer);
    await userEvent.click(checkButton());
    await moveTo(simulator, backend, { state: "checking" });
    await moveTo(simulator, backend, { state: "upToDate" });
    await act(async () => check.release());
    await waitFor(() =>
      expect(checkButton().getAttribute("aria-disabled")).toBe("false"),
    );
    expect(headline().getAttribute("aria-live")).toBe("polite");
    // Résultat lu : la région se tait de nouveau.
    await waitFor(() =>
      expect(headline().getAttribute("aria-live")).toBeNull(),
    );
    // La vérification horaire suivante n'est pas annoncée.
    await moveTo(simulator, backend, { state: "checking" });
    await waitFor(() =>
      expect(headline().textContent).toBe("Checking for updates…"),
    );
    expect(headline().getAttribute("aria-live")).toBeNull();
    await moveTo(simulator, backend, AVAILABLE);
    await waitFor(() =>
      expect(headline().textContent).toBe(
        "ChatonLM version 0.9.1 is available",
      ),
    );
    expect(headline().getAttribute("aria-live")).toBeNull();
  });

  it("keeps announcing a download the user asked for until it is ready", async () => {
    const { simulator, backend } = setup({
      state: AVAILABLE,
      autoDownload: false,
    });
    const updates = await card(simulator);
    await userEvent.click(
      await within(updates).findByRole("button", { name: "Download" }),
    );
    await moveTo(simulator, backend, DOWNLOADING);
    // Plus long que le délai de fin d'annonce : le téléchargement continue.
    await act(() => new Promise((resolve) => setTimeout(resolve, 1500)));
    expect(headline().getAttribute("aria-live")).toBe("polite");
    await moveTo(simulator, backend, READY);
    await waitFor(() =>
      expect(headline().textContent).toBe(
        "ChatonLM version 0.9.1 is ready to install",
      ),
    );
    expect(headline().getAttribute("aria-live")).toBe("polite");
    await waitFor(() =>
      expect(headline().getAttribute("aria-live")).toBeNull(),
    );
  });

  it("downloads on demand when Auto-download is off, then restarts to update", async () => {
    const { simulator, backend } = setup({
      state: AVAILABLE,
      autoDownload: false,
    });
    const updates = await card(simulator);
    await waitFor(() =>
      expect(headline().textContent).toBe(
        "ChatonLM version 0.9.1 is available",
      ),
    );
    expect(within(updates).getByText("Released on Oct 7, 2026")).toBeTruthy();
    expect(within(updates).getByText("What's new")).toBeTruthy();
    expect(screen.getByTestId("update-notes").textContent).toBe(NOTES);
    // Nouveautés nommées par leur titre (groupe).
    expect(
      within(updates)
        .getByRole("group", { name: "What's new" })
        .contains(screen.getByTestId("update-notes")),
    ).toBe(true);

    await userEvent.click(
      within(updates).getByRole("button", { name: "Download" }),
    );
    expect(simulator.callsTo("download_update")).toEqual([{}]);
    await moveTo(simulator, backend, DOWNLOADING);
    const bar = await screen.findByRole("progressbar", {
      name: "Downloading ChatonLM version 0.9.1…",
    });
    expect(headline().textContent).toBe("Downloading ChatonLM version 0.9.1…");
    expect(bar.getAttribute("aria-valuenow")).toBe("25");
    expect(screen.getByTestId("update-progress").textContent).toBe(
      "4.5 MB of 18 MB (25%)",
    );
    expect(bar.getAttribute("aria-valuetext")).toBe("4.5 MB of 18 MB (25%)");
    // Download parti avec son état : le focus va au titre de la carte.
    expect(
      within(updates).queryByRole("button", { name: "Download" }),
    ).toBeNull();
    expect(document.activeElement).toBe(
      within(updates).getByRole("heading", { name: "Updates" }),
    );
    // Pas de vérification pendant un téléchargement.
    expect(checkButton().getAttribute("aria-disabled")).toBe("true");
    await userEvent.click(checkButton());
    expect(simulator.callsTo("check_for_updates")).toEqual([]);

    await moveTo(simulator, backend, READY);
    await waitFor(() =>
      expect(headline().textContent).toBe(
        "ChatonLM version 0.9.1 is ready to install",
      ),
    );
    expect(
      screen.getByText(
        "ChatonLM will close, then start again with the new version.",
      ),
    ).toBeTruthy();
    expect(screen.getByTestId("update-notes").textContent).toBe(NOTES);
    expect(screen.queryByRole("progressbar")).toBeNull();
    // AppImage : remplacée sans mot de passe.
    expect(screen.queryByTestId("update-password")).toBeNull();
    // L'app relancée ne répond jamais.
    simulator.handle("restart_to_update", () => new Promise(() => {}));
    await userEvent.click(
      screen.getByRole("button", { name: "Restart to update" }),
    );
    const restarting = await screen.findByRole("button", {
      name: "Restarting…",
    });
    expect(restarting.getAttribute("aria-disabled")).toBe("true");
    await userEvent.click(restarting);
    expect(simulator.callsTo("restart_to_update")).toEqual([{}]);
  });

  it("runs one action at a time", async () => {
    const { simulator } = setup({ state: AVAILABLE, autoDownload: false });
    const updates = await card(simulator);
    // Téléchargement lancé, pas encore commencé : la commande attend.
    simulator.handle("download_update", () => new Promise(() => {}));
    const download = await within(updates).findByRole("button", {
      name: "Download",
    });
    await userEvent.click(download);
    expect(download.getAttribute("aria-disabled")).toBe("true");
    await userEvent.click(download);
    expect(checkButton().getAttribute("aria-disabled")).toBe("true");
    await userEvent.click(checkButton());
    expect(simulator.callsTo("download_update")).toHaveLength(1);
    expect(simulator.callsTo("check_for_updates")).toEqual([]);
  });

  it("offers no Download button when updates are downloaded automatically", async () => {
    const { simulator } = setup({ state: AVAILABLE });
    const updates = await card(simulator);
    await waitFor(() =>
      expect(headline().textContent).toBe(
        "ChatonLM version 0.9.1 is available",
      ),
    );
    expect(
      within(updates).queryByRole("button", { name: "Download" }),
    ).toBeNull();
  });

  it("shows a download of unknown size without a percentage", async () => {
    const { simulator } = setup({
      state: {
        state: "downloading",
        version: "0.9.1",
        received: 3 * MIB,
        total: null,
      },
    });
    await card(simulator);
    const bar = await screen.findByRole("progressbar");
    expect(bar.getAttribute("aria-valuenow")).toBeNull();
    expect(screen.getByTestId("update-progress").textContent).toBe("3.0 MB");
  });

  it.each([
    ["check", "Couldn't check for updates"],
    ["download", "Couldn't download the update"],
    ["signature", "The downloaded update couldn't be verified"],
    ["install", "Couldn't install the update"],
    ["io", "Couldn't read the downloaded update"],
  ] as const)(
    "shows a %s error with its detail, and Retry checks again",
    async (code, title) => {
      const detail = "error sending request: connection refused";
      const { simulator } = setup({
        state: { state: "error", code, detail },
      });
      await card(simulator);
      await waitFor(() => expect(headline().textContent).toBe(title));
      expect(screen.getByRole("alert").textContent).toBe(detail);
      await userEvent.click(screen.getByRole("button", { name: "Retry" }));
      expect(simulator.callsTo("check_for_updates")).toEqual([{}]);
      expect(simulator.callsTo("download_update")).toEqual([]);
      expect(simulator.callsTo("restart_to_update")).toEqual([]);
      // Check for updates reste possible, la commande finie.
      await waitFor(() =>
        expect(checkButton().getAttribute("aria-disabled")).toBe("false"),
      );
    },
  );

  it("shows a refused command until the state changes", async () => {
    const { simulator, backend } = setup();
    await card(simulator);
    backend.updateErrors.check = {
      kind: "system",
      message: "the update server sent an invalid answer",
    };
    await userEvent.click(checkButton());
    const alert = await screen.findByRole("alert");
    expect(
      [...alert.querySelectorAll("p")].map((line) => line.textContent),
    ).toEqual([
      "Couldn't check for updates",
      "the update server sent an invalid answer",
    ]);
    // L'état reste lisible au-dessus.
    expect(headline().textContent).toBe("ChatonLM is up to date");

    await userEvent.click(screen.getByRole("button", { name: "Retry" }));
    expect(simulator.callsTo("check_for_updates")).toHaveLength(2);
    await screen.findByRole("alert");

    // Un nouvel état du moteur la remplace.
    await moveTo(simulator, backend, { state: "checking" });
    await waitFor(() => expect(screen.queryByRole("alert")).toBeNull());
    expect(screen.queryByRole("button", { name: "Retry" })).toBeNull();
  });

  it("shows a refused restart, without staying on Restarting…", async () => {
    const { simulator, backend } = setup({ state: READY });
    await card(simulator);
    backend.updateErrors.restart = {
      kind: "system",
      message: "Request dismissed",
    };
    await userEvent.click(
      await screen.findByRole("button", { name: "Restart to update" }),
    );
    const alert = await screen.findByRole("alert");
    expect(alert.textContent).toContain("Couldn't install the update");
    expect(alert.textContent).toContain("Request dismissed");
    expect(screen.queryByRole("button", { name: "Restarting…" })).toBeNull();
    expect(
      screen
        .getByRole("button", { name: "Restart to update" })
        .getAttribute("aria-disabled"),
    ).toBe("false");
  });

  it("says a failure once when the engine reports it as its state too", async () => {
    const { simulator, backend } = setup({ state: READY });
    await card(simulator);
    const detail = "pkexec: Request dismissed";
    // Le moteur pose l'état d'erreur, relu par la carte, puis refuse.
    simulator.handle("restart_to_update", () => {
      backend.updates = {
        ...backend.updates,
        state: { state: "error", code: "install", detail },
      };
      throw { kind: "system", message: detail };
    });
    await userEvent.click(
      await screen.findByRole("button", { name: "Restart to update" }),
    );
    await waitFor(() =>
      expect(headline().textContent).toBe("Couldn't install the update"),
    );
    await waitFor(() =>
      expect(screen.getByRole("button", { name: "Retry" })).toBeTruthy(),
    );
    expect(screen.getAllByRole("alert").map((a) => a.textContent)).toEqual([
      detail,
    ]);
  });

  it("stays on Restarting… once the restart is accepted", async () => {
    const { simulator } = setup({ state: READY });
    await card(simulator);
    // Le moteur a accepté : ChatonLM se ferme (l'état n'a pas encore changé).
    await userEvent.click(
      await screen.findByRole("button", { name: "Restart to update" }),
    );
    await waitFor(() =>
      expect(simulator.callsTo("restart_to_update")).toHaveLength(1),
    );
    await waitFor(() =>
      expect(simulator.callsTo("update_status").length).toBeGreaterThan(1),
    );
    await act(() => new Promise((resolve) => setTimeout(resolve, 50)));
    const restarting = screen.getByRole("button", { name: "Restarting…" });
    expect(restarting.getAttribute("aria-disabled")).toBe("true");
    expect(screen.queryByRole("button", { name: "Restart to update" })).toBe(
      null,
    );
    expect(checkButton().getAttribute("aria-disabled")).toBe("true");
  });

  it.each([
    ["appImage", null],
    ["deb", "Your system is asking for your password to install it."],
  ] as const)(
    "says the update is being installed (%s), without Restart to update",
    async (installer, password) => {
      const { simulator, backend } = setup({ state: READY, installer });
      const updates = await card(simulator);
      await screen.findByRole("button", { name: "Restart to update" });
      // Lancée depuis la zone de notification : les services s'arrêtent.
      await moveTo(simulator, backend, INSTALLING);
      await waitFor(() =>
        expect(headline().textContent).toBe(
          "Installing ChatonLM version 0.9.1…",
        ),
      );
      expect(
        within(updates).getByText(
          "ChatonLM is stopping its services, then will start again with the new version.",
        ),
      ).toBeTruthy();
      expect(
        within(updates).queryByRole("button", { name: "Restart to update" }),
      ).toBeNull();
      expect(
        within(updates).queryByRole("button", { name: "Restarting…" }),
      ).toBeNull();
      expect(screen.queryByTestId("update-password")?.textContent ?? null).toBe(
        password,
      );
      expect(checkButton().getAttribute("aria-disabled")).toBe("true");
      await userEvent.click(checkButton());
      expect(simulator.callsTo("check_for_updates")).toEqual([]);
    },
  );

  it("shows a restart refused from the system tray under the ready update", async () => {
    // Relance refusée (serveur injoignable) : le moteur garde « prête » et
    // note le refus ; la carte le montre, « Restart to update » reste.
    const { simulator } = setup({ state: READY, lastError: UNREACHABLE });
    const updates = await card(simulator);
    await waitFor(() =>
      expect(headline().textContent).toBe(
        "ChatonLM version 0.9.1 is ready to install",
      ),
    );
    const alert = within(updates).getByRole("alert");
    expect([...alert.querySelectorAll("p")].map((p) => p.textContent)).toEqual([
      "Couldn't install the update",
      UNREACHABLE.detail,
    ]);
    const restart = within(updates).getByRole("button", {
      name: "Restart to update",
    });
    expect(restart.getAttribute("aria-disabled")).toBe("false");
    expect(within(updates).queryByRole("button", { name: "Retry" })).toBeNull();
  });

  it("says a refused restart once when the engine keeps it too", async () => {
    const { simulator, backend } = setup({ state: READY });
    const updates = await card(simulator);
    // Le moteur garde le refus (lastError), relu par la carte, puis refuse.
    simulator.handle("restart_to_update", () => {
      backend.updates = { ...backend.updates, lastError: UNREACHABLE };
      throw { kind: "invalid", message: UNREACHABLE.detail };
    });
    await userEvent.click(
      await screen.findByRole("button", { name: "Restart to update" }),
    );
    await waitFor(() =>
      expect(screen.getByTestId("update-last-error")).toBeTruthy(),
    );
    await waitFor(() =>
      expect(
        within(updates)
          .getByRole("button", { name: "Restart to update" })
          .getAttribute("aria-disabled"),
      ).toBe("false"),
    );
    expect(
      within(updates)
        .getAllByRole("alert")
        .map((alert) => alert.textContent),
    ).toEqual([`Couldn't install the update${UNREACHABLE.detail}`]);
    expect(within(updates).queryByRole("button", { name: "Retry" })).toBeNull();
  });

  it("keeps the update offered when a background check fails, and says so quietly", async () => {
    const { simulator, backend } = setup({
      state: AVAILABLE,
      autoDownload: false,
    });
    const updates = await card(simulator);
    await within(updates).findByRole("button", { name: "Download" });
    // Vérification horaire hors ligne : l'annonce reste, l'échec est noté.
    await moveTo(simulator, backend, AVAILABLE, { lastError: OFFLINE });
    const note = await screen.findByTestId("update-last-error");
    expect(note.textContent).toBe(
      `The last check for updates failed: ${OFFLINE.detail}`,
    );
    expect(note.getAttribute("role")).toBeNull();
    expect(within(updates).queryByRole("alert")).toBeNull();
    expect(headline().textContent).toBe("ChatonLM version 0.9.1 is available");
    expect(screen.getByTestId("update-notes").textContent).toBe(NOTES);
    expect(
      within(updates).getByRole("button", { name: "Download" }),
    ).toBeTruthy();
    // Prête : Restart to update reste aussi.
    await moveTo(simulator, backend, READY, { lastError: OFFLINE });
    await within(updates).findByRole("button", { name: "Restart to update" });
    expect(screen.getByTestId("update-last-error").textContent).toBe(
      `The last check for updates failed: ${OFFLINE.detail}`,
    );
    // Vérification réussie : la note disparaît.
    await moveTo(simulator, backend, READY, { lastError: null });
    await waitFor(() =>
      expect(screen.queryByTestId("update-last-error")).toBeNull(),
    );
  });

  it("does not repeat a refused restart once the engine says why", async () => {
    const { simulator, backend } = setup({ state: READY });
    const updates = await card(simulator);
    const reason =
      "ChatonLM can't replace the AppImage in its folder. Move it to a folder of your own (for example ~/Applications), then start ChatonLM again from there.";
    // Dossier devenu non inscriptible : l'état le dit, la commande refuse.
    simulator.handle("restart_to_update", () => {
      backend.updates = {
        ...backend.updates,
        state: {
          state: "unsupported",
          reason: "readOnlyFolder",
          version: "0.9.1",
        },
      };
      throw {
        kind: "invalid",
        message: "ChatonLM cannot replace its AppImage in this folder.",
      };
    });
    await userEvent.click(
      await screen.findByRole("button", { name: "Restart to update" }),
    );
    await waitFor(() =>
      expect(headline().textContent).toBe(
        "ChatonLM version 0.9.1 is available",
      ),
    );
    await waitFor(() =>
      expect(checkButton().getAttribute("aria-disabled")).toBe("false"),
    );
    expect(within(updates).getByText(reason)).toBeTruthy();
    expect(within(updates).queryByRole("alert")).toBeNull();
    expect(
      within(updates).queryByText(
        "ChatonLM cannot replace its AppImage in this folder.",
      ),
    ).toBeNull();
    expect(within(updates).queryByRole("button", { name: "Retry" })).toBeNull();
  });

  it("tells beforehand that a .deb asks for the password", async () => {
    const { simulator } = setup({ state: READY, installer: "deb" });
    await card(simulator);
    expect((await screen.findByTestId("update-password")).textContent).toBe(
      "Your system will ask for your password to install it.",
    );
    expect(
      screen.getByRole("button", { name: "Restart to update" }),
    ).toBeTruthy();
  });

  it("Windows installer: says it will appear for a moment, never asks for a password", async () => {
    const { simulator, backend } = setup({ state: READY, installer: "nsis" });
    await card(simulator);
    expect((await screen.findByTestId("update-installer")).textContent).toBe(
      "The ChatonLM installer will appear for a moment.",
    );
    expect(screen.queryByTestId("update-password")).toBeNull();
    expect(
      screen.getByRole("button", { name: "Restart to update" }),
    ).toBeTruthy();
    await moveTo(simulator, backend, INSTALLING);
    await waitFor(() =>
      expect(headline().textContent).toBe("Installing ChatonLM version 0.9.1…"),
    );
    expect(screen.queryByTestId("update-installer")).toBeNull();
    expect(screen.queryByTestId("update-password")).toBeNull();
  });

  it("Windows: says the installation folder can't be written, the update still announced", async () => {
    const { simulator } = setup({
      state: {
        state: "unsupported",
        reason: "installFolderReadOnly",
        version: "0.9.1",
      },
      installer: "nsis",
    });
    const updates = await card(simulator);
    await waitFor(() =>
      expect(headline().textContent).toBe(
        "ChatonLM version 0.9.1 is available",
      ),
    );
    expect(
      within(updates).getByText(
        "ChatonLM can't write to its installation folder. Install it again with its installer, in a folder of your own.",
      ),
    ).toBeTruthy();
    expect(
      within(updates).queryByRole("button", { name: "Download" }),
    ).toBeNull();
  });

  it.each([
    [
      "noPackage",
      "This copy of ChatonLM wasn't installed from a package (AppImage or .deb), so it can't update itself.",
    ],
    [
      "packageManager",
      "ChatonLM was installed by your package manager: updates come through it.",
    ],
    // Windows : copie posée sans l'installateur.
    [
      "notInstalled",
      "This copy of ChatonLM wasn't installed with its installer, so it can't update itself. Download the latest installer to update.",
    ],
  ] as const)(
    "says why ChatonLM can't update itself (%s), with Check and Auto-download disabled",
    async (reason, text) => {
      // Ce que le moteur produit : ni installeur ni version annoncée.
      const { simulator } = setup({
        state: { state: "unsupported", reason, version: null },
        installer: null,
      });
      const updates = await card(simulator);
      await waitFor(() =>
        expect(headline().textContent).toBe(
          "Updates can't be installed from ChatonLM",
        ),
      );
      const why = within(updates).getByText(text);
      // Check for updates : désactivé, décrit par la raison, sans effet.
      const check = checkButton();
      expect(check.getAttribute("aria-disabled")).toBe("true");
      expect(check.getAttribute("aria-describedby")).toBe(why.id);
      await userEvent.click(check);
      expect(simulator.callsTo("check_for_updates")).toEqual([]);
      // Auto-download : désactivé, décrit aussi par la raison.
      const toggle = autoSwitch();
      expect((toggle as HTMLButtonElement).disabled).toBe(true);
      expect(toggle.getAttribute("aria-describedby")?.split(" ")).toContain(
        why.id,
      );
      await userEvent.click(toggle);
      expect(simulator.callsTo("set_auto_download")).toEqual([]);
      expect(
        within(updates).queryByRole("button", { name: "Download" }),
      ).toBeNull();
    },
  );

  it("says the AppImage must be started again from a folder of the user's own", async () => {
    const text =
      "ChatonLM can't replace the AppImage in its folder. Move it to a folder of your own (for example ~/Applications), then start ChatonLM again from there.";
    // Ce que le moteur produit : une AppImage, la version annoncée ou non.
    const { simulator, backend } = setup({
      state: { state: "unsupported", reason: "readOnlyFolder", version: null },
    });
    const updates = await card(simulator);
    await waitFor(() =>
      expect(headline().textContent).toBe(
        "Updates can't be installed from ChatonLM",
      ),
    );
    expect(within(updates).getByText(text)).toBeTruthy();
    await moveTo(simulator, backend, {
      state: "unsupported",
      reason: "readOnlyFolder",
      version: "0.9.1",
    });
    await waitFor(() =>
      expect(headline().textContent).toBe(
        "ChatonLM version 0.9.1 is available",
      ),
    );
    expect(within(updates).getByText(text)).toBeTruthy();
    expect(
      within(updates).queryByRole("button", { name: "Download" }),
    ).toBeNull();
    // Vérifier reste utile (une version plus récente), le réglage aussi
    // (pour une relance depuis un autre dossier).
    expect(checkButton().getAttribute("aria-disabled")).toBe("false");
    expect(checkButton().getAttribute("aria-describedby")).toBeNull();
    expect((autoSwitch() as HTMLButtonElement).disabled).toBe(false);
  });

  it("keeps a state received while the status is being read", async () => {
    const { simulator, backend } = setup();
    await card(simulator);
    await waitFor(() =>
      expect(headline().textContent).toBe("ChatonLM is up to date"),
    );
    // Réponse plus ancienne que l'évènement arrivé pendant la lecture.
    simulator.handle("update_status", async () => {
      const stale = backend.updates;
      await simulator.emitEvent("update-status", {
        ...stale,
        state: AVAILABLE,
      });
      return stale;
    });
    await userEvent.click(checkButton());
    await waitFor(() =>
      expect(simulator.callsTo("update_status").length).toBeGreaterThan(1),
    );
    await waitFor(() =>
      expect(checkButton().getAttribute("aria-disabled")).toBe("false"),
    );
    expect(headline().textContent).toBe("ChatonLM version 0.9.1 is available");
  });

  it("stops listening when the card goes away", async () => {
    const { simulator } = setup();
    await card(simulator);
    expect(simulator.listening("update-status")).toBe(1);
    await userEvent.click(screen.getByTestId("settings-tab-models"));
    await screen.findByTestId("models-settings");
    await waitFor(() => expect(simulator.listening("update-status")).toBe(0));
  });

  it("explains a status that can't be read, and tries again", async () => {
    const { simulator, backend } = setup();
    const real = backend.updates;
    let fail = true;
    simulator.handle("update_status", () => {
      if (fail) throw { kind: "system", message: "database is locked" };
      return real;
    });
    await card(simulator);
    const alert = await screen.findByRole("alert");
    expect(alert.textContent).toBe(
      "Couldn't read the update status: database is locked",
    );
    fail = false;
    await userEvent.click(screen.getByRole("button", { name: "Try again" }));
    await waitFor(() =>
      expect(headline().textContent).toBe("ChatonLM is up to date"),
    );
    expect(screen.queryByRole("alert")).toBeNull();
  });
});

describe("Opened for updates from the system tray (card=updates)", () => {
  afterEach(() => {
    delete (Element.prototype as Partial<Element>).scrollIntoView;
  });

  /** jsdom n'a pas scrollIntoView : cartes défilées, dans l'ordre. */
  function spyScroll() {
    const scrolled: Element[] = [];
    Element.prototype.scrollIntoView = function (this: Element) {
      scrolled.push(this);
    };
    return scrolled;
  }

  it("types the card, and nothing else", () => {
    expect(validateSettingsSearch({ card: "updates" })).toEqual({
      card: "updates",
    });
    expect(
      validateSettingsSearch({ section: "general", card: "updates" }),
    ).toEqual({ section: "general", card: "updates" });
    expect(validateSettingsSearch({ card: "other" })).toEqual({});
  });

  it("scrolls the card into view and focuses its title", async () => {
    const scrolled = spyScroll();
    const { simulator } = setup({
      state: READY,
      path: "/settings?section=general&card=updates",
    });
    const updates = await card(simulator);
    const title = within(updates).getByRole("heading", { name: "Updates" });
    await waitFor(() => expect(document.activeElement).toBe(title));
    expect(scrolled).toEqual([updates]);
  });

  it("does it again on each request, even from elsewhere", async () => {
    const scrolled = spyScroll();
    const { simulator } = setup({
      state: READY,
      path: "/settings?card=updates",
    });
    const updates = await card(simulator);
    const title = within(updates).getByRole("heading", { name: "Updates" });
    await waitFor(() => expect(document.activeElement).toBe(title));
    const elsewhere = screen.getByRole("button", { name: "Reset to defaults" });
    act(() => elsewhere.focus());
    await waitFor(() => expect(simulator.listening("navigate")).toBe(1));
    await simulator.emitEvent(
      "navigate",
      "/settings?section=general&card=updates",
    );
    await waitFor(() => expect(document.activeElement).toBe(title));
    expect(scrolled).toEqual([updates, updates]);
  });

  it("leaves the focus alone when Settings is opened for something else", async () => {
    const scrolled = spyScroll();
    const { simulator } = setup({ state: READY });
    await card(simulator);
    await screen.findByRole("button", { name: "Restart to update" });
    await act(() => new Promise((resolve) => setTimeout(resolve, 50)));
    expect(document.activeElement?.textContent).not.toBe("Updates");
    expect(scrolled).toEqual([]);
  });
});

describe("Auto-download updates", () => {
  it("is turned off and on with the official texts, and Saved each time", async () => {
    const { simulator, backend } = setup();
    await card(simulator);
    const toggle = await waitFor(autoSwitch);
    expect(toggle.getAttribute("aria-checked")).toBe("true");
    const description = screen.getByText(
      "Automatically download updates when available.",
    );
    expect(toggle.getAttribute("aria-describedby")).toBe(description.id);

    await userEvent.click(toggle);
    await screen.findByText("Saved");
    expect(simulator.callsTo("set_auto_download")).toEqual([
      { enabled: false },
    ]);
    expect(backend.updates.autoDownload).toBe(false);
    await waitFor(() =>
      expect(autoSwitch().getAttribute("aria-checked")).toBe("false"),
    );
    expect(
      screen.getByText("Updates will not be downloaded automatically."),
    ).toBeTruthy();

    await userEvent.click(autoSwitch());
    await waitFor(() =>
      expect(autoSwitch().getAttribute("aria-checked")).toBe("true"),
    );
    expect(simulator.callsTo("set_auto_download")).toEqual([
      { enabled: false },
      { enabled: true },
    ]);
  });

  it("shows why the choice could not be saved, without Saved", async () => {
    const { simulator, backend } = setup();
    await card(simulator);
    backend.updateErrors.autoDownload = {
      kind: "system",
      message: "database is locked",
    };
    await userEvent.click(await waitFor(autoSwitch));
    const alert = await screen.findByRole("alert");
    expect(alert.textContent).toBe(
      "Couldn't save this setting: database is locked",
    );
    expect(screen.queryByText("Saved")).toBeNull();
    expect(autoSwitch().getAttribute("aria-checked")).toBe("true");
    // Réussi ensuite : l'erreur disparaît.
    backend.updateErrors.autoDownload = null;
    await userEvent.click(autoSwitch());
    await screen.findByText("Saved");
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("is left alone by Reset to defaults", async () => {
    const { simulator, backend } = setup({ autoDownload: false });
    await card(simulator);
    await waitFor(() =>
      expect(autoSwitch().getAttribute("aria-checked")).toBe("false"),
    );
    await userEvent.click(
      screen.getByRole("button", { name: "Reset to defaults" }),
    );
    await screen.findByText("Saved");
    expect(simulator.callsTo("set_auto_download")).toEqual([]);
    expect(backend.updates.autoDownload).toBe(false);
    expect(autoSwitch().getAttribute("aria-checked")).toBe("false");
  });
});

describe("Updates card languages", () => {
  it("takes every text from the catalogs", async () => {
    const detail = "connection refused";
    const { simulator, backend } = setup({
      state: AVAILABLE,
      autoDownload: false,
      checkedAt: Date.UTC(2026, 9, 7, 12, 0),
      ollama: "0.12.3",
    });
    const updates = await card(simulator);
    await within(updates).findByText("Built-in Ollama 0.12.3");
    usePseudoLanguage();
    await within(updates).findByText("⟦Updates⟧");
    const allow = [NOTES, detail, OFFLINE.detail, UNREACHABLE.detail];
    const states: UpdateState[] = [
      { state: "idle" },
      { state: "checking" },
      { state: "upToDate" },
      AVAILABLE,
      DOWNLOADING,
      { state: "downloading", version: "0.9.1", received: MIB, total: null },
      READY,
      INSTALLING,
      { state: "error", code: "check", detail },
      { state: "error", code: "download", detail },
      { state: "error", code: "signature", detail },
      { state: "error", code: "install", detail },
      { state: "error", code: "io", detail },
      { state: "unsupported", reason: "readOnlyFolder", version: null },
      { state: "unsupported", reason: "readOnlyFolder", version: "0.9.1" },
    ];
    for (const state of states) {
      await moveTo(simulator, backend, state);
      expect(untranslatedTexts(updates, allow)).toEqual([]);
    }
    // Sans installeur : raisons, boutons désactivés.
    for (const reason of ["noPackage", "packageManager"] as const) {
      await moveTo(
        simulator,
        backend,
        { state: "unsupported", reason, version: null },
        { installer: null },
      );
      expect(untranslatedTexts(updates, allow)).toEqual([]);
    }
    // .deb : mot de passe dit à l'avance, puis demandé.
    await moveTo(simulator, backend, READY, { installer: "deb" });
    await screen.findByTestId("update-password");
    expect(untranslatedTexts(updates, allow)).toEqual([]);
    await moveTo(simulator, backend, INSTALLING, { installer: "deb" });
    expect(untranslatedTexts(updates, allow)).toEqual([]);
    // Échecs gardés sous l'annonce : vérification de fond, relance refusée.
    await moveTo(simulator, backend, AVAILABLE, {
      installer: "appImage",
      lastError: OFFLINE,
    });
    await screen.findByTestId("update-last-error");
    expect(untranslatedTexts(updates, allow)).toEqual([]);
    await moveTo(simulator, backend, READY, { lastError: UNREACHABLE });
    await within(updates).findByRole("alert");
    expect(untranslatedTexts(updates, allow)).toEqual([]);
    // Commande refusée et réglage non enregistré.
    await moveTo(
      simulator,
      backend,
      { state: "upToDate" },
      { lastError: null },
    );
    backend.updateErrors.check = { kind: "system", message: detail };
    backend.updateErrors.autoDownload = { kind: "system", message: detail };
    await userEvent.click(
      within(updates).getByRole("button", { name: "⟦Check for updates⟧" }),
    );
    await userEvent.click(
      screen.getByRole("switch", { name: "⟦Auto-download updates⟧" }),
    );
    await waitFor(() =>
      expect(within(updates).getAllByRole("alert")).toHaveLength(2),
    );
    expect(untranslatedTexts(updates, allow)).toEqual([]);
  });

  it("speaks French and Spanish", async () => {
    const checkedAt = Date.UTC(2026, 9, 7, 12, 0);
    const { simulator, backend } = setup({
      state: DOWNLOADING,
      checkedAt,
      ollama: "0.12.3",
    });
    const updates = await card(simulator);
    await within(updates).findByText("Built-in Ollama 0.12.3");
    act(() => setLanguage("fr"));
    await within(updates).findByRole("heading", { name: "Mises à jour" });
    expect(within(updates).getByText("Ollama intégré 0.12.3")).toBeTruthy();
    expect(headline().textContent).toBe(
      "Téléchargement de la version 0.9.1 de ChatonLM…",
    );
    expect(screen.getByTestId("update-progress").textContent).toMatch(
      /^4,5\sMo sur 18\sMo \(25\s%\)$/,
    );
    expect(
      screen.getByRole("switch", {
        name: "Télécharger automatiquement les mises à jour",
      }),
    ).toBeTruthy();
    await moveTo(simulator, backend, AVAILABLE, { autoDownload: false });
    await within(updates).findByText("Publiée le 7 oct. 2026");
    expect(headline().textContent).toBe(
      "La version 0.9.1 de ChatonLM est disponible",
    );
    await moveTo(
      simulator,
      backend,
      { state: "unsupported", reason: "packageManager", version: null },
      { installer: null },
    );
    // Espaces insécables gardées (le normaliseur par défaut les changerait).
    await within(updates).findByText(
      "ChatonLM a été installé par votre gestionnaire de paquets\u00a0: les mises à jour passent par lui.",
      { normalizer: (text) => text },
    );
    await moveTo(simulator, backend, {
      state: "unsupported",
      reason: "readOnlyFolder",
      version: null,
    });
    await within(updates).findByText(
      "ChatonLM ne peut pas remplacer l'AppImage dans son dossier. Déplacez-la dans un dossier à vous (par exemple ~/Applications), puis relancez ChatonLM depuis ce dossier.",
    );
    await moveTo(simulator, backend, INSTALLING, { installer: "deb" });
    await waitFor(() =>
      expect(headline().textContent).toBe(
        "Installation de la version 0.9.1 de ChatonLM…",
      ),
    );
    expect(screen.getByTestId("update-password").textContent).toBe(
      "Votre système vous demande votre mot de passe pour l'installer.",
    );
    await moveTo(simulator, backend, AVAILABLE, {
      installer: "appImage",
      lastError: OFFLINE,
    });
    // Espace insécable gardée (le normaliseur par défaut la changerait).
    expect((await screen.findByTestId("update-last-error")).textContent).toBe(
      `La dernière vérification des mises à jour a échoué\u00a0: ${OFFLINE.detail}`,
    );
    await moveTo(
      simulator,
      backend,
      { state: "upToDate" },
      { lastError: null },
    );
    // Espace insécable gardée (le normaliseur par défaut la changerait).
    await within(updates).findByText(
      `Dernière vérification\u00a0: ${dateTime("fr", checkedAt)}`,
      { normalizer: (text) => text },
    );

    act(() => setLanguage("es"));
    await within(updates).findByRole("heading", { name: "Actualizaciones" });
    expect(headline().textContent).toBe("ChatonLM está actualizado");
    expect(
      within(updates).getByText(
        `Última comprobación: ${dateTime("es", checkedAt)}`,
      ),
    ).toBeTruthy();
    expect(
      within(updates).getByRole("button", { name: "Buscar actualizaciones" }),
    ).toBeTruthy();
    expect(
      screen.getByText(
        "Las actualizaciones no se descargarán automáticamente.",
      ),
    ).toBeTruthy();
    await moveTo(simulator, backend, INSTALLING);
    await waitFor(() =>
      expect(headline().textContent).toBe(
        "Instalando la versión 0.9.1 de ChatonLM…",
      ),
    );
    expect(
      within(updates).getByText(
        "ChatonLM está deteniendo sus servicios y volverá a abrirse con la nueva versión.",
      ),
    ).toBeTruthy();
  });
});
