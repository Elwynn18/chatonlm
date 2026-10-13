// Bouton « Read aloud » des réponses face au simulateur (faux moteur de
// src/test/tts.ts) : états (repos, préparation, téléchargement de la voix,
// lecture, erreur), accord avant le téléchargement, réponse dans une autre
// langue, une seule lecture dans l'app, réponse en cours d'écriture lue par
// la lecture automatique, arrêt à l'ouverture d'une autre conversation ;
// puis les textes en pseudo-langue et en français.

import {
  act,
  configure,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { Message } from "@/bindings/Message";
import type { ReadAloudError } from "@/bindings/ReadAloudError";
import { setLanguage } from "@/i18n";
import { resetStreams } from "@/lib/streams";
import { fakeBackend, type FakeBackend } from "@/test/backend";
import { untranslatedTexts, usePseudoLanguage } from "@/test/i18n";
import { renderApp } from "@/test/renderApp";
import { installSimulator } from "@/test/simulator";
import { VOICE_EN } from "@/test/tts";

configure({ asyncUtilTimeout: 5000 });
vi.setConfig({ testTimeout: 30_000 });

// Flux des réponses : magasin du module, vidé entre les essais.
afterEach(() => {
  resetStreams();
});

const MODEL = "qwen3:0.6b";

/** Réponse nommée par l'état de la lecture. */
const answer = (messageId: number) => ({ kind: "message", messageId }) as const;
const FIRST = "Hello there. How can I help?";
const SECOND = "Paris is the capital of France.";
const THIRD = "Paris est la capitale de la France.";

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

/** Conversation `id` avec ces messages, enregistrée dans le faux backend. */
function store(backend: FakeBackend, id: string, messages: Message[]) {
  backend.chats.set(id, {
    id,
    title: id,
    createdAt: 1,
    updatedAt: 1,
    messages,
  });
}

/**
 * Conversation chat-1 : deux questions, deux réponses (2 et 4) ; `prepare`
 * règle le faux backend avant le premier rendu.
 */
function setup(
  prepare?: (backend: FakeBackend) => void,
  messages: Message[] = [
    message(1, "user", "Hi"),
    message(2, "assistant", FIRST),
    message(3, "user", "Capital of France?"),
    message(4, "assistant", SECOND),
  ],
) {
  const simulator = installSimulator();
  const backend = fakeBackend(simulator, { selectedModel: MODEL });
  store(backend, "chat-1", messages);
  prepare?.(backend);
  const app = renderApp("/c/chat-1");
  return { simulator, backend, tts: backend.tts, app };
}

/** Barre d'actions de la réponse qui contient `text`. */
function actionsOf(text: string): HTMLElement {
  const reply = [
    ...document.querySelectorAll<HTMLElement>("[data-role=assistant]"),
  ].find((element) => element.textContent?.includes(text));
  const actions = reply
    ?.closest(".mb-8")
    ?.querySelector<HTMLElement>("[data-role=actions]");
  if (!actions) throw new Error(`no actions under "${text}"`);
  return actions;
}

/** Bouton de lecture de la réponse qui contient `text`. */
function readButton(text: string): HTMLButtonElement {
  return actionsOf(text).querySelector<HTMLButtonElement>("[data-read-aloud]")!;
}

/** Bloc de la réponse qui contient `text` (texte, outils, actions). */
function replyOf(text: string): HTMLElement {
  const reply = [
    ...document.querySelectorAll<HTMLElement>("[data-role=assistant]"),
  ].find((element) => element.textContent?.includes(text));
  const box = reply?.closest<HTMLElement>(".mb-8");
  if (!box) throw new Error(`no reply "${text}"`);
  return box;
}

/**
 * Textes annoncés pour cette réponse : région de la lecture (étape, erreur)
 * et région du bouton (refus d'un clic), dans cet ordre, sans les vides.
 */
function announced(text: string): string {
  return [
    ...replyOf(text).querySelectorAll(
      "[data-read-aloud-status], [data-read-aloud-reason]",
    ),
  ]
    .map((region) => region.textContent ?? "")
    .filter(Boolean)
    .join(" | ");
}

/** Texte qui décrit le bouton de cette réponse (aria-describedby). */
function description(text: string): string | null {
  const id = readButton(text).getAttribute("aria-describedby");
  return id === null ? null : (document.getElementById(id)?.textContent ?? "");
}

/** Texte montré à côté du bouton (progression ou raison), vide sinon. */
function shown(text: string): string {
  const actions = actionsOf(text);
  return (
    actions.querySelector(
      "[data-testid=read-aloud-progress], [data-testid=read-aloud-problem]",
    )?.textContent ?? ""
  );
}

/** Attend l'étape montrée par le bouton de cette réponse. */
async function phaseIs(text: string, phase: string) {
  await waitFor(() => expect(readButton(text).dataset.readAloud).toBe(phase));
}

async function ready() {
  await waitFor(() => expect(readButton(SECOND)).toBeTruthy());
}

describe("Read aloud button", () => {
  it("next to Copy under each finished response, not under the questions", async () => {
    setup();
    await ready();
    for (const text of [FIRST, SECOND]) {
      expect(
        within(actionsOf(text))
          .getAllByRole("button")
          .map((button) => button.getAttribute("aria-label")),
      ).toEqual(["Copy", "Read aloud"]);
      expect(readButton(text).title).toBe("Read aloud");
      expect(announced(text)).toBe("");
    }
    expect(screen.getAllByRole("button", { name: "Read aloud" })).toHaveLength(
      2,
    );
  });

  it("reads: Preparing to read…, then Stop reading, which stops", async () => {
    const { simulator, tts } = setup((backend) =>
      backend.tts.install(VOICE_EN),
    );
    await ready();
    await userEvent.click(readButton(SECOND));
    expect(simulator.callsTo("read_aloud")).toEqual([
      { chatId: "chat-1", messageId: 4, download: false },
    ]);
    await waitFor(() => expect(shown(SECOND)).toBe("Preparing to read…"));
    expect(readButton(SECOND).getAttribute("aria-label")).toBe("Stop reading");
    expect(readButton(SECOND).getAttribute("aria-busy")).toBe("true");
    expect(announced(SECOND)).toBe("Preparing to read…");

    await tts.play();
    await phaseIs(SECOND, "playing");
    expect(readButton(SECOND).title).toBe("Stop reading");
    expect(readButton(SECOND).getAttribute("aria-busy")).toBeNull();
    expect(shown(SECOND)).toBe("");
    expect(announced(SECOND)).toBe("Reading aloud");
    // L'autre réponse reste au repos.
    expect(readButton(FIRST).getAttribute("aria-label")).toBe("Read aloud");

    await userEvent.click(readButton(SECOND));
    expect(simulator.callsTo("read_aloud_stop")).toHaveLength(1);
    await waitFor(() =>
      expect(readButton(SECOND).getAttribute("aria-label")).toBe("Read aloud"),
    );
    expect(announced(SECOND)).toBe("");
    expect(simulator.callsTo("read_aloud")).toHaveLength(1);
  });

  it("the state of the engine is read when the chat opens", async () => {
    setup((backend) => {
      backend.tts.install(VOICE_EN);
      backend.tts.state = {
        source: { kind: "message", messageId: 2 },
        phase: "playing",
        percent: null,
        error: null,
      };
    });
    await ready();
    await waitFor(() =>
      expect(readButton(FIRST).getAttribute("aria-label")).toBe("Stop reading"),
    );
    expect(readButton(SECOND).getAttribute("aria-label")).toBe("Read aloud");
  });

  it("the voice is missing: asked first; Download and read downloads it with its progress, then reads", async () => {
    const { simulator, tts } = setup();
    await ready();
    await userEvent.click(readButton(SECOND));
    let dialog = await screen.findByRole("alertdialog", {
      name: "Download the voice?",
    });
    expect(dialog.textContent).toContain(
      "Reading aloud in English needs the English voice (175 MB). It's downloaded once from the Internet (files from GitHub and Hugging Face) and stays on this computer.",
    );
    // Annuler : rien n'est téléchargé.
    await userEvent.click(
      within(dialog).getByRole("button", { name: "Cancel" }),
    );
    expect(screen.queryByRole("alertdialog")).toBeNull();
    expect(simulator.callsTo("read_aloud")).toEqual([
      { chatId: "chat-1", messageId: 4, download: false },
    ]);
    expect(readButton(SECOND).getAttribute("aria-label")).toBe("Read aloud");

    await userEvent.click(readButton(SECOND));
    dialog = await screen.findByRole("alertdialog");
    await userEvent.click(
      within(dialog).getByRole("button", { name: "Download and read" }),
    );
    expect(simulator.callsTo("read_aloud").at(-1)).toEqual({
      chatId: "chat-1",
      messageId: 4,
      download: true,
    });
    await waitFor(() =>
      expect(shown(SECOND)).toBe("Downloading the voice… 0%"),
    );
    expect(readButton(SECOND).getAttribute("aria-label")).toBe("Stop reading");
    expect(announced(SECOND)).toBe("Downloading the voice…");
    await tts.setState({ percent: 42 });
    await waitFor(() =>
      expect(shown(SECOND)).toBe("Downloading the voice… 42%"),
    );
    // Le pourcentage n'est pas annoncé à chaque pas.
    expect(announced(SECOND)).toBe("Downloading the voice…");

    await tts.finish(VOICE_EN);
    await tts.setState({ phase: "preparing", percent: null });
    await waitFor(() => expect(shown(SECOND)).toBe("Preparing to read…"));
    await tts.play();
    await phaseIs(SECOND, "playing");
  });

  it("without ONNX Runtime: no dialog, the reason under the response at once", async () => {
    setup((backend) => {
      for (const voice of backend.tts.voices) {
        voice.runtimeProblem = { kind: "missing", detail: "libonnxruntime" };
      }
    });
    await ready();
    await userEvent.click(readButton(SECOND));
    const text = "Reading aloud isn't available on this computer.";
    await waitFor(() => expect(shown(SECOND)).toBe(text));
    expect(screen.queryByRole("alertdialog")).toBeNull();
    expect(announced(SECOND)).toBe(text);
  });

  it("the voice already downloading (ModelHub): no dialog, the reading waits for it; Stop cancels that download", async () => {
    const { simulator, tts } = setup((backend) =>
      backend.tts.downloads.push({
        id: VOICE_EN,
        name: "Pocket TTS · English (Alba)",
        state: "downloading",
        completedBytes: 1,
        totalBytes: 2,
        error: null,
      }),
    );
    await ready();
    await userEvent.click(readButton(SECOND));
    await phaseIs(SECOND, "downloading");
    expect(screen.queryByRole("alertdialog")).toBeNull();
    expect(simulator.callsTo("read_aloud")).toEqual([
      { chatId: "chat-1", messageId: 4, download: false },
    ]);
    await userEvent.click(readButton(SECOND));
    await phaseIs(SECOND, "idle");
    expect(tts.downloads[0].state).toBe("cancelled");
    expect(shown(SECOND)).toBe("");
  });

  it("asks for the voice of the language of the response", async () => {
    const { simulator } = setup((backend) => {
      backend.tts.install(VOICE_EN);
      backend.tts.languages.set(2, "fr");
    });
    await ready();
    await userEvent.click(readButton(FIRST));
    const dialog = await screen.findByRole("alertdialog");
    expect(dialog.textContent).toContain(
      "Reading aloud in French needs the French voice (175 MB).",
    );
    await userEvent.click(
      within(dialog).getByRole("button", { name: "Download and read" }),
    );
    expect(simulator.callsTo("read_aloud").at(-1)).toEqual({
      chatId: "chat-1",
      messageId: 2,
      download: true,
    });
    expect(simulator.callsTo("read_aloud")).toHaveLength(2);
  });

  it.each([
    [
      "other",
      "Reading aloud is available in English, French and Spanish.",
    ] as const,
    ["empty", "There's nothing to read aloud in this response."] as const,
  ])(
    "a response the engine can't read (%s): the button is turned off and says why",
    async (reading, text) => {
      const { simulator } = setup((backend) => {
        backend.tts.install(VOICE_EN);
        backend.tts.languages.set(4, reading);
      });
      await ready();
      await userEvent.click(readButton(SECOND));
      await waitFor(() => expect(shown(SECOND)).toBe(text));
      const button = readButton(SECOND);
      expect(button.getAttribute("aria-disabled")).toBe("true");
      expect(button.title).toBe(text);
      expect(announced(SECOND)).toBe(text);
      // Le nom reste « Read aloud », la raison le décrit.
      expect(button.getAttribute("aria-label")).toBe("Read aloud");
      expect(description(SECOND)).toBe(text);
      // Inactif : un autre clic ne redemande rien.
      await userEvent.click(button);
      expect(simulator.callsTo("read_aloud")).toHaveLength(1);
      // L'autre réponse se lit toujours.
      expect(readButton(FIRST).getAttribute("aria-disabled")).toBeNull();
    },
  );

  it("one reading at a time: another response being read puts this one back to rest", async () => {
    const { tts } = setup((backend) => backend.tts.install(VOICE_EN));
    await ready();
    await userEvent.click(readButton(FIRST));
    await phaseIs(FIRST, "preparing");
    await tts.play();
    await phaseIs(FIRST, "playing");

    await userEvent.click(readButton(SECOND));
    await waitFor(() =>
      expect(readButton(SECOND).getAttribute("aria-label")).toBe(
        "Stop reading",
      ),
    );
    expect(readButton(FIRST).getAttribute("aria-label")).toBe("Read aloud");
    expect(announced(FIRST)).toBe("");
  });

  it.each([
    [
      "noOutputDevice",
      "Couldn't read aloud: no audio output device was found.",
    ],
    ["runtimeUnavailable", "Reading aloud isn't available on this computer."],
    ["downloadFailed", "The voice couldn't be downloaded. Try again."],
    ["engineFailed", "Couldn't read this response aloud."],
  ] as [ReadAloudError, string][])(
    "an error of the reading (%s) is shown under its response only",
    async (error, text) => {
      const { tts } = setup((backend) => backend.tts.install(VOICE_EN));
      await ready();
      await userEvent.click(readButton(SECOND));
      await waitFor(() =>
        expect(readButton(SECOND).dataset.readAloud).toBe("preparing"),
      );
      // Fin en erreur : la réponse reste nommée avec l'erreur.
      await tts.end(error);
      await waitFor(() => expect(shown(SECOND)).toBe(text));
      expect(announced(SECOND)).toBe(text);
      expect(description(SECOND)).toBe(text);
      expect(readButton(SECOND).getAttribute("aria-label")).toBe("Read aloud");
      expect(replyOf(FIRST).textContent).not.toContain(text);
      expect(announced(FIRST)).toBe("");
      // Nouvel essai : l'erreur s'efface.
      await userEvent.click(readButton(SECOND));
      await waitFor(() => expect(shown(SECOND)).not.toBe(text));
    },
  );

  it("an error stays under its response when another one is then refused or asks for its voice", async () => {
    const { tts } = setup(
      (backend) => {
        backend.tts.install(VOICE_EN);
        backend.tts.languages.set(4, "other");
        backend.tts.languages.set(6, "fr");
      },
      [
        message(1, "user", "Hi"),
        message(2, "assistant", FIRST),
        message(3, "user", "Capital of France?"),
        message(4, "assistant", SECOND),
        message(5, "user", "En français ?"),
        message(6, "assistant", THIRD),
      ],
    );
    await waitFor(() => expect(readButton(THIRD)).toBeTruthy());
    const error = "Couldn't read aloud: no audio output device was found.";
    await userEvent.click(readButton(FIRST));
    await phaseIs(FIRST, "preparing");
    await tts.end("noOutputDevice");
    await waitFor(() => expect(shown(FIRST)).toBe(error));

    // Réponse dans une autre langue : sa raison, pas l'erreur de la première.
    const reason = "Reading aloud is available in English, French and Spanish.";
    await userEvent.click(readButton(SECOND));
    await waitFor(() => expect(shown(SECOND)).toBe(reason));
    expect(announced(SECOND)).toBe(reason);
    expect(shown(FIRST)).toBe(error);

    // Voix absente : le dialogue, sans erreur sous cette réponse.
    await userEvent.click(readButton(THIRD));
    const dialog = await screen.findByRole("alertdialog");
    expect(shown(THIRD)).toBe("");
    expect(announced(THIRD)).toBe("");
    await userEvent.click(
      within(dialog).getByRole("button", { name: "Cancel" }),
    );
    expect(shown(THIRD)).toBe("");
    expect(shown(FIRST)).toBe(error);
  });

  it("a command refused by the engine shows its message", async () => {
    const { backend } = setup();
    backend.tts.failures.set("read_aloud", {
      kind: "notFound",
      message: "This message no longer exists.",
    });
    await ready();
    await userEvent.click(readButton(SECOND));
    await waitFor(() =>
      expect(shown(SECOND)).toBe("This message no longer exists."),
    );
    expect(readButton(SECOND).getAttribute("aria-disabled")).toBeNull();
  });
});

describe("Read aloud and the response being written", () => {
  it("automatic reading: Stop reading under the response while it's read, Copy only once finished", async () => {
    const simulator = installSimulator();
    const backend = fakeBackend(simulator, {
      selectedModel: MODEL,
      thinkPrefs: { [MODEL]: false },
    });
    backend.tts.install(VOICE_EN);
    store(backend, "chat-1", [
      message(1, "user", "Hi"),
      message(2, "assistant", FIRST),
    ]);
    const app = renderApp("/c/chat-1");
    await waitFor(() => expect(readButton(FIRST)).toBeTruthy());
    const replay = simulator.queue("existing-chat", { stepwise: true });
    void app.send({
      chatId: "chat-1",
      content: "Say hello in one word.",
      model: MODEL,
      think: false,
    });
    await replay.until("content");
    const reply = () =>
      [...document.querySelectorAll("[data-role=assistant]")].at(-1)!;
    await waitFor(() => expect(reply().textContent).toBe('"'));
    const actions = () =>
      reply().closest(".mb-8")!.querySelector("[data-role=actions]");
    // En cours d'écriture, pas lue : aucune action, mais la région des
    // annonces est déjà là, vide.
    expect(actions()).toBeNull();
    const region = reply()
      .closest(".mb-8")!
      .querySelector("[data-read-aloud-status]");
    expect(region?.getAttribute("role")).toBe("status");
    expect(region?.textContent).toBe("");

    await backend.tts.setState({ source: answer(4), phase: "preparing" });
    await waitFor(() => expect(region?.textContent).toBe("Preparing to read…"));
    expect(
      reply().closest(".mb-8")!.querySelector("[data-read-aloud-status]"),
    ).toBe(region);
    await backend.tts.play();
    await waitFor(() => expect(actions()).not.toBeNull());
    const stop = within(actions() as HTMLElement).getByRole("button", {
      name: "Stop reading",
    });
    expect(
      within(actions() as HTMLElement).queryByRole("button", { name: "Copy" }),
    ).toBeNull();
    await userEvent.click(stop);
    expect(simulator.callsTo("read_aloud_stop")).toHaveLength(1);
    await waitFor(() => expect(actions()).toBeNull());
    // Le focus du bouton retiré va au texte de la réponse, focalisable le
    // temps d'y être.
    expect(document.activeElement).toBe(reply());
    expect(reply().getAttribute("tabindex")).toBe("-1");
    act(() => (document.activeElement as HTMLElement).blur());
    expect(reply().hasAttribute("tabindex")).toBe(false);

    // Lecture automatique en erreur avant la fin de la réponse : le bouton
    // et l'erreur, tout de suite.
    await backend.tts.setState({
      source: answer(4),
      phase: "idle",
      error: "noOutputDevice",
    });
    await waitFor(() => expect(actions()).not.toBeNull());
    expect(actions()!.textContent).toContain(
      "Couldn't read aloud: no audio output device was found.",
    );

    // Lue de nouveau, puis finie pendant la lecture : Copier s'ajoute, le
    // bouton de lecture reste le même (focus gardé).
    await backend.tts.setState({ source: answer(4), phase: "playing" });
    await waitFor(() => expect(actions()).not.toBeNull());
    const button = within(actions() as HTMLElement).getByRole("button", {
      name: "Stop reading",
    });
    button.focus();
    await replay.finish();
    await waitFor(() =>
      expect(
        within(actions() as HTMLElement)
          .getAllByRole("button")
          .map((element) => element.getAttribute("aria-label")),
      ).toEqual(["Copy", "Stop reading"]),
    );
    expect(document.activeElement).toBe(button);
  });

  it("a tool turn shows Stop reading only while it is read", async () => {
    const tools = message(4, "assistant", "Let me look that up.", {
      toolCalls: [
        {
          name: "web_search",
          arguments: '{"query":"capital of France"}',
          result: "[]",
          error: null,
        },
      ],
    });
    const { tts } = setup(
      (backend) => backend.tts.install(VOICE_EN),
      [message(1, "user", "Hi"), message(2, "assistant", FIRST), tools],
    );
    await waitFor(() => expect(readButton(FIRST)).toBeTruthy());
    const toolActions = () =>
      [...document.querySelectorAll<HTMLElement>("[data-role=assistant]")]
        .find((element) => element.textContent?.includes("Let me look"))!
        .closest(".mb-8")!
        .querySelector("[data-role=actions]");
    expect(toolActions()).toBeNull();
    await tts.setState({ source: answer(4), phase: "playing" });
    await waitFor(() => expect(toolActions()).not.toBeNull());
    expect(
      within(toolActions() as HTMLElement)
        .getAllByRole("button")
        .map((element) => element.getAttribute("aria-label")),
    ).toEqual(["Stop reading"]);
    await tts.end();
    await waitFor(() => expect(toolActions()).toBeNull());
    // Lecture automatique en erreur : le bouton et l'erreur restent.
    await tts.setState({
      source: answer(4),
      phase: "idle",
      error: "engineFailed",
    });
    await waitFor(() => expect(toolActions()).not.toBeNull());
    expect(toolActions()!.textContent).toContain(
      "Couldn't read this response aloud.",
    );
    expect(
      within(toolActions() as HTMLElement)
        .getAllByRole("button")
        .map((element) => element.getAttribute("aria-label")),
    ).toEqual(["Read aloud"]);
  });
});

describe("Read aloud when the conversation changes", () => {
  it("opening another conversation stops the reading; the settings and back don't", async () => {
    const { simulator, tts, backend, app } = setup((backend) =>
      backend.tts.install(VOICE_EN),
    );
    store(backend, "chat-2", [
      message(10, "user", "Other"),
      message(11, "assistant", "Another answer."),
    ]);
    await ready();
    await userEvent.click(readButton(SECOND));
    await phaseIs(SECOND, "preparing");
    await tts.play();
    await phaseIs(SECOND, "playing");

    await act(() => app.router.navigate({ to: "/settings" }));
    await screen.findByTestId("settings");
    await act(() =>
      app.router.navigate({ to: "/c/$chatId", params: { chatId: "chat-1" } }),
    );
    await ready();
    expect(readButton(SECOND).getAttribute("aria-label")).toBe("Stop reading");
    expect(simulator.callsTo("read_aloud_stop")).toEqual([]);

    await act(() =>
      app.router.navigate({ to: "/c/$chatId", params: { chatId: "chat-2" } }),
    );
    await waitFor(() =>
      expect(simulator.callsTo("read_aloud_stop")).toHaveLength(1),
    );
  });

  it("stopped at each change of conversation whatever the known state, not when a new chat gets its id", async () => {
    // Lecture automatique au repos tant que la langue de la réponse n'est
    // pas connue : l'état connu ne dit pas s'il y a une lecture à arrêter.
    const { simulator, backend, app } = setup();
    store(backend, "chat-2", [message(10, "user", "Other")]);
    store(backend, "chat-3", [message(20, "user", "Third")]);
    await ready();
    const stops = () => simulator.callsTo("read_aloud_stop").length;
    const open = (chatId: string) =>
      act(() => app.router.navigate({ to: "/c/$chatId", params: { chatId } }));
    await open("chat-2");
    await waitFor(() => expect(stops()).toBe(1));
    await open("new");
    await screen.findByTestId("new-chat");
    await waitFor(() => expect(stops()).toBe(2));
    // Nouvelle conversation créée : la page passe à son identifiant.
    await open("chat-3");
    await act(() => app.router.navigate({ to: "/settings" }));
    await screen.findByTestId("settings");
    await open("chat-3");
    await screen.findByText("Third");
    expect(stops()).toBe(2);
  });
});

describe("Read aloud in other languages", () => {
  it("pseudo-language: no text written in the code (each state, dialog)", async () => {
    usePseudoLanguage();
    const { tts } = setup();
    await ready();
    const list = () => screen.getByTestId("chat");
    const allow = [FIRST, SECOND, "Hi", "Capital of France?", MODEL, "chat-1"];
    const leftovers = () =>
      untranslatedTexts(
        list().querySelector("[data-role=message-list]")!,
        allow,
      );
    expect(leftovers()).toEqual([]);

    await userEvent.click(readButton(SECOND));
    const dialog = await screen.findByRole("alertdialog");
    expect(untranslatedTexts(dialog)).toEqual([]);
    await userEvent.click(
      within(dialog).getByRole("button", { name: "⟦Download and read⟧" }),
    );
    await waitFor(() =>
      expect(readButton(SECOND).dataset.readAloud).toBe("downloading"),
    );
    await tts.setState({ percent: 30 });
    await waitFor(() => expect(shown(SECOND)).toContain("30%"));
    expect(leftovers()).toEqual([]);
    await tts.setState({ phase: "preparing", percent: null });
    await phaseIs(SECOND, "preparing");
    expect(leftovers()).toEqual([]);
    await tts.play();
    await phaseIs(SECOND, "playing");
    expect(leftovers()).toEqual([]);
    await tts.end("noOutputDevice");
    await waitFor(() =>
      expect(
        actionsOf(SECOND).querySelector("[data-testid=read-aloud-problem]"),
      ).not.toBeNull(),
    );
    expect(leftovers()).toEqual([]);
  });

  it("in French: the button, the dialog with the French voice, the progress", async () => {
    const { tts } = setup((backend) => backend.tts.languages.set(4, "fr"));
    await ready();
    act(() => setLanguage("fr"));
    expect(readButton(SECOND).getAttribute("aria-label")).toBe(
      "Lire à voix haute",
    );
    await userEvent.click(readButton(SECOND));
    const dialog = await screen.findByRole("alertdialog", {
      name: "Télécharger la voix ?",
    });
    expect(dialog.textContent).toContain(
      "La lecture en français nécessite la voix française (175 Mo).",
    );
    await userEvent.click(
      within(dialog).getByRole("button", { name: "Télécharger et lire" }),
    );
    await tts.setState({ percent: 42 });
    await waitFor(() =>
      expect(shown(SECOND)).toBe("Téléchargement de la voix… 42 %"),
    );
    expect(readButton(SECOND).getAttribute("aria-label")).toBe(
      "Arrêter la lecture",
    );
    await tts.play();
    await waitFor(() => expect(announced(SECOND)).toBe("Lecture en cours"));
  });
});
