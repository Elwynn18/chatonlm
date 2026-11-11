// Modèles distants (propre à ChatonLM ; remplace les modèles cloud
// d'Ollama) : carte de Settings › Models face au simulateur (faux moteur de
// src/test/remote.ts) — réglage coupé par défaut, ajout d'un fournisseur
// (la clé ne revient jamais), serveur « custom » et son adresse,
// vérification, changement, coupure, retrait, ancien Ollama d'une autre
// machine proposé comme serveur compatible — puis les actions sous une
// erreur d'un modèle distant ; textes en pseudo-langue et en français.

import {
  act,
  cleanup,
  configure,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { ChatErrorKind } from "@/bindings/ChatErrorKind";
import type { RemoteProviderInput } from "@/bindings/RemoteProviderInput";
import { RemoteErrorActions } from "@/components/RemoteErrorActions";
import { setLanguage } from "@/i18n";
import { isRemoteError } from "@/lib/remoteModels";
import { fakeBackend } from "@/test/backend";
import { untranslatedTexts, usePseudoLanguage } from "@/test/i18n";
import { type FakeRemote, INVALID_URL, remoteOf } from "@/test/remote";
import { renderApp } from "@/test/renderApp";
import { installSimulator, type Simulator } from "@/test/simulator";

configure({ asyncUtilTimeout: 5000 });
vi.setConfig({ testTimeout: 30_000 });

afterEach(() => cleanup());

/** Clé des essais : jamais rendue à la page. */
const KEY = "sk-or-v1-secret-abcd1234";

async function setup(prepare?: (remote: FakeRemote) => void) {
  const simulator = installSimulator();
  const backend = fakeBackend(simulator);
  const remote = remoteOf(simulator);
  prepare?.(remote);
  renderApp("/settings?section=models");
  const card = await screen.findByTestId("remote-models-settings");
  if (!remote.failures.has("remote_providers")) {
    await waitFor(() =>
      expect(simulator.callsTo("remote_providers").length).toBeGreaterThan(0),
    );
  }
  return { simulator, backend, remote, card, inCard: within(card) };
}

function saved(simulator: Simulator): RemoteProviderInput[] {
  return simulator
    .callsTo("save_remote_provider")
    .map((args) => args.input as RemoteProviderInput);
}

const item = (id: string) =>
  within(screen.getByTestId(`remote-provider-${id}`));

describe("remote models in Settings › Models", () => {
  it("are off by default and turned on by their switch", async () => {
    const { simulator, backend, inCard } = await setup();
    const toggle = inCard.getByRole("switch", { name: "Use remote models" });
    expect(toggle.getAttribute("aria-checked")).toBe("false");
    expect(inCard.getByText(/No provider yet/)).toBeTruthy();
    await userEvent.click(toggle);
    await waitFor(() => expect(backend.settings.remoteModels).toBe(true));
    expect(simulator.callsTo("update_settings").at(-1)).toEqual({
      patch: { remoteModels: true },
    });
    await waitFor(() =>
      expect(toggle.getAttribute("aria-checked")).toBe("true"),
    );
  });

  it("add a provider whose key never comes back to the page", async () => {
    const { simulator, remote, inCard } = await setup();
    await userEvent.click(
      inCard.getByRole("button", { name: "Add a provider" }),
    );
    const form = within(screen.getByTestId("remote-provider-form"));
    expect((form.getByLabelText("Provider") as HTMLSelectElement).value).toBe(
      "openRouter",
    );
    // Pas d'adresse pour un fournisseur connu.
    expect(form.queryByLabelText("Server address")).toBeNull();
    await userEvent.type(form.getByLabelText("API key"), KEY);
    await userEvent.click(form.getByRole("button", { name: "Save" }));
    await waitFor(() =>
      expect(screen.queryByTestId("remote-provider-form")).toBeNull(),
    );
    expect(saved(simulator)).toEqual([
      {
        id: null,
        kind: "openRouter",
        name: "",
        baseUrl: null,
        apiKey: KEY,
        enabled: true,
      },
    ]);
    expect(remote.keys.get("openrouter")).toBe(KEY);
    await waitFor(() =>
      expect(
        item("openrouter").getByText("API key ending in 1234"),
      ).toBeTruthy(),
    );
    // Nom par défaut (celui du service) et sorte.
    expect(item("openrouter").getAllByText("OpenRouter")).toHaveLength(2);
    // La clé n'est nulle part dans la page, champs compris.
    expect(document.body.innerHTML).not.toContain(KEY);
    for (const input of document.querySelectorAll("input")) {
      expect(input.value).not.toContain(KEY);
    }
  });

  it("an OpenAI-compatible server needs its address", async () => {
    const { simulator, inCard } = await setup();
    await userEvent.click(
      inCard.getByRole("button", { name: "Add a provider" }),
    );
    const form = within(screen.getByTestId("remote-provider-form"));
    await userEvent.selectOptions(
      form.getByLabelText("Provider"),
      "Other server (OpenAI-compatible)",
    );
    const address = form.getByLabelText("Server address");
    expect(
      form.getByText(/Optional: only if the server asks for one/),
    ).toBeTruthy();
    await userEvent.type(address, "ftp://example.com");
    await userEvent.click(form.getByRole("button", { name: "Save" }));
    expect((await form.findByRole("alert")).textContent).toBe(INVALID_URL);
    await userEvent.clear(address);
    await userEvent.type(address, "http://192.168.1.5:1234/v1");
    await userEvent.click(form.getByRole("button", { name: "Save" }));
    await waitFor(() =>
      expect(
        item("custom").getByText(
          "OpenAI-compatible server at http://192.168.1.5:1234",
        ),
      ).toBeTruthy(),
    );
    expect(item("custom").getByText("No API key")).toBeTruthy();
    expect(saved(simulator).at(-1)).toMatchObject({
      kind: "custom",
      baseUrl: "http://192.168.1.5:1234/v1",
      apiKey: null,
    });
  });

  it("an Ollama set up on another computer in 0.9 is offered as an OpenAI-compatible server", async () => {
    const simulator = installSimulator();
    const backend = fakeBackend(simulator);
    backend.settings.formerServerUrl = "http://192.168.1.20:11434";
    remoteOf(simulator);
    renderApp("/settings?section=models");
    const former = await screen.findByTestId("remote-former-server");
    expect(former.querySelector("p")?.textContent).toBe(
      "ChatonLM used the Ollama at http://192.168.1.20:11434. To use its models again, add it as “Other server (OpenAI-compatible)”.",
    );
    await userEvent.click(
      within(former).getByRole("button", { name: "Add it" }),
    );
    const form = within(screen.getByTestId("remote-provider-form"));
    expect(form.getByLabelText<HTMLSelectElement>("Provider").value).toBe(
      "custom",
    );
    expect(form.getByLabelText<HTMLInputElement>("Server address").value).toBe(
      "http://192.168.1.20:11434/v1",
    );
    await userEvent.click(form.getByRole("button", { name: "Save" }));
    await waitFor(() =>
      expect(saved(simulator).at(-1)).toMatchObject({
        kind: "custom",
        baseUrl: "http://192.168.1.20:11434/v1",
      }),
    );
    // Ajouté : la phrase s'en va.
    await waitFor(() =>
      expect(screen.queryByTestId("remote-former-server")).toBeNull(),
    );
  });

  it("an Ollama of this computer is not offered", async () => {
    const simulator = installSimulator();
    const backend = fakeBackend(simulator);
    backend.settings.formerServerUrl = "http://127.0.0.1:11434";
    remoteOf(simulator);
    renderApp("/settings?section=models");
    await screen.findByTestId("remote-models-settings");
    await waitFor(() =>
      expect(simulator.callsTo("remote_providers").length).toBeGreaterThan(0),
    );
    expect(screen.queryByTestId("remote-former-server")).toBeNull();
  });

  it("check says whether the key works", async () => {
    const { remote } = await setup((remote) => {
      remote.add("mistral", {}, "sk-mistral-12345678");
      remote.checks.set("mistral", {
        ok: false,
        problem: "key",
        status: null,
        models: 0,
      });
    });
    await userEvent.click(
      await item("mistral").findByRole("button", { name: "Check" }),
    );
    expect((await item("mistral").findByRole("status")).textContent).toBe(
      "The provider refused this API key.",
    );
    for (const [check, text] of [
      [
        { ok: true, problem: null, status: null, models: 1 },
        "Key accepted: 1 model available.",
      ],
      [
        { ok: false, problem: "server", status: 503, models: 0 },
        "The provider answered with an error (HTTP 503).",
      ],
      [
        { ok: false, problem: "unreachable", status: null, models: 0 },
        "The provider can't be reached. Check your internet connection and the address.",
      ],
    ] as const) {
      remote.checks.set("mistral", { ...check });
      await userEvent.click(
        item("mistral").getByRole("button", { name: "Check" }),
      );
      await waitFor(() =>
        expect(item("mistral").getByRole("status").textContent).toBe(text),
      );
    }
  });

  it("edit keeps the key unless a new one is typed", async () => {
    const { simulator, remote } = await setup((remote) =>
      remote.add("mistral", {}, "sk-mistral-12345678"),
    );
    await userEvent.click(
      await item("mistral").findByRole("button", { name: "Edit" }),
    );
    const form = within(screen.getByTestId("remote-provider-form"));
    expect(form.queryByLabelText("Provider")).toBeNull();
    expect(form.getByText("Leave empty to keep the saved key.")).toBeTruthy();
    const name = form.getByLabelText("Name (optional)");
    await userEvent.clear(name);
    await userEvent.type(name, "Work");
    await userEvent.click(form.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(item("mistral").getByText("Work")).toBeTruthy());
    expect(saved(simulator).at(-1)).toMatchObject({
      id: "mistral",
      name: "Work",
      apiKey: null,
    });
    expect(remote.keys.get("mistral")).toBe("sk-mistral-12345678");
  });

  it("a saved key can be removed, after a confirmation", async () => {
    const { simulator, remote } = await setup((remote) =>
      remote.add("mistral", {}, "sk-mistral-12345678"),
    );
    const removeKey = await item("mistral").findByRole("button", {
      name: "Remove key",
    });
    // Annulé : rien n'est envoyé.
    await userEvent.click(removeKey);
    await userEvent.click(
      within(screen.getByRole("alertdialog")).getByRole("button", {
        name: "Cancel",
      }),
    );
    expect(saved(simulator)).toEqual([]);
    await userEvent.click(removeKey);
    const dialog = within(screen.getByRole("alertdialog"));
    expect(dialog.getByText("Remove the API key of Mistral?")).toBeTruthy();
    await userEvent.click(dialog.getByRole("button", { name: "Remove key" }));
    await waitFor(() =>
      expect(item("mistral").getByText("No API key")).toBeTruthy(),
    );
    expect(saved(simulator)).toEqual([
      {
        id: "mistral",
        kind: "mistral",
        name: "Mistral",
        baseUrl: null,
        apiKey: "",
        enabled: true,
      },
    ]);
    expect(remote.keys.has("mistral")).toBe(false);
    // Plus de clé : plus de bouton.
    expect(
      item("mistral").queryByRole("button", { name: "Remove key" }),
    ).toBeNull();
  });

  it("a server's new address saved without a key drops the key, and says so first", async () => {
    const { simulator, remote } = await setup((remote) =>
      remote.add(
        "custom",
        { baseUrl: "http://192.168.1.2:8080" },
        "sk-local-12345678",
      ),
    );
    await userEvent.click(
      await item("custom").findByRole("button", { name: "Edit" }),
    );
    const form = within(screen.getByTestId("remote-provider-form"));
    const dropped = () => form.queryByTestId("remote-key-dropped");
    const address = form.getByLabelText("Server address");
    const key = form.getByLabelText("API key");
    expect(dropped()).toBeNull();
    // Même serveur écrit autrement : la clé reste.
    await userEvent.type(address, "/v1/");
    expect(dropped()).toBeNull();
    await userEvent.clear(address);
    await userEvent.type(address, "http://192.168.1.9:8080/v1");
    expect(dropped()?.textContent).toBe(
      "The new address doesn't keep the saved key: it is removed when you save. Type it again to keep it.",
    );
    await userEvent.type(key, "sk-other-87654321");
    expect(dropped()).toBeNull();
    await userEvent.clear(key);
    expect(dropped()).not.toBeNull();
    await userEvent.click(form.getByRole("button", { name: "Save" }));
    await waitFor(() =>
      expect(item("custom").getByText("No API key")).toBeTruthy(),
    );
    expect(saved(simulator).at(-1)).toMatchObject({
      baseUrl: "http://192.168.1.9:8080/v1",
      apiKey: null,
    });
    expect(remote.keys.has("custom")).toBe(false);
  });

  it("a provider can be turned off, then removed after a confirmation", async () => {
    const { remote } = await setup((remote) =>
      remote.add("mistral", {}, "sk-mistral-12345678"),
    );
    const toggle = await item("mistral").findByRole("switch", {
      name: "Use Mistral",
    });
    await userEvent.click(toggle);
    await waitFor(() => expect(remote.providers[0].enabled).toBe(false));
    await waitFor(() =>
      expect(toggle.getAttribute("aria-checked")).toBe("false"),
    );
    await userEvent.click(
      item("mistral").getByRole("button", { name: "Remove" }),
    );
    const dialog = within(screen.getByRole("alertdialog"));
    expect(dialog.getByText("Remove Mistral?")).toBeTruthy();
    await userEvent.click(dialog.getByRole("button", { name: "Remove" }));
    await waitFor(() =>
      expect(screen.queryByTestId("remote-provider-mistral")).toBeNull(),
    );
    expect(remote.keys.has("mistral")).toBe(false);
  });

  it("a failure to read the providers is said", async () => {
    const { inCard } = await setup((remote) =>
      remote.failures.set("remote_providers", {
        kind: "database",
        message: "disk error",
      }),
    );
    expect((await inCard.findByRole("alert")).textContent).toBe(
      "Couldn't read the providers: disk error",
    );
  });

  it("texts are translated", async () => {
    const { card } = await setup((remote) => {
      remote.add("mistral", {}, "sk-mistral-12345678");
      remote.add("custom", { name: "Box" });
    });
    usePseudoLanguage();
    await userEvent.click(
      within(card).getByRole("button", { name: /Add a provider/ }),
    );
    const allowed = [
      "Mistral",
      "Box",
      "5678",
      "http://192.168.1.2:8080",
      "https://example.com/v1",
      "/v1",
    ];
    expect(untranslatedTexts(card, allowed)).toEqual([]);
  });

  it("the key's removal and the dropped key are said in the catalogs' words", async () => {
    await setup((remote) =>
      remote.add("custom", { name: "Box" }, "sk-local-12345678"),
    );
    usePseudoLanguage();
    await userEvent.click(
      await item("custom").findByRole("button", { name: /Remove key/ }),
    );
    const dialog = screen.getByRole("alertdialog");
    expect(untranslatedTexts(dialog, ["Box"])).toEqual([]);
    await userEvent.click(
      within(dialog).getByRole("button", { name: /Cancel/ }),
    );
    await userEvent.click(item("custom").getByRole("button", { name: /Edit/ }));
    const form = screen.getByTestId("remote-provider-form");
    const address = within(form).getByLabelText(/Server address/);
    await userEvent.clear(address);
    await userEvent.type(address, "http://192.168.1.9:8080");
    expect(within(form).getByTestId("remote-key-dropped")).toBeTruthy();
    expect(
      untranslatedTexts(form, [
        "Box",
        "http://192.168.1.9:8080",
        "https://example.com/v1",
        "/v1",
      ]),
    ).toEqual([]);
  });

  it("the former Ollama of another computer is said in the catalogs' words", async () => {
    const simulator = installSimulator();
    const backend = fakeBackend(simulator);
    backend.settings.formerServerUrl = "http://192.168.1.20:11434";
    remoteOf(simulator);
    renderApp("/settings?section=models");
    const former = await screen.findByTestId("remote-former-server");
    usePseudoLanguage();
    expect(untranslatedTexts(former, ["http://192.168.1.20:11434"])).toEqual(
      [],
    );
  });

  it("the former Ollama of another computer, in French", async () => {
    const simulator = installSimulator();
    const backend = fakeBackend(simulator);
    backend.settings.formerServerUrl = "http://192.168.1.20:11434";
    remoteOf(simulator);
    renderApp("/settings?section=models");
    const former = await screen.findByTestId("remote-former-server");
    act(() => setLanguage("fr"));
    await waitFor(() =>
      expect(former.querySelector("p")?.textContent).toBe(
        "ChatonLM utilisait l'Ollama de http://192.168.1.20:11434. Pour retrouver ses modèles, ajoutez-le comme «\u00a0Autre serveur (compatible OpenAI)\u00a0».",
      ),
    );
    expect(
      within(former).getByRole("button", { name: "L'ajouter" }),
    ).toBeTruthy();
  });

  it("in French", async () => {
    const { card } = await setup((remote) =>
      remote.add("mistral", {}, "sk-mistral-12345678"),
    );
    act(() => setLanguage("fr"));
    expect(within(card).getByText("Modèles distants")).toBeTruthy();
    expect(
      within(card).getByRole("switch", {
        name: "Utiliser des modèles distants",
      }),
    ).toBeTruthy();
    expect(
      item("mistral").getByText("Clé d'API finissant par 5678"),
    ).toBeTruthy();
  });
});

describe("actions under a remote model error", () => {
  it("knows its kinds", () => {
    for (const kind of [
      "remoteOff",
      "remoteKey",
      "remoteLimit",
      "remoteUnreachable",
    ] as ChatErrorKind[]) {
      expect(isRemoteError(kind)).toBe(true);
    }
    for (const kind of ["webSearchRefused", "model", undefined] as (
      ChatErrorKind | undefined
    )[]) {
      expect(isRemoteError(kind)).toBe(false);
    }
  });

  it("each error has its way out", async () => {
    const names = (container: HTMLElement) =>
      [...container.querySelectorAll("button")].map((b) => b.textContent);
    for (const [kind, expected] of [
      ["remoteOff", ["Open settings"]],
      ["remoteKey", ["Check the key", "Retry"]],
      ["remoteLimit", ["Retry", "Open settings"]],
      ["remoteUnreachable", ["Retry"]],
    ] as const) {
      const onOpenSettings = vi.fn();
      const onRetry = vi.fn();
      const { container } = render(
        <RemoteErrorActions
          kind={kind}
          onOpenSettings={onOpenSettings}
          onRetry={onRetry}
        />,
      );
      expect(names(container)).toEqual(expected);
      for (const button of container.querySelectorAll("button")) {
        await userEvent.click(button);
      }
      expect(onRetry).toHaveBeenCalledTimes(
        expected.filter((name) => name === "Retry").length,
      );
      expect(onOpenSettings).toHaveBeenCalledTimes(
        expected.filter((name) => name !== "Retry").length,
      );
      cleanup();
    }
  });

  it("without a message to send again, no Retry", () => {
    const { container } = render(
      <RemoteErrorActions kind="remoteUnreachable" onOpenSettings={() => {}} />,
    );
    expect(container.querySelectorAll("button")).toHaveLength(0);
  });

  it("translated", () => {
    usePseudoLanguage();
    const { container } = render(
      <RemoteErrorActions
        kind="remoteKey"
        onOpenSettings={() => {}}
        onRetry={() => {}}
      />,
    );
    expect(untranslatedTexts(container)).toEqual([]);
  });

  // Essai à part : la pseudo-langue marque les textes jusqu'à la fin d'un essai.
  it("in French", () => {
    act(() => setLanguage("fr"));
    render(
      <RemoteErrorActions
        kind="remoteKey"
        onOpenSettings={() => {}}
        onRetry={() => {}}
      />,
    );
    expect(
      screen.getByRole("button", { name: "Vérifier la clé" }),
    ).toBeTruthy();
  });
});
