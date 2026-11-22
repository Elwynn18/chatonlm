// « Erase my data » (Settings › General) face au simulateur : zone rouge
// sous « Reset to defaults », boîte qui liste ce qui est effacé et gardé
// (Cancel, Échap), effacement confirmé (bouton occupé, nouvelle
// conversation, historique vide, réglages par défaut, annonce), échec dit,
// fichiers restés annoncés, textes tous traduits.

import {
  act,
  configure,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { getProofreadState, setDraft } from "@/hooks/useProofread";
import { setLanguage } from "@/i18n";
import { newChatIsEphemeral, setNewChatEphemeral } from "@/lib/ephemeral";
import { fakeBackend, type StoredChat } from "@/test/backend";
import { untranslatedTexts, usePseudoLanguage } from "@/test/i18n";
import { renderApp } from "@/test/renderApp";
import { installSimulator } from "@/test/simulator";

configure({ asyncUtilTimeout: 5000 });

function stored(id: string): StoredChat {
  const at = Date.now() - 60_000;
  return {
    id,
    title: `Chat ${id}`,
    createdAt: at,
    updatedAt: at,
    messages: [],
  };
}

function setup() {
  const simulator = installSimulator();
  const backend = fakeBackend(simulator);
  backend.chats.set("secret", stored("secret"));
  Object.assign(backend.settings, {
    language: "en",
    theme: "dark",
    systemPrompt: "Answer in verse.",
  });
  const app = renderApp("/settings");
  return { simulator, backend, app };
}

/** Bouton de la zone (pas celui de la boîte). */
async function eraseButton() {
  const zone = await screen.findByTestId("erase-data");
  return within(zone).getByRole("button", { name: "Erase my data" });
}

async function openDialog() {
  await userEvent.click(await eraseButton());
  return screen.getByRole("alertdialog", { name: "Erase your data?" });
}

/** Barre latérale ouverte, conversation « secret » listée. */
async function historyShown() {
  const toggle = await screen.findByRole("button", {
    name: /^(Show|Hide) sidebar$/,
  });
  if (toggle.getAttribute("aria-label") === "Show sidebar") {
    await userEvent.click(toggle);
  }
  await waitFor(() =>
    expect(document.querySelector('[data-chat-id="secret"]')).toBeTruthy(),
  );
}

describe("Erase my data", () => {
  it("is a red zone of its own under Reset to defaults", async () => {
    setup();
    const zone = await screen.findByTestId("erase-data");
    const reset = screen.getByRole("button", { name: "Reset to defaults" });
    expect(
      reset.compareDocumentPosition(zone) & Node.DOCUMENT_POSITION_FOLLOWING,
    ).toBeTruthy();
    expect(
      within(zone).getByRole("heading", { name: "Erase my data" }),
    ).toBeTruthy();
    expect(zone.className).toContain("border-red-600/40");
    expect((await eraseButton()).className).toContain("bg-red-600");
  });

  it("lists what is erased and what is kept; Cancel and Escape erase nothing", async () => {
    const { simulator } = setup();
    let dialog = await openDialog();
    for (const text of [
      "Erased from this computer:",
      "Your conversations, with their attachments and images",
      "Your courses and transcriptions",
      "Your settings, including the dictation dictionary",
      "Kept:",
      "Your models, their files (text, images, transcription, voices) and the models you chose",
      "The interface language",
      "This can't be undone.",
    ]) {
      expect(within(dialog).getByText(text)).toBeTruthy();
    }
    // Accord qui efface : le focus est sur Cancel, le bouton est rouge.
    expect(document.activeElement?.textContent).toBe("Cancel");
    const confirm = within(dialog).getByRole("button", {
      name: "Erase my data",
    });
    expect(confirm.className).toContain("bg-red-600");

    await userEvent.click(
      within(dialog).getByRole("button", { name: "Cancel" }),
    );
    expect(screen.queryByRole("alertdialog")).toBeNull();
    await openDialog();
    await userEvent.keyboard("{Escape}");
    expect(screen.queryByRole("alertdialog")).toBeNull();
    // Le focus reste dans la boîte (Tab, Maj+Tab).
    dialog = await openDialog();
    await userEvent.tab();
    expect(dialog.contains(document.activeElement)).toBe(true);
    await userEvent.tab({ shift: true });
    await userEvent.tab({ shift: true });
    expect(dialog.contains(document.activeElement)).toBe(true);
    await userEvent.keyboard("{Escape}");
    expect(simulator.callsTo("erase_user_data")).toHaveLength(0);
  });

  it("erases once confirmed, busy meanwhile, then shows an empty app and says so", async () => {
    const { simulator, backend, app } = setup();
    await historyShown();
    // Prochaine conversation demandée éphémère (chaton-fantôme) : la
    // nouvelle conversation de l'état vide est ordinaire.
    act(() => setNewChatEphemeral(true));
    // Texte laissé sur la page Proofread.
    act(() => setDraft("my secret letter"));
    let release = () => {};
    backend.eraseHold = new Promise((resolve) => (release = resolve));
    const dialog = await openDialog();
    await userEvent.click(
      within(dialog).getByRole("button", { name: "Erase my data" }),
    );
    // Occupé pendant l'effacement.
    const busy = await within(screen.getByTestId("erase-data")).findByRole(
      "button",
      { name: "Erasing…" },
    );
    expect(busy).toHaveProperty("disabled", true);
    expect(busy.getAttribute("aria-busy")).toBe("true");
    expect(simulator.callsTo("erase_user_data")).toHaveLength(1);

    const reads = simulator.callsTo("get_settings").length;
    await act(async () => release());
    await screen.findByTestId("new-chat");
    expect(app.path()).toBe("/c/new");
    expect(newChatIsEphemeral()).toBe(false);
    expect(getProofreadState().draft).toBe("");
    await waitFor(() =>
      expect(document.querySelector('[data-chat-id="secret"]')).toBeNull(),
    );
    expect(
      within(screen.getByTestId("notice")).getByText(
        "Your data has been erased.",
      ),
    ).toBeTruthy();
    // Réglages relus : ceux par défaut, langue gardée.
    await waitFor(() =>
      expect(simulator.callsTo("get_settings").length).toBeGreaterThan(reads),
    );
    expect(backend.settings.language).toBe("en");
    expect(backend.settings.theme).toBe("system");
    expect(backend.settings.systemPrompt).toBe("");
    expect(backend.settings.selectedModel).toBe("qwen3:0.6b");
  });

  it("announces the files that stayed", async () => {
    const { backend } = setup();
    backend.eraseReport = {
      leftovers: ["logs/sd-server.log"],
      browsingDataCleared: false,
    };
    const dialog = await openDialog();
    await userEvent.click(
      within(dialog).getByRole("button", { name: "Erase my data" }),
    );
    expect(
      await within(screen.getByTestId("notice")).findByText(
        "Your data has been erased. Some files could not be erased: logs/sd-server.log, page data.",
      ),
    ).toBeTruthy();
  });

  it("the notice goes away after a few seconds", async () => {
    setup();
    const dialog = await openDialog();
    vi.useFakeTimers({ shouldAdvanceTime: true });
    try {
      await userEvent.click(
        within(dialog).getByRole("button", { name: "Erase my data" }),
      );
      await within(screen.getByTestId("notice")).findByText(
        "Your data has been erased.",
      );
      await act(async () => {
        await vi.advanceTimersByTimeAsync(6100);
      });
      expect(screen.getByTestId("notice").textContent).toBe("");
    } finally {
      vi.useRealTimers();
    }
  });

  it("says why the data could not be erased, and stays on the page", async () => {
    const { backend, app } = setup();
    await historyShown();
    backend.eraseError = { kind: "database", message: "disk full" };
    act(() => setDraft("kept"));
    const consoleError = vi
      .spyOn(console, "error")
      .mockImplementation(() => {});
    const dialog = await openDialog();
    await userEvent.click(
      within(dialog).getByRole("button", { name: "Erase my data" }),
    );
    const zone = screen.getByTestId("erase-data");
    expect((await within(zone).findByRole("alert")).textContent).toBe(
      "Your data could not be erased (disk full). The work in progress was stopped.",
    );
    consoleError.mockRestore();
    expect(app.path()).toBe("/settings");
    expect(document.querySelector('[data-chat-id="secret"]')).toBeTruthy();
    expect(screen.getByTestId("notice").textContent).toBe("");
    expect(getProofreadState().draft).toBe("kept");
    // De nouveau permis.
    expect(await eraseButton()).toHaveProperty("disabled", false);
  });

  it("a refusal before anything was stopped says only why", async () => {
    const { backend } = setup();
    backend.eraseError = {
      kind: "invalid",
      message: "Your data is already being erased.",
    };
    const consoleError = vi
      .spyOn(console, "error")
      .mockImplementation(() => {});
    const dialog = await openDialog();
    await userEvent.click(
      within(dialog).getByRole("button", { name: "Erase my data" }),
    );
    const zone = screen.getByTestId("erase-data");
    expect((await within(zone).findByRole("alert")).textContent).toBe(
      "Your data could not be erased: Your data is already being erased.",
    );
    consoleError.mockRestore();
  });

  it("every text comes from the catalogs, in French too", async () => {
    const { backend } = setup();
    backend.eraseError = { kind: "database", message: "disk full" };
    const consoleError = vi
      .spyOn(console, "error")
      .mockImplementation(() => {});
    let dialog = await openDialog();
    await userEvent.click(
      within(dialog).getByRole("button", { name: "Erase my data" }),
    );
    await within(screen.getByTestId("erase-data")).findByRole("alert");
    consoleError.mockRestore();
    dialog = await openDialog();
    usePseudoLanguage();
    expect(
      untranslatedTexts(screen.getByTestId("erase-data"), ["disk full"]),
    ).toEqual([]);
    expect(untranslatedTexts(dialog)).toEqual([]);
    act(() => setLanguage("fr"));
    expect(
      within(dialog).getByRole("button", { name: "⟦Effacer mes données⟧" }),
    ).toBeTruthy();
  });
});
