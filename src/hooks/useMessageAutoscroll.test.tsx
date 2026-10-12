import { act, cleanup, render, screen } from "@testing-library/react";
import { useEffect } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Message } from "@/bindings/Message";
import { MessageList } from "@/components/MessageList";
import { useMessageAutoscroll } from "./useMessageAutoscroll";

// jsdom ne met rien en page : hauteurs simulées. Zone de 600 px ; un message
// fait 40 px, une réponse 20 px de plus par tranche de 50 caractères de son
// texte ; des écarts peuvent être imposés par index de message, ainsi que la
// hauteur du moment du corps d'un bloc de raisonnement (aperçu, repli).
const CONTAINER_HEIGHT = 600;
const MESSAGE_HEIGHT = 40;
const extraHeight = new Map<number, number>();
const thinkingHeight = new Map<number, number>();

/** Corps du bloc de raisonnement d'un message (désigné par son en-tête). */
function thinkingBody(element: HTMLElement) {
  const id = element
    .querySelector("button[aria-controls]")
    ?.getAttribute("aria-controls");
  return id ? document.getElementById(id) : null;
}

function heightOf(element: HTMLElement) {
  const index = Number(element.dataset.messageIndex);
  const reply = element.querySelector("[data-role='assistant']");
  const grown = reply
    ? 20 * Math.floor((reply.textContent ?? "").length / 50)
    : 0;
  const thinking = thinkingBody(element) ? (thinkingHeight.get(index) ?? 0) : 0;
  return MESSAGE_HEIGHT + grown + thinking + (extraHeight.get(index) ?? 0);
}

function messageElements(container: Element) {
  return [...container.querySelectorAll<HTMLElement>("[data-message-index]")];
}

function spacer() {
  return screen
    .getByTestId("container")
    .querySelector<HTMLElement>("[data-spacer]")!;
}

const layout: Record<string, (this: HTMLElement) => number> = {
  clientHeight() {
    return this.dataset.testid === "container" ? CONTAINER_HEIGHT : 0;
  },
  offsetHeight() {
    if (this.dataset.messageIndex !== undefined) return heightOf(this);
    const item = this.closest<HTMLElement>("[data-message-index]");
    return item && thinkingBody(item) === this
      ? (thinkingHeight.get(Number(item.dataset.messageIndex)) ?? 0)
      : 0;
  },
  offsetTop() {
    const container = this.closest("[data-testid='container']");
    if (!container || this.dataset.messageIndex === undefined) return 0;
    const index = Number(this.dataset.messageIndex);
    return messageElements(container)
      .filter((element) => Number(element.dataset.messageIndex) < index)
      .reduce((sum, element) => sum + heightOf(element), 0);
  },
  scrollHeight() {
    if (this.dataset.testid !== "container") return 0;
    const items = messageElements(this).reduce(
      (sum, element) => sum + heightOf(element),
      0,
    );
    return items + (parseFloat(spacer().style.height) || 0);
  },
};

let scrolls: { time: number; top: number }[] = [];
let now = 0;
let frames: { id: number; callback: FrameRequestCallback }[] = [];
let frameIds = 0;
const saved = new Map<string, PropertyDescriptor | undefined>();

function runFrame() {
  const due = frames;
  frames = [];
  for (const frame of due) frame.callback(now);
}

beforeEach(() => {
  for (const [name, get] of Object.entries(layout)) {
    saved.set(
      name,
      Object.getOwnPropertyDescriptor(HTMLElement.prototype, name),
    );
    Object.defineProperty(HTMLElement.prototype, name, {
      configurable: true,
      get,
    });
  }
  saved.set(
    "scrollTo",
    Object.getOwnPropertyDescriptor(HTMLElement.prototype, "scrollTo"),
  );
  Object.defineProperty(HTMLElement.prototype, "scrollTo", {
    configurable: true,
    writable: true,
    value(options: ScrollToOptions) {
      scrolls.push({ time: now, top: options.top ?? 0 });
    },
  });
  vi.stubGlobal("requestAnimationFrame", (callback: FrameRequestCallback) => {
    frames.push({ id: ++frameIds, callback });
    return frameIds;
  });
  vi.stubGlobal("cancelAnimationFrame", (id: number) => {
    frames = frames.filter((frame) => frame.id !== id);
  });
});

afterEach(() => {
  cleanup();
  for (const [name, descriptor] of saved) {
    if (descriptor)
      Object.defineProperty(HTMLElement.prototype, name, descriptor);
    else
      delete (HTMLElement.prototype as unknown as Record<string, unknown>)[
        name
      ];
  }
  saved.clear();
  vi.unstubAllGlobals();
  vi.useRealTimers();
  extraHeight.clear();
  thinkingHeight.clear();
  scrolls = [];
  frames = [];
  now = 0;
});

function message(id: number, role: "user" | "assistant", content: string) {
  return {
    id,
    role,
    content,
    thinking: "",
    model: role === "assistant" ? "m" : null,
    createdAt: id,
    updatedAt: id,
    thinkingStartedAt: null,
    thinkingEndedAt: null,
    attachments: [],
    toolCalls: [],
  } satisfies Message;
}

/** Cinq échanges déjà en base : le message envoyé sera le 11e (index 10). */
const history = Array.from({ length: 10 }, (_, i) =>
  message(i + 1, i % 2 === 0 ? "user" : "assistant", `m${i}`),
);
const sent = message(11, "user", "Again");

function Harness({
  messages,
  isStreaming,
  onReady,
}: {
  messages: Message[];
  isStreaming: boolean;
  onReady: (onMessageSent: () => void) => void;
}) {
  const { containerRef, spacerHeight, onMessageSent } = useMessageAutoscroll({
    messages,
    isStreaming,
  });
  useEffect(() => onReady(onMessageSent), [onReady, onMessageSent]);
  return (
    <section ref={containerRef} data-testid="container">
      <MessageList
        messages={messages}
        spacerHeight={spacerHeight}
        isStreaming={isStreaming}
        error={null}
      />
    </section>
  );
}

function setup(initial: Message[] = history) {
  let onMessageSent = () => {};
  const onReady = (callback: () => void) => {
    onMessageSent = callback;
  };
  const view = render(
    <Harness messages={initial} isStreaming={false} onReady={onReady} />,
  );
  const show = (messages: Message[], isStreaming: boolean) =>
    view.rerender(
      <Harness
        messages={messages}
        isStreaming={isStreaming}
        onReady={onReady}
      />,
    );
  // Envoi : comme dans ChatView, onMessageSent puis le rendu avec le message.
  const send = (isStreaming: boolean) =>
    act(() => {
      onMessageSent();
      show([...initial, sent], isStreaming);
    });
  return { show, send };
}

/** Réponse qui a raisonné ; sans fin de raisonnement, il est en cours. */
function thinkingReply(id: number, content: string, ended: boolean) {
  return {
    ...message(id, "assistant", content),
    thinking: "Okay, the user wants a short answer.",
    thinkingStartedAt: 1_000,
    thinkingEndedAt: ended ? 3_000 : null,
  } satisfies Message;
}

class FakeResizeObserver {
  static current: FakeResizeObserver | null = null;
  targets = new Set<Element>();
  constructor(private callback: ResizeObserverCallback) {
    FakeResizeObserver.current = this;
  }
  observe(target: Element) {
    this.targets.add(target);
  }
  unobserve(target: Element) {
    this.targets.delete(target);
  }
  disconnect() {
    this.targets.clear();
  }
  fire() {
    this.callback([], this as unknown as ResizeObserver);
  }
}

/** Défilement le plus bas que permet le contenu du moment. */
function maxScroll() {
  return screen.getByTestId("container").scrollHeight - CONTAINER_HEIGHT;
}

describe("useMessageAutoscroll", () => {
  it("just viewing a chat adds no space", () => {
    setup();
    expect(spacer().style.height).toBe("0px");
  });

  it("during a fast reply (a render every 16 ms), sets the space and scrolls to the sent message", () => {
    const { show, send } = setup();
    send(true);
    // Espace posé dès le rendu du message envoyé, avant tout affichage :
    // 600 - 40 (message envoyé) + 40 % de 600 pendant la réponse.
    expect(spacer().style.height).toBe("800px");

    // Réponse : un rendu toutes les 16 ms ; images toutes les 16,7 ms.
    let reply = "";
    let nextRender = 16;
    let nextFrame = 16.7;
    const end = 400;
    while (Math.min(nextRender, nextFrame) <= end) {
      if (nextFrame <= nextRender) {
        now = nextFrame;
        act(runFrame);
        nextFrame += 16.7;
      } else {
        now = nextRender;
        reply += "token ";
        show([...history, sent, message(12, "assistant", reply)], true);
        const replyHeight = heightOf(messageElements(document.body)[11]);
        const expected =
          Math.max(0, CONTAINER_HEIGHT - MESSAGE_HEIGHT - replyHeight) +
          CONTAINER_HEIGHT * 0.4;
        expect(spacer().style.height).toBe(`${expected}px`);
        nextRender += 16;
      }
    }

    // Un seul défilement, deux images après l'envoi, pendant la réponse ; il
    // atteint le message (400 px) grâce à l'espace déjà en place.
    expect(scrolls).toEqual([{ time: 16.7 * 2, top: 400 }]);
    // Réponse longue à ce stade : l'espace a suivi.
    expect(heightOf(messageElements(document.body)[11])).toBeGreaterThan(
      MESSAGE_HEIGHT,
    );

    // Fin de la réponse : place réservée retirée, aucun autre défilement.
    show([...history, sent, message(12, "assistant", reply)], false);
    act(runFrame);
    act(runFrame);
    expect(scrolls).toHaveLength(1);
    const replyHeight = heightOf(messageElements(document.body)[11]);
    expect(spacer().style.height).toBe(
      `${CONTAINER_HEIGHT - MESSAGE_HEIGHT - replyHeight}px`,
    );
  });

  it("a long sent message is scrolled to its end", () => {
    const { send } = setup();
    extraHeight.set(10, 400);
    send(true);
    act(runFrame);
    act(runFrame);
    // Bas de la zone : messages (10 × 40 + 440) + espace (600 - 440 + 240)
    // - 600.
    expect(scrolls).toEqual([{ time: 0, top: 840 + 400 - 600 }]);
  });

  it("a reply that ends while its thinking folds: the space covers the fold, the browser never clips the scroll", async () => {
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
    vi.stubGlobal("ResizeObserver", FakeResizeObserver);
    const { show, send } = setup();
    const observer = FakeResizeObserver.current!;
    send(true);
    act(runFrame);
    act(runFrame);
    // Le message envoyé (index 10) est en haut : 10 messages × 40 px.
    expect(scrolls).toEqual([{ time: 0, top: 400 }]);
    const scrollTop = 400;

    // Raisonnement en cours : aperçu de 192 px (12rem).
    thinkingHeight.set(11, 192);
    await act(async () =>
      show([...history, sent, thinkingReply(12, "", false)], true),
    );
    // 600 - 40 - (40 + 192) + 240
    expect(spacer().style.height).toBe("568px");

    // Premier fragment du contenu : le raisonnement est fini, son corps se
    // replie en 300 ms ; la réponse, courte, finit dans la foulée (moins de
    // 300 ms après ce fragment), corps encore à 192 px.
    await act(async () =>
      show([...history, sent, thinkingReply(12, "Hello", true)], true),
    );
    await act(async () =>
      show([...history, sent, thinkingReply(12, "Hello", true)], false),
    );
    // Espace de fin calculé avec le corps à sa hauteur finale (nulle) :
    // 600 - 40 - 40, et non 600 - 40 - (40 + 192).
    expect(spacer().style.height).toBe("520px");
    expect(maxScroll()).toBeGreaterThanOrEqual(scrollTop);

    // Repli image par image. À chaque image, la mise en page (où le
    // navigateur rogne scrollTop si le contenu ne permet plus ce défilement)
    // précède les rappels du ResizeObserver : le contenu doit toujours
    // permettre de garder le message envoyé en haut.
    for (let height = 180; height >= 0; height -= 12) {
      thinkingHeight.set(11, height);
      expect(maxScroll()).toBeGreaterThanOrEqual(scrollTop);
      act(() => observer.fire());
      act(() => vi.advanceTimersByTime(16));
    }
    // Anti-rebond écoulé après le repli : même espace, même défilement possible.
    act(() => vi.advanceTimersByTime(100));
    expect(spacer().style.height).toBe("520px");
    expect(maxScroll()).toBe(scrollTop);
    expect(scrolls).toHaveLength(1);
  });

  it("a thinking still folding above the sent message: the scroll aims at the message's final place", () => {
    // Réponse précédente (index 9) tout juste finie : le corps de son
    // raisonnement se replie encore, 60 px à perdre.
    const initial = [...history.slice(0, 9), thinkingReply(10, "m9", true)];
    thinkingHeight.set(9, 60);
    const { send } = setup(initial);
    send(true);
    act(runFrame);
    act(runFrame);
    // Le message envoyé est à 460 px pour l'instant, mais à 400 px une fois
    // le repli fini : le défilement vise 400, pas 460 (sinon le message
    // finirait 60 px au-dessus du haut de la zone).
    expect(scrolls).toEqual([{ time: 0, top: 400 }]);
    thinkingHeight.set(9, 0);
    const container = screen.getByTestId("container");
    expect(messageElements(container)[10].offsetTop).toBe(400);
  });

  it("watches each message for size changes (debounced) and folding (at once)", async () => {
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
    vi.stubGlobal("ResizeObserver", FakeResizeObserver);

    const { show, send } = setup();
    const observer = FakeResizeObserver.current!;
    const container = screen.getByTestId("container");
    expect(observer.targets.has(container)).toBe(true);
    expect(
      messageElements(container).every((e) => observer.targets.has(e)),
    ).toBe(true);

    // Envoi : le message est suivi dès son apparition.
    send(false);
    expect(spacer().style.height).toBe("560px");
    const optimistic = messageElements(container)[10];
    expect(observer.targets.has(optimistic)).toBe(true);
    // Le message relu en base remplace celui affiché à l'envoi.
    const stored = message(13, "user", "Again");
    show([...history, stored], false);
    const replaced = messageElements(container)[10];
    expect(replaced).not.toBe(optimistic);
    expect(observer.targets.has(replaced)).toBe(true);
    expect(observer.targets.has(optimistic)).toBe(false);

    // Image ou code coloré : le message grandit sans nouveau rendu.
    extraHeight.set(10, 100);
    act(() => observer.fire());
    expect(spacer().style.height).toBe("560px");
    act(() => vi.advanceTimersByTime(100));
    expect(spacer().style.height).toBe("460px");

    // Bloc replié : mise à jour sans attendre.
    extraHeight.set(10, 0);
    await act(async () => {
      replaced
        .querySelector("[data-role='user']")!
        .setAttribute("data-expanded", "false");
    });
    expect(spacer().style.height).toBe("560px");
  });
});
