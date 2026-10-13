// Page Proofread face au simulateur : édition, travail par passages (flux,
// passages à venir atténués), revue (marques, nombre de corrections),
// frappe ou clic → édition du texte final (touches mortes, Retour arrière,
// Suppr, défilement gardé), Undo et Redo, Copy et copie d'une sélection,
// Stop, revue partielle, erreurs et leurs actions, passages gardés,
// Rephrase, raccourcis, focus, sélecteur du modèle, état gardé hors de la
// page, sélection corrigée par le raccourci global (« Show », page mise de
// côté puis rétablie), barre latérale ; textes traduits (pseudo-langue,
// français).

import {
  act,
  configure,
  fireEvent,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { SelectionResult } from "@/bindings/SelectionResult";
import type { SettingsPatch } from "@/bindings/SettingsPatch";
import { resetProofreadForTests } from "@/hooks/useProofread";
import { setLanguage } from "@/i18n";
import { type FakeBackend, fakeBackend } from "@/test/backend";
import { untranslatedTexts, usePseudoLanguage } from "@/test/i18n";
import { fakeSplit } from "@/test/proofread";
import { renderApp } from "@/test/renderApp";
import { installSimulator } from "@/test/simulator";

configure({ asyncUtilTimeout: 5000 });
vi.setConfig({ testTimeout: 30_000 });

beforeEach(() => resetProofreadForTests());

const TEXT = "Teh film was good.\n\nI will recieve it tomorrow.";
const CORRECTED = "The film was good.\n\nI will receive it tomorrow.";

function setup(
  prepare: (backend: FakeBackend) => void = () => {},
  path = "/proofread",
) {
  const simulator = installSimulator();
  const backend = fakeBackend(simulator);
  prepare(backend);
  const app = renderApp(path);
  return { simulator, backend, fake: backend.proofread, app };
}

const editor = () =>
  screen.getByRole<HTMLTextAreaElement>("textbox", { name: "Text to correct" });
const queryEditor = () =>
  screen.queryByRole<HTMLTextAreaElement>("textbox", {
    name: "Text to correct",
  });
const review = () => screen.getByTestId("proofread-review");
const statusText = () => screen.getByTestId("proofread-status").textContent;
const button = (name: string) =>
  screen.getByRole<HTMLButtonElement>("button", { name });

/** Texte final montré : sans les mots supprimés ni les textes cachés. */
function shownText(element: Element) {
  const copy = element.cloneNode(true) as Element;
  for (const skipped of copy.querySelectorAll("[data-skip]")) skipped.remove();
  return copy.textContent;
}

/** Mots marqués (sans le texte lu par les lecteurs d'écran). */
function marks(kind: "del" | "ins") {
  return [...review().querySelectorAll(kind)].map((mark) => {
    const copy = mark.cloneNode(true) as Element;
    for (const hidden of copy.querySelectorAll(".sr-only")) hidden.remove();
    return copy.textContent;
  });
}

/** Page prête, texte collé dans la zone. */
async function typeText(user: ReturnType<typeof userEvent.setup>, text = TEXT) {
  await screen.findByTestId("proofread");
  await user.click(editor());
  await user.paste(text);
}

/**
 * execCommand de l'édition native (absent de jsdom), sur la zone de texte
 * qui a le focus : insertText, delete et forwardDelete ; les autres
 * commandes ne font rien. Commandes notées dans `calls`.
 */
function nativeEditing() {
  const calls: string[] = [];
  const setValue = Object.getOwnPropertyDescriptor(
    HTMLTextAreaElement.prototype,
    "value",
  )!.set!;
  const execCommand = (command: string, _ui?: boolean, value = "") => {
    calls.push(command);
    const area = document.activeElement;
    if (!(area instanceof HTMLTextAreaElement)) return false;
    const { selectionStart: start, selectionEnd: end, value: text } = area;
    let from = start;
    let to = end;
    let inserted = "";
    if (command === "insertText") inserted = value;
    else if (command === "delete") from = start === end ? start - 1 : start;
    else if (command === "forwardDelete") to = start === end ? end + 1 : end;
    else return false;
    from = Math.max(0, from);
    to = Math.min(text.length, to);
    // Valeur posée sous le suivi de React : l'évènement « input » la donne.
    setValue.call(area, text.slice(0, from) + inserted + text.slice(to));
    area.setSelectionRange(from + inserted.length, from + inserted.length);
    area.dispatchEvent(new Event("input", { bubbles: true }));
    return true;
  };
  Object.defineProperty(document, "execCommand", {
    configurable: true,
    value: execCommand,
  });
  return {
    calls,
    restore: () => Reflect.deleteProperty(document, "execCommand"),
  };
}

/** Revue d'une correction complète de TEXT. */
async function corrected(user: ReturnType<typeof userEvent.setup>) {
  const context = setup();
  await typeText(user);
  await user.click(button("Correct"));
  await context.fake.lastJob().finish();
  await screen.findByTestId("proofread-review");
  return context;
}

describe("Proofread page", () => {
  it("is in the sidebar between Chat and Study, highlighted on its page", async () => {
    const user = userEvent.setup();
    setup();
    await screen.findByTestId("proofread");
    const toggle = screen.getByRole("button", {
      name: /^(Show|Hide) sidebar$/,
    });
    if (toggle.getAttribute("aria-label") === "Show sidebar") {
      await user.click(toggle);
    }
    const link = await screen.findByRole("link", { name: "Proofread" });
    expect(link.getAttribute("href")).toBe("/proofread");
    expect(link.className.split(" ")).toContain("bg-accent");
    const nav = link.closest("nav")!;
    expect(
      within(nav)
        .getAllByRole("link")
        .slice(0, 5)
        .map((element) => element.textContent),
    ).toEqual(["Chat", "Proofread", "Study", "ModelHub", "Settings"]);
    expect(screen.getByRole("heading", { level: 1 }).textContent).toBe(
      "Proofread",
    );
  });

  it("starts empty: placeholder, Correct and Rephrase disabled until there is text", async () => {
    const user = userEvent.setup();
    setup();
    await screen.findByTestId("proofread");
    expect(editor().placeholder).toBe("Paste or type text to correct");
    expect(button("Correct").disabled).toBe(true);
    expect(button("Rephrase").disabled).toBe(true);
    await user.click(editor());
    await user.keyboard("   ");
    expect(button("Correct").disabled).toBe(true);
    await user.keyboard("x");
    expect(button("Correct").disabled).toBe(false);
    expect(button("Rephrase").disabled).toBe(false);
    // Le raccourci est dit dans le titre et pour les lecteurs d'écran.
    expect(button("Correct").title).toBe("Correct (Ctrl+Enter)");
    expect(button("Correct").getAttribute("aria-keyshortcuts")).toBe(
      "Control+Enter",
    );
    expect(button("Rephrase").title).toBe("Rephrase (Ctrl+Shift+Enter)");
  });

  it("a cross clears the text, by the native editing, and leaves when there is none", async () => {
    const user = userEvent.setup();
    setup();
    await screen.findByTestId("proofread");
    const cross = () =>
      screen.queryByRole("button", { name: "Clear the text" });
    expect(cross()).toBeNull();
    await user.click(editor());
    // Des espaces seuls se vident aussi.
    await user.keyboard(" ");
    expect(cross()?.title).toBe("Clear the text");
    await user.paste(TEXT);
    const native = nativeEditing();
    try {
      await user.click(cross()!);
      expect(editor().value).toBe("");
      // Tout le texte, effacé par l'édition native : Ctrl+Z le retrouve.
      expect(native.calls).toEqual(["delete"]);
    } finally {
      native.restore();
    }
    expect(document.activeElement).toBe(editor());
    expect(cross()).toBeNull();
    expect(button("Correct").disabled).toBe(true);

    // Sans édition native (jsdom) : la valeur est posée.
    await user.paste(TEXT);
    await user.click(cross()!);
    expect(editor().value).toBe("");
    expect(document.activeElement).toBe(editor());
  });

  it("the cross clears a review too, and is hidden while a correction runs", async () => {
    const user = userEvent.setup();
    const { fake } = setup();
    await typeText(user);
    const cross = () =>
      screen.queryByRole("button", { name: "Clear the text" });
    await user.click(button("Correct"));
    await screen.findByRole("button", { name: "Stop" });
    expect(cross()).toBeNull();
    await fake.lastJob().finish();
    await screen.findByTestId("proofread-review");

    await user.click(cross()!);
    expect(screen.queryByTestId("proofread-review")).toBeNull();
    expect(editor().value).toBe("");
    expect(document.activeElement).toBe(editor());
    expect(cross()).toBeNull();
    // Plus rien à rétablir ni à annuler.
    expect(screen.queryByRole("button", { name: "Undo" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Redo" })).toBeNull();
  });

  it("the cross clears the error shown under the text", async () => {
    const user = userEvent.setup();
    const { fake } = setup((backend) => {
      backend.settings.selectedModel = "";
    });
    await typeText(user);
    await user.click(button("Correct"));
    await fake.lastJob().finish();
    expect(screen.getByRole("alert")).toBeTruthy();
    await user.click(screen.getByRole("button", { name: "Clear the text" }));
    expect(screen.queryByRole("alert")).toBeNull();
    expect(editor().value).toBe("");
  });

  it("corrects passage by passage: started, streamed passage, pending ones dimmed, then the review with marks", async () => {
    const user = userEvent.setup();
    const { simulator, fake } = setup();
    await typeText(user);
    await user.click(button("Correct"));

    expect(simulator.callsTo("proofread_run")).toEqual([
      {
        jobId: expect.any(String),
        text: TEXT,
        mode: "correct",
        onEvent: expect.stringMatching(/^__CHANNEL__:/),
      },
    ]);
    // Stop à la place des boutons, qui prend le focus perdu.
    expect(screen.queryByRole("button", { name: "Correct" })).toBeNull();
    expect(document.activeElement).toBe(button("Stop"));
    expect(queryEditor()).toBeNull();
    expect(statusText()).toBe("Correcting…");

    const job = fake.lastJob();
    await job.step(); // started
    const running = screen.getByTestId("proofread-running");
    expect(
      [...running.querySelectorAll("[data-state='pending']")].map(
        (element) => element.textContent,
      ),
    ).toEqual(["Teh film was good.", "I will recieve it tomorrow."]);
    expect(statusText()).toBe("Correcting…Part 1 of 2");

    await job.step(); // premier fragment
    expect(running.querySelector("[data-state='streaming']")?.textContent).toBe(
      "The film ",
    );
    await job.step(2); // second fragment, passage terminé
    expect(running.querySelector("[data-state='streaming']")).toBeNull();
    expect(running.querySelectorAll("del")).toHaveLength(1);
    expect(
      [...running.querySelectorAll("[data-state='pending']")].map(
        (element) => element.textContent,
      ),
    ).toEqual(["I will recieve it tomorrow."]);
    expect(statusText()).toBe("Correcting…Part 2 of 2");

    await job.finish();
    expect(shownText(review())).toBe(CORRECTED);
    expect(marks("del")).toEqual(["Teh", "recieve"]);
    expect(marks("ins")).toEqual(["The", "receive"]);
    // Marques lisibles par un lecteur d'écran.
    expect(review().querySelector("del")?.textContent).toBe("deleted: Teh");
    expect(review().querySelector("ins")?.textContent).toBe("inserted: The");
    expect(screen.getByRole("status").textContent).toBe(
      "2 correctionsqwen3:0.6b",
    );
    expect(screen.getByRole("region", { name: "Corrected text" })).toBe(
      review(),
    );
    expect(document.activeElement).toBe(review());
    expect(
      ["Undo", "Copy", "Rephrase", "Correct"].map(
        (name) => button(name).disabled,
      ),
    ).toEqual([false, false, false, false]);
  });

  it("says when there is no mistake", async () => {
    const user = userEvent.setup();
    const { fake } = setup();
    await typeText(user, "All is fine here.");
    await user.click(button("Correct"));
    await fake.lastJob().finish();
    await screen.findByTestId("proofread-review");
    expect(statusText()).toBe("No mistakes foundqwen3:0.6b");
    expect(marks("del")).toEqual([]);
  });

  it("Undo (button or Ctrl+Z) goes back to the text before", async () => {
    const user = userEvent.setup();
    const { fake } = await corrected(user);
    await user.click(button("Undo"));
    expect(editor().value).toBe(TEXT);
    expect(screen.queryByTestId("proofread-review")).toBeNull();

    await user.click(button("Correct"));
    await fake.lastJob().finish();
    review().focus();
    await user.keyboard("{Control>}z{/Control}");
    expect(editor().value).toBe(TEXT);
  });

  it("Undo can be redone (Ctrl+Shift+Z, Ctrl+Y or Redo) until the text changes", async () => {
    const user = userEvent.setup();
    const editing = nativeEditing();
    try {
      const { simulator } = await corrected(user);
      await user.keyboard("{Control>}z{/Control}");
      expect(editor().value).toBe(TEXT);
      expect(document.activeElement).toBe(editor());
      expect(button("Redo").title).toBe("Redo (Ctrl+Shift+Z or Ctrl+Y)");
      // Depuis la zone de texte : avant le rétablir natif de la route racine.
      await user.keyboard("{Control>}{Shift>}Z{/Shift}{/Control}");
      expect(shownText(review())).toBe(CORRECTED);
      expect(marks("del")).toEqual(["Teh", "recieve"]);
      expect(statusText()).toBe("2 correctionsqwen3:0.6b");
      expect(editing.calls).not.toContain("redo");

      review().focus();
      await user.keyboard("{Control>}z{/Control}");
      await user.keyboard("{Control>}y{/Control}");
      expect(shownText(review())).toBe(CORRECTED);
      await user.click(button("Undo"));
      await user.click(button("Redo"));
      expect(shownText(review())).toBe(CORRECTED);
      // Sans nouvelle correction.
      expect(simulator.callsTo("proofread_run")).toHaveLength(1);

      // Texte modifié après Undo : plus de Redo.
      await user.click(button("Undo"));
      await user.click(editor());
      await user.keyboard("x");
      expect(screen.queryByRole("button", { name: "Redo" })).toBeNull();
      await user.keyboard("{Control>}{Shift>}Z{/Shift}{/Control}");
      expect(screen.queryByTestId("proofread-review")).toBeNull();
      expect(editing.calls).toContain("redo");
    } finally {
      editing.restore();
    }
  });

  it("Ctrl+Enter from the text area: focus on Stop, then on the review", async () => {
    const user = userEvent.setup();
    const { fake } = setup();
    await typeText(user);
    await user.keyboard("{Control>}{Enter}{/Control}");
    expect(document.activeElement).toBe(button("Stop"));
    await fake.lastJob().finish();
    await screen.findByTestId("proofread-review");
    expect(document.activeElement).toBe(review());
  });

  it("Backspace in the review edits the final text and erases", async () => {
    const user = userEvent.setup();
    await corrected(user);
    const prevented: boolean[] = [];
    const record = (event: KeyboardEvent) =>
      prevented.push(event.defaultPrevented);
    window.addEventListener("keydown", record);
    try {
      // Sans execCommand (jsdom) : repli, le dernier caractère retiré.
      await user.keyboard("{Backspace}");
      expect(document.activeElement).toBe(editor());
      expect(editor().value).toBe(CORRECTED.slice(0, -1));
      // Pas de navigation dans l'historique pour autant.
      expect(prevented).toEqual([true]);
    } finally {
      window.removeEventListener("keydown", record);
    }
  });

  it("Delete in the review erases by the native editing", async () => {
    const user = userEvent.setup();
    const editing = nativeEditing();
    try {
      const { fake } = await corrected(user);
      expect(fireEvent.keyDown(review(), { key: "Delete" })).toBe(false);
      expect(document.activeElement).toBe(editor());
      expect(editing.calls).toEqual(["insertText", "forwardDelete"]);
      // Curseur à la fin : rien après lui.
      expect(editor().value).toBe(CORRECTED);

      await user.click(button("Correct"));
      await fake.lastJob().finish();
      await screen.findByTestId("proofread-review");
      expect(fireEvent.keyDown(review(), { key: "Backspace" })).toBe(false);
      expect(editor().value).toBe(CORRECTED.slice(0, -1));
      expect(editing.calls.at(-1)).toBe("delete");
    } finally {
      editing.restore();
    }
  });

  it("a dead key or an input method key in the review moves to the text area, kept", async () => {
    const user = userEvent.setup();
    const { fake } = await corrected(user);
    // Pas empêchée : l'accent compose dans la zone de texte.
    expect(fireEvent.keyDown(review(), { key: "Dead" })).toBe(true);
    expect(document.activeElement).toBe(editor());
    expect(editor().value).toBe(CORRECTED);

    await user.click(button("Correct"));
    await fake.lastJob().finish();
    await screen.findByTestId("proofread-review");
    expect(
      fireEvent.keyDown(review(), { key: "Process", isComposing: true }),
    ).toBe(true);
    expect(document.activeElement).toBe(editor());
  });

  it("a click keeps the scroll position of the review", async () => {
    const user = userEvent.setup();
    await corrected(user);
    Object.defineProperty(review(), "scrollTop", {
      configurable: true,
      value: 120,
    });
    // Zone cachée pendant la revue, toujours montée.
    const area = document.querySelector<HTMLTextAreaElement>(
      "[data-testid=proofread] textarea",
    )!;
    let top = 0;
    Object.defineProperty(area, "scrollTop", {
      configurable: true,
      get: () => top,
      set: (value: number) => {
        top = value;
      },
    });
    const inserted = [...review().querySelectorAll("ins")][1];
    const text = [...inserted.childNodes].find(
      (node) => node.nodeType === Node.TEXT_NODE,
    )!;
    window.getSelection()!.collapse(text, 3);
    fireEvent.click(review());
    expect(document.activeElement).toBe(area);
    expect(area.scrollTop).toBe(120);
  });

  it("copying a part of the review gives the final text only", async () => {
    const user = userEvent.setup();
    await corrected(user);
    review().focus();
    const range = document.createRange();
    range.selectNodeContents(review());
    window.getSelection()!.removeAllRanges();
    window.getSelection()!.addRange(range);
    let data = await user.copy();
    expect(data?.getData("text/plain")).toBe(CORRECTED);
    // Du milieu d'un mot supprimé (« T|eh ») à « film » : « The film ».
    const deleted = review().querySelector("del")!;
    const start = [...deleted.childNodes].find(
      (node) => node.nodeType === Node.TEXT_NODE,
    )!;
    const after = review().querySelector("ins")!.nextSibling!;
    range.setStart(start, 1);
    range.setEnd(after, " film".length);
    data = await user.copy();
    expect(data?.getData("text/plain")).toBe("The film");
    expect(screen.getByTestId("proofread-review")).toBeTruthy();
    window.getSelection()!.removeAllRanges();
  });

  it("the next keystroke edits the final text, the key landing at its end", async () => {
    const user = userEvent.setup();
    await corrected(user);
    expect(document.activeElement).toBe(review());
    await user.keyboard("!");
    expect(document.activeElement).toBe(editor());
    expect(editor().value).toBe(`${CORRECTED}!`);
    expect(screen.queryByTestId("proofread-review")).toBeNull();
    // Le texte modifié est celui de la prochaine correction.
    expect(button("Correct").disabled).toBe(false);
  });

  it("Space or Enter on a focused button activates it instead of editing", async () => {
    const user = userEvent.setup();
    await corrected(user);
    button("Copy").focus();
    await user.keyboard(" ");
    expect(screen.getByTestId("proofread-review")).toBeTruthy();
    expect(await screen.findByText("Copied")).toBeTruthy();
  });

  it("a click in the review edits the final text, the caret where it was clicked", async () => {
    const user = userEvent.setup();
    await corrected(user);
    // « I will receive » : clic dans le mot ajouté « receive », après « rec ».
    const inserted = [...review().querySelectorAll("ins")][1];
    const text = [...inserted.childNodes].find(
      (node) => node.nodeType === Node.TEXT_NODE,
    )!;
    window.getSelection()!.collapse(text, 3);
    fireEvent.click(review());
    const area = editor();
    expect(area.value).toBe(CORRECTED);
    const offset = CORRECTED.indexOf("receive") + 3;
    expect([area.selectionStart, area.selectionEnd]).toEqual([offset, offset]);
    expect(document.activeElement).toBe(area);
  });

  it("a click in a deleted word puts the caret where it was", async () => {
    const user = userEvent.setup();
    await corrected(user);
    const deleted = [...review().querySelectorAll("del")][1];
    const text = [...deleted.childNodes].find(
      (node) => node.nodeType === Node.TEXT_NODE,
    )!;
    window.getSelection()!.collapse(text, 4);
    fireEvent.click(review());
    const offset = CORRECTED.indexOf("receive");
    expect(editor().selectionStart).toBe(offset);
  });

  it("a text selected in the review stays (for copying), no editing", async () => {
    const user = userEvent.setup();
    await corrected(user);
    const range = document.createRange();
    range.selectNodeContents(review());
    window.getSelection()!.removeAllRanges();
    window.getSelection()!.addRange(range);
    fireEvent.click(review());
    expect(screen.getByTestId("proofread-review")).toBeTruthy();
    window.getSelection()!.removeAllRanges();
  });

  it("copies the final text", async () => {
    const user = userEvent.setup();
    const writeText = vi.spyOn(navigator.clipboard, "writeText");
    await corrected(user);
    await user.click(button("Copy"));
    expect(writeText).toHaveBeenCalledWith(CORRECTED);
    expect(await screen.findByText("Copied")).toBeTruthy();
  });

  it("Correct again from the review works on the final text", async () => {
    const user = userEvent.setup();
    const { simulator, fake } = await corrected(user);
    await user.click(button("Rephrase"));
    expect(simulator.callsTo("proofread_run").at(-1)).toMatchObject({
      text: CORRECTED,
      mode: "rephrase",
    });
    await fake.lastJob().finish();
    await screen.findByTestId("proofread-review");
    // Undo revient au texte d'avant cette reformulation.
    await user.click(button("Undo"));
    expect(editor().value).toBe(CORRECTED);
  });

  it("Rephrase shows the new text without marks", async () => {
    const user = userEvent.setup();
    const { fake } = setup();
    await typeText(user, "Teh film was very good.");
    await user.click(button("Rephrase"));
    expect(statusText()).toBe("Rephrasing…");
    await fake.lastJob().finish();
    const shown = await screen.findByRole("region", { name: "Rephrased text" });
    expect(shownText(shown)).toBe("The film was excellent.");
    expect(shown.querySelectorAll("del, ins")).toHaveLength(0);
    expect(statusText()).toBe("Text rephrasedqwen3:0.6b");
  });

  it("Ctrl+Enter corrects and Ctrl+Shift+Enter rephrases, from the text area", async () => {
    const user = userEvent.setup();
    const { simulator, fake } = setup();
    await typeText(user);
    await user.keyboard("{Control>}{Enter}{/Control}");
    expect(simulator.callsTo("proofread_run").at(-1)?.mode).toBe("correct");
    // Pendant le travail, une autre demande est ignorée.
    await user.keyboard("{Control>}{Shift>}{Enter}{/Shift}{/Control}");
    expect(simulator.callsTo("proofread_run")).toHaveLength(1);
    await fake.lastJob().finish();
    await screen.findByTestId("proofread-review");
    await user.keyboard("{Control>}{Shift>}{Enter}{/Shift}{/Control}");
    expect(simulator.callsTo("proofread_run").at(-1)).toMatchObject({
      mode: "rephrase",
      text: CORRECTED,
    });
  });

  it("Escape stops: finished passages kept, the rest left as it was", async () => {
    const user = userEvent.setup();
    const { simulator, fake } = setup();
    await typeText(user);
    await user.click(button("Correct"));
    const job = fake.lastJob();
    await job.step(4); // started, passage 1 terminé
    await user.keyboard("{Escape}");
    expect(simulator.callsTo("proofread_cancel")).toEqual([
      { jobId: job.jobId },
    ]);
    expect(button("Stopping…").disabled).toBe(true);
    await job.finish();
    expect(job.delivered.at(-1)).toEqual({ event: "done", stopped: true });
    expect(shownText(review())).toBe(
      "The film was good.\n\nI will recieve it tomorrow.",
    );
    expect(screen.getByTestId("proofread-note-partial").textContent).toBe(
      "Stopped: the rest of the text was left as it was.",
    );
    expect(statusText()).toBe("1 correctionqwen3:0.6b");
  });

  it("a command answered without its final event ends after a grace delay", async () => {
    const user = userEvent.setup();
    const { fake } = setup();
    await typeText(user);
    await user.click(button("Correct"));
    const job = fake.lastJob();
    await job.step(4); // started, passage 1 terminé
    await job.answer();
    // Encore en cours le temps que les derniers messages arrivent.
    expect(screen.getByTestId("proofread-running")).toBeTruthy();
    await screen.findByTestId("proofread-review", {}, { timeout: 4000 });
    expect(shownText(review())).toBe(
      "The film was good.\n\nI will recieve it tomorrow.",
    );
    // Le reste n'a pas été relu : dit, même sans arrêt ni erreur.
    expect(screen.getByTestId("proofread-note-partial").textContent).toBe(
      "The rest of the text was left as it was.",
    );
  });

  it("a partial review never says “No mistakes found” nor “Text rephrased”", async () => {
    const user = userEvent.setup();
    const { fake } = setup();
    await typeText(user, "All is fine.\n\nI will recieve it.");
    await user.click(button("Correct"));
    await fake.lastJob().step(4); // started, passage 1 (sans faute) terminé
    await user.keyboard("{Escape}");
    await fake.lastJob().finish();
    await screen.findByTestId("proofread-review");
    expect(statusText()).toBe("No mistakes in the corrected partqwen3:0.6b");
    expect(screen.getByTestId("proofread-note-partial")).toBeTruthy();

    await user.click(button("Undo"));
    await user.click(button("Rephrase"));
    await fake.lastJob().step(4);
    await user.keyboard("{Escape}");
    await fake.lastJob().finish();
    await screen.findByTestId("proofread-review");
    expect(statusText()).toBe("Part of the text rephrasedqwen3:0.6b");
  });

  it("Stop before any finished passage goes back to the text", async () => {
    const user = userEvent.setup();
    const { fake } = setup();
    await typeText(user);
    await user.click(button("Correct"));
    await fake.lastJob().step();
    await user.click(button("Stop"));
    await fake.lastJob().finish();
    expect(editor().value).toBe(TEXT);
    expect(document.activeElement).toBe(editor());
  });

  it("Escape does nothing outside a correction", async () => {
    const user = userEvent.setup();
    const { simulator } = await corrected(user);
    await user.keyboard("{Escape}");
    expect(simulator.callsTo("proofread_cancel")).toEqual([]);
    expect(screen.getByTestId("proofread-review")).toBeTruthy();
  });

  it("keeps passages the guard rejected, and says so", async () => {
    const user = userEvent.setup();
    const { fake } = setup();
    await typeText(user, "Teh start.\n\nKEEP what is your name?");
    await user.click(button("Correct"));
    await fake.lastJob().finish();
    await screen.findByTestId("proofread-review");
    expect(shownText(review())).toBe("The start.\n\nKEEP what is your name?");
    expect(screen.getByTestId("proofread-note-kept").textContent).toBe(
      "Some parts were left unchanged: the model's answer did not look like a correction.",
    );
  });

  it("does not say “No mistakes found” when the guard kept every passage", async () => {
    const user = userEvent.setup();
    const { fake } = setup();
    await typeText(user, "KEEP what is your name?");
    await user.click(button("Correct"));
    await fake.lastJob().finish();
    await screen.findByTestId("proofread-review");
    expect(statusText()).toBe("No corrections madeqwen3:0.6b");
    expect(screen.getByTestId("proofread-note-kept")).toBeTruthy();
  });

  it("no model: the error, a way to the model picker and to the ModelHub", async () => {
    const user = userEvent.setup();
    const { fake } = setup((backend) => {
      backend.settings.selectedModel = "";
    });
    await typeText(user);
    await user.click(button("Correct"));
    await fake.lastJob().finish();
    const alert = screen.getByRole("alert");
    expect(within(alert).getByText("Choose a model for the correction first"));
    // Rien n'a été corrigé : la zone de texte revient, avec le texte.
    expect(editor().value).toBe(TEXT);
    await user.click(
      within(alert).getByRole("button", { name: "Choose a model" }),
    );
    expect(document.activeElement).toBe(screen.getByLabelText("Model"));
    expect(
      within(alert)
        .getByRole("link", { name: "Open ModelHub" })
        .getAttribute("href"),
    ).toBe("/hub");
    // La frappe efface l'erreur.
    await user.click(editor());
    await user.keyboard("x");
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("model not found: the message of the engine and the same actions", async () => {
    const user = userEvent.setup();
    const { fake } = setup((backend) => {
      backend.settings.proofreadModel = "gemma3:4b";
    });
    await typeText(user);
    await user.click(button("Correct"));
    await fake.lastJob().finish();
    const alert = screen.getByRole("alert");
    expect(within(alert).getByText('Model "gemma3:4b" not found'));
    expect(within(alert).getByRole("button", { name: "Choose a model" }));
    expect(
      within(alert).queryByRole("button", { name: "Try again" }),
    ).toBeNull();
    expect(editor().value).toBe(TEXT);
  });

  it("an error after some passages keeps them, with Try again", async () => {
    const user = userEvent.setup();
    const { simulator, fake } = setup((backend) => {
      backend.proofread.failAt = {
        afterParts: 1,
        code: "unreachable",
        message: "Could not connect to Ollama",
      };
    });
    await typeText(user);
    await user.click(button("Correct"));
    await fake.lastJob().finish();
    const alert = await screen.findByRole("alert");
    expect(within(alert).getByText("Could not connect to Ollama"));
    expect(shownText(review())).toBe(
      "The film was good.\n\nI will recieve it tomorrow.",
    );
    expect(screen.getByTestId("proofread-note-partial").textContent).toBe(
      "The rest of the text was left as it was.",
    );
    fake.failAt = null;
    await user.click(within(alert).getByRole("button", { name: "Try again" }));
    // Même action, sur le texte montré.
    expect(simulator.callsTo("proofread_run").at(-1)).toMatchObject({
      mode: "correct",
      text: "The film was good.\n\nI will recieve it tomorrow.",
    });
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("a refused command is shown as an error", async () => {
    const user = userEvent.setup();
    setup((backend) => {
      backend.proofread.failures.set("proofread_run", {
        kind: "internal",
        message: "database is locked",
      });
    });
    await typeText(user);
    await user.click(button("Correct"));
    const alert = await screen.findByRole("alert");
    expect(within(alert).getByText("database is locked"));
    expect(editor().value).toBe(TEXT);
  });

  it("model picker: same as chat first, installed models, saved value shown even when missing", async () => {
    const user = userEvent.setup();
    const { simulator } = setup((backend) => {
      backend.settings.proofreadModel = "mistral:7b";
    });
    await screen.findByTestId("proofread");
    const select = screen.getByLabelText<HTMLSelectElement>("Model");
    await waitFor(() =>
      expect([...select.options].map((option) => option.text)).toEqual([
        "Same as chat (qwen3:0.6b)",
        "mistral:7b (not installed)",
        "qwen3:0.6b",
        "qwen3.5:0.8b",
      ]),
    );
    expect(select.value).toBe("mistral:7b");
    await user.selectOptions(select, "qwen3.5:0.8b");
    await user.selectOptions(select, "");
    expect(
      simulator
        .callsTo("update_settings")
        .map((args) => args.patch as SettingsPatch),
    ).toEqual([{ proofreadModel: "qwen3.5:0.8b" }, { proofreadModel: "" }]);
    await waitFor(() =>
      expect([...select.options].map((option) => option.text)).toEqual([
        "Same as chat (qwen3:0.6b)",
        "qwen3:0.6b",
        "qwen3.5:0.8b",
      ]),
    );
  });

  it("a correction goes on while another page is shown, its result waits", async () => {
    const user = userEvent.setup();
    const { fake, app } = setup();
    await typeText(user);
    await user.click(button("Correct"));
    await act(() => app.router.navigate({ to: "/settings" }));
    await screen.findByTestId("settings");
    await fake.lastJob().finish();
    await act(() => app.router.navigate({ to: "/proofread" }));
    expect(shownText(await screen.findByTestId("proofread-review"))).toBe(
      CORRECTED,
    );
  });

  it("the draft stays while another page is shown", async () => {
    const user = userEvent.setup();
    const { app } = setup();
    await typeText(user, "A draft.");
    await act(() => app.router.navigate({ to: "/settings" }));
    await screen.findByTestId("settings");
    await act(() => app.router.navigate({ to: "/proofread" }));
    await screen.findByTestId("proofread");
    expect(editor().value).toBe("A draft.");
  });
});

describe("Corrected selection (Show)", () => {
  const selection = (): SelectionResult => {
    const parts = fakeSplit("Teh cat sleeps.\n\nI recieve mail.");
    return {
      model: "gemma3:4b",
      parts,
      results: [{ text: "The cat sleeps.", kept: false }, null],
    };
  };

  it("shows the last corrected selection in review, once per request", async () => {
    const { simulator, app, fake } = setup((backend) => {
      backend.proofread.lastSelection = selection();
    }, "/proofread?selection=1");
    const shown = await screen.findByTestId("proofread-review");
    expect(shownText(shown)).toBe("The cat sleeps.\n\nI recieve mail.");
    expect(marks("del")).toEqual(["Teh"]);
    expect(statusText()).toBe("1 correctiongemma3:4b");
    expect(screen.getByTestId("proofread-note-selection").textContent).toBe(
      "Text selected in another app, then corrected.",
    );
    expect(screen.getByTestId("proofread-note-partial")).toBeTruthy();
    expect(simulator.callsTo("proofread_last_selection")).toHaveLength(1);

    // Undo : le texte d'origine de la sélection.
    await userEvent.click(button("Undo"));
    expect(editor().value).toBe("Teh cat sleeps.\n\nI recieve mail.");

    // Nouvelle action « Show » : nouvelle demande, relue.
    fake.lastSelection = {
      model: "gemma3:4b",
      parts: fakeSplit("recieve"),
      results: [{ text: "receive", kept: false }],
    };
    await act(() =>
      app.router.navigate({
        href: "/proofread?selection=1",
        state: { navigateRequest: 41 },
        replace: true,
      }),
    );
    await waitFor(() => expect(shownText(review())).toBe("receive"));
    expect(simulator.callsTo("proofread_last_selection")).toHaveLength(2);

    // Autre page, puis retour par l'historique : demande déjà servie, la
    // page garde son travail.
    await userEvent.click(button("Undo"));
    await act(() => app.router.navigate({ to: "/settings" }));
    await screen.findByTestId("settings");
    await act(() => app.router.history.back());
    await screen.findByTestId("proofread");
    expect(app.router.state.location.search).toEqual({ selection: true });
    expect(editor().value).toBe("recieve");
    expect(simulator.callsTo("proofread_last_selection")).toHaveLength(2);
  });

  /** Nouvelle action « Show » (numéro de demande comme la route racine). */
  let requests = 1000;
  async function show(context: ReturnType<typeof setup>) {
    context.fake.lastSelection = selection();
    requests += 1;
    await act(() =>
      context.app.router.navigate({
        href: "/proofread?selection=1",
        state: { navigateRequest: requests },
      }),
    );
    await screen.findByTestId("proofread-note-selection");
  }

  const backToYourText = () =>
    within(screen.getByTestId("proofread-note-saved")).getByRole("button", {
      name: "Back to your text",
    });

  it("sets a draft aside, and Back to your text brings it back", async () => {
    const user = userEvent.setup();
    const context = setup();
    await typeText(user, "My long draft, never corrected.");
    await show(context);
    expect(shownText(review())).toBe("The cat sleeps.\n\nI recieve mail.");
    expect(screen.getByTestId("proofread-note-saved").textContent).toBe(
      "Your text was set aside to show the corrected selection.Back to your text",
    );
    // Undo reste celui de la sélection ; la remarque reste.
    await user.click(button("Undo"));
    expect(editor().value).toBe("Teh cat sleeps.\n\nI recieve mail.");
    await user.click(backToYourText());
    expect(editor().value).toBe("My long draft, never corrected.");
    expect(screen.queryByTestId("proofread-note-saved")).toBeNull();
    expect(document.activeElement).toBe(editor());
  });

  it("sets a review aside, and a second Show keeps the text set aside first", async () => {
    const user = userEvent.setup();
    const context = await corrected(user);
    await show(context);
    await show(context);
    expect(shownText(review())).toBe("The cat sleeps.\n\nI recieve mail.");
    await user.click(backToYourText());
    expect(shownText(review())).toBe(CORRECTED);
    expect(marks("del")).toEqual(["Teh", "recieve"]);
    expect(document.activeElement).toBe(review());
    // Son Undo est le sien.
    await user.click(button("Undo"));
    expect(editor().value).toBe(TEXT);
  });

  it("stops a correction in progress cleanly: finished passages kept aside", async () => {
    const user = userEvent.setup();
    const context = setup();
    await typeText(user);
    await user.click(button("Correct"));
    const job = context.fake.lastJob();
    await job.step(4); // started, passage 1 terminé
    await show(context);
    expect(context.simulator.callsTo("proofread_cancel")).toEqual([
      { jobId: job.jobId },
    ]);
    // La fin du travail arrêté ne touche plus la page.
    await job.finish();
    expect(shownText(review())).toBe("The cat sleeps.\n\nI recieve mail.");
    await user.click(backToYourText());
    expect(shownText(review())).toBe(
      "The film was good.\n\nI will recieve it tomorrow.",
    );
    expect(screen.getByTestId("proofread-note-partial").textContent).toBe(
      "Stopped: the rest of the text was left as it was.",
    );
    await user.click(button("Undo"));
    expect(editor().value).toBe(TEXT);
  });

  it("nothing to set aside: no note", async () => {
    const context = setup();
    await screen.findByTestId("proofread");
    await show(context);
    expect(screen.queryByTestId("proofread-note-saved")).toBeNull();
  });

  it("nothing to show: the page stays as it was", async () => {
    const { simulator } = setup(() => {}, "/proofread?selection=1");
    await screen.findByTestId("proofread");
    await waitFor(() =>
      expect(simulator.callsTo("proofread_last_selection")).toHaveLength(1),
    );
    expect(editor().value).toBe("");
  });
});

describe("Proofread in other languages", () => {
  it("has no hard-coded text: editing, running, review, notes and errors", async () => {
    const user = userEvent.setup();
    const { fake } = setup((backend) => {
      backend.proofread.failAt = {
        afterParts: 1,
        code: "failed",
        message: "boom",
      };
    });
    await typeText(user, "Teh start.\n\nKEEP this.\n\nEnd.");
    usePseudoLanguage();
    const allow = [
      "Teh start.",
      "The start.",
      "KEEP this.",
      "End.",
      "Teh",
      "The",
      "qwen3:0.6b",
      "qwen3.5:0.8b",
      "boom",
    ];
    expect(untranslatedTexts(document.body, allow)).toEqual([]);
    await user.click(button("⟦Correct⟧"));
    const job = fake.lastJob();
    await job.step(2);
    expect(untranslatedTexts(document.body, allow)).toEqual([]);
    await job.finish();
    await screen.findByTestId("proofread-review");
    expect(screen.getByRole("alert")).toBeTruthy();
    expect(untranslatedTexts(document.body, allow)).toEqual([]);
  });

  it("has no hard-coded text: Redo, partial review, text set aside", async () => {
    const user = userEvent.setup();
    const { fake, app } = setup();
    await typeText(user, "All is fine.\n\nMy draft.");
    await user.click(button("Correct"));
    await fake.lastJob().step(4);
    usePseudoLanguage();
    await user.keyboard("{Escape}");
    await fake.lastJob().finish();
    await screen.findByTestId("proofread-review");
    const allow = [
      "All is fine.",
      "My draft.",
      "Teh cat sleeps.",
      "The cat sleeps.",
      "I recieve mail.",
      "Teh",
      "The",
      "qwen3:0.6b",
      "qwen3.5:0.8b",
      "gemma3:4b",
    ];
    expect(untranslatedTexts(document.body, allow)).toEqual([]);
    fake.lastSelection = {
      model: "gemma3:4b",
      parts: fakeSplit("Teh cat sleeps.\n\nI recieve mail."),
      results: [{ text: "The cat sleeps.", kept: false }, null],
    };
    await act(() =>
      app.router.navigate({
        href: "/proofread?selection=1",
        state: { navigateRequest: 7 },
      }),
    );
    await screen.findByTestId("proofread-note-saved");
    expect(untranslatedTexts(document.body, allow)).toEqual([]);
    await user.click(button("⟦Undo⟧"));
    expect(screen.getByRole("button", { name: "⟦Redo⟧" })).toBeTruthy();
    expect(untranslatedTexts(document.body, allow)).toEqual([]);
  });

  it("in French and Spanish: title, plural, keys", async () => {
    const user = userEvent.setup();
    await corrected(user);
    act(() => setLanguage("fr"));
    expect(screen.getByRole("heading", { level: 1 }).textContent).toBe(
      "Correction",
    );
    expect(statusText()).toBe("2 correctionsqwen3:0.6b");
    expect(button("Corriger").title).toBe("Corriger (Ctrl+Entrée)");
    expect(button("Reformuler").title).toBe("Reformuler (Ctrl+Maj+Entrée)");
    await user.click(button("Annuler"));
    expect(button("Rétablir").title).toBe("Rétablir (Ctrl+Maj+Z ou Ctrl+Y)");
    await user.click(button("Rétablir"));
    expect(review().querySelector("del")?.textContent).toBe(
      "supprimé\u00a0: Teh",
    );
    act(() => setLanguage("es"));
    expect(screen.getByRole("heading", { level: 1 }).textContent).toBe(
      "Corrección",
    );
    expect(statusText()).toBe("2 correccionesqwen3:0.6b");
    expect(button("Reformular").title).toBe("Reformular (Ctrl+Shift+Enter)");
  });
});
