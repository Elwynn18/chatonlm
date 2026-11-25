// Page Tools face au simulateur : titre, cartes des outils dans l'ordre de la
// table, liens vers les pages des outils (Tools reste surlignée), pastille de
// la carte Atelier (un projet calcule, un projet attend l'utilisateur, tenue à
// jour par « workshop-event »), textes dans les trois langues.

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
import { TOOLS } from "@/lib/tools";
import { fakeBackend } from "@/test/backend";
import { untranslatedTexts, usePseudoLanguage } from "@/test/i18n";
import { renderApp } from "@/test/renderApp";
import { installSimulator } from "@/test/simulator";

configure({ asyncUtilTimeout: 5000 });
vi.setConfig({ testTimeout: 30_000 });

function setup(path = "/tools") {
  const simulator = installSimulator();
  const backend = fakeBackend(simulator);
  act(() => setSidebarOpen(true));
  return { ...renderApp(path), backend };
}

const cards = () =>
  within(screen.getByTestId("tools"))
    .getAllByRole("link")
    .map((card) => [
      card.getAttribute("data-testid"),
      card.getAttribute("href"),
      card.textContent,
    ]);

const toolsEntry = () =>
  document.querySelector<HTMLElement>('nav a[href="/tools"]')!;

describe("Tools page", () => {
  it("shows a card per tool, Study, Proofread, Transcription then Workshop, under the Tools title", async () => {
    setup();
    await screen.findByTestId("tools");
    expect(screen.getByRole("heading", { level: 1 }).textContent).toBe("Tools");
    expect(TOOLS.map((tool) => tool.id)).toEqual([
      "study",
      "proofread",
      "transcription",
      "workshop",
    ]);
    expect(cards()).toEqual([
      [
        "tool-card-study",
        "/study",
        "StudyTurn any topic into a short course, with lessons and quizzes.",
      ],
      [
        "tool-card-proofread",
        "/proofread",
        "ProofreadCorrect or rephrase a text and see every change.",
      ],
      [
        "tool-card-transcription",
        "/transcription",
        "TranscriptionTurn a video from the web or an audio file into text, then into a document to read.",
      ],
      [
        "tool-card-workshop",
        "/workshop",
        "WorkshopHand a long task to your models: they plan it, write it part by part and check every part.",
      ],
    ]);
    // Aucun projet : pas de pastille.
    expect(screen.queryByTestId("workshop-badge")).toBeNull();
    // Chaque carte a son icône.
    for (const card of within(screen.getByTestId("tools")).getAllByRole(
      "link",
    )) {
      expect(card.querySelector("svg")).toBeTruthy();
    }
    expect(toolsEntry().className.split(" ")).toContain("bg-accent");
  });

  it("opens each tool's page from its card; Tools stays highlighted", async () => {
    const user = userEvent.setup();
    const { path } = setup();
    await screen.findByTestId("tools");
    await user.click(screen.getByTestId("tool-card-proofread"));
    await screen.findByTestId("proofread");
    expect(path()).toBe("/proofread");
    expect(screen.getByRole("heading", { level: 1 }).textContent).toBe(
      "Proofread",
    );
    expect(toolsEntry().className.split(" ")).toContain("bg-accent");

    await user.click(toolsEntry());
    await screen.findByTestId("tools");
    expect(path()).toBe("/tools");
    await user.click(screen.getByTestId("tool-card-study"));
    await screen.findByTestId("study-home");
    expect(path()).toBe("/study");
    expect(toolsEntry().className.split(" ")).toContain("bg-accent");

    await user.click(toolsEntry());
    await user.click(await screen.findByTestId("tool-card-workshop"));
    await screen.findByTestId("workshop-home");
    expect(path()).toBe("/workshop");
    expect(screen.getByRole("heading", { level: 1 }).textContent).toBe(
      "Workshop",
    );
    expect(toolsEntry().className.split(" ")).toContain("bg-accent");
  });

  it("marks the Workshop card while a project works, and when one needs the user", async () => {
    const { backend } = setup();
    const workshop = backend.workshop;
    const project = workshop.seed({ task: "Write a guide" });
    const card = await screen.findByTestId("tool-card-workshop");
    expect(within(card).queryByTestId("workshop-badge")).toBeNull();

    // Le moteur prévient : le projet calcule.
    await workshop.update(project.summary.id, { state: "running" });
    const working = await within(card).findByTestId("workshop-badge");
    expect(working.getAttribute("data-badge")).toBe("working");
    expect(card.textContent).toContain("A project is working");

    // Une question attend : « Needs you », dit aussi en entier.
    await workshop.ask(project.summary.id, {
      type: "framing",
      value: ["For whom?"],
    });
    await waitFor(() =>
      expect(
        within(card).getByTestId("workshop-badge").getAttribute("data-badge"),
      ).toBe("needsYou"),
    );
    expect(card.textContent).toContain("Needs you");
    expect(card.textContent).toContain("A project needs you");

    // Fini : plus de pastille.
    await workshop.update(project.summary.id, { state: "done" });
    await waitFor(() =>
      expect(within(card).queryByTestId("workshop-badge")).toBeNull(),
    );
  });

  it("is translated: Spanish, French, no text outside the catalogs", async () => {
    setup();
    await screen.findByTestId("tools");
    act(() => setLanguage("es"));
    await waitFor(() =>
      expect(screen.getByRole("heading", { level: 1 }).textContent).toBe(
        "Herramientas",
      ),
    );
    expect(toolsEntry().textContent).toBe("Herramientas");
    expect(screen.getByTestId("tool-card-proofread").textContent).toContain(
      "Corrección",
    );
    act(() => setLanguage("fr"));
    await waitFor(() =>
      expect(screen.getByRole("heading", { level: 1 }).textContent).toBe(
        "Outils",
      ),
    );
    expect(screen.getByTestId("tool-card-study").textContent).toContain(
      "Étudier",
    );
    expect(screen.getByTestId("tool-card-workshop").textContent).toContain(
      "Atelier",
    );
    act(() => setLanguage("en"));

    usePseudoLanguage();
    expect(untranslatedTexts(screen.getByTestId("tools"))).toEqual([]);
    expect(untranslatedTexts(toolsEntry())).toEqual([]);
  });
});
