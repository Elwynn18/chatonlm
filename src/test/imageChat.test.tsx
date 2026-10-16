// Génération d'images dans la conversation (propre à ChatonLM ; contrat
// .tools/research/84-image-chat-contract.md § 4) : l'app face au simulateur.
// Mode Image de la zone de saisie (requête `image: true`, refus locaux, lien
// vers l'onglet Images du ModelHub), outil `generate_image` appelé par le
// modèle (avancement, aperçu, image affichée, Save image…), arrêt et erreur.
// Les séquences sont construites ici, d'après les types du moteur (§ 1).

import {
  act,
  configure,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it } from "vitest";
import type { ChatEvent } from "@/bindings/ChatEvent";
import type { ImageProgress } from "@/bindings/ImageProgress";
import type { Message } from "@/bindings/Message";
import { attachmentImageUrl } from "@/lib/attachments";
import * as streams from "@/lib/streams";
import { fakeBackend, type StoredChat } from "@/test/backend";
import { untranslatedTexts, usePseudoLanguage } from "@/test/i18n";
import { KLEIN_ID, Z_IMAGE_ID } from "@/test/images";
import { composer, renderApp, texts } from "@/test/renderApp";
import {
  installSimulator,
  SEQUENCE_TIME,
  type Sequence,
} from "@/test/simulator";

configure({ asyncUtilTimeout: 5000 });

afterEach(() => {
  streams.resetStreams();
});

const MODEL = "qwen3:0.6b";
const TOOL_MODEL = "chatonlm-image";
const PROMPT = "A red kitten on a cream sofa";
const IMAGE_MODEL = "FLUX.2 klein 4B";
const RESULT = JSON.stringify({
  attachmentId: 42,
  width: 512,
  height: 512,
  seed: 7,
  model: IMAGE_MODEL,
});
const PREVIEW = "data:image/png;base64,iVBORw0KGgo=";
const NOTES = "/home/u/notes.txt";

function setup({
  selectedModel = MODEL,
  models = [MODEL, TOOL_MODEL],
  chosen = true,
  gpu = true,
  engineAvailable = true,
  missingCpuFeatures = [] as string[],
  chats = [] as StoredChat[],
  path = "/c/new",
} = {}) {
  const simulator = installSimulator({ latencyMs: 10 });
  const backend = fakeBackend(simulator, {
    selectedModel,
    models,
    details: [
      {
        model: TOOL_MODEL,
        capabilities: ["completion", "tools"],
        thinking: null,
        alwaysThinks: false,
      },
    ],
    files: { [NOTES]: { size: 13 } },
  });
  backend.images.machine = {
    ...backend.images.machine,
    gpu,
    engineAvailable,
    missingCpuFeatures,
  };
  if (chosen) {
    backend.images.install(KLEIN_ID);
    backend.images.select(KLEIN_ID);
  }
  for (const chat of chats) backend.chats.set(chat.id, chat);
  const app = renderApp(path);
  return { simulator, backend, app };
}

function userMessage(content: string): Message {
  return {
    id: 1,
    role: "user",
    content,
    thinking: "",
    model: null,
    createdAt: SEQUENCE_TIME,
    updatedAt: SEQUENCE_TIME,
    thinkingStartedAt: null,
    thinkingEndedAt: null,
    attachments: [],
    toolCalls: [],
  };
}

function progress(fields: Partial<ImageProgress>): ImageProgress {
  return {
    stage: "starting",
    step: null,
    total: null,
    preview: null,
    cpu: false,
    width: 512,
    height: 512,
    ...fields,
  };
}

const imageEvent = (fields: Partial<ImageProgress>): ChatEvent => ({
  type: "imageProgress",
  messageId: 2,
  index: 0,
  progress: progress(fields),
});

const READY: ChatEvent = {
  type: "imageReady",
  messageId: 2,
  index: 0,
  attachment: {
    id: 42,
    filename: "image-7.png",
    kind: "image",
    mime: "image/png",
    size: 2048,
  },
};

/** Appel `generate_image` annoncé par la réponse 2. */
function callEvent(args: Record<string, unknown>): ChatEvent {
  return {
    type: "toolCalls",
    messageId: 2,
    toolCalls: [
      {
        name: "generate_image",
        arguments: JSON.stringify(args),
        result: null,
        error: null,
      },
    ],
  };
}

/**
 * Envoi du mode Image d'une nouvelle conversation, comme le moteur (§ 2.3) :
 * pas d'Ollama, un appel synthétique, puis `tail` (fin de l'image).
 */
function buttonSequence(tail: ChatEvent[], model = MODEL): Sequence {
  return {
    description: "Image mode send (built by the test).",
    request: { streamId: "stream-1", content: PROMPT, model, image: true },
    events: [
      { type: "chatCreated", chatId: "chat-1" },
      { type: "userMessage", message: userMessage(PROMPT) },
      { type: "replyStarted", messageId: 2, createdAt: SEQUENCE_TIME },
      callEvent({ prompt: PROMPT }),
      ...tail,
    ],
    error: null,
  };
}

/** Image réussie : avancement, image enregistrée, résultat, fin. */
const SUCCESS: ChatEvent[] = [
  imageEvent({ stage: "starting" }),
  imageEvent({ stage: "loading" }),
  imageEvent({ stage: "generating", step: 1, total: 4, preview: PREVIEW }),
  READY,
  { type: "toolResult", messageId: 2, index: 0, result: RESULT, error: null },
  { type: "done", messageId: 2 },
];

/** Réponse finie : flux retiré, bouton d'envoi revenu. */
async function replyEnded() {
  await waitFor(() => {
    expect(streams.streamsFor("chat-1")).toHaveLength(0);
    expect(streams.pendingNewStream()).toBeNull();
    screen.getByLabelText("Send message");
  });
}

/** Zone de saisie (pseudo-langue comprise), une fois la page montée. */
async function field() {
  await screen.findByTestId("new-chat");
  return waitFor(() => {
    const textarea = document.querySelector("textarea");
    if (!textarea) throw new Error("no message field yet");
    return textarea;
  });
}

/** Tape et envoie, une fois `model` choisi par le sélecteur. */
async function sendTo(model: string, text: string) {
  await field();
  await waitFor(() =>
    expect(screen.getByTitle("Select model").textContent).toBe(model),
  );
  await userEvent.type(composer(), `${text}{Enter}`);
}

/** Bouton « Image » (bascule). */
const imageButton = () => screen.findByRole("button", { name: "Image" });

/** Active le mode Image (bouton montré une fois la machine lue). */
async function enableImageMode() {
  await userEvent.click(await imageButton());
  await waitFor(() =>
    expect(composer().placeholder).toBe("Describe the image to create"),
  );
}

const generated = () =>
  document.querySelector<HTMLImageElement>("[data-role=generated-image] img");

describe("image mode of the composer", () => {
  it("sends the text as is with image: true, shows the progress, then the image", async () => {
    const { simulator } = setup();
    const replay = simulator.queue(buttonSequence(SUCCESS), {
      stepwise: true,
    });
    await enableImageMode();
    const toggle = await imageButton();
    expect(toggle.getAttribute("aria-pressed")).toBe("true");
    expect(toggle.className).toContain("bg-accent");
    // Carte graphique : pas de ligne ambre.
    expect(screen.queryByTestId("image-no-gpu")).toBeNull();
    // Ni Think ni recherche web pour cet envoi.
    expect(screen.queryByRole("button", { name: "Web search" })).toBeNull();
    await userEvent.type(composer(), `${PROMPT}{Enter}`);
    await replay.started;
    expect(replay.request?.image).toBe(true);
    expect(replay.request?.think).toBeUndefined();
    expect(replay.request?.webSearch).toBeUndefined();

    await replay.until("toolCalls");
    const running = await screen.findByText("Creating an image…");
    const block = running.closest<HTMLElement>("[data-role=image-progress]")!;
    expect(within(block).getByText(PROMPT)).toBeTruthy();
    expect(screen.queryByRole("progressbar")).toBeNull();

    await replay.step(3);
    const bar = await screen.findByRole("progressbar", {
      name: "Image progress",
    });
    expect(bar.getAttribute("aria-valuenow")).toBe("1");
    expect(bar.getAttribute("aria-valuemax")).toBe("4");
    expect(within(block).getByText("Step 1 of 4")).toBeTruthy();
    const preview = within(block).getByAltText<HTMLImageElement>(
      "Preview of the image being created",
    );
    expect(preview.getAttribute("src")).toBe(PREVIEW);

    await replay.finish();
    await replyEnded();
    // Relue en base : l'image à la place du JSON, sans bloc de texte vide.
    const image = generated();
    expect(image?.getAttribute("src")).toBe(attachmentImageUrl(42));
    expect(image?.getAttribute("alt")).toBe(PROMPT);
    expect(image?.getAttribute("width")).toBe("512");
    expect(image?.getAttribute("height")).toBe("512");
    expect(screen.getByText(`Made with ${IMAGE_MODEL}`)).toBeTruthy();
    expect(screen.queryByText(/"attachmentId"/)).toBeNull();
    expect(texts("assistant")).toEqual([]);
    expect(screen.queryByText("Creating an image…")).toBeNull();
    const header = screen.getByRole("button", { name: "Created an image" });
    expect(header.getAttribute("aria-expanded")).toBe("false");
    await userEvent.click(header);
    expect(header.getAttribute("aria-expanded")).toBe("true");
    // Mode gardé après l'envoi, dans la conversation créée.
    expect((await imageButton()).getAttribute("aria-pressed")).toBe("true");
    expect(composer().placeholder).toBe("Describe the image to create");
    expect(composer().value).toBe("");
  });

  it("Save image… asks the backend, then says Saved; a failure is shown", async () => {
    const { simulator, backend } = setup();
    simulator.queue(buttonSequence(SUCCESS));
    await enableImageMode();
    await userEvent.type(composer(), `${PROMPT}{Enter}`);
    await replyEnded();
    await userEvent.click(
      await screen.findByRole("button", { name: "Save image…" }),
    );
    await screen.findByRole("button", { name: "Saved" });
    expect(backend.imageSaves).toEqual([42]);

    // Annulé : rien n'est dit.
    backend.imageSave = false;
    await waitFor(() => screen.getByRole("button", { name: "Save image…" }), {
      timeout: 4000,
    });
    await userEvent.click(screen.getByRole("button", { name: "Save image…" }));
    await waitFor(() => expect(backend.imageSaves).toEqual([42, 42]));
    expect(screen.queryByRole("button", { name: "Saved" })).toBeNull();

    backend.imageSave = { kind: "system", message: "disk full" };
    await userEvent.click(screen.getByRole("button", { name: "Save image…" }));
    expect(
      (await screen.findByText("Couldn't save the image: disk full"))
        .textContent,
    ).toBeTruthy();
  });

  it("refuses files here, composer kept, until they are removed", async () => {
    const { backend } = setup();
    backend.picks.push([NOTES]);
    await enableImageMode();
    await userEvent.click(
      screen.getByRole("button", { name: "Upload multiple files" }),
    );
    await screen.findByTestId("composer-file");
    await userEvent.type(composer(), `${PROMPT}{Enter}`);
    const notice = await screen.findByTestId("image-notice");
    expect(notice.textContent).toContain(
      "Image mode doesn't take attached files: remove them to create an image.",
    );
    expect(composer().value).toBe(PROMPT);
    await userEvent.click(
      screen.getByRole("button", { name: "Remove notes.txt" }),
    );
    await waitFor(() =>
      expect(screen.queryByTestId("image-notice")).toBeNull(),
    );
  });

  it("without an image model chosen: a notice and a link to the Images tab", async () => {
    const { app } = setup({ chosen: false });
    await enableImageMode();
    await userEvent.type(composer(), `${PROMPT}{Enter}`);
    const notice = await screen.findByTestId("image-notice");
    expect(notice.textContent).toContain(
      "No image model chosen: pick one in the ModelHub, Images tab.",
    );
    expect(composer().value).toBe(PROMPT);
    await userEvent.click(
      within(notice).getByRole("link", { name: "Choose an image model" }),
    );
    await waitFor(() => expect(app.path()).toMatch(/^\/hub\/images\/?$/));
  });

  it("the model picker lists the installed image models, chooses one, and leads to the Images tab", async () => {
    const { backend, app } = setup({ chosen: false });
    backend.images.install(KLEIN_ID, Z_IMAGE_ID);
    await field();
    await waitFor(() =>
      expect(screen.getByTitle("Select model").textContent).toBe(MODEL),
    );
    await enableImageMode();
    const picker = await screen.findByTitle("Select image model");
    await waitFor(() =>
      expect(picker.textContent).toBe("Choose an image model"),
    );
    await userEvent.click(picker);
    const list = screen.getByRole("listbox", { name: "Image models" });
    expect(
      within(list)
        .getAllByRole("option")
        .map((option) => option.textContent),
    ).toEqual([IMAGE_MODEL, "Z-Image-Turbo", "More image models"]);
    await userEvent.click(
      within(list).getByRole("option", { name: "Z-Image-Turbo" }),
    );
    await waitFor(() => expect(picker.textContent).toBe("Z-Image-Turbo"));
    expect(backend.images.selected).toBe(Z_IMAGE_ID);
    // Mode coupé : le sélecteur d'Ollama revient.
    await userEvent.click(await imageButton());
    expect(screen.getByTitle("Select model").textContent).toBe(MODEL);
    await enableImageMode();
    await userEvent.click(await screen.findByTitle("Select image model"));
    await userEvent.click(
      screen.getByRole("option", { name: "More image models" }),
    );
    await waitFor(() => expect(app.path()).toMatch(/^\/hub\/images\/?$/));
  });

  it("does not need a language model, and turning it off sends normally again", async () => {
    const { simulator } = setup({ selectedModel: "", models: [] });
    simulator.queue(buttonSequence(SUCCESS, ""));
    await enableImageMode();
    await userEvent.type(composer(), `${PROMPT}{Enter}`);
    await replyEnded();
    expect(generated()).not.toBeNull();
    await userEvent.click(await imageButton());
    expect(composer().placeholder).toBe("Send a message");
    expect((await imageButton()).getAttribute("aria-pressed")).toBe("false");
  });

  it("warns in amber without a graphics card, only in image mode", async () => {
    setup({ gpu: false });
    await imageButton();
    expect(screen.queryByTestId("image-no-gpu")).toBeNull();
    await enableImageMode();
    const line = screen.getByTestId("image-no-gpu");
    expect(line.textContent).toBe(
      "No graphics card: each image takes several minutes on the processor",
    );
    expect(line.className).toContain("text-amber-700");
  });

  it("is greyed out with the reason when the processor lacks AVX2", async () => {
    setup({ missingCpuFeatures: ["AVX2"] });
    const button = await imageButton();
    expect(button.getAttribute("aria-disabled")).toBe("true");
    expect(button.title).toBe(
      "This computer can't create images: its processor lacks instructions the image engine needs (AVX2).",
    );
    await userEvent.click(button);
    expect(composer().placeholder).toBe("Send a message");
  });

  it("is not shown when the image engine isn't shipped", async () => {
    setup({ engineAvailable: false });
    await screen.findByRole("button", { name: "Upload multiple files" });
    // Machine lue : toujours pas de bouton.
    await act(() => new Promise((resolve) => setTimeout(resolve, 50)));
    expect(screen.queryByRole("button", { name: "Image" })).toBeNull();
  });
});

describe("the generate_image tool called by the model", () => {
  /** Envoi ordinaire à chatonlm-image, qui appelle l'outil (§ 2.6). */
  function toolSequence(tail: ChatEvent[]): Sequence {
    return {
      description: "generate_image called by the model (built by the test).",
      request: { streamId: "stream-1", content: PROMPT, model: TOOL_MODEL },
      events: [
        { type: "chatCreated", chatId: "chat-1" },
        { type: "userMessage", message: userMessage(PROMPT) },
        { type: "replyStarted", messageId: 2, createdAt: SEQUENCE_TIME },
        { type: "thinking", text: "The user wants an image." },
        callEvent({ prompt: PROMPT, width: 512, height: 512 }),
        ...tail,
      ],
      error: null,
    };
  }

  it("shows the processor warning and the queue, then the image and the next reply", async () => {
    const { simulator } = setup({ selectedModel: TOOL_MODEL });
    const replay = simulator.queue(
      toolSequence([
        imageEvent({ stage: "queued", cpu: true }),
        imageEvent({ stage: "generating", step: 2, total: 8, cpu: true }),
        READY,
        {
          type: "toolResult",
          messageId: 2,
          index: 0,
          result: RESULT,
          error: null,
        },
        { type: "replyStarted", messageId: 3, createdAt: SEQUENCE_TIME },
        { type: "content", text: "Here is your image." },
        { type: "done", messageId: 3 },
      ]),
      { stepwise: true },
    );
    await screen.findByRole("button", { name: "Image" });
    await sendTo(TOOL_MODEL, PROMPT);
    await replay.started;
    expect(replay.request?.image).toBeUndefined();
    await replay.until("toolCalls");
    await replay.step();
    await screen.findByText("Waiting for another image…");
    const warning = screen.getByText(
      "Made by the processor (no graphics card): this takes several minutes.",
    );
    expect(warning.className).toContain("text-amber-700");
    await replay.step();
    await screen.findByText("Step 2 of 8");
    expect(screen.queryByText("Waiting for another image…")).toBeNull();

    // Image enregistrée avant le résultat : pièce jointe du message vivant.
    await replay.step();
    await waitFor(() =>
      expect(
        streams
          .streamsFor("chat-1")[0]
          ?.replies[0]?.attachments.map((a) => a.id),
      ).toEqual([42]),
    );
    await replay.finish();
    await replyEnded();
    expect(generated()?.getAttribute("src")).toBe(attachmentImageUrl(42));
    expect(texts("assistant")).toEqual(["Here is your image."]);
  });

  it("an image stopped by Stop says so once the reply has ended", async () => {
    const { simulator } = setup({ selectedModel: TOOL_MODEL });
    const replay = simulator.queue(
      toolSequence([
        imageEvent({ stage: "generating", step: 1, total: 4 }),
        { type: "cancelled" },
      ]),
      { stepwise: true },
    );
    await sendTo(TOOL_MODEL, PROMPT);
    await replay.started;
    await replay.until("imageProgress");
    await screen.findByText("Creating an image…");
    expect(screen.queryByText("Image stopped")).toBeNull();
    await userEvent.click(screen.getByLabelText("Stop"));
    await waitFor(() => expect(replay.cancelRequested).toBe(true));
    await replay.finish();
    await replyEnded();
    await screen.findByText("Image stopped");
    expect(screen.queryByText("Creating an image…")).toBeNull();
    expect(generated()).toBeNull();
  });

  it("an error stays with the call, with a link to the image models", async () => {
    const { simulator, app } = setup({ selectedModel: TOOL_MODEL });
    simulator.queue(
      toolSequence([
        imageEvent({ stage: "starting" }),
        {
          type: "toolResult",
          messageId: 2,
          index: 0,
          result: null,
          error: "The image engine stopped unexpectedly",
        },
        { type: "replyStarted", messageId: 3, createdAt: SEQUENCE_TIME },
        { type: "content", text: "Sorry, no image." },
        { type: "done", messageId: 3 },
      ]),
    );
    await sendTo(TOOL_MODEL, PROMPT);
    await replyEnded();
    const line = await screen.findByText(
      "Couldn't create the image: The image engine stopped unexpectedly",
    );
    expect(line.className).toContain("text-red-600");
    // Pas de bloc « Error: … » en plus.
    expect(screen.queryByText(/^Error: /)).toBeNull();
    expect(document.querySelectorAll("[data-role=tool]")).toHaveLength(0);
    await userEvent.click(screen.getByRole("link", { name: "Image models" }));
    await waitFor(() => expect(app.path()).toMatch(/^\/hub\/images\/?$/));
  });

  /** Conversation d'un bot d'Easy Link dont la réponse 2 a appelé l'outil. */
  function botChat(replyRunning: number | null): StoredChat {
    return {
      id: "bot",
      title: "Telegram",
      createdAt: SEQUENCE_TIME,
      updatedAt: SEQUENCE_TIME,
      source: "telegram",
      replyRunning,
      messages: [
        userMessage(PROMPT),
        {
          ...userMessage(""),
          id: 2,
          role: "assistant",
          model: TOOL_MODEL,
          toolCalls: [
            {
              name: "generate_image",
              arguments: JSON.stringify({ prompt: PROMPT }),
              result: null,
              error: null,
            },
          ],
        },
      ],
    };
  }

  it("an image of an Easy Link bot's reply still running is in progress", async () => {
    setup({ chats: [botChat(2)], path: "/c/bot" });
    const line = await screen.findByText("Creating an image…");
    expect(line.closest("[data-role=image-progress]")).not.toBeNull();
    expect(screen.getByTestId("image-frame")).toBeTruthy();
    expect(screen.queryByText("Image stopped")).toBeNull();
  });

  it("the same image once the bot's reply has ended without it is stopped", async () => {
    setup({ chats: [botChat(null)], path: "/c/bot" });
    await screen.findByText("Image stopped");
    expect(screen.queryByText("Creating an image…")).toBeNull();
  });
});

describe("sequences recorded by the engine", () => {
  const FOX = "A red fox in the snow, watercolor";

  it("image-button: progress in a 1024 px frame, then the image", async () => {
    const { simulator } = setup();
    const replay = simulator.queue("image-button", { stepwise: true });
    await enableImageMode();
    await userEvent.type(composer(), `${FOX}{Enter}`);
    await replay.started;
    await replay.until("imageProgress");
    const frame = await screen.findByTestId("image-frame");
    expect(frame.style.aspectRatio).toBe("1024 / 1024");
    expect(frame.style.width).toBe("512px");
    await replay.finish();
    await replyEnded();
    const image = generated();
    expect(image?.getAttribute("src")).toBe(attachmentImageUrl(1));
    expect(image?.getAttribute("width")).toBe("1024");
    expect(image?.getAttribute("alt")).toBe(FOX);
    expect(screen.getByText("Made with Fake image model")).toBeTruthy();
    expect(texts("assistant")).toEqual([]);
  });

  it("image-tool: the image between the call and the next reply", async () => {
    const { simulator } = setup({ selectedModel: TOOL_MODEL });
    simulator.queue("image-tool");
    await sendTo(TOOL_MODEL, FOX);
    await replyEnded();
    expect(generated()?.getAttribute("src")).toBe(attachmentImageUrl(1));
    expect(texts("assistant")).toEqual(["Here is your image."]);
    expect(
      screen.getByRole("button", { name: "Created an image" }),
    ).toBeTruthy();
  });

  it("image-button-no-model: the refusal of the engine in the notice, text given back", async () => {
    // Choix retiré après la lecture des modèles par la page : le moteur
    // refuse avec son texte, montré par la zone de saisie, qui rend le
    // texte ; les modèles relus, le lien vers l'onglet Images vient.
    const { simulator, backend, app } = setup();
    simulator.queue("image-button-no-model");
    await enableImageMode();
    await userEvent.type(composer(), FOX);
    await waitFor(() =>
      expect(
        screen.getByLabelText<HTMLButtonElement>("Send message").disabled,
      ).toBe(false),
    );
    backend.images.select("");
    await userEvent.type(composer(), "{Enter}");
    const notice = await screen.findByTestId("image-notice");
    expect(notice.textContent).toContain(
      "No image model chosen: pick one in the ModelHub, Images tab.",
    );
    await waitFor(() => expect(composer().value).toBe(FOX));
    // Une seule erreur : celle de l'avis.
    expect(screen.getAllByRole("alert")).toHaveLength(1);
    const link = await within(notice).findByRole("link", {
      name: "Choose an image model",
    });
    await userEvent.click(link);
    await waitFor(() => expect(app.path()).toMatch(/^\/hub\/images\/?$/));
  });
});

describe("translations of the image screens", () => {
  it("leaves no untranslated text in the image mode of the composer and its notices", async () => {
    setup({ gpu: false, chosen: false });
    usePseudoLanguage();
    const textarea = await field();
    await userEvent.click(await screen.findByRole("button", { name: /Image/ }));
    await waitFor(() => screen.getByTestId("image-no-gpu"));
    await userEvent.type(textarea, `${PROMPT}{Enter}`);
    await screen.findByTestId("image-notice");
    expect(untranslatedTexts(document.body, [PROMPT, MODEL])).toEqual([]);
  });

  it("leaves no untranslated text in the progress, the result and the error", async () => {
    const { simulator, backend } = setup({ selectedModel: TOOL_MODEL });
    backend.imageSave = { kind: "system", message: "disk full" };
    usePseudoLanguage();
    const replay = simulator.queue(
      buttonSequence(
        [
          imageEvent({
            stage: "generating",
            step: 1,
            total: 4,
            preview: PREVIEW,
            cpu: true,
          }),
          READY,
          {
            type: "toolResult",
            messageId: 2,
            index: 0,
            result: RESULT,
            error: null,
          },
          { type: "done", messageId: 2 },
        ],
        TOOL_MODEL,
      ),
      { stepwise: true },
    );
    const textarea = await field();
    await waitFor(() =>
      expect(screen.getByTitle(/Select model/).textContent).toBe(TOOL_MODEL),
    );
    await userEvent.click(await screen.findByRole("button", { name: /Image/ }));
    // Mode Image : le sélecteur montre le modèle d'images.
    await waitFor(() =>
      expect(screen.getByTitle(/Select image model/).textContent).toBe(
        IMAGE_MODEL,
      ),
    );
    await userEvent.type(textarea, `${PROMPT}{Enter}`);
    await replay.started;
    await replay.until("imageProgress");
    await screen.findByRole("progressbar");
    const allow = [PROMPT, IMAGE_MODEL, TOOL_MODEL, "disk full"];
    expect(untranslatedTexts(document.body, allow)).toEqual([]);
    await replay.finish();
    await waitFor(() => expect(generated()).not.toBeNull());
    const save = document.querySelector<HTMLButtonElement>(
      "[data-role=generated-image] button",
    )!;
    await userEvent.click(save);
    await screen.findByRole("alert");
    expect(untranslatedTexts(document.body, allow)).toEqual([]);
  });
});
