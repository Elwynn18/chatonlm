// Lecture à voix haute des textes d'un cours face au simulateur (faux moteur
// de src/test/tts.ts) : le bouton des réponses du chat sous une section, un
// exemple, les « à retenir », une explication d'un nouvel essai et les
// réponses du tuteur ; le moteur reçoit la clé du texte (il le relit en
// base) ; préparation, accord avant le téléchargement de la voix, « Stop »,
// un seul bouton en lecture, erreurs dites à côté du bouton ; lecture
// arrêtée quand le texte quitte l'écran ; textes traduits.

import {
  act,
  configure,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { Course } from "@/bindings/Course";
import type { ReadAloudSource } from "@/bindings/ReadAloudSource";
import { resetStudyStreamsForTests } from "@/hooks/useStudy";
import { type FakeBackend, fakeBackend } from "@/test/backend";
import { untranslatedTexts, usePseudoLanguage } from "@/test/i18n";
import { renderApp } from "@/test/renderApp";
import { installSimulator } from "@/test/simulator";
import type { FakeStudy } from "@/test/study";
import { VOICE_EN } from "@/test/tts";

configure({ asyncUtilTimeout: 5000 });
vi.setConfig({ testTimeout: 30_000 });

beforeEach(() => resetStudyStreamsForTests());

/**
 * Cours « c1 » commencé (deux paliers écrits), ouvert à `position`, voix
 * anglaise installée sauf `voice: false`.
 */
function setup(
  position: string,
  {
    voice = true,
    before = () => {},
  }: {
    voice?: boolean;
    before?: (
      study: FakeStudy,
      course: Course,
      backend: FakeBackend,
    ) => Promise<void> | void;
  } = {},
) {
  const simulator = installSimulator();
  const backend = fakeBackend(simulator);
  const study = backend.study;
  const course = study.seed({ id: "c1", minutes: 10 });
  course.position = position;
  if (voice) backend.tts.install(VOICE_EN);
  return {
    simulator,
    backend,
    study,
    course,
    tts: backend.tts,
    async open() {
      await before(study, course, backend);
      const app = renderApp("/study/c1");
      await screen.findByTestId("study-path");
      return app;
    },
  };
}

const passage = (key: string): ReadAloudSource => ({
  kind: "study",
  courseId: "c1",
  passage: key,
});

/** Zone de lecture du texte `key` (bouton, textes, région des annonces). */
function zone(key: string): HTMLElement {
  const found = screen
    .getAllByTestId("study-read-aloud")
    .find((element) => element.dataset.passage === key);
  if (!found) throw new Error(`no read-aloud button for "${key}"`);
  return found;
}

const readButton = (key: string) =>
  zone(key).querySelector<HTMLButtonElement>("[data-read-aloud]")!;
const announced = (key: string) =>
  zone(key).querySelector("[data-read-aloud-status]")?.textContent;
const passages = () =>
  screen
    .getAllByTestId("study-read-aloud")
    .map((element) => element.dataset.passage);

describe("Read aloud in a lesson", () => {
  it("reads a section like a chat answer: the engine gets the key of the text, then Preparing, Stop", async () => {
    const user = userEvent.setup();
    const { simulator, tts, open } = setup("L0:s0");
    await open();
    expect(passages()).toEqual(["L0:s0"]);
    const button = readButton("L0:s0");
    expect(button.getAttribute("aria-label")).toBe("Read aloud");
    expect(button.dataset.readAloud).toBe("idle");
    // Région des annonces là avant la lecture, vide.
    expect(
      zone("L0:s0")
        .querySelector("[data-read-aloud-status]")
        ?.getAttribute("role"),
    ).toBe("status");
    expect(announced("L0:s0")).toBe("");

    await user.click(button);
    // La clé seule : le texte est relu en base par le moteur.
    expect(simulator.callsTo("read_aloud_study")).toEqual([
      { courseId: "c1", passage: "L0:s0", download: false },
    ]);
    expect(simulator.callsTo("read_aloud")).toEqual([]);
    await waitFor(() =>
      expect(readButton("L0:s0").dataset.readAloud).toBe("preparing"),
    );
    expect(
      within(zone("L0:s0")).getByTestId("read-aloud-progress").textContent,
    ).toBe("Preparing to read…");
    expect(announced("L0:s0")).toBe("Preparing to read…");
    expect(tts.state.source).toEqual(passage("L0:s0"));

    await tts.play();
    await waitFor(() =>
      expect(readButton("L0:s0").getAttribute("aria-label")).toBe(
        "Stop reading",
      ),
    );
    expect(announced("L0:s0")).toBe("Reading aloud");
    await user.click(readButton("L0:s0"));
    expect(simulator.callsTo("read_aloud_stop")).toHaveLength(1);
    await waitFor(() =>
      expect(readButton("L0:s0").getAttribute("aria-label")).toBe("Read aloud"),
    );
    expect(announced("L0:s0")).toBe("");
  });

  it("a missing voice is downloaded only with the learner's agreement", async () => {
    const user = userEvent.setup();
    const { simulator, open } = setup("L0:s1", { voice: false });
    await open();
    await user.click(readButton("L0:s1"));
    const dialog = await screen.findByRole("alertdialog");
    expect(within(dialog).getByRole("heading").textContent).toBe(
      "Download the voice?",
    );
    expect(simulator.callsTo("voice_download")).toEqual([]);
    await user.click(
      within(dialog).getByRole("button", { name: "Download and read" }),
    );
    expect(simulator.callsTo("read_aloud_study")).toEqual([
      { courseId: "c1", passage: "L0:s1", download: false },
      { courseId: "c1", passage: "L0:s1", download: true },
    ]);
    await waitFor(() =>
      expect(
        within(zone("L0:s1")).getByTestId("read-aloud-progress").textContent,
      ).toBe("Downloading the voice… 0%"),
    );
  });

  it("only the button of the text being read shows Stop", async () => {
    const { tts, open } = setup("L0:s0", {
      before: (_, course) => {
        course.levels[0].explains.push(
          { section: 1, text: "Simpler words for another section." },
          { section: 0, text: "Simpler words for section 1." },
        );
      },
    });
    await open();
    // L'explication gardée porte son rang dans le palier, pas dans la carte.
    expect(passages()).toEqual(["L0:s0", "L0:explain1"]);
    await tts.setState({ source: passage("L0:explain1"), phase: "playing" });
    await waitFor(() =>
      expect(readButton("L0:explain1").getAttribute("aria-label")).toBe(
        "Stop reading",
      ),
    );
    expect(readButton("L0:s0").getAttribute("aria-label")).toBe("Read aloud");
    expect(announced("L0:s0")).toBe("");
    // Une réponse du chat lue ailleurs, ou le même passage d'un autre
    // cours : aucun « Stop » ici.
    await tts.setState({
      source: { kind: "message", messageId: 0 },
      phase: "playing",
    });
    await waitFor(() =>
      expect(readButton("L0:explain1").getAttribute("aria-label")).toBe(
        "Read aloud",
      ),
    );
    await tts.setState({
      source: { kind: "study", courseId: "other", passage: "L0:s0" },
      phase: "idle",
      error: "engineFailed",
    });
    expect(readButton("L0:s0").getAttribute("aria-label")).toBe("Read aloud");
    expect(
      within(zone("L0:s0")).queryByTestId("read-aloud-problem"),
    ).toBeNull();
  });

  it("errors are told next to the button, in the words of a course", async () => {
    const user = userEvent.setup();
    const { tts, open } = setup("L0:s0");
    await open();
    await user.click(readButton("L0:s0"));
    await waitFor(() =>
      expect(readButton("L0:s0").dataset.readAloud).toBe("preparing"),
    );
    await tts.end("engineFailed");
    const problem = await within(zone("L0:s0")).findByTestId(
      "read-aloud-problem",
    );
    expect(problem.textContent).toBe("Couldn't read this text aloud.");
    expect(announced("L0:s0")).toBe("Couldn't read this text aloud.");
    expect(readButton("L0:s0").getAttribute("aria-describedby")).toBe(
      problem.id,
    );
  });

  it("a text with nothing to read, or in another language: the button says why", async () => {
    const user = userEvent.setup();
    const { tts, open } = setup("L0:s0", {
      before: (_, __, backend) => {
        backend.tts.passages.set("L0:s0", "empty");
        backend.tts.passages.set("L0:s1", "other");
      },
    });
    await open();
    await user.click(readButton("L0:s0"));
    await waitFor(() =>
      expect(
        within(zone("L0:s0")).getByTestId("read-aloud-problem").textContent,
      ).toBe("There's nothing to read aloud in this text."),
    );
    expect(readButton("L0:s0").getAttribute("aria-disabled")).toBe("true");
    expect(tts.state.phase).toBe("idle");
    await user.click(screen.getByRole("button", { name: "Continue" }));
    await user.click(readButton("L0:s1"));
    await waitFor(() =>
      expect(
        within(zone("L0:s1")).getByTestId("read-aloud-problem").textContent,
      ).toBe("Reading aloud is available in English, French and Spanish."),
    );
  });

  it("a refused command is shown", async () => {
    const user = userEvent.setup();
    const { tts, open } = setup("L0:s0");
    tts.failures.set("read_aloud_study", {
      kind: "internal",
      message: "The database is locked",
    });
    await open();
    await user.click(readButton("L0:s0"));
    expect(
      (await within(zone("L0:s0")).findByTestId("read-aloud-problem"))
        .textContent,
    ).toBe("The database is locked");
  });
});

describe("Read aloud stops with its text", () => {
  it("leaving the card while it is read stops the reading", async () => {
    const user = userEvent.setup();
    const { simulator, tts, open } = setup("L0:s0");
    await open();
    await user.click(readButton("L0:s0"));
    await tts.play();
    await waitFor(() =>
      expect(readButton("L0:s0").getAttribute("aria-label")).toBe(
        "Stop reading",
      ),
    );
    await user.click(screen.getByRole("button", { name: "Continue" }));
    expect(screen.getByRole("heading", { level: 3 }).textContent).toBe(
      "Heading 1.2",
    );
    expect(simulator.callsTo("read_aloud_stop")).toHaveLength(1);
    await waitFor(() => expect(tts.state.phase).toBe("idle"));
    // La carte suivante ne lit rien d'elle-même.
    expect(readButton("L0:s1").getAttribute("aria-label")).toBe("Read aloud");
    expect(simulator.callsTo("read_aloud_study")).toHaveLength(1);
  });

  it("leaving the card at rest stops nothing, nor a chat answer being read", async () => {
    const user = userEvent.setup();
    const { simulator, tts, open } = setup("L0:s0");
    await open();
    await user.click(screen.getByRole("button", { name: "Continue" }));
    await tts.setState({
      source: { kind: "message", messageId: 4 },
      phase: "playing",
    });
    await user.click(screen.getByRole("button", { name: "Continue" }));
    expect(screen.getByRole("heading", { level: 3 }).textContent).toBe(
      "Heading 1.3",
    );
    expect(simulator.callsTo("read_aloud_stop")).toEqual([]);
  });

  it("a reading that starts after its card was left is stopped", async () => {
    const user = userEvent.setup();
    const { simulator, tts, open } = setup("L0:s0");
    await open();
    await user.click(screen.getByRole("button", { name: "Continue" }));
    // Réponse du moteur arrivée après le changement de carte.
    await tts.setState({ source: passage("L0:s0"), phase: "preparing" });
    await waitFor(() =>
      expect(simulator.callsTo("read_aloud_stop")).toHaveLength(1),
    );
    // Le texte montré, lui, se lit.
    await tts.setState({ source: passage("L0:s1"), phase: "playing" });
    await waitFor(() =>
      expect(readButton("L0:s1").getAttribute("aria-label")).toBe(
        "Stop reading",
      ),
    );
    expect(simulator.callsTo("read_aloud_stop")).toHaveLength(1);
  });

  it("leaving the course page stops the reading", async () => {
    const user = userEvent.setup();
    const { simulator, tts, open } = setup("L0:s0");
    const app = await open();
    await user.click(readButton("L0:s0"));
    await tts.play();
    await act(() => app.router.navigate({ to: "/study" }));
    await screen.findByTestId("study-home");
    expect(simulator.callsTo("read_aloud_stop")).toHaveLength(1);
  });
});

describe("Read aloud on the other texts of a level", () => {
  it("worked example: the problem at once, the solution once it is all shown", async () => {
    const user = userEvent.setup();
    const { simulator, open } = setup("L0:example");
    await open();
    expect(passages()).toEqual(["L0:example"]);
    await user.click(screen.getByRole("button", { name: "Show the solution" }));
    // Une étape sur deux : rien n'est lu avant d'être montré.
    expect(passages()).toEqual(["L0:example"]);
    await user.click(
      screen.getByRole("button", { name: "Show the next step" }),
    );
    expect(passages()).toEqual(["L0:example", "L0:solution"]);
    await user.click(readButton("L0:solution"));
    expect(simulator.callsTo("read_aloud_study")).toEqual([
      { courseId: "c1", passage: "L0:solution", download: false },
    ]);
  });

  it("key points: once they are all revealed, with the common mistake", async () => {
    const user = userEvent.setup();
    const { simulator, open } = setup("L0:recap");
    await open();
    expect(screen.queryAllByTestId("study-read-aloud")).toEqual([]);
    await user.click(
      screen.getByRole("button", { name: "Reveal the next key point" }),
    );
    expect(screen.getByTestId("study-pitfall")).toBeTruthy();
    expect(passages()).toEqual(["L0:recap"]);
    await user.click(readButton("L0:recap"));
    expect(simulator.callsTo("read_aloud_study")).toEqual([
      { courseId: "c1", passage: "L0:recap", download: false },
    ]);
  });

  it("the explanation of a new attempt and the tutor's kept answers", async () => {
    const user = userEvent.setup();
    const { simulator, study, open } = setup("L1:a1:explain", {
      before: async (s, course) => {
        for (const level of [0, 1]) {
          for (const question of course.levels[level].attempts[0].questions) {
            await s.answer("c1", question.id, level === 0);
          }
        }
        await s.finishRemedy("c1", 1);
        course.levels[1].asks.push(
          { question: "Why?", answer: "Because." },
          { question: "How?", answer: "Like this." },
        );
      },
    });
    await open();
    expect(screen.getByTestId("study-remedy")).toBeTruthy();
    expect(passages()).toEqual(["L1:a1:explain"]);
    await user.click(screen.getByRole("button", { name: /Ask a question/ }));
    expect(passages()).toEqual(["L1:a1:explain", "L1:ask0", "L1:ask1"]);
    await user.click(readButton("L1:ask1"));
    expect(simulator.callsTo("read_aloud_study")).toEqual([
      { courseId: "c1", passage: "L1:ask1", download: false },
    ]);
    // Une réponse encore en flux n'a pas de bouton : il vient avec la
    // réponse enregistrée.
    await user.type(screen.getByLabelText("Your question"), "And then?{Enter}");
    await study.lastStream().step();
    expect(screen.getByTestId("study-ask-live")).toBeTruthy();
    expect(passages()).toEqual(["L1:a1:explain", "L1:ask0", "L1:ask1"]);
    await study.lastStream().finish();
    await waitFor(() =>
      expect(passages()).toEqual([
        "L1:a1:explain",
        "L1:ask0",
        "L1:ask1",
        "L1:ask2",
      ]),
    );
  });

  it("questions, results and the intro have no read-aloud button", async () => {
    const user = userEvent.setup();
    const { open } = setup("intro");
    await open();
    expect(screen.queryAllByTestId("study-read-aloud")).toEqual([]);
    await user.click(screen.getAllByTestId("study-path-node")[0]);
    for (let i = 0; i < 3; i += 1) {
      await user.click(screen.getByRole("button", { name: "Continue" }));
    }
    await user.click(screen.getByRole("button", { name: "Show everything" }));
    await user.click(screen.getByRole("button", { name: "Continue" }));
    await user.click(
      screen.getByRole("button", { name: "Reveal the next key point" }),
    );
    await user.click(screen.getByRole("button", { name: "Continue" }));
    expect(screen.getByTestId("study-question")).toBeTruthy();
    expect(screen.queryAllByTestId("study-read-aloud")).toEqual([]);
  });
});

describe("Read aloud in a lesson, in other languages", () => {
  it("has no hard-coded text: button, preparation, dialog, errors", async () => {
    const user = userEvent.setup();
    const { tts, course, open } = setup("L0:s0", { voice: false });
    await open();
    usePseudoLanguage();
    const texts = (value: unknown, out: string[] = []): string[] => {
      if (typeof value === "string") {
        if (value) out.push(value);
      } else if (value && typeof value === "object") {
        for (const item of Object.values(value)) texts(item, out);
      }
      return out;
    };
    const allow = [...texts(course), "qwen3:0.6b", "?"];
    expect(readButton("L0:s0").getAttribute("aria-label")).toBe("⟦Read aloud⟧");
    await user.click(readButton("L0:s0"));
    await screen.findByRole("alertdialog");
    expect(untranslatedTexts(document.body, allow)).toEqual([]);
    await user.click(
      within(screen.getByRole("alertdialog")).getAllByRole("button")[1],
    );
    await waitFor(() =>
      expect(readButton("L0:s0").dataset.readAloud).toBe("downloading"),
    );
    expect(untranslatedTexts(document.body, allow)).toEqual([]);
    await tts.end("engineFailed");
    await waitFor(() =>
      expect(
        within(zone("L0:s0")).getByTestId("read-aloud-problem").textContent,
      ).toBe("⟦Couldn't read this text aloud.⟧"),
    );
    expect(untranslatedTexts(document.body, allow)).toEqual([]);
  });
});
