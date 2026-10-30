// Onglet « VS Code » des réglages face au simulateur (faux moteur de
// src/test/bridge.ts) : onglet de la page, interrupteur coupé par défaut et
// sa phrase, états « unavailable » et « failed » (dont un changement venu du
// moteur), appairage (refusé pont fermé, code épelé, heure de fin, consigne,
// annulation, code consommé par un éditeur), éditeurs appairés (connecté,
// dernière connexion, jamais connecté) et retrait, évènement « bridge » qui
// met l'écran à jour, réponses et évènements dans le désordre ; textes en
// pseudo-langue, en français et en espagnol.

import {
  act,
  configure,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import type { BridgeState } from "@/bindings/BridgeState";
import { setLanguage } from "@/i18n";
import { formatDateTime, formatTime } from "@/i18n/format";
import { fakeBackend } from "@/test/backend";
import {
  BRIDGE_PAIRING_MS,
  type FakeBridge,
  PAIR_REFUSED,
} from "@/test/bridge";
import { untranslatedTexts, usePseudoLanguage } from "@/test/i18n";
import { renderApp } from "@/test/renderApp";
import { installSimulator } from "@/test/simulator";

configure({ asyncUtilTimeout: 5000 });
vi.setConfig({ testTimeout: 30_000 });

/**
 * Onglet affiché après `prepare` du faux moteur, état lu et écouteur de
 * « bridge » inscrit (un évènement émis avant serait perdu).
 */
async function setup(prepare?: (bridge: FakeBridge) => void) {
  const simulator = installSimulator();
  const backend = fakeBackend(simulator);
  prepare?.(backend.bridge);
  const app = renderApp("/settings?section=vscode");
  const page = await screen.findByTestId("vscode-settings");
  if (!backend.bridge.failures.has("bridge_state")) {
    await screen.findByTestId("vscode-editors");
  }
  await waitFor(() => expect(simulator.listening("bridge")).toBe(1));
  return { simulator, backend, bridge: backend.bridge, app, page };
}

const toggle = () =>
  screen.getByRole("switch", { name: "Allow VS Code to connect" });
const editors = () => within(screen.getByTestId("vscode-editors"));
const pairButton = () =>
  editors().getByRole<HTMLButtonElement>("button", { name: "Pair an editor" });
const status = () => screen.getByTestId("vscode-status");

describe("VS Code settings", () => {
  it("is a tab of the settings, between Easy Link and Keyboard shortcuts", async () => {
    await setup();
    const tab = screen.getByTestId("settings-tab-vscode");
    expect(tab.textContent).toBe("VS Code");
    expect(tab.getAttribute("aria-current")).toBe("page");
    expect(tab.getAttribute("href")).toBe("/settings?section=vscode");
    expect(
      [...screen.getByTestId("settings-tabs").querySelectorAll("a")]
        .map((link) => link.textContent)
        .slice(-3),
    ).toEqual(["Easy Link", "VS Code", "Keyboard shortcuts"]);
  });

  it("off by default, with what the link is: local, never through the network", async () => {
    const { page } = await setup();
    expect(toggle().getAttribute("aria-checked")).toBe("false");
    const description = document.getElementById(
      toggle().getAttribute("aria-describedby")!,
    );
    expect(description?.textContent).toBe(
      "The ChatonLM extension for VS Code talks with your models through a link that stays on this computer: nothing goes through the network.",
    );
    expect(status().textContent).toBe("");
    expect(editors().getByText("No editor is paired yet.")).toBeTruthy();
    // Pas de Marketplace pour l'instant : aucun lien.
    expect(page.querySelector("a")).toBeNull();
  });

  it("the switch opens and closes the bridge, saved at once", async () => {
    const { simulator } = await setup();
    // Pont fermé : pas de code possible, et la raison est dite.
    expect(pairButton().disabled).toBe(true);
    expect(
      document.getElementById(pairButton().getAttribute("aria-describedby")!)
        ?.textContent,
    ).toBe("Allow VS Code to connect first to pair an editor.");
    await userEvent.click(toggle());
    await waitFor(() =>
      expect(toggle().getAttribute("aria-checked")).toBe("true"),
    );
    await screen.findByText("Saved");
    expect(status().getAttribute("data-state")).toBe("listening");
    expect(pairButton().disabled).toBe(false);
    expect(
      editors().queryByText(
        "Allow VS Code to connect first to pair an editor.",
      ),
    ).toBeNull();
    await userEvent.click(toggle());
    await waitFor(() =>
      expect(toggle().getAttribute("aria-checked")).toBe("false"),
    );
    expect(simulator.callsTo("bridge_set_enabled")).toEqual([
      { enabled: true },
      { enabled: false },
    ]);
  });

  it("a system without a private folder for the link: said in plain words, no pairing", async () => {
    await setup((bridge) => {
      bridge.openStatus = "unavailable";
    });
    await userEvent.click(toggle());
    await waitFor(() =>
      expect(status().getAttribute("data-state")).toBe("unavailable"),
    );
    expect(status().textContent).toBe(
      "VS Code can't connect: this system gives ChatonLM no private folder for the link.",
    );
    expect(toggle().getAttribute("aria-checked")).toBe("true");
    expect(pairButton().disabled).toBe(true);
  });

  it("a bridge that fails later: the event shows why, and the code goes away", async () => {
    const { bridge } = await setup((fake) => fake.enable());
    await userEvent.click(pairButton());
    await screen.findByTestId("vscode-pairing");
    await bridge.setStatus("failed");
    await waitFor(() =>
      expect(status().textContent).toBe(
        "The link with VS Code couldn't be opened (another copy of ChatonLM may already be using it). Turn it off and on again to retry.",
      ),
    );
    expect(screen.queryByTestId("vscode-pairing")).toBeNull();
    expect(pairButton().disabled).toBe(true);
  });

  it("a skeleton until the state is read", async () => {
    const simulator = installSimulator();
    const backend = fakeBackend(simulator);
    let answer!: (state: BridgeState) => void;
    simulator.handle(
      "bridge_state",
      () => new Promise<BridgeState>((resolve) => (answer = resolve)),
    );
    renderApp("/settings?section=vscode");
    expect(await screen.findByTestId("vscode-loading")).toBeTruthy();
    expect(screen.queryByTestId("vscode-editors")).toBeNull();
    await act(async () => answer(structuredClone(backend.bridge.state)));
    await screen.findByTestId("vscode-editors");
    expect(screen.queryByTestId("vscode-loading")).toBeNull();
  });

  it("the reason when the state can't be read, then Try again", async () => {
    const { bridge } = await setup((fake) => {
      fake.failures.set("bridge_state", {
        kind: "internal",
        message: "database is locked",
      });
    });
    expect((await screen.findByRole("alert")).textContent).toBe(
      "Couldn't read the VS Code settings: database is locked",
    );
    bridge.failures.clear();
    await userEvent.click(screen.getByRole("button", { name: "Try again" }));
    await screen.findByTestId("vscode-editors");
    expect(screen.queryByRole("alert")).toBeNull();
  });
});

describe("VS Code: pairing an editor", () => {
  it("the code, spelled out, its end of validity and the command to run", async () => {
    const before = Date.now();
    const { simulator, bridge } = await setup((fake) => fake.enable());
    await userEvent.click(pairButton());
    const box = await screen.findByTestId("vscode-pairing");
    expect(simulator.callsTo("bridge_pair")).toEqual([{}]);
    const shown = within(box).getByRole("img", {
      name: "Pairing code: 7 3 1 2 0 5",
    });
    // Un seul texte, sans espace : copié tel quel ; sélectionnable.
    expect(shown.textContent).toBe("731205");
    expect(shown.className.split(" ")).toContain("select-text");
    // Lu aussitôt : il prend le focus du bouton qu'il remplace.
    expect(document.activeElement).toBe(shown);
    const expiresAt = bridge.state.pairing!.expiresAt;
    expect(expiresAt).toBeGreaterThanOrEqual(before + BRIDGE_PAIRING_MS);
    expect(
      within(box).getByText(`Valid until ${formatTime("en", expiresAt)}.`),
    ).toBeTruthy();
    const instructions = within(box).getByText(
      (_, element) =>
        element?.tagName === "P" &&
        element.textContent ===
          "In VS Code, run ChatonLM: Pair and type this code.",
    );
    expect(instructions.querySelector("code")?.textContent).toBe(
      "ChatonLM: Pair",
    );
    expect(
      editors().queryByRole("button", { name: "Pair an editor" }),
    ).toBeNull();
  });

  it("Cancel gives the code up: “Pair an editor” is back, with the focus", async () => {
    const { simulator, bridge } = await setup((fake) => fake.enable());
    await userEvent.click(pairButton());
    const box = await screen.findByTestId("vscode-pairing");
    await userEvent.click(within(box).getByRole("button", { name: "Cancel" }));
    await waitFor(() =>
      expect(screen.queryByTestId("vscode-pairing")).toBeNull(),
    );
    expect(simulator.callsTo("bridge_cancel_pairing")).toEqual([{}]);
    expect(bridge.state.pairing).toBeNull();
    expect(document.activeElement).toBe(pairButton());
  });

  it("an editor that types the code appears when the backend says so, connected, and the code goes away", async () => {
    const { bridge } = await setup((fake) => fake.enable());
    await userEvent.click(pairButton());
    await screen.findByTestId("vscode-pairing");
    const client = await bridge.pairClient("VS Code on laptop");
    await waitFor(() =>
      expect(screen.queryByTestId("vscode-pairing")).toBeNull(),
    );
    const rows = editors().getAllByTestId("vscode-editor");
    expect(rows).toHaveLength(1);
    expect(within(rows[0]).getByText("VS Code on laptop")).toBeTruthy();
    expect(
      within(rows[0]).getByText(
        `Paired on ${formatDateTime("en", client.pairedAt)}`,
      ),
    ).toBeTruthy();
    expect(
      within(rows[0]).getByTestId("vscode-editor-connection").textContent,
    ).toBe("Connected");
    expect(editors().queryByText("No editor is paired yet.")).toBeNull();
    // Déconnecté : sa dernière connexion.
    await bridge.setConnected(client.id, false);
    await waitFor(() =>
      expect(
        within(rows[0]).getByTestId("vscode-editor-connection").textContent,
      ).toBe(
        `Last connected on ${formatDateTime("en", bridge.state.clients[0].lastSeenAt!)}`,
      ),
    );
    expect(pairButton()).toBeTruthy();
  });

  it("an expired code goes away on its own", async () => {
    const { bridge } = await setup((fake) => fake.enable());
    await userEvent.click(pairButton());
    await screen.findByTestId("vscode-pairing");
    await bridge.expirePairing();
    await waitFor(() =>
      expect(screen.queryByTestId("vscode-pairing")).toBeNull(),
    );
    expect(pairButton()).toBeTruthy();
  });

  it("a code that can't be given: the reason of the backend", async () => {
    const { simulator, bridge } = await setup((fake) => fake.enable());
    // Le pont s'est fermé dans le moteur sans que la page le sache encore.
    bridge.state.enabled = false;
    bridge.state.status = "off";
    await userEvent.click(pairButton());
    expect((await editors().findByRole("alert")).textContent).toBe(
      PAIR_REFUSED,
    );
    expect(simulator.callsTo("bridge_pair")).toHaveLength(1);
    expect(screen.queryByTestId("vscode-pairing")).toBeNull();
  });
});

describe("VS Code: paired editors", () => {
  it("each with its date and connection; Remove forgets one", async () => {
    const lastSeen = Date.parse("2026-10-20T09:15:00Z");
    const { simulator, bridge } = await setup((fake) => {
      fake.addClient("Desktop", { connected: true, lastSeenAt: Date.now() });
      fake.addClient("Laptop", { lastSeenAt: lastSeen });
      fake.addClient("");
    });
    const rows = editors().getAllByTestId("vscode-editor");
    expect(
      editors().getByRole("list", { name: "Paired editors" }),
    ).toBeTruthy();
    expect(
      rows.map(
        (row) =>
          within(row).getByTestId("vscode-editor-connection").textContent,
      ),
    ).toEqual([
      "Connected",
      `Last connected on ${formatDateTime("en", lastSeen)}`,
      "Never connected",
    ]);
    expect(within(rows[2]).getByText("Unnamed editor")).toBeTruthy();

    const laptop = bridge.state.clients[1].id;
    const remove = within(rows[1]).getByRole("button", { name: "Remove" });
    // Le nom de l'éditeur est lu avec le bouton.
    expect(
      document.getElementById(remove.getAttribute("aria-describedby")!)
        ?.textContent,
    ).toBe("Laptop");
    await userEvent.click(remove);
    await waitFor(() =>
      expect(editors().getAllByTestId("vscode-editor")).toHaveLength(2),
    );
    expect(simulator.callsTo("bridge_unlink")).toEqual([{ clientId: laptop }]);
    expect(editors().queryByText("Laptop")).toBeNull();
    // Le focus reste dans la carte.
    expect(document.activeElement).toBe(
      editors().getByRole("heading", { name: "Paired editors" }),
    );
  });

  it("an editor that connects is shown connected, from the event", async () => {
    const { bridge } = await setup((fake) => {
      fake.enable();
      fake.addClient("Desktop", { lastSeenAt: Date.now() - 60_000 });
    });
    const connection = editors().getByTestId("vscode-editor-connection");
    expect(connection.textContent).toMatch(/^Last connected on /);
    await bridge.setConnected(bridge.state.clients[0].id, true);
    await waitFor(() => expect(connection.textContent).toBe("Connected"));
  });

  it("closing the bridge keeps the editors paired", async () => {
    const { bridge } = await setup((fake) => {
      fake.enable();
      fake.addClient("Desktop", { connected: true, lastSeenAt: Date.now() });
    });
    await userEvent.click(toggle());
    await waitFor(() =>
      expect(
        editors().getByTestId("vscode-editor-connection").textContent,
      ).toMatch(/^Last connected on /),
    );
    expect(bridge.state.clients).toHaveLength(1);
    expect(editors().getByText("Desktop")).toBeTruthy();
  });
});

describe("VS Code: answers and events out of order", () => {
  it("a state read while an event arrives keeps the event, more recent", async () => {
    const simulator = installSimulator();
    const backend = fakeBackend(simulator);
    const bridge = backend.bridge;
    const stale = structuredClone(bridge.state);
    let answer!: (state: BridgeState) => void;
    simulator.handle(
      "bridge_state",
      () => new Promise<BridgeState>((resolve) => (answer = resolve)),
    );
    renderApp("/settings?section=vscode");
    await screen.findByTestId("vscode-loading");
    await waitFor(() => expect(simulator.listening("bridge")).toBe(1));
    bridge.enable();
    await bridge.emit();
    await waitFor(() =>
      expect(toggle().getAttribute("aria-checked")).toBe("true"),
    );
    // La lecture partie avant répond après, avec l'état d'avant.
    await act(async () => answer(stale));
    await act(() => new Promise((resolve) => setTimeout(resolve, 30)));
    expect(toggle().getAttribute("aria-checked")).toBe("true");
  });

  it("a command answered after a newer event: the state is read again, not set back", async () => {
    const { simulator, bridge } = await setup((fake) => fake.enable());
    let answer!: (state: BridgeState) => void;
    simulator.handle(
      "bridge_pair",
      () => new Promise<BridgeState>((resolve) => (answer = resolve)),
    );
    const reads = simulator.callsTo("bridge_state").length;
    await userEvent.click(pairButton());
    await waitFor(() =>
      expect(simulator.callsTo("bridge_pair")).toHaveLength(1),
    );
    // Le moteur a donné un code (réponse en route), puis un éditeur l'a
    // consommé : l'évènement, plus récent, arrive avant la réponse.
    bridge.state.pairing = {
      code: "555111",
      expiresAt: Date.now() + BRIDGE_PAIRING_MS,
    };
    const answered = structuredClone(bridge.state);
    await bridge.pairClient("Desktop");
    await editors().findByText("Desktop");
    await act(async () => answer(answered));
    await waitFor(() =>
      expect(simulator.callsTo("bridge_state")).toHaveLength(reads + 1),
    );
    await act(() => new Promise((resolve) => setTimeout(resolve, 30)));
    expect(screen.queryByTestId("vscode-pairing")).toBeNull();
    expect(editors().getByText("Desktop")).toBeTruthy();
  });
});

describe("VS Code in other languages", () => {
  /** Tous les états à la fois : pont en échec, éditeurs, code. */
  function everything(fake: FakeBridge) {
    fake.enable();
    fake.state.status = "failed";
    fake.addClient("Desktop", { connected: true, lastSeenAt: Date.now() });
    fake.addClient("Laptop", { lastSeenAt: Date.now() - 3_600_000 });
    fake.addClient("");
    fake.state.pairing = {
      code: "482915",
      expiresAt: Date.now() + BRIDGE_PAIRING_MS,
    };
  }

  it("is all translated", async () => {
    const { page, bridge } = await setup(everything);
    usePseudoLanguage();
    const allowed = ["482915", "Desktop", "Laptop"];
    expect(untranslatedTexts(page, allowed)).toEqual([]);
    // Pont coupé : la consigne sous « Pair an editor ».
    bridge.state = {
      enabled: false,
      status: "off",
      clients: [],
      pairing: null,
    };
    await bridge.emit();
    await waitFor(() =>
      expect(screen.queryByTestId("vscode-pairing")).toBeNull(),
    );
    expect(untranslatedTexts(page, allowed)).toEqual([]);
  });

  it("in French", async () => {
    const { page } = await setup(everything);
    act(() => setLanguage("fr"));
    expect(screen.getByTestId("settings-tab-vscode").textContent).toBe(
      "VS Code",
    );
    expect(
      screen.getByRole("switch", { name: "Autoriser VS Code à se connecter" }),
    ).toBeTruthy();
    expect(page.textContent).toContain(
      "L'extension ChatonLM pour VS Code converse avec vos modèles par un lien qui reste sur cet ordinateur : rien ne passe par le réseau.",
    );
    expect(status().textContent).toBe(
      "Le lien avec VS Code n'a pas pu être ouvert (une autre copie de ChatonLM l'utilise peut-être). Désactivez-le puis réactivez-le pour réessayer.",
    );
    expect(
      editors().getByRole("list", { name: "Éditeurs appairés" }),
    ).toBeTruthy();
    expect(editors().getByText("Éditeur sans nom")).toBeTruthy();
    expect(editors().getByText("Connecté")).toBeTruthy();
    expect(
      editors().getByTestId("vscode-code").getAttribute("aria-label"),
    ).toBe("Code d'appairage : 4 8 2 9 1 5");
    expect(page.textContent).toContain(
      "Dans VS Code, lancez ChatonLM: Pair et tapez ce code.",
    );
    expect(editors().getAllByRole("button", { name: "Retirer" })).toHaveLength(
      3,
    );
  });

  it("in Spanish", async () => {
    const { page } = await setup(everything);
    act(() => setLanguage("es"));
    expect(
      screen.getByRole("switch", { name: "Permitir que VS Code se conecte" }),
    ).toBeTruthy();
    expect(
      editors().getByRole("list", { name: "Editores emparejados" }),
    ).toBeTruthy();
    expect(
      editors().getByTestId("vscode-code").getAttribute("aria-label"),
    ).toBe("Código de emparejamiento: 4 8 2 9 1 5");
    expect(page.textContent).toContain(
      "En VS Code, ejecuta ChatonLM: Pair y escribe este código.",
    );
    expect(editors().getByText("Nunca conectado")).toBeTruthy();
  });
});
