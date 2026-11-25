// Vue d'un projet de l'Atelier face au simulateur : le projet de
// démonstration avance (frise des phases, activité en clair, plan à
// approuver modifié puis approuvé, parties et panneau de la partie,
// vérifications avec leur méthode et leurs mesures, rapport à la fin) ;
// plan refusé avec une note ; questions de cadrage, de stratégie et
// d'action ; Pause, Reprendre, Arrêter (confirmation), menu ⋯ (ouvrir le
// dossier, supprimer avec les fichiers) ; états en file, cède la place,
// échoué ; version d'un seul jet ; journal ; projet supprimé ; textes
// traduits.

import {
  act,
  configure,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { setLanguage } from "@/i18n";
import { setSidebarOpen } from "@/lib/sidebar";
import { type FakeBackend, fakeBackend } from "@/test/backend";
import { untranslatedTexts, usePseudoLanguage } from "@/test/i18n";
import { renderApp } from "@/test/renderApp";
import { installSimulator } from "@/test/simulator";
import {
  check,
  DEMO_PLAN,
  DEMO_REPORT,
  part,
  WORKSHOP_TIME,
} from "@/test/workshop";

configure({ asyncUtilTimeout: 5000 });
vi.setConfig({ testTimeout: 30_000 });

function setup(prepare: (backend: FakeBackend) => void = () => {}) {
  const simulator = installSimulator();
  const backend = fakeBackend(simulator);
  prepare(backend);
  act(() => setSidebarOpen(true));
  const app = renderApp("/workshop/1");
  return { simulator, backend, workshop: backend.workshop, app };
}

const view = () => screen.getByTestId("workshop-project-view");
const activity = () => screen.getByTestId("workshop-activity-text").textContent;
const marks = () =>
  Object.fromEntries(
    screen
      .getAllByTestId("workshop-phase")
      .map((phase) => [
        phase.getAttribute("data-phase"),
        phase.getAttribute("data-mark"),
      ]),
  );
const rows = () =>
  screen
    .getAllByTestId("workshop-part-row")
    .map((row) => [
      row.getAttribute("data-part"),
      row.getAttribute("data-state"),
    ]);
const panel = () => screen.getByTestId("workshop-part-panel");

describe("Workshop project view", () => {
  it("follows the demo project from the strategy to the report, with the plan approved after changes", async () => {
    const user = userEvent.setup();
    const { simulator, workshop } = setup((b) => {
      b.workshop.demo();
    });
    await screen.findByTestId("workshop-project-view");
    expect(screen.getByTestId("workshop-project-title").textContent).toBe(
      "Write a 3,000-word guide to urban permaculture for beginners.",
    );
    expect(activity()).toBe("Waiting for its turn: position 1 in the queue.");
    await waitFor(() =>
      expect(screen.getByTestId("workshop-plan").textContent).toContain(
        "The plan isn't ready yet.",
      ),
    );

    await workshop.advance(1);
    await waitFor(() =>
      expect(activity()).toBe("Choosing the best way to do it (vote 2 of 3)…"),
    );
    expect(marks()).toMatchObject({
      collect: "done",
      strategy: "current",
      plan: "pending",
    });
    // Une phase en cours a son halo (trois pulsations, motion-safe).
    const current = () =>
      screen
        .getAllByTestId("workshop-phase")
        .find((phase) => phase.getAttribute("data-mark") === "current")!;
    expect(current().getAttribute("aria-current")).toBe("step");
    await waitFor(() =>
      expect(
        current().querySelector(".motion-safe\\:animate-study-pulse"),
      ).toBeTruthy(),
    );
    expect(current().textContent).toContain("Strategy, in progress");
    expect(screen.getByTestId("workshop-gauge-model").textContent).toContain(
      "qwen3:0.6b",
    );

    await workshop.advance(1);
    await waitFor(() =>
      expect(screen.getByTestId("workshop-strategy").textContent).toBe(
        "Long-form writing",
      ),
    );
    await waitFor(() => expect(activity()).toBe("Drafting plan 1 of 3…"));
    await waitFor(() =>
      expect(marks()).toMatchObject({
        strategy: "done",
        framing: "done",
        spec: "done",
        plan: "current",
      }),
    );

    // Le plan attend l'accord : bandeau, plan modifiable.
    await workshop.advance(1);
    const needYou = await screen.findByTestId("workshop-need-you");
    expect(within(needYou).getByRole("alert").textContent).toBe("I need you");
    expect(activity()).toBe("I need you: answer below so I can go on.");
    const titles = () =>
      within(needYou)
        .getAllByTestId("workshop-plan-part-title")
        .map((input) => (input as HTMLInputElement).value);
    expect(titles()).toEqual([
      "Introduction",
      "The soil",
      "Water",
      "Compost",
      "Conclusion",
    ]);
    expect(needYou.textContent).toContain("3000 words in all");
    const approve = within(needYou).getByTestId("workshop-plan-approve");
    expect(approve.textContent).toBe("Approve the plan");
    const water = within(needYou).getByRole<HTMLInputElement>("textbox", {
      name: "Title of part 3",
    });
    await user.clear(water);
    await user.type(water, "Watering");
    await user.click(
      within(needYou).getByRole("button", { name: "Move “Compost” up" }),
    );
    const words = within(needYou).getByRole<HTMLInputElement>("spinbutton", {
      name: "Words in part 2",
    });
    await user.clear(words);
    await user.type(words, "1000");
    expect(titles()).toEqual([
      "Introduction",
      "The soil",
      "Compost",
      "Watering",
      "Conclusion",
    ]);
    expect(approve.textContent).toBe("Approve my changes");
    expect(needYou.textContent).toContain("3200 words in all");
    // Un titre vide ne part pas.
    await user.clear(water);
    expect(approve.getAttribute("aria-disabled")).toBe("true");
    await user.type(water, "Watering");
    await user.click(approve);
    await waitFor(() =>
      expect(screen.queryByTestId("workshop-need-you")).toBeNull(),
    );
    const [sent] = simulator.callsTo("workshop_answer");
    expect(sent.id).toBe(1);
    const answer = sent.answer as {
      questionId: number;
      answer: {
        type: string;
        value: {
          approved: boolean;
          edited: typeof DEMO_PLAN;
          note: string | null;
        };
      };
    };
    expect(answer.questionId).toBe(1);
    expect(answer.answer.type).toBe("plan");
    expect(answer.answer.value.approved).toBe(true);
    expect(answer.answer.value.note).toBeNull();
    expect(
      answer.answer.value.edited.bricks.map((b) => [b.id, b.title, b.words]),
    ).toEqual([
      [1, "Introduction", 300],
      [2, "The soil", 1000],
      [4, "Compost", 800],
      [3, "Watering", 800],
      [5, "Conclusion", 300],
    ]);

    // Construction : les parties, celle au travail montrée à droite.
    await workshop.advance(1);
    await waitFor(() =>
      expect(rows()).toEqual([
        ["1", "pending"],
        ["2", "running"],
        ["3", "pending"],
        ["4", "pending"],
        ["5", "pending"],
      ]),
    );
    await waitFor(() => expect(activity()).toBe("Writing “The soil”…"));
    await waitFor(() => expect(panel().getAttribute("data-part")).toBe("2"));
    expect(screen.getByTestId("workshop-part-title").textContent).toBe(
      "The soil",
    );
    const introRow = rows().findIndex(([id]) => id === "1");
    expect(
      screen.getAllByTestId("workshop-part-row")[introRow].textContent,
    ).toContain("written at the end");
    // La partie au travail a son halo.
    expect(
      screen
        .getAllByTestId("workshop-part-row")[1]
        .querySelector("[data-busy]")!.className,
    ).toContain("motion-safe:animate-study-pulse");

    await workshop.advance(1);
    await waitFor(() => expect(activity()).toBe("Fixing “Compost”…"));
    // La partie au travail est maintenant « Compost », essai 2.
    await waitFor(() => expect(panel().getAttribute("data-part")).toBe("4"));
    expect(screen.getByTestId("workshop-part-state").textContent).toBe(
      "Fixing",
    );
    expect(panel().textContent).toContain("Try 2");
    // Un clic ouvre une autre partie : texte et vérifications.
    // Nom d'une ligne : tout son texte (titre, état, mots), lu tel quel.
    const soil = screen.getByRole("button", {
      name: /^The soil,\s?Done,\s?810 of 800 words$/,
    });
    expect(soil.getAttribute("aria-label")).toBeNull();
    expect(soil.getAttribute("aria-pressed")).toBe("false");
    await user.click(soil);
    await waitFor(() => expect(panel().getAttribute("data-part")).toBe("2"));
    // Ligne choisie : élément actif (aplat roux, texte encre, contour décalé).
    expect(soil.getAttribute("aria-pressed")).toBe("true");
    expect(soil.className).toContain("bg-accent");
    expect(soil.className).toContain("text-neutral-900");
    expect(soil.className).toContain("focus-visible:outline-offset-2");
    expect(soil.className).not.toContain("ring-");
    await within(panel()).findByTestId("workshop-part-text");
    expect(panel().textContent).toContain("A living soil feeds the plants.");
    expect(panel().textContent).toContain("Passed: 2 of 2");
    // Critère nommé par son texte (spécification retenue).
    expect(panel().textContent).toContain(
      "C1: Explains how to build a living soil",
    );
    expect(panel().textContent).toContain("judged on form");
    expect(panel().textContent).toContain(
      "measured by the program · 810 words (target: 600 to 1000)",
    );
    expect(screen.getByTestId("workshop-gauge-parts").textContent).toContain(
      "1/5",
    );
    const bar = screen.getByRole("progressbar", { name: "Parts" });
    expect(bar.getAttribute("aria-valuenow")).toBe("1");
    expect(bar.getAttribute("aria-valuetext")).toBe("1/5 parts");

    // Livraison : phases faites, rapport.
    await workshop.advance(1);
    const report = await screen.findByTestId("workshop-report");
    expect(activity()).toBe("Done! Everything is in the project folder.");
    await waitFor(() =>
      expect(Object.values(marks()).every((mark) => mark === "done")).toBe(
        true,
      ),
    );
    expect(report.textContent).toContain(
      "Written part by part, then assembled and checked. 3020 words written, 3000 words asked for",
    );
    expect(screen.getByTestId("workshop-report-file").textContent).toBe(
      "Deliverable: urban-permaculture.md",
    );
    expect(
      within(report).getByTestId("workshop-report-review").textContent,
    ).toContain("1 point to review");
    expect(
      within(report).getByTestId("workshop-report-review").textContent,
    ).toContain("200 g of manure per square metre");
    expect(
      within(report).getByTestId("workshop-report-doubtful").textContent,
    ).toContain("Worms like coffee grounds");
    expect(
      within(report).getByTestId("workshop-report-signals").textContent,
    ).toContain("Workshop words in the text");
    expect(
      within(report).getByTestId("workshop-report-not-checked").textContent,
    ).toBe("Not checkedThe sources.");
    expect(
      within(report).getByTestId("workshop-report-warnings").textContent,
    ).toBe(
      "Along the wayNo plan was flawless: the best of three was kept.The plan kept has a flaw.",
    );
    expect(
      within(report).getByTestId("workshop-report-proofread").textContent,
    ).toBe(
      "Language proofreading: 10 passages read, 3 corrected, 1 corrections set aside.",
    );
    expect(
      within(report)
        .getAllByTestId("workshop-report-part")
        .map((row) => row.getAttribute("data-state")),
    ).toEqual(["done", "toReview"]);
    // Plus d'action sur un projet fini, sauf le menu.
    expect(screen.queryByTestId("workshop-pause")).toBeNull();
    expect(screen.queryByTestId("workshop-stop")).toBeNull();
    await user.click(within(report).getByTestId("workshop-report-open"));
    expect(simulator.callsTo("workshop_open_folder")).toEqual([{ id: 1 }]);
    await user.click(
      within(report).getByRole("button", { name: "Open the deliverable" }),
    );
    expect(simulator.callsTo("workshop_open_file")).toEqual([{ id: 1 }]);
    expect(screen.queryByTestId("workshop-open-error")).toBeNull();
    // Refus du moteur : dit sous le rapport.
    workshop.failures.set("workshop_open_file", {
      kind: "system",
      message: "No application can open this file.",
    });
    await user.click(
      within(report).getByRole("button", { name: "Open the deliverable" }),
    );
    expect((await screen.findByTestId("workshop-open-error")).textContent).toBe(
      "That didn't work: No application can open this file.",
    );
  });

  it("keeps a notice that comes during the first read of the project", async () => {
    let release = () => {};
    const { simulator, workshop } = setup((b) => {
      b.workshop.seed({ task: "Guide", state: "running" });
      release = b.workshop.hold("workshop_get");
    });
    await waitFor(() =>
      expect(simulator.callsTo("workshop_get").length).toBe(1),
    );
    // La question arrive pendant la toute première lecture (sans données) :
    // React Query la reprend au lieu de la relancer.
    await workshop.ask(1, { type: "approvePlan", value: DEMO_PLAN });
    await act(async () => release());
    const needYou = await screen.findByTestId("workshop-need-you");
    expect(within(needYou).getByRole("alert").textContent).toBe("I need you");
  });

  it("reads the log up to the last event announced during its first read", async () => {
    let release = () => {};
    const { simulator, workshop } = setup((b) => {
      b.workshop.seed({ task: "Guide", state: "running" });
      release = b.workshop.hold("workshop_events");
    });
    await waitFor(() =>
      expect(simulator.callsTo("workshop_events").length).toBe(1),
    );
    await workshop.emit(1, {
      type: "activity",
      activity: { code: "assembling" },
    });
    await act(async () => release());
    await waitFor(() => expect(activity()).toBe("Putting the parts together…"));
  });

  it("asks for another plan with a note", async () => {
    const user = userEvent.setup();
    const { simulator, workshop } = setup((b) => {
      b.workshop.seed({ task: "Guide", state: "running" });
    });
    await screen.findByTestId("workshop-project-view");
    await workshop.ask(1, { type: "approvePlan", value: DEMO_PLAN });
    const needYou = await screen.findByTestId("workshop-need-you");
    const refuse = within(needYou).getByRole<HTMLButtonElement>("button", {
      name: "Ask for another plan",
    });
    expect(refuse.disabled).toBe(true);
    await user.type(
      within(needYou).getByRole("textbox", { name: "What should change?" }),
      "  One part per season  ",
    );
    await user.click(refuse);
    await waitFor(() =>
      expect(simulator.callsTo("workshop_answer")).toEqual([
        {
          id: 1,
          answer: {
            questionId: 1,
            answer: {
              type: "plan",
              value: {
                approved: false,
                edited: null,
                note: "One part per season",
              },
            },
          },
        },
      ]),
    );
  });

  it("says what to fill in before the plan can be approved, and that a note only goes with a refusal", async () => {
    const user = userEvent.setup();
    const { simulator, workshop } = setup((b) => {
      b.workshop.seed({ task: "Guide", state: "running" });
    });
    await screen.findByTestId("workshop-project-view");
    await workshop.ask(1, { type: "approvePlan", value: DEMO_PLAN });
    const needYou = await screen.findByTestId("workshop-need-you");
    const approve = within(needYou).getByTestId("workshop-plan-approve");
    expect(approve.getAttribute("aria-disabled")).toBeNull();
    expect(screen.queryByTestId("workshop-plan-problem")).toBeNull();

    // Partie 2 à 0 mot : le message la nomme, le champ est marqué, le
    // bouton reste atteignable et décrit par le message, sans rien envoyer.
    const words = within(needYou).getByRole<HTMLInputElement>("spinbutton", {
      name: "Words in part 2",
    });
    await user.clear(words);
    const problem = screen.getByTestId("workshop-plan-problem");
    expect(problem.getAttribute("role")).toBe("alert");
    expect(problem.textContent).toBe(
      "Give part 2 (“The soil”) a number of words to approve the plan.",
    );
    expect(words.getAttribute("aria-invalid")).toBe("true");
    expect(words.getAttribute("aria-describedby")).toBe(problem.id);
    expect(approve.getAttribute("aria-disabled")).toBe("true");
    expect(approve.getAttribute("aria-describedby")).toBe(problem.id);
    approve.focus();
    expect(document.activeElement).toBe(approve);
    await user.click(approve);
    expect(simulator.callsTo("workshop_answer")).toEqual([]);
    await user.type(words, "500");
    expect(screen.queryByTestId("workshop-plan-problem")).toBeNull();
    expect(words.getAttribute("aria-invalid")).toBeNull();

    // Partie 3 sans titre.
    const water = within(needYou).getByRole<HTMLInputElement>("textbox", {
      name: "Title of part 3",
    });
    await user.clear(water);
    expect(screen.getByTestId("workshop-plan-problem").textContent).toBe(
      "Give part 3 a title to approve the plan.",
    );
    expect(water.getAttribute("aria-invalid")).toBe("true");
    await user.type(water, "Water");

    // Titre du document vide.
    const title = within(needYou).getByRole<HTMLInputElement>("textbox", {
      name: "Title of the document",
    });
    await user.clear(title);
    expect(screen.getByTestId("workshop-plan-problem").textContent).toBe(
      "Give the document a title to approve the plan.",
    );
    expect(title.getAttribute("aria-invalid")).toBe("true");
    expect(approve.getAttribute("aria-disabled")).toBe("true");
    await user.click(approve);
    expect(simulator.callsTo("workshop_answer")).toEqual([]);
    await user.type(title, DEMO_PLAN.title);
    expect(approve.getAttribute("aria-disabled")).toBeNull();

    // Une note tapée : il est dit qu'elle ne part qu'avec un refus.
    expect(screen.queryByTestId("workshop-plan-note-ignored")).toBeNull();
    await user.type(
      within(needYou).getByRole("textbox", { name: "What should change?" }),
      "Shorter",
    );
    const ignored = screen.getByTestId("workshop-plan-note-ignored");
    expect(ignored.textContent).toBe(
      "Approving doesn't send your note: it's only used when you ask for another plan.",
    );
    expect(approve.getAttribute("aria-describedby")).toBe(ignored.id);
    await user.click(approve);
    await waitFor(() =>
      expect(simulator.callsTo("workshop_answer")[0].answer).toEqual({
        questionId: 1,
        answer: {
          type: "plan",
          value: {
            approved: true,
            edited: { ...DEMO_PLAN, bricks: expect.any(Array) },
            note: null,
          },
        },
      }),
    );
  });

  it("approves an unchanged plan without sending it back", async () => {
    const user = userEvent.setup();
    const { simulator, workshop } = setup((b) => {
      b.workshop.seed({ task: "Guide", state: "running" });
    });
    await screen.findByTestId("workshop-project-view");
    await workshop.ask(1, { type: "approvePlan", value: DEMO_PLAN });
    await user.click(await screen.findByTestId("workshop-plan-approve"));
    await waitFor(() =>
      expect(simulator.callsTo("workshop_answer")[0].answer).toEqual({
        questionId: 1,
        answer: {
          type: "plan",
          value: { approved: true, edited: null, note: null },
        },
      }),
    );
  });

  it("answers the framing questions, the strategy question and an action, and says when an answer fails", async () => {
    const user = userEvent.setup();
    const { simulator, workshop } = setup((b) => {
      b.workshop.seed({ task: "Guide", state: "running" });
    });
    await screen.findByTestId("workshop-project-view");

    await workshop.ask(1, {
      type: "framing",
      value: ["For whom?", "How long?"],
    });
    const framing = await screen.findByTestId("workshop-question-framing");
    expect(framing.textContent).toContain("1. For whom?");
    const answers = within(framing).getAllByTestId("workshop-framing-answer");
    await user.type(answers[0], " Beginners ");
    workshop.failures.set("workshop_answer", {
      kind: "invalid",
      message: "This question no longer waits for an answer",
    });
    await user.click(
      within(framing).getByRole("button", { name: "Send my answers" }),
    );
    expect(
      (await screen.findByTestId("workshop-answer-error")).textContent,
    ).toBe(
      "Couldn't send your answer: This question no longer waits for an answer",
    );
    workshop.failures.delete("workshop_answer");
    await user.click(
      within(framing).getByRole("button", { name: "Send my answers" }),
    );
    await waitFor(() =>
      expect(screen.queryByTestId("workshop-question-framing")).toBeNull(),
    );
    expect(simulator.callsTo("workshop_answer")[1].answer).toEqual({
      questionId: 1,
      answer: { type: "framing", value: ["Beginners", ""] },
    });

    await workshop.ask(1, {
      type: "strategy",
      value: ["longForm", "generic"],
    });
    const strategy = await screen.findByTestId("workshop-question-strategy");
    expect(
      within(strategy).getByRole<HTMLInputElement>("radio", {
        name: "Long-form writing",
      }).checked,
    ).toBe(true);
    await user.click(within(strategy).getByRole("radio", { name: "General" }));
    await user.click(
      within(strategy).getByRole("button", { name: "Continue" }),
    );
    await waitFor(() =>
      expect(screen.queryByTestId("workshop-question-strategy")).toBeNull(),
    );
    expect(simulator.callsTo("workshop_answer")[2].answer).toEqual({
      questionId: 2,
      answer: { type: "strategy", value: "generic" },
    });

    await workshop.ask(1, {
      type: "approveAction",
      value: { tool: "webSearch", target: "urban compost rules" },
    });
    const action = await screen.findByTestId("workshop-question-action");
    expect(action.textContent).toContain("Search the web");
    expect(action.textContent).toContain("urban compost rules");
    await user.click(
      within(action).getByRole("button", { name: "Don't allow" }),
    );
    await waitFor(() =>
      expect(simulator.callsTo("workshop_answer")[3].answer).toEqual({
        questionId: 3,
        answer: { type: "action", value: false },
      }),
    );
  });

  it("pauses, resumes, stops after a confirmation, and deletes with the files from the menu", async () => {
    const user = userEvent.setup();
    const { simulator, app, workshop } = setup((b) => {
      b.workshop.seed({ task: "Guide", state: "running" });
    });
    await screen.findByTestId("workshop-project-view");
    // Un seul bouton Pause / Resume : le focus reste dessus d'un état à
    // l'autre (aria-disabled pendant l'appel, jamais disabled).
    const toggle = screen.getByRole("button", { name: "Pause" });
    const release = workshop.hold("workshop_pause");
    await user.click(toggle);
    expect(document.activeElement).toBe(toggle);
    expect(toggle.getAttribute("aria-disabled")).toBe("true");
    expect(toggle.hasAttribute("disabled")).toBe(false);
    // Un second clic pendant l'appel n'envoie rien.
    await user.click(toggle);
    expect(simulator.callsTo("workshop_pause")).toHaveLength(1);
    await act(async () => release());
    await waitFor(() =>
      expect(view().getAttribute("data-state")).toBe("paused"),
    );
    expect(activity()).toBe(
      "Paused. Nothing is lost: resume it whenever you like.",
    );
    expect(screen.queryByRole("button", { name: "Pause" })).toBeNull();
    expect(screen.getByRole("button", { name: "Resume" })).toBe(toggle);
    expect(toggle.getAttribute("data-testid")).toBe("workshop-resume");
    expect(document.activeElement).toBe(toggle);
    expect(toggle.hasAttribute("disabled")).toBe(false);
    await user.click(toggle);
    await waitFor(() =>
      expect(view().getAttribute("data-state")).toBe("queued"),
    );
    expect(toggle.textContent).toBe("Pause");
    expect(document.activeElement).toBe(toggle);
    expect(simulator.callsTo("workshop_pause")).toEqual([{ id: 1 }]);
    expect(simulator.callsTo("workshop_resume")).toEqual([{ id: 1 }]);

    // Arrêter : confirmation, « Cancel » a le focus (un Entrée ne vaut pas accord).
    await user.click(screen.getByRole("button", { name: "Stop" }));
    const dialog = await screen.findByRole("alertdialog", {
      name: "Stop “Guide”?",
    });
    expect(document.activeElement?.textContent).toBe("Cancel");
    await user.click(
      within(dialog).getByRole("button", { name: "Stop the project" }),
    );
    await waitFor(() =>
      expect(view().getAttribute("data-state")).toBe("cancelled"),
    );
    expect(simulator.callsTo("workshop_cancel")).toEqual([{ id: 1 }]);
    expect(activity()).toBe("Project stopped.");
    // Pause et Stop ont disparu : le focus est sur le titre, pas perdu.
    expect(screen.queryByRole("button", { name: "Stop" })).toBeNull();
    expect(document.activeElement).toBe(
      screen.getByTestId("workshop-project-title"),
    );

    // Menu ⋯ : ouvrir le dossier, puis supprimer avec les fichiers.
    const more = screen.getByRole("button", { name: "More actions" });
    // Liste de boutons dépliée, pas un menu : pas d'aria-haspopup.
    expect(more.hasAttribute("aria-haspopup")).toBe(false);
    await user.click(more);
    expect(more.getAttribute("aria-expanded")).toBe("true");
    await user.keyboard("{Escape}");
    expect(more.getAttribute("aria-expanded")).toBe("false");
    expect(document.activeElement).toBe(more);
    await user.click(more);
    await user.click(screen.getByRole("button", { name: "Open the folder" }));
    expect(simulator.callsTo("workshop_open_folder")).toEqual([{ id: 1 }]);
    await user.click(more);
    await user.click(
      screen.getByRole("button", { name: "Delete the project…" }),
    );
    const confirm = await screen.findByRole("alertdialog", {
      name: "Delete “Guide”?",
    });
    expect(confirm.textContent).toContain(
      "Its folder is /home/user/Documents/ChatonLM/Guide.",
    );
    // Effacer les fichiers : bouton destructif, rouge du thème.
    const deleteFiles = within(confirm).getByRole("button", {
      name: "Delete the files too",
    });
    expect(deleteFiles.className).toContain("bg-red-600");
    expect(
      within(confirm).getByRole("button", { name: "Keep the files" }).className,
    ).not.toContain("red");
    await user.click(deleteFiles);
    await waitFor(() => expect(app.path()).toBe("/workshop"));
    expect(simulator.callsTo("workshop_delete")).toEqual([
      { id: 1, deleteFiles: true },
    ]);
    expect(await screen.findByTestId("workshop-empty")).toBeTruthy();
  });

  it("says when the project gives way to the user, when it failed (and can be resumed), and when it's gone", async () => {
    const user = userEvent.setup();
    const { workshop } = setup((b) => {
      b.workshop.seed({ task: "Guide", state: "yielding" });
    });
    await screen.findByTestId("workshop-project-view");
    expect(activity()).toBe(
      "You're using the app: the project waits until you're done.",
    );
    await workshop.update(1, { state: "failed", error: "contextExceeded" });
    await waitFor(() =>
      expect(activity()).toBe("The model ran out of context."),
    );
    expect(screen.getByTestId("workshop-activity-text").className).toContain(
      "text-red-600",
    );
    expect(screen.getByRole("button", { name: "Resume" })).toBeTruthy();
    await workshop.update(1, { error: "brandNewCode" });
    await waitFor(() => expect(activity()).toBe("Something went wrong."));

    // Supprimé ailleurs : la page le dit.
    workshop.projects.delete(1);
    await workshop.notify({ projectId: 1, deleted: true });
    await waitFor(() =>
      expect(screen.getByTestId("workshop-project-error").textContent).toBe(
        "This project no longer exists.",
      ),
    );
    await user.click(screen.getByRole("link", { name: "Projects" }));
    await screen.findByTestId("workshop-empty");
  });

  it("says a finished part without checks had no separate check (not “not yet”), and folds the task behind a button", async () => {
    const user = userEvent.setup();
    setup((b) => {
      const project = b.workshop.seed({
        task: "Write the tomato guide",
        state: "running",
      });
      project.parts = [
        part(1, { title: "Introduction", kind: "intro", state: "done" }),
        part(2, { title: "Varieties", state: "pending" }),
        part(3, { title: "Pests", state: "refused" }),
        part(4, { title: "Harvest", state: "running" }),
      ];
    });
    await screen.findByTestId("workshop-project-view");
    const open = async (id: string, title: string) => {
      await user.click(
        (await screen.findAllByTestId("workshop-part-row")).find(
          (row) => row.getAttribute("data-part") === id,
        )!,
      );
      await waitFor(() =>
        expect(screen.getByTestId("workshop-part-title").textContent).toBe(
          title,
        ),
      );
    };
    await open("1", "Introduction");
    // Rien n'affirme une vérification qui n'a pas eu lieu.
    expect(panel().textContent).toContain("No separate check for this part.");
    expect(panel().textContent).not.toContain("Not checked yet.");
    expect(panel().textContent).not.toContain("whole");
    await open("3", "Pests");
    expect(panel().textContent).toContain("No separate check for this part.");
    await open("2", "Varieties");
    expect(panel().textContent).toContain("Not checked yet.");
    await open("4", "Harvest");
    expect(panel().textContent).toContain("Not checked yet.");

    const task = screen.getByTestId("workshop-task-detail");
    expect(task.textContent).not.toContain("Write the tomato guide");
    const toggle = screen.getByRole("button", { name: "Task" });
    expect(toggle.getAttribute("aria-expanded")).toBe("false");
    await user.click(toggle);
    expect(toggle.getAttribute("aria-expanded")).toBe("true");
    expect(task.textContent).toContain("Write the tomato guide");
  });

  it("shows the one-go version as a single part, and the log in words, each event once", async () => {
    const user = userEvent.setup();
    const { workshop, simulator } = setup((b) => {
      const project = b.workshop.seed({
        task: "Short guide",
        state: "running",
      });
      project.parts = [
        part(0, {
          title: "Short guide",
          targetWords: 2000,
          state: "checking",
          words: 1980,
          checks: [
            check("C1", false, {
              method: "formJudge",
              evidence: "The guide never names a source.",
            }),
          ],
        }),
      ];
    });
    await screen.findByTestId("workshop-project-view");
    // Lecture qui rend aussi des événements déjà reçus (lectures croisées) :
    // chacun n'est gardé qu'une fois.
    simulator.handle("workshop_events", () => workshop.project(1).events);
    await workshop.emit(1, { type: "phaseStarted", phase: "build" });
    await waitFor(() =>
      expect(
        screen
          .getAllByTestId("workshop-phase")
          .find((phase) => phase.getAttribute("data-phase") === "build")!
          .getAttribute("data-mark"),
      ).toBe("current"),
    );
    await workshop.emit(1, {
      type: "activity",
      activity: { code: "checkingDirect" },
    });
    await waitFor(() =>
      expect(activity()).toBe("Checking the one-go version…"),
    );
    await workshop.emit(1, { type: "warning", code: "lengthCapped" });
    expect(rows()).toEqual([["0", "checking"]]);
    expect(screen.getByTestId("workshop-plan").textContent).toContain(
      "One-go version",
    );
    expect(screen.getByTestId("workshop-plan").textContent).toContain(
      "It splits the work into parts only if that version fails.",
    );
    await waitFor(() =>
      expect(screen.getByTestId("workshop-part-title").textContent).toBe(
        "One-go version",
      ),
    );
    expect(panel().textContent).toContain("1980 of 2000 words");
    expect(panel().textContent).toContain("Passed: 0 of 1");
    expect(panel().textContent).toContain(
      "judged on form · The guide never names a source.",
    );

    const toggle = screen.getByRole("button", { name: "Activity log" });
    expect(screen.queryAllByTestId("workshop-log-entry")).toHaveLength(0);
    await user.click(toggle);
    await waitFor(() =>
      expect(screen.getAllByTestId("workshop-log-entry")).toHaveLength(3),
    );
    expect(
      screen
        .getAllByTestId("workshop-log-entry")
        .map((entry) => entry.lastElementChild?.textContent),
    ).toEqual([
      "Phase “Writing” started.",
      "Checking the one-go version…",
      "The length asked for was too much for this model and was reduced.",
    ]);
  });

  it("stays quiet when the one-go version disappears before the list turns to the plan", async () => {
    let release = () => {};
    const { simulator, workshop } = setup((b) => {
      const project = b.workshop.seed({
        task: "Short guide",
        state: "running",
      });
      project.parts = [part(0, { title: "Short guide", state: "running" })];
    });
    await screen.findByTestId("workshop-project-view");
    await waitFor(() =>
      expect(screen.getByTestId("workshop-part-title").textContent).toBe(
        "One-go version",
      ),
    );
    // Version abandonnée : la partie 0 n'existe plus, mais la liste (encore
    // en route) la montre un instant.
    release = workshop.hold("workshop_parts");
    workshop.project(1).parts = [
      part(1, { title: "Introduction", kind: "intro", state: "pending" }),
      part(2, { title: "Soil", state: "running" }),
    ];
    const reads = simulator.callsTo("workshop_part").length;
    await workshop.emit(1, { type: "warning", code: "directFailed" });
    await waitFor(() =>
      expect(simulator.callsTo("workshop_part").length).toBeGreaterThan(reads),
    );
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 20));
    });
    expect(within(panel()).queryByRole("alert")).toBeNull();
    await act(async () => release());
    await waitFor(() =>
      expect(screen.getByTestId("workshop-part-title").textContent).toBe(
        "Soil",
      ),
    );
    expect(within(panel()).queryByRole("alert")).toBeNull();
  });

  it("says no check of the whole was noted rather than that all passed", async () => {
    const { workshop } = setup((b) => {
      b.workshop.seed({ task: "Guide", state: "running" });
    });
    await screen.findByTestId("workshop-project-view");
    await workshop.finish(1, { ...DEMO_REPORT, checks: [] });
    const checks = await screen.findByTestId("workshop-report-checks");
    expect(checks.textContent).toContain("No checks noted.");
    expect(checks.textContent).not.toContain("Every check passed.");
  });

  it("dates the log entries when the log spans several days", async () => {
    const user = userEvent.setup();
    const { workshop } = setup((b) => {
      b.workshop.seed({ task: "Guide", state: "running" });
    });
    await screen.findByTestId("workshop-project-view");
    await workshop.emit(1, { type: "phaseStarted", phase: "collect" });
    await workshop.emit(1, { type: "phaseDone", phase: "collect" });
    await user.click(screen.getByTestId("workshop-log-toggle"));
    const times = () =>
      screen
        .getAllByTestId("workshop-log-time")
        .map((time) => time.textContent);
    await waitFor(() => expect(times()).toHaveLength(2));
    // Même jour : l'heure seule.
    expect(times().some((time) => time?.includes("2023"))).toBe(false);
    await workshop.emit(
      1,
      { type: "phaseStarted", phase: "strategy" },
      WORKSHOP_TIME + 3 * 24 * 3_600_000,
    );
    await waitFor(() => expect(times()).toHaveLength(3));
    expect(times().every((time) => time?.includes("2023"))).toBe(true);
  });

  it("says a project address without a number leads nowhere, in words", async () => {
    const simulator = installSimulator();
    fakeBackend(simulator);
    act(() => setSidebarOpen(true));
    renderApp("/workshop/abc");
    const error = await screen.findByTestId("workshop-project-error");
    expect(error.textContent).toBe("Project not found.");
    expect(simulator.callsTo("workshop_get")).toEqual([]);
  });

  it("is translated: French, Spanish, no text outside the catalogs", async () => {
    const user = userEvent.setup();
    const { workshop } = setup((b) => {
      b.workshop.demo();
    });
    await screen.findByTestId("workshop-project-view");
    // Rang dans la file sans ordinal à accorder (« position 1 », pas « 1e »).
    act(() => setLanguage("fr"));
    await waitFor(() =>
      expect(activity()).toBe(
        "Attend son tour\u00a0: position 1 dans la file.",
      ),
    );
    act(() => setLanguage("en"));
    for (let step = 0; step < 3; step += 1) await workshop.advance(1);
    await screen.findByTestId("workshop-need-you");
    act(() => setLanguage("fr"));
    await waitFor(() =>
      expect(
        within(screen.getByTestId("workshop-need-you")).getByRole("alert")
          .textContent,
      ).toBe("J'ai besoin de vous"),
    );
    act(() => setLanguage("es"));
    await waitFor(() =>
      expect(
        within(screen.getByTestId("workshop-need-you")).getByRole("alert")
          .textContent,
      ).toBe("Te necesito"),
    );
    act(() => setLanguage("en"));
    await user.click(screen.getByTestId("workshop-plan-approve"));
    for (let step = 0; step < 3; step += 1) await workshop.advance(1);
    await screen.findByTestId("workshop-report");
    await user.click(screen.getByRole("button", { name: "Activity log" }));
    await user.click(screen.getByText("Task"));

    usePseudoLanguage();
    const allowed = [
      "Write a 3,000-word guide to urban permaculture for beginners.",
      "qwen3:0.6b",
      "urban-permaculture.md",
      ...DEMO_PLAN.bricks.map((brick) => brick.title),
      "200 g of manure per square metre",
      "Worms like coffee grounds",
      "The soil section",
      ...["Explains how to build a living soil", "C1"],
      // Heures du journal (format de la langue).
      "AM",
      "PM",
    ];
    expect(untranslatedTexts(view(), allowed)).toEqual([]);
  });
});
