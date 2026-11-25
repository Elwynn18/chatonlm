// Nouveau projet de l'Atelier face au simulateur : curseur d'autonomie (niveau
// des réglages d'abord, Maj+Tab dans la tâche passe au suivant, aussi sous
// WebKitGTK qui donne la touche « Unidentified », Tab seul ne le change pas),
// modèle (Auto et modèle retenu, avertissements montrés, refus qui empêche le
// lancement), stratégies « bientôt » grisées, nom du dossier proposé puis
// modifié, pièces jointes (sélecteur natif, image refusée), création (requête
// envoyée, vue du projet ouverte), refus du moteur ; textes traduits.

import {
  act,
  configure,
  fireEvent,
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

configure({ asyncUtilTimeout: 5000 });
vi.setConfig({ testTimeout: 30_000 });

function setup(prepare: (backend: FakeBackend) => void = () => {}) {
  const simulator = installSimulator();
  const backend = fakeBackend(simulator, {
    files: {
      "/docs/notes.md": {},
      "/docs/brief.pdf": {},
      "/docs/photo.png": {},
    },
  });
  prepare(backend);
  act(() => setSidebarOpen(true));
  const app = renderApp("/workshop/new");
  return { simulator, backend, workshop: backend.workshop, app };
}

const task = () =>
  screen.getByRole<HTMLTextAreaElement>("textbox", {
    name: "What should the Workshop do?",
  });
const level = () =>
  screen.getByTestId("workshop-autonomy").getAttribute("data-level");
const help = () => screen.getByTestId("workshop-autonomy-help").textContent;
const startButton = () =>
  screen.getByRole<HTMLButtonElement>("button", { name: "Start the project" });

async function ready() {
  await screen.findByTestId("workshop-form");
  // Réglages lus (autonomie proposée) et modèle vérifié.
  await screen.findByTestId("workshop-model-check");
}

describe("New Workshop project", () => {
  it("starts at the settings' autonomy and moves to the next level with Shift+Tab in the task", async () => {
    const user = userEvent.setup();
    setup((b) => {
      b.workshop.settings.autonomy = "acceptActions";
    });
    await ready();
    await waitFor(() => expect(level()).toBe("acceptActions"));
    expect(help()).toBe(
      "It asks you to approve the plan only; then it works on its own, web included.",
    );
    // Le niveau actif est en aplat roux, texte encre.
    const active = screen
      .getAllByTestId("workshop-autonomy-level")
      .find((label) => label.getAttribute("data-level") === "acceptActions")!;
    expect(active.className).toContain("bg-accent");
    expect(active.className).toContain("text-neutral-900");

    await user.click(task());
    await user.keyboard("{Shift>}{Tab}{/Shift}");
    expect(level()).toBe("autonomous");
    expect(document.activeElement).toBe(task());
    expect(help()).toBe(
      "It asks nothing: its questions become written assumptions, and it approves its own plan.",
    );
    // WebKitGTK : Maj+Tab arrive en « Unidentified », code « Tab ».
    const event = fireEvent.keyDown(task(), {
      key: "Unidentified",
      code: "Tab",
      shiftKey: true,
    });
    expect(event).toBe(false); // empêché : le focus ne part pas
    expect(level()).toBe("plan");
    await user.keyboard("{Shift>}{Tab}{/Shift}");
    expect(level()).toBe("ask");
    // Tab seul : le focus suit son cours, le niveau ne change pas.
    await user.keyboard("{Tab}");
    expect(level()).toBe("ask");
    expect(document.activeElement).not.toBe(task());
    // Le niveau se choisit aussi à la souris.
    await user.click(screen.getByRole("radio", { name: "Plan" }));
    expect(level()).toBe("plan");
  });

  it("describes the shortcut to the task, and lets Escape then Shift+Tab leave the task once", async () => {
    const user = userEvent.setup();
    setup();
    await ready();
    const shortcut = screen.getByTestId("workshop-autonomy-shortcut");
    expect(shortcut.textContent).toBe(
      "Shift+Tab in the task changes it; Esc then Shift+Tab leaves the task.",
    );
    expect(task().getAttribute("aria-describedby")?.split(" ")).toEqual([
      screen.getByTestId("workshop-autonomy-help").id,
      shortcut.id,
    ]);

    await user.click(task());
    await waitFor(() => expect(level()).toBe("ask"));
    await user.keyboard("{Escape}{Shift>}{Tab}{/Shift}");
    expect(level()).toBe("ask");
    expect(document.activeElement).not.toBe(task());
    // Une fois seulement : de retour dans la tâche, Maj+Tab change le niveau.
    await user.click(task());
    await user.keyboard("{Shift>}{Tab}{/Shift}");
    expect(level()).toBe("acceptActions");
    expect(document.activeElement).toBe(task());
    // Échap puis une autre touche : Maj+Tab change de nouveau le niveau.
    await user.keyboard("{Escape}a{Shift>}{Tab}{/Shift}");
    expect(level()).toBe("autonomous");
    expect(document.activeElement).toBe(task());
  });

  it("shows the model the Workshop will use, its warnings, and refuses a model that can't be used", async () => {
    const user = userEvent.setup();
    const { simulator } = setup((b) => {
      b.workshop.modelCheck = (model) =>
        model === ""
          ? { model: "qwen3:0.6b", auto: true, warnings: [], error: null }
          : model === "qwen3.5:0.8b"
            ? {
                model,
                auto: false,
                warnings: ["tooSmall", "noThinking"],
                error: null,
              }
            : {
                model: null,
                auto: false,
                warnings: [],
                error: "contextTooSmall",
              };
    });
    await ready();
    expect(screen.getByTestId("workshop-model-check").textContent).toBe(
      "The Workshop will use qwen3:0.6b, the largest model that fits.",
    );
    await user.type(task(), "Write a guide");
    expect(startButton().disabled).toBe(false);

    const model = screen.getByRole<HTMLSelectElement>("combobox", {
      name: "Model",
    });
    expect([...model.options].map((option) => option.value)).toEqual([
      "",
      "qwen3:0.6b",
      "qwen3.5:0.8b",
    ]);
    await user.selectOptions(model, "qwen3.5:0.8b");
    await waitFor(() =>
      expect(
        screen
          .getAllByTestId("workshop-model-warning")
          .map((item) => item.getAttribute("data-code")),
      ).toEqual(["tooSmall", "noThinking"]),
    );
    expect(screen.getByTestId("workshop-model-check").textContent).toContain(
      "This model is very small: the result will likely be poor.",
    );
    expect(simulator.callsTo("workshop_model_check")).toEqual([
      { model: null },
      { model: "qwen3.5:0.8b" },
    ]);
    // Avertissement seulement : le lancement reste permis.
    expect(startButton().disabled).toBe(false);

    await user.selectOptions(model, "qwen3:0.6b");
    const refusal = await screen.findByTestId("workshop-model-error");
    expect(refusal.textContent).toBe(
      "This model can't hold enough context for a project (at least 16,384 tokens).",
    );
    expect(startButton().disabled).toBe(true);
  });

  it("creates the project with the task, documents, autonomy, model, strategy and folder, then opens it", async () => {
    const user = userEvent.setup();
    const { simulator, backend, app } = setup((b) => {
      b.picks.push(["/docs/notes.md", "/docs/photo.png", "/docs/brief.pdf"]);
    });
    await ready();
    expect(startButton().disabled).toBe(true);
    await user.type(task(), "Guide : la permaculture ?{Enter}Pour débutants.");
    // Nom proposé : celui que le moteur tirerait du titre.
    const folder = screen.getByRole<HTMLInputElement>("textbox", {
      name: "Folder name",
    });
    expect(folder.value).toBe("Guide - la permaculture");
    expect(screen.getByTestId("workshop-form").textContent).toContain(
      "Created in /home/user/Documents/ChatonLM",
    );

    await user.click(screen.getByRole("button", { name: "Attach documents" }));
    await waitFor(() =>
      expect(
        screen.getAllByTestId("workshop-attachment").map((c) => c.textContent),
      ).toEqual(["notes.md", "brief.pdf"]),
    );
    expect(
      screen.getByText("photo.png: a project can't use images"),
    ).toBeTruthy();

    // Stratégies que le moteur ne sait pas encore mener : grisées.
    const strategy = screen.getByRole<HTMLSelectElement>("combobox", {
      name: "Strategy",
    });
    expect(
      [...strategy.options].map((o) => [o.value, o.disabled, o.textContent]),
    ).toEqual([
      ["", false, "Auto (chosen for the task)"],
      ["generic", false, "General"],
      ["longForm", false, "Long-form writing"],
      ["research", true, "Documented research (soon)"],
      ["documents", true, "Document analysis (soon)"],
      ["code", true, "Code (soon)"],
    ]);
    await user.selectOptions(strategy, "longForm");
    await user.clear(folder);
    await user.type(folder, "Permaculture");
    await user.click(task());
    await user.keyboard("{Shift>}{Tab}{/Shift}");
    expect(level()).toBe("acceptActions");

    await user.click(startButton());
    await waitFor(() => expect(app.path()).toBe("/workshop/1"));
    expect(simulator.callsTo("workshop_create")).toEqual([
      {
        project: {
          task: "Guide : la permaculture ?\nPour débutants.",
          attachmentIds: ["staged-1", "staged-3"],
          autonomy: "acceptActions",
          strategy: "longForm",
          folderName: "Permaculture",
        },
      },
    ]);
    // Documents repris par le moteur ; l'image refusée, libérée.
    expect(backend.staged.size).toBe(0);
    expect(simulator.callsTo("unstage")).toEqual([{ ids: ["staged-2"] }]);
    await screen.findByTestId("workshop-project-view");
    expect(screen.getByTestId("workshop-project-title").textContent).toBe(
      "Guide : la permaculture ?",
    );
  });

  it("leaves the folder name to the engine when it isn't changed, and shows a refusal", async () => {
    const user = userEvent.setup();
    const { simulator, workshop } = setup();
    await ready();
    await user.type(task(), "Write a report");
    workshop.failures.set("workshop_create", {
      kind: "invalid",
      message: "The projects folder can't be created.",
    });
    await user.click(startButton());
    const error = await screen.findByTestId("workshop-create-error");
    expect(error.textContent).toBe("The projects folder can't be created.");
    expect(simulator.callsTo("workshop_create")[0].project).toEqual({
      task: "Write a report",
      attachmentIds: [],
      autonomy: "ask",
    });
    expect(startButton().disabled).toBe(false);
  });

  it("is translated: French, Spanish, no text outside the catalogs", async () => {
    const user = userEvent.setup();
    setup();
    await ready();
    act(() => setLanguage("fr"));
    await waitFor(() =>
      expect(screen.getByRole("heading", { level: 1 }).textContent).toBe(
        "Atelier",
      ),
    );
    expect(help()).toBe(
      "Il vous demande d'approuver le plan, et avant chaque action qui a un effet (une recherche web, la lecture d'une page).",
    );
    expect(screen.getByTestId("workshop-autonomy").textContent).toContain(
      "Maj+Tab",
    );
    act(() => setLanguage("es"));
    await waitFor(() =>
      expect(screen.getByRole("heading", { level: 1 }).textContent).toBe(
        "Taller",
      ),
    );
    act(() => setLanguage("en"));
    await user.type(task(), "Write a guide");

    usePseudoLanguage();
    const form = screen.getByTestId("workshop-form");
    expect(
      untranslatedTexts(form, [
        "Write a guide",
        "qwen3:0.6b",
        "qwen3.5:0.8b",
        "/home/user/Documents/ChatonLM",
        ".txt, .md, .pdf, .docx",
      ]),
    ).toEqual([]);
    expect(within(form).getAllByRole("radio")).toHaveLength(4);
  });
});
