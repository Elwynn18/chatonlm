import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { clearMocks, mockIPC, mockWindows } from "@tauri-apps/api/mocks";
import { createRef, type ReactNode } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { CloudStatus } from "@/bindings/CloudStatus";
import type { Message } from "@/bindings/Message";
import type { ModelDetails } from "@/bindings/ModelDetails";
import type { SettingsPatch } from "@/bindings/SettingsPatch";
import type { Think } from "@/bindings/Think";
import { modelInfo, thinkingModel } from "@/test/backend";
import { ChatForm, type ChatFormHandle } from "./ChatForm";

afterEach(() => {
  cleanup();
  clearMocks();
});

interface State {
  isStreaming: boolean;
  isDisabled: boolean;
}

/**
 * Zone de saisie de la conversation « c ». `aside` est rendu à côté, comme la
 * barre latérale ou une boîte de dialogue ; `update` change l'état de la
 * réponse ou du serveur.
 */
function renderForm({
  isStreaming = false,
  isDisabled = false,
  selectedModel = "qwen3:0.6b",
  aside = null as ReactNode,
  /** Modèles installés (sélecteur). */
  models = [] as string[],
  /** Réponses de model_details ; un autre modèle est absent (erreur). */
  details = [] as ModelDetails[],
  /** Réglages Think mémorisés, par modèle. */
  prefs = {} as Record<string, Think>,
  /** model_details de ces modèles ne répond qu'une fois la promesse tenue. */
  heldDetails = {} as Record<string, Promise<void>>,
  /** Réglage persistant de la recherche web. */
  webSearchEnabled = false,
  /** Réponse de cloud_status. */
  cloud = { disabled: false, source: "none" } as CloudStatus | null,
} = {}) {
  const settings = { selectedModel, serverUrl: "", webSearchEnabled };
  const saved: { model: string; think: Think }[] = [];
  const server = { cloud };
  const handle = createRef<ChatFormHandle>();
  /** Commandes reçues, dans l'ordre. */
  const commands: string[] = [];
  const detailsOf = (model: string) => {
    const found = details.find((d) => d.model === model);
    if (found) return found;
    throw { kind: "notFound", message: `model '${model}' not found` };
  };
  mockIPC(
    (command, args) => {
      const model = (args as { model?: string } | undefined)?.model ?? "";
      commands.push(command);
      switch (command) {
        case "cloud_status":
          return server.cloud;
        case "get_settings":
          return { ...settings };
        case "update_settings":
          Object.assign(settings, (args as { patch: SettingsPatch }).patch);
          return { ...settings };
        case "list_models":
          return models.map(modelInfo);
        case "model_details": {
          const held = heldDetails[model];
          return held ? held.then(() => detailsOf(model)) : detailsOf(model);
        }
        case "get_think_pref":
          return prefs[model] ?? null;
        case "set_think_pref":
          saved.push(args as { model: string; think: Think });
          return null;
      }
      throw new Error(`unexpected command ${command}`);
    },
    { shouldMockEvents: true },
  );
  mockWindows("main");
  const onSubmit = vi.fn();
  const onCancel = vi.fn();
  const client = new QueryClient({
    defaultOptions: { queries: { networkMode: "always", retry: false } },
  });
  let state: State = { isStreaming, isDisabled };
  const ui = () => (
    <QueryClientProvider client={client}>
      {aside}
      <ChatForm
        chatId="c"
        hasMessages
        isStreaming={state.isStreaming}
        isDisabled={state.isDisabled}
        onSubmit={onSubmit}
        onCancel={onCancel}
        ref={handle}
      />
    </QueryClientProvider>
  );
  const { rerender } = render(ui());
  return {
    onSubmit,
    onCancel,
    /** Appels de set_think_pref. */
    saved,
    /** Réglages enregistrés (update_settings). */
    settings,
    /** État du serveur, que le test peut changer. */
    server,
    /** Méthodes de la zone de saisie (ChatFormHandle). */
    handle,
    commands,
    textarea: screen.getByLabelText("Message") as HTMLTextAreaElement,
    update: (patch: Partial<State>) => {
      state = { ...state, ...patch };
      rerender(ui());
    },
  };
}

/** Laisse passer les focus différés (setTimeout 0) de la zone de saisie. */
const settle = () => new Promise((resolve) => setTimeout(resolve, 20));

/**
 * z-index de la couche d'un élément dans la zone de saisie : celui de son
 * ancêtre le plus haut qui en porte un sous la zone, car un z-index ne joue
 * qu'à l'intérieur du contexte d'empilement de ses ancêtres.
 */
function layer(element: Element, composer: Element) {
  let z = 0;
  for (
    let el: Element | null = element;
    el && el !== composer;
    el = el.parentElement
  ) {
    const match = /(?:^|\s)z-(\d+)(?:\s|$)/.exec(el.className);
    if (match) z = Number(match[1]);
  }
  return z;
}

describe("ChatForm", () => {
  it("Enter sends with the selected model, Shift+Enter adds a line", async () => {
    const { onSubmit, textarea } = renderForm();
    await waitFor(() =>
      expect(screen.getByTitle("Select model").textContent).toBe("qwen3:0.6b"),
    );
    await userEvent.type(textarea, "Hello{Shift>}{Enter}{/Shift}there");
    expect(onSubmit).not.toHaveBeenCalled();
    await userEvent.type(textarea, "{Enter}");
    // Pas de bouton Think (modèle sans capacité thinking) : think absent.
    expect(onSubmit).toHaveBeenCalledWith(
      "Hello\nthere",
      "qwen3:0.6b",
      undefined,
      [],
      [],
      false,
    );
    expect((textarea as HTMLTextAreaElement).value).toBe("");
  });

  it("does not send an empty message, nor during an IME composition", async () => {
    const { onSubmit, textarea } = renderForm();
    await waitFor(() =>
      expect(screen.getByTitle("Select model").textContent).toBe("qwen3:0.6b"),
    );
    await userEvent.type(textarea, "   {Enter}");
    expect(onSubmit).not.toHaveBeenCalled();
    expect(
      (screen.getByLabelText("Send message") as HTMLButtonElement).disabled,
    ).toBe(true);

    fireEvent.change(textarea, { target: { value: "日本" } });
    fireEvent.compositionStart(textarea);
    fireEvent.keyDown(textarea, { key: "Enter" });
    expect(onSubmit).not.toHaveBeenCalled();
  });

  it("without a model, nothing can be sent", async () => {
    const { onSubmit, textarea } = renderForm({ selectedModel: "" });
    await waitFor(() =>
      expect(screen.getByTitle("Select model").textContent).toBe(
        "Select a model",
      ),
    );
    await userEvent.type(textarea, "Hello{Enter}");
    expect(onSubmit).not.toHaveBeenCalled();
  });

  it("during a reply: Enter does nothing, Stop and Escape cancel", async () => {
    const { onSubmit, onCancel, textarea } = renderForm({ isStreaming: true });
    await userEvent.type(textarea, "Next{Enter}");
    expect(onSubmit).not.toHaveBeenCalled();
    await userEvent.click(screen.getByLabelText("Stop"));
    expect(onCancel).toHaveBeenCalledTimes(1);
    fireEvent.keyDown(window, { key: "Escape" });
    expect(onCancel).toHaveBeenCalledTimes(2);
  });

  it("Escape already used elsewhere does not stop the reply", () => {
    const { onCancel } = renderForm({ isStreaming: true });
    const used = new KeyboardEvent("keydown", {
      key: "Escape",
      cancelable: true,
    });
    used.preventDefault();
    window.dispatchEvent(used);
    expect(onCancel).not.toHaveBeenCalled();
  });

  it("is inactive while Ollama is unreachable", async () => {
    const { textarea, update } = renderForm();
    // Modèles chargés d'abord : « Loading... » vient alors de isDisabled.
    await waitFor(() =>
      expect(screen.getByTitle("Select model").textContent).toBe("qwen3:0.6b"),
    );
    await userEvent.type(textarea, "Hello");
    update({ isDisabled: true });
    expect(textarea.disabled).toBe(true);
    expect(screen.getByTitle("Select model").textContent).toBe("Loading...");
    expect(
      (screen.getByLabelText("Send message") as HTMLButtonElement).disabled,
    ).toBe(true);
    // Le sélecteur et le bouton restent sous le voile.
    const veil = screen.getByTestId("composer-veil");
    const composer = veil.parentElement as HTMLElement;
    expect(layer(screen.getByTitle("Select model"), composer)).toBeLessThan(
      layer(veil, composer),
    );
    expect(layer(screen.getByLabelText("Send message"), composer)).toBeLessThan(
      layer(veil, composer),
    );
  });

  it("Stop stays clickable above the veil while Ollama is unreachable", async () => {
    const { onCancel } = renderForm({ isStreaming: true, isDisabled: true });
    const veil = screen.getByTestId("composer-veil");
    const composer = veil.parentElement as HTMLElement;
    const stop = screen.getByLabelText("Stop");
    expect(layer(stop, composer)).toBeGreaterThan(layer(veil, composer));
    expect(layer(screen.getByTitle("Select model"), composer)).toBeLessThan(
      layer(veil, composer),
    );
    await userEvent.click(stop);
    expect(onCancel).toHaveBeenCalledTimes(1);
  });

  it("typing elsewhere moves the focus to the message field", () => {
    const { textarea } = renderForm();
    (document.activeElement as HTMLElement | null)?.blur();
    fireEvent.keyDown(document.body, { key: "a" });
    expect(document.activeElement).toBe(textarea);
  });

  it("Space activates the focused button instead of typing in the message field", async () => {
    const onNewChat = vi.fn();
    const { textarea, onCancel } = renderForm({
      isStreaming: true,
      aside: (
        <button type="button" onClick={onNewChat}>
          New chat
        </button>
      ),
    });
    await settle();
    const newChat = screen.getByText("New chat");
    newChat.focus();
    await userEvent.keyboard(" ");
    expect(onNewChat).toHaveBeenCalledTimes(1);
    expect(document.activeElement).toBe(newChat);

    screen.getByLabelText("Stop").focus();
    await userEvent.keyboard(" ");
    expect(onCancel).toHaveBeenCalledTimes(1);
    expect(textarea.value).toBe("");

    // Les autres caractères vont toujours dans la zone de saisie.
    newChat.focus();
    await userEvent.keyboard("a");
    expect(document.activeElement).toBe(textarea);
    expect(onNewChat).toHaveBeenCalledTimes(1);
  });

  it("Space on a focused link goes to the message field (a link ignores it)", async () => {
    const { textarea } = renderForm({ aside: <a href="#c">Chat</a> });
    await settle();
    screen.getByText("Chat").focus();
    await userEvent.keyboard(" ");
    expect(document.activeElement).toBe(textarea);
  });

  it("takes the focus on mount, even during a reply", async () => {
    const { textarea } = renderForm({ isStreaming: true });
    await waitFor(() => expect(document.activeElement).toBe(textarea));
  });

  it("on mount, leaves the focus to a field but takes it from the chat link", async () => {
    // Champ de renommage déjà ouvert quand la conversation s'affiche
    renderForm({ aside: <input aria-label="Chat title" autoFocus /> });
    await settle();
    expect(document.activeElement).toBe(screen.getByLabelText("Chat title"));
    cleanup();

    // Le lien de la barre latérale qui vient d'ouvrir la conversation
    const { textarea } = renderForm({
      aside: (
        <a href="#c" ref={(link) => link?.focus()}>
          Chat
        </a>
      ),
    });
    await waitFor(() => expect(document.activeElement).toBe(textarea));
  });

  it("a reply that ends puts the focus back in the message field when it is free", async () => {
    const { textarea, update } = renderForm({ isStreaming: true });
    await settle();
    // Arrêt par le bouton : le focus est sur Stop, dans la zone de saisie
    screen.getByLabelText("Stop").focus();
    update({ isStreaming: false });
    await waitFor(() => expect(document.activeElement).toBe(textarea));

    update({ isStreaming: true });
    textarea.blur();
    update({ isStreaming: false });
    await waitFor(() => expect(document.activeElement).toBe(textarea));
  });

  it.each([
    ["a rename field", <input key="f" aria-label="Target" />],
    [
      "a sidebar button",
      <button key="b" type="button" aria-label="Target">
        New chat
      </button>,
    ],
    [
      "a dialog",
      <div key="d" role="alertdialog" aria-modal="true">
        <button type="button" aria-label="Target">
          OK
        </button>
      </div>,
    ],
  ])("a reply that ends does not take the focus from %s", async (_, aside) => {
    const { textarea, update, onSubmit } = renderForm({
      isStreaming: true,
      aside,
    });
    await settle();
    const target = screen.getByLabelText("Target");
    target.focus();
    update({ isStreaming: false });
    await settle();
    expect(document.activeElement).toBe(target);
    await userEvent.keyboard("{Enter}");
    expect(onSubmit).not.toHaveBeenCalled();
    expect(textarea.value).toBe("");
  });
  it("a reply that ends keeps a menu open and the focus where it is", async () => {
    const { textarea, update } = renderForm({
      isStreaming: true,
      aside: <div role="menu" />,
    });
    await settle();
    textarea.blur();
    update({ isStreaming: false });
    await settle();
    expect(document.activeElement).toBe(document.body);
  });

  it("on mount, leaves the focus to an open dialog", async () => {
    renderForm({
      aside: (
        <div role="alertdialog" aria-modal="true">
          <button type="button" autoFocus>
            OK
          </button>
        </div>
      ),
    });
    await settle();
    expect(document.activeElement).toBe(screen.getByText("OK"));
  });

  it("a reply that ends leaves the focus in the model search", async () => {
    const { update, onSubmit } = renderForm({ isStreaming: true });
    await waitFor(() =>
      expect(screen.getByTitle("Select model").textContent).toContain(
        "qwen3:0.6b",
      ),
    );
    await userEvent.click(screen.getByTitle("Select model"));
    const search = await screen.findByLabelText("Find model");
    await waitFor(() => expect(document.activeElement).toBe(search));
    update({ isStreaming: false });
    await settle();
    expect(document.activeElement).toBe(search);
    await userEvent.keyboard("{Enter}");
    expect(onSubmit).not.toHaveBeenCalled();
  });

  it("a reply that ends keeps text selected elsewhere", async () => {
    const { update } = renderForm({
      isStreaming: true,
      aside: <p>Earlier answer</p>,
    });
    await settle();
    (document.activeElement as HTMLElement | null)?.blur();
    window
      .getSelection()
      ?.selectAllChildren(screen.getByText("Earlier answer"));
    update({ isStreaming: false });
    await settle();
    expect(document.activeElement).toBe(document.body);
    expect(window.getSelection()?.toString()).toBe("Earlier answer");
  });
});

const QWEN = "qwen3:0.6b";
const GPT = "gpt-oss:20b";
const NEMOTRON = "nemotron-3.5-nano";
const ON_OFF = thinkingModel(QWEN, { values: [false, true], default: true });
const LEVELS = thinkingModel(GPT, {
  values: ["low", "medium", "high"],
  default: "medium",
});

const thinkToggle = () => screen.findByRole("button", { name: "Think" });
const thinkLevels = () =>
  screen.findByRole("button", { name: /^Thinking level/ });
const picker = () => screen.getByTitle("Select model");
const uploadButton = () =>
  screen.getByRole("button", { name: "Upload multiple files" });

/** Tape un message et l'envoie avec Entrée. */
async function send(textarea: HTMLTextAreaElement, text = "Hello") {
  await userEvent.type(textarea, `${text}{Enter}`);
}

describe("ChatForm: Think button", () => {
  it("toggle: before the model picker, sent explicitly, remembered", async () => {
    const { onSubmit, saved, textarea } = renderForm({ details: [ON_OFF] });
    const toggle = await thinkToggle();
    // Place officielle : avant le sélecteur de modèle.
    expect(
      toggle.compareDocumentPosition(picker()) &
        Node.DOCUMENT_POSITION_FOLLOWING,
    ).toBeTruthy();
    expect(toggle.getAttribute("aria-pressed")).toBe("true");
    await send(textarea);
    expect(onSubmit).toHaveBeenLastCalledWith(
      "Hello",
      QWEN,
      true,
      [],
      [],
      false,
    );

    await userEvent.click(toggle);
    expect(toggle.getAttribute("aria-pressed")).toBe("false");
    await waitFor(() => expect(saved).toEqual([{ model: QWEN, think: false }]));
    await send(textarea);
    // false envoyé, pas omis (sinon Ollama raisonnerait).
    expect(onSubmit).toHaveBeenLastCalledWith(
      "Hello",
      QWEN,
      false,
      [],
      [],
      false,
    );
  });

  it("the remembered choice is read back", async () => {
    const { onSubmit, textarea } = renderForm({
      details: [ON_OFF],
      prefs: { [QWEN]: false },
    });
    const toggle = await thinkToggle();
    await waitFor(() =>
      expect(toggle.getAttribute("aria-pressed")).toBe("false"),
    );
    await send(textarea);
    expect(onSubmit).toHaveBeenLastCalledWith(
      "Hello",
      QWEN,
      false,
      [],
      [],
      false,
    );
  });

  it("levels: the chosen level is sent and remembered", async () => {
    const { onSubmit, saved, textarea } = renderForm({
      selectedModel: GPT,
      details: [LEVELS],
    });
    const pill = await thinkLevels();
    expect(pill.title).toBe("Thinking level: Medium");
    await send(textarea);
    expect(onSubmit).toHaveBeenLastCalledWith(
      "Hello",
      GPT,
      "medium",
      [],
      [],
      false,
    );

    await userEvent.click(pill);
    await userEvent.click(screen.getByRole("menuitemradio", { name: "High" }));
    await waitFor(() => expect(saved).toEqual([{ model: GPT, think: "high" }]));
    await send(textarea);
    expect(onSubmit).toHaveBeenLastCalledWith(
      "Hello",
      GPT,
      "high",
      [],
      [],
      false,
    );
  });

  it("levels with On (Nemotron 3.5 Nano): full thinking by default, sent", async () => {
    const { onSubmit, saved, textarea } = renderForm({
      selectedModel: NEMOTRON,
      details: [
        thinkingModel(NEMOTRON, {
          values: [false, true, "medium"],
          default: true,
        }),
      ],
    });
    const pill = await thinkLevels();
    expect(pill.title).toBe("Thinking level: On");
    await send(textarea);
    // true explicite : pas omis, ni remplacé par "medium" (effort réduit).
    expect(onSubmit).toHaveBeenLastCalledWith(
      "Hello",
      NEMOTRON,
      true,
      [],
      [],
      false,
    );

    await userEvent.click(pill);
    expect(
      screen.getAllByRole("menuitemradio").map((item) => item.textContent),
    ).toEqual(["Off", "On", "Medium"]);
    await userEvent.click(
      screen.getByRole("menuitemradio", { name: "Medium" }),
    );
    await waitFor(() =>
      expect(saved).toEqual([{ model: NEMOTRON, think: "medium" }]),
    );
    await send(textarea);
    expect(onSubmit).toHaveBeenLastCalledWith(
      "Hello",
      NEMOTRON,
      "medium",
      [],
      [],
      false,
    );
  });

  it.each([
    [
      "Ollama says so ([true])",
      thinkingModel(QWEN, { values: [true], default: true }),
    ],
    [
      "ChatonLM saw it think with think off",
      { ...thinkingModel(QWEN, null), alwaysThinks: true },
    ],
  ] as [string, ModelDetails][])(
    "a model that always thinks (%s): no button, think not sent",
    async (_, details) => {
      const { onSubmit, textarea } = renderForm({ details: [details] });
      await waitFor(() => expect(picker().textContent).toBe(QWEN));
      expect(screen.queryByRole("button", { name: "Think" })).toBeNull();
      await send(textarea);
      expect(onSubmit).toHaveBeenLastCalledWith(
        "Hello",
        QWEN,
        undefined,
        [],
        [],
        false,
      );
    },
  );

  it.each([
    [
      "cannot think ([false])",
      [thinkingModel(QWEN, { values: [false], default: false })],
    ],
    [
      "has no thinking capability",
      [
        {
          model: QWEN,
          capabilities: ["completion"],
          thinking: null,
          alwaysThinks: false,
        },
      ],
    ],
    ["has no details (not installed, Ollama down)", []],
  ])(
    "a model that %s: no button, think not sent",
    async (_, details: ModelDetails[]) => {
      const { onSubmit, textarea } = renderForm({ details });
      await waitFor(() => expect(picker().textContent).toBe(QWEN));
      await settle();
      expect(screen.queryByRole("button", { name: "Think" })).toBeNull();
      expect(
        screen.queryByRole("button", { name: /^Thinking level/ }),
      ).toBeNull();
      await send(textarea);
      expect(onSubmit).toHaveBeenLastCalledWith(
        "Hello",
        QWEN,
        undefined,
        [],
        [],
        false,
      );
    },
  );

  it("another model gets its own control and remembered choice", async () => {
    let release = () => {};
    const { onSubmit, textarea } = renderForm({
      models: [QWEN, GPT],
      details: [ON_OFF, LEVELS],
      prefs: { [GPT]: "low" },
      heldDetails: { [GPT]: new Promise((resolve) => (release = resolve)) },
    });
    await thinkToggle();
    await userEvent.click(picker());
    await userEvent.click(await screen.findByRole("option", { name: GPT }));
    await waitFor(() => expect(picker().textContent).toBe(GPT));
    // Détails de GPT en chargement : aucun bouton, pas celui de QWEN.
    const noButton = () => {
      expect(screen.queryByRole("button", { name: "Think" })).toBeNull();
      expect(
        screen.queryByRole("button", { name: /^Thinking level/ }),
      ).toBeNull();
    };
    noButton();
    // Son réglage (« low ») connu entre-temps ne change rien.
    await settle();
    noButton();
    release();
    const pill = await thinkLevels();
    await waitFor(() => expect(pill.title).toBe("Thinking level: Low"));
    expect(screen.queryByRole("button", { name: "Think" })).toBeNull();
    await send(textarea);
    expect(onSubmit).toHaveBeenLastCalledWith(
      "Hello",
      GPT,
      "low",
      [],
      [],
      false,
    );
  });

  it("hidden while Ollama is unreachable, like the official tools", async () => {
    const { update } = renderForm({ details: [ON_OFF] });
    await thinkToggle();
    update({ isDisabled: true });
    expect(screen.queryByRole("button", { name: "Think" })).toBeNull();
    update({ isDisabled: false });
    expect(screen.getByRole("button", { name: "Think" })).toBeTruthy();
  });

  it("Escape in the Think menu does not stop the reply", async () => {
    const { onCancel } = renderForm({
      isStreaming: true,
      selectedModel: GPT,
      details: [LEVELS],
    });
    await userEvent.click(await thinkLevels());
    await waitFor(() =>
      expect(document.activeElement?.getAttribute("role")).toBe(
        "menuitemradio",
      ),
    );
    await userEvent.keyboard("{Escape}");
    expect(screen.queryByRole("menu")).toBeNull();
    expect(onCancel).not.toHaveBeenCalled();
    await userEvent.keyboard("{Escape}");
    expect(onCancel).toHaveBeenCalledTimes(1);
  });

  it("opening the Think menu closes the model picker, and the other way round", async () => {
    renderForm({ models: [QWEN, GPT], selectedModel: GPT, details: [LEVELS] });
    const pill = await thinkLevels();
    await userEvent.click(picker());
    expect(screen.getByRole("listbox")).toBeTruthy();
    // Sans mousedown (clavier, clic qui ne prend pas le focus) : seule la
    // fermeture mutuelle peut fermer l'autre menu.
    fireEvent.click(pill);
    expect(screen.getByRole("menu")).toBeTruthy();
    expect(screen.queryByRole("listbox")).toBeNull();

    fireEvent.click(picker());
    expect(screen.getByRole("listbox")).toBeTruthy();
    expect(screen.queryByRole("menu")).toBeNull();
  });
});

describe("ChatForm: Tab order", () => {
  const tab = () => userEvent.keyboard("{Tab}");
  const shiftTab = () => userEvent.keyboard("{Shift>}{Tab}{/Shift}");

  // Ordre natif, pas le cycle de l'officiel : celui-ci ne sort jamais de la
  // zone de saisie (piège clavier), la barre latérale devient inatteignable.
  it("follows the document order through +, Think, the picker, voice mode and Send", async () => {
    const { textarea } = renderForm({ details: [ON_OFF] });
    const toggle = await thinkToggle();
    await userEvent.type(textarea, "Hello");
    const sendButton = screen.getByLabelText("Send message");
    const voiceButton = screen.getByLabelText("Start voice mode");
    textarea.focus();

    await tab();
    expect(document.activeElement).toBe(uploadButton());
    await tab();
    expect(document.activeElement).toBe(toggle);
    await tab();
    expect(document.activeElement).toBe(picker());
    await tab();
    expect(document.activeElement).toBe(voiceButton);
    await tab();
    expect(document.activeElement).toBe(sendButton);
    for (let i = 0; i < 5; i++) await shiftTab();
    expect(document.activeElement).toBe(textarea);
  });

  it("lets the focus leave the message area", async () => {
    const { textarea } = renderForm({ details: [ON_OFF] });
    const toggle = await thinkToggle();
    const sendButton = screen.getByLabelText("Send message");
    // Tab n'est jamais détourné, ni dans la zone ni sur les contrôles.
    for (const [target, shiftKey] of [
      [textarea, false],
      [textarea, true],
      [toggle, false],
      [sendButton, false],
    ] as const) {
      const event = new KeyboardEvent("keydown", {
        key: "Tab",
        shiftKey,
        bubbles: true,
        cancelable: true,
      });
      target.dispatchEvent(event);
      expect(event.defaultPrevented).toBe(false);
    }
    // Maj+Tab depuis la zone de saisie : avant elle dans le document, pas
    // vers le dernier contrôle.
    textarea.focus();
    await shiftTab();
    expect(document.activeElement).not.toBe(sendButton);
    expect(document.activeElement).not.toBe(textarea);
  });

  it("puts the Think button in the order after a model change", async () => {
    const { textarea } = renderForm({
      models: [QWEN, GPT],
      details: [LEVELS],
    });
    await waitFor(() => expect(picker().textContent).toBe(QWEN));
    await settle();
    textarea.focus();
    await tab();
    await tab();
    expect(document.activeElement).toBe(picker());

    await userEvent.click(picker());
    await userEvent.click(await screen.findByRole("option", { name: GPT }));
    const pill = await thinkLevels();
    textarea.focus();
    await tab();
    await tab();
    expect(document.activeElement).toBe(pill);
    await tab();
    expect(document.activeElement).toBe(picker());
  });
});

const TOOLS: ModelDetails = {
  model: QWEN,
  capabilities: ["completion", "tools"],
  thinking: null,
  alwaysThinks: false,
};

const globe = () => screen.queryByRole("button", { name: "Web search" });

describe("ChatForm: web search", () => {
  it("shows the globe after Think for a model with tools, and toggles the setting", async () => {
    const { settings } = renderForm({
      details: [
        { ...TOOLS, capabilities: [...TOOLS.capabilities, "thinking"] },
      ],
    });
    const toggle = await thinkToggle();
    const button = await screen.findByRole("button", { name: "Web search" });
    // Place officielle : juste après Think.
    expect(toggle.nextElementSibling).toBe(button);
    expect(button.title).toBe("Enable web search");
    expect(button.getAttribute("aria-pressed")).toBe("false");
    expect(button.className).toContain("text-neutral-500");

    await userEvent.click(button);
    await waitFor(() => expect(button.title).toBe("Disable web search"));
    expect(settings.webSearchEnabled).toBe(true);
    expect(button.getAttribute("aria-pressed")).toBe("true");
    expect(button.className).toContain("bg-accent");

    await userEvent.click(button);
    await waitFor(() => expect(button.title).toBe("Enable web search"));
    expect(settings.webSearchEnabled).toBe(false);
  });

  it("no globe for a model without tools, or with unknown details", async () => {
    renderForm({ details: [ON_OFF] });
    await thinkToggle();
    expect(globe()).toBeNull();
    cleanup();

    const { commands } = renderForm();
    await waitFor(() => expect(picker().textContent).toBe(QWEN));
    await settle();
    expect(globe()).toBeNull();
    // État du nuage demandé seulement pour un modèle qui sait chercher.
    expect(commands).not.toContain("cloud_status");
  });

  it("the cloud turned off on the server hides the globe and turns web search off", async () => {
    const { settings, onSubmit, textarea } = renderForm({
      details: [TOOLS],
      webSearchEnabled: true,
      cloud: { disabled: true, source: "env" },
    });
    await waitFor(() => expect(settings.webSearchEnabled).toBe(false));
    expect(globe()).toBeNull();
    await send(textarea);
    expect(onSubmit).toHaveBeenLastCalledWith(
      "Hello",
      QWEN,
      undefined,
      [],
      [],
      false,
    );
  });

  it("sends webSearch only for a model with tools, the button on, never blocked by the account", async () => {
    const { onSubmit, textarea, commands } = renderForm({
      details: [TOOLS],
      webSearchEnabled: true,
    });
    await waitFor(() =>
      expect(globe()?.getAttribute("aria-pressed")).toBe("true"),
    );
    await send(textarea);
    expect(onSubmit).toHaveBeenLastCalledWith(
      "Hello",
      QWEN,
      undefined,
      [],
      [],
      true,
    );
    // Plus de bandeau ni de lecture du compte : l'erreur d'envoi le remplace.
    expect(screen.queryByText("Web search requires an Ollama account")).toBe(
      null,
    );
    expect(commands).not.toContain("account_status");
    cleanup();

    // Réglage resté actif, modèle sans outils : rien de demandé.
    const other = renderForm({ webSearchEnabled: true });
    await waitFor(() => expect(picker().textContent).toBe(QWEN));
    await send(other.textarea);
    expect(other.onSubmit).toHaveBeenLastCalledWith(
      "Hello",
      QWEN,
      undefined,
      [],
      [],
      false,
    );
  });
});

describe("ChatForm: retry of a message (web search error)", () => {
  const IMAGE = {
    id: 8,
    filename: "red.png",
    kind: "image" as const,
    mime: "image/png",
    size: 4,
  };
  const NOTES = {
    id: 7,
    filename: "notes.txt",
    kind: "text" as const,
    mime: "text/plain",
    size: 3,
  };
  const sent: Message = {
    id: 5,
    role: "user",
    content: "What is new?",
    thinking: "",
    model: null,
    createdAt: 1,
    updatedAt: 1,
    thinkingStartedAt: null,
    thinkingEndedAt: null,
    attachments: [NOTES, IMAGE],
    toolCalls: [],
  };

  it("turns web search off and sends the message again as an edit, the composer untouched", async () => {
    const { handle, onSubmit, settings, textarea } = renderForm({
      details: [{ ...TOOLS, ...ON_OFF, capabilities: ["tools", "thinking"] }],
      webSearchEnabled: true,
      prefs: { [QWEN]: false },
    });
    await waitFor(() =>
      expect(globe()?.getAttribute("aria-pressed")).toBe("true"),
    );
    await thinkToggle();
    await userEvent.type(textarea, "Draft");
    act(() => handle.current?.retry(sent, { webSearch: false }));
    // Modèle et Think de la zone de saisie, recherche web coupée ; images
    // retirées (capacités connues sans vision), comme un envoi.
    expect(onSubmit).toHaveBeenLastCalledWith(
      "What is new?",
      QWEN,
      false,
      [],
      [NOTES],
      false,
      5,
    );
    await waitFor(() => expect(settings.webSearchEnabled).toBe(false));
    expect(textarea.value).toBe("Draft");
  });

  it("gives the focus back to the message field", async () => {
    const { handle, textarea } = renderForm({ details: [TOOLS] });
    await waitFor(() => expect(globe()).not.toBeNull());
    await settle();
    textarea.blur();
    expect(document.activeElement).toBe(document.body);
    act(() => handle.current?.retry(sent, { webSearch: false }));
    expect(document.activeElement).toBe(textarea);
  });

  it("with web search: turns it back on and asks for it", async () => {
    const { handle, onSubmit, settings } = renderForm({
      details: [{ ...TOOLS, capabilities: ["tools", "vision"] }],
    });
    await waitFor(() =>
      expect(globe()?.getAttribute("aria-pressed")).toBe("false"),
    );
    act(() => handle.current?.retry(sent, { webSearch: true }));
    expect(onSubmit).toHaveBeenLastCalledWith(
      "What is new?",
      QWEN,
      undefined,
      [],
      [NOTES, IMAGE],
      true,
      5,
    );
    await waitFor(() => expect(settings.webSearchEnabled).toBe(true));
    await waitFor(() =>
      expect(globe()?.getAttribute("aria-pressed")).toBe("true"),
    );
  });

  it("with web search, for a model that cannot search: asked for as a send would", async () => {
    const { handle, onSubmit } = renderForm({ webSearchEnabled: true });
    await waitFor(() => expect(picker().textContent).toBe(QWEN));
    act(() => handle.current?.retry(sent, { webSearch: true }));
    expect(onSubmit).toHaveBeenLastCalledWith(
      "What is new?",
      QWEN,
      undefined,
      [],
      [NOTES, IMAGE],
      false,
      5,
    );
  });

  it("keeps the images for a model that can see", async () => {
    const { handle, onSubmit } = renderForm({
      details: [{ ...TOOLS, capabilities: ["tools", "vision"] }],
    });
    await waitFor(() => expect(globe()).not.toBeNull());
    act(() => handle.current?.retry(sent, { webSearch: false }));
    expect(onSubmit).toHaveBeenLastCalledWith(
      "What is new?",
      QWEN,
      undefined,
      [],
      [NOTES, IMAGE],
      false,
      5,
    );
  });

  it("does nothing during a reply or while Ollama is unreachable", async () => {
    const { handle, onSubmit, update, settings } = renderForm({
      details: [TOOLS],
      webSearchEnabled: true,
      isStreaming: true,
    });
    await waitFor(() => expect(globe()).not.toBeNull());
    act(() => handle.current?.retry(sent, { webSearch: false }));
    update({ isStreaming: false, isDisabled: true });
    act(() => handle.current?.retry(sent, { webSearch: false }));
    await settle();
    expect(onSubmit).not.toHaveBeenCalled();
    expect(settings.webSearchEnabled).toBe(true);
  });
});
