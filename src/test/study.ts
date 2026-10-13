// Faux moteur des cours (onglet Study) pour le simulateur (tests vitest
// seulement).
//
// Cours en mémoire, statuts calculés comme le moteur (contrat 47, § 1) :
// essai réussi = toutes ses questions répondues, bonnes ≥ passMark ;
// palier `passed`, `review` (« Continue anyway »), `current` (premier ni
// l'un ni l'autre), `locked` après lui ; cours fini à la dernière réponse
// du bilan. Chaque changement émet « study-updated » (tâche suivante), sauf
// study_progress.
//
// Génération : avec `auto` (par défaut), tout est prêt aussitôt (plan à la
// création, palier 0 avec le plan, autres paliers et bilan au démarrage,
// nouvel essai à la demande) ; sans, une partie demandée attend en file
// (`pending`, comme dans le moteur) et le test la fait avancer
// (`finishPlan`, `readyLevel`, `finishRemedy`, `readyFinal`, échecs par
// `failPlan`, `failLevel`...). Contenu déterministe, comme `chatonlm-study`
// du faux serveur : la bonne option contient « Right », vrai/faux vaut vrai,
// le trou attend « right », ordre et association à remettre en place.
//
// Écriture d'un palier montrée par `writeLevel` (« generating », leçon
// absente puis enregistrée avant les questions, comme le moteur). Pause
// (study_pause) : enregistrée dans le cours, ce qui était « generating »
// redevient « pending » ; une demande qui exige une génération
// (study_remedy, study_regenerate, study_retry, study_replan,
// study_clarify) reprend le cours.
//
// Flux (study_explain, study_ask) : rien n'est livré tout seul ; le test
// les fait avancer par `step()` ou `finish()` (deux `delta`, puis `done`,
// réponse enregistrée) ; après study_stream_cancel, `finish()` livre
// `done { stopped: true }` (rien d'enregistré).

import { act } from "@testing-library/react";
import type { AnswerValue } from "@/bindings/AnswerValue";
import type { Answered } from "@/bindings/Answered";
import type { AppError } from "@/bindings/AppError";
import type { Attempt } from "@/bindings/Attempt";
import type { Course } from "@/bindings/Course";
import type { CourseSummary } from "@/bindings/CourseSummary";
import type { Lesson } from "@/bindings/Lesson";
import type { Level } from "@/bindings/Level";
import type { NewCourse } from "@/bindings/NewCourse";
import type { Plan } from "@/bindings/Plan";
import type { Question } from "@/bindings/Question";
import type { QuestionBody } from "@/bindings/QuestionBody";
import type { Settings } from "@/bindings/Settings";
import type { StagedFile } from "@/bindings/StagedFile";
import type { StudyError } from "@/bindings/StudyError";
import type { StudyStreamEvent } from "@/bindings/StudyStreamEvent";
import type { Args, FakeChannel, Simulator } from "@/test/simulator";

export type QuestionType = QuestionBody["type"];

/** Types tournés d'un cran par palier et par essai (comme MIX du moteur). */
export const MIX: readonly QuestionType[] = [
  "choice",
  "trueFalse",
  "blank",
  "order",
  "match",
];

/** Solution et explication d'une question (cachées à la page). */
interface Solution {
  expected: AnswerValue;
  explanation: string;
}

/** Réponse en flux d'un « I don't understand » ou d'un « Ask a question ». */
export interface FakeStudyStream {
  kind: "explain" | "ask";
  streamId: string;
  courseId: string;
  level: number;
  section: number | null;
  question: string | null;
  /** Évènements prévus (deux `delta`, puis `done`). */
  readonly planned: StudyStreamEvent[];
  readonly delivered: StudyStreamEvent[];
  cancelled: boolean;
  finished: boolean;
  step(count?: number): Promise<void>;
  /** Livre la fin (après Stop : `done { stopped: true }`, rien d'enregistré). */
  finish(): Promise<void>;
  /** Livre une erreur et termine. */
  fail(error: StudyError): Promise<void>;
}

export interface SeedOptions {
  id?: string;
  topic?: string;
  status?: Course["status"];
  minutes?: number;
  level?: Course["level"];
  model?: string;
  web?: boolean;
  /** Paliers prêts (tous par défaut pour un cours commencé, le 0 sinon). */
  readyLevels?: number;
  /** Bilan prêt (défaut : cours commencé et tous les paliers prêts). */
  finalReady?: boolean;
  error?: StudyError | null;
  sources?: Course["sources"];
  updatedAt?: number;
  /** Génération en pause. */
  paused?: boolean;
}

export interface FakeStudy {
  courses: Map<string, Course>;
  /** Génération immédiate (voir l'en-tête). */
  auto: boolean;
  /** Erreur renvoyée par une commande, par nom. */
  failures: Map<string, AppError>;
  /** Flux reçus, dans l'ordre. */
  streams: FakeStudyStream[];
  lastStream(): FakeStudyStream;
  /** Réponse de study_save_notes (false : boîte annulée). */
  saveResult: boolean;
  /** Prochain plan : normal, ou sujet ambigu (deux sens). */
  nextPlan: "normal" | "unclear";
  /** Cours ajouté directement dans un état donné (sans évènement). */
  seed(options?: SeedOptions): Course;
  course(id: string): Course;
  /** Solution d'une question (réponse juste). */
  solution(questionId: string): AnswerValue;
  /** Une réponse fausse à la question. */
  wrong(questionId: string): AnswerValue;
  /** Enregistre une réponse (comme study_answer) et émet. */
  answer(courseId: string, questionId: string, correct: boolean): Promise<void>;
  /** Plan prêt (cours `planning`), palier 0 prêt sauf `auto` faux. */
  finishPlan(courseId: string): Promise<void>;
  failPlan(courseId: string, error: StudyError): Promise<void>;
  readyLevel(courseId: string, level: number): Promise<void>;
  /**
   * Palier en cours d'écriture : sa leçon (`lesson`), puis ses questions
   * (leçon enregistrée).
   */
  writeLevel(
    courseId: string,
    level: number,
    stage: "lesson" | "questions",
  ): Promise<void>;
  failLevel(courseId: string, level: number, error: StudyError): Promise<void>;
  /** Nouvel essai demandé prêt. */
  finishRemedy(courseId: string, level: number): Promise<void>;
  failRemedy(courseId: string, level: number, error: StudyError): Promise<void>;
  readyFinal(courseId: string): Promise<void>;
  failFinal(courseId: string, error: StudyError): Promise<void>;
  /** Change un cours puis émet « study-updated ». */
  update(courseId: string, change: (course: Course) => void): Promise<void>;
  /** Émet « study-updated » (dans act). */
  emit(courseId: string, deleted?: boolean): Promise<void>;
}

/** Contenu d'une question d'un type, et sa solution. */
function makeQuestion(id: string, type: QuestionType): [Question, Solution] {
  const explanation = `Because of ${id}.`;
  switch (type) {
    case "choice":
      return [
        {
          id,
          body: {
            type,
            prompt: `Which one is right (${id})?`,
            options: ["Wrong first", "Right answer", "Wrong last"],
          },
        },
        { expected: { type, index: 1 }, explanation },
      ];
    case "trueFalse":
      return [
        { id, body: { type, statement: `Statement ${id} holds.` } },
        { expected: { type, value: true }, explanation },
      ];
    case "blank":
      return [
        { id, body: { type, prompt: `The answer of ${id} is ___ here.` } },
        { expected: { type, text: "right" }, explanation },
      ];
    case "order":
      return [
        {
          id,
          body: {
            type,
            prompt: `Put the steps of ${id} in order.`,
            items: ["Step two", "Step one", "Step three"],
          },
        },
        { expected: { type, order: [1, 0, 2] }, explanation },
      ];
    case "match":
      return [
        {
          id,
          body: {
            type,
            prompt: `Match the pairs of ${id}.`,
            left: ["Cat", "Dog", "Cow"],
            right: ["Woof", "Moo", "Meow"],
          },
        },
        { expected: { type, pairs: [2, 0, 1] }, explanation },
      ];
  }
}

/** Plan déterministe de `count` paliers. */
function makePlan(topic: string, count: number, version: number): Plan {
  const title = topic ? `Course on ${topic}` : "Course on your documents";
  return {
    title: version > 1 ? `${title} (plan ${version})` : title,
    kind: "concept",
    summary: `A short course about ${topic || "your documents"}.`,
    objectives: ["Know the basics", "Use the main idea", "Avoid the trap"],
    levels: Array.from({ length: count }, (_, i) => ({
      title: `Level title ${i + 1}`,
      goal: `Goal of level ${i + 1}`,
      points: [`Point ${i + 1}a`, `Point ${i + 1}b`],
    })),
    scope: "",
    further: ["Next topic A", "Next topic B", "Next topic C"],
  };
}

function makeLesson(i: number): Lesson {
  return {
    hook: `Did you know this about level ${i + 1}?`,
    sections: [0, 1, 2].map((k) => ({
      heading: `Heading ${i + 1}.${k + 1}`,
      body: `Body of section ${i + 1}.${k + 1}.`,
    })),
    example: {
      problem: `Problem of level ${i + 1}.`,
      solution: `First step ${i + 1}.\n\nSecond step ${i + 1}.`,
    },
    keyPoints: [`Key point ${i + 1}a`, `Key point ${i + 1}b`],
    pitfall: `Pitfall of level ${i + 1}.`,
  };
}

function emptyLevel(index: number): Level {
  return {
    index,
    state: "pending",
    error: null,
    lesson: null,
    check: null,
    attempts: [],
    remedy: null,
    remedyError: null,
    status: "locked",
    failedAttempts: 0,
    explains: [],
    asks: [],
    estimatedSeconds: 0,
  };
}

function summaryOf(course: Course): CourseSummary {
  const done = course.levels.filter(
    (level) => level.status === "passed" || level.status === "review",
  ).length;
  const finished = course.status === "finished";
  const final = course.finalQuiz?.questions ?? [];
  return {
    id: course.id,
    title: course.plan?.title ?? course.topic,
    topic: course.topic,
    level: course.level,
    minutes: course.minutes,
    status: course.status,
    levelsDone: done,
    levelsTotal: course.plan?.levels.length ?? 0,
    levelsReady: course.levels.filter((level) => level.state === "ready")
      .length,
    pending: pendingOf(course),
    paused: course.paused,
    finalCorrect: finished
      ? final.filter((q) => course.answers[q.id]?.correct).length
      : null,
    finalTotal: finished ? final.length : null,
    createdAt: course.createdAt,
    updatedAt: course.updatedAt,
    finishedAt: course.finishedAt,
  };
}

const unwritten = (state: Level["state"] | null | undefined) =>
  state === "pending" || state === "generating";

/** Des parties restent à écrire (comme `study::data::pending` du moteur). */
function pendingOf(course: Course): boolean {
  const levels = course.levels;
  switch (course.status) {
    case "planning":
      return true;
    case "planned":
      return unwritten(levels[0]?.state);
    case "active":
    case "finished":
      return (
        levels.some(
          (level) => unwritten(level.state) || unwritten(level.remedy),
        ) ||
        (!levels.some((level) => level.state === "failed") &&
          unwritten(course.finalQuiz?.state))
      );
    default:
      return false;
  }
}

function invalid(message: string): AppError {
  return { kind: "invalid", message };
}

const sameValue = (a: AnswerValue, b: AnswerValue) =>
  JSON.stringify(a) === JSON.stringify(b);

/** Correction simple (texte : casse et espaces ; une faute de frappe : close). */
function grade(given: AnswerValue, solution: Solution): Answered {
  const expected = solution.expected;
  let correct: boolean;
  let close = false;
  let detail: boolean[] = [];
  if (given.type === "blank" && expected.type === "blank") {
    const a = given.text.trim().toLowerCase();
    const b = expected.text.toLowerCase();
    correct = a === b;
    if (!correct && a.length === b.length) {
      const diff = [...a].filter((c, i) => c !== b[i]).length;
      if (diff === 1 || [...a].sort().join("") === [...b].sort().join("")) {
        correct = true;
        close = true;
      }
    }
  } else if (given.type === "order" && expected.type === "order") {
    detail = expected.order.map((item, i) => given.order[i] === item);
    correct = detail.every(Boolean);
  } else if (given.type === "match" && expected.type === "match") {
    detail = expected.pairs.map((pair, i) => given.pairs[i] === pair);
    correct = detail.every(Boolean);
  } else {
    correct = sameValue(given, expected);
  }
  return {
    given,
    correct,
    close,
    expected,
    detail,
    explanation: solution.explanation,
    at: Date.now(),
  };
}

/** Gestionnaires des commandes study_*. */
export function fakeStudy(
  simulator: Simulator,
  backend: { settings: Settings; staged: Map<string, StagedFile> },
): FakeStudy {
  const solutions = new Map<string, Solution>();
  /** Paliers passés par « Continue anyway », par cours. */
  const skipped = new Map<string, Set<number>>();
  const planVersions = new Map<string, number>();
  let lastId = 0;
  let clock = 1_700_000_000_000;
  const now = () => (clock += 1000);

  const fake: FakeStudy = {
    courses: new Map(),
    auto: true,
    failures: new Map(),
    streams: [],
    lastStream() {
      const stream = fake.streams.at(-1);
      if (!stream) throw simulator.fail("no study stream received");
      return stream;
    },
    saveResult: true,
    nextPlan: "normal",
    seed(options = {}) {
      lastId += 1;
      const id = options.id ?? `course-${lastId}`;
      const minutes = options.minutes ?? 20;
      const status = options.status ?? "active";
      const at = options.updatedAt ?? now();
      const course: Course = {
        id,
        topic: options.topic ?? "Photosynthesis",
        level: options.level ?? "beginner",
        minutes,
        language: "en",
        model: options.model ?? "qwen3:0.6b",
        web: options.web ?? false,
        status: "planning",
        error: null,
        clarify: null,
        plan: null,
        sources: options.sources ?? [],
        levels: [],
        finalQuiz: null,
        answers: {},
        position: "",
        elapsedMs: 0,
        estimatedSeconds: minutes * 60,
        createdAt: at,
        updatedAt: at,
        finishedAt: null,
        paused: options.paused ?? false,
        queueBusy: false,
      };
      fake.courses.set(id, course);
      if (status === "planning") return course;
      if (status === "unclear") {
        course.status = "unclear";
        course.clarify = {
          question: "Which meaning do you want?",
          options: ["The planet", "The metal"],
        };
        return course;
      }
      if (status === "failed") {
        course.status = "failed";
        course.error = options.error ?? {
          code: "failed",
          message: "The plan could not be written.",
        };
        return course;
      }
      writePlan(course);
      const started = status === "active" || status === "finished";
      const ready = options.readyLevels ?? (started ? course.levels.length : 1);
      for (let i = 0; i < ready; i += 1) writeLevel(course, i);
      if (started) {
        course.status = "active";
        if (options.finalReady ?? ready === course.levels.length) {
          writeFinal(course);
        }
      }
      if (status === "finished") {
        for (const level of course.levels) {
          for (const question of level.attempts[0].questions) {
            record(course, question.id, fake.solution(question.id));
          }
        }
        for (const question of course.finalQuiz?.questions ?? []) {
          record(course, question.id, fake.solution(question.id));
        }
      }
      refresh(course);
      course.updatedAt = at;
      return course;
    },
    course(id) {
      const course = fake.courses.get(id);
      if (!course) throw simulator.fail(`unknown course ${id}`);
      return course;
    },
    solution(questionId) {
      const solution = solutions.get(questionId);
      if (!solution) throw simulator.fail(`unknown question ${questionId}`);
      return solution.expected;
    },
    wrong(questionId) {
      const expected = fake.solution(questionId);
      switch (expected.type) {
        case "choice":
          return { type: "choice", index: expected.index === 0 ? 1 : 0 };
        case "trueFalse":
          return { type: "trueFalse", value: !expected.value };
        case "blank":
          return { type: "blank", text: "nothing like it" };
        case "order":
          return { type: "order", order: [...expected.order].reverse() };
        case "match":
          return {
            type: "match",
            pairs: expected.pairs.map((_, i) => expected.pairs[(i + 1) % 3]),
          };
      }
    },
    async answer(courseId, questionId, correct) {
      const course = fake.course(courseId);
      record(
        course,
        questionId,
        correct ? fake.solution(questionId) : fake.wrong(questionId),
      );
      refresh(course);
      await fake.emit(courseId);
    },
    async finishPlan(courseId) {
      const course = fake.course(courseId);
      writePlan(course);
      writeLevel(course, 0);
      refresh(course);
      await fake.emit(courseId);
    },
    async failPlan(courseId, error) {
      await fake.update(courseId, (course) => {
        course.status = "failed";
        course.error = error;
      });
    },
    async readyLevel(courseId, level) {
      await fake.update(courseId, (course) => writeLevel(course, level));
    },
    async writeLevel(courseId, level, stage) {
      await fake.update(courseId, (course) => {
        course.levels[level] = {
          ...emptyLevel(level),
          status: course.levels[level].status,
          state: "generating",
          lesson: stage === "questions" ? makeLesson(level) : null,
        };
      });
    },
    async failLevel(courseId, level, error) {
      await fake.update(courseId, (course) => {
        course.levels[level].state = "failed";
        course.levels[level].error = error;
      });
    },
    async finishRemedy(courseId, level) {
      await fake.update(courseId, (course) => addAttempt(course, level));
    },
    async failRemedy(courseId, level, error) {
      await fake.update(courseId, (course) => {
        course.levels[level].remedy = "failed";
        course.levels[level].remedyError = error;
      });
    },
    async readyFinal(courseId) {
      await fake.update(courseId, writeFinal);
    },
    async failFinal(courseId, error) {
      await fake.update(courseId, (course) => {
        course.finalQuiz = {
          state: "failed",
          error,
          questions: [],
          estimatedSeconds: 0,
        };
      });
    },
    async update(courseId, change) {
      const course = fake.course(courseId);
      change(course);
      refresh(course);
      course.updatedAt = now();
      await fake.emit(courseId);
    },
    async emit(courseId, deleted = false) {
      await simulator.emitEvent("study-updated", { courseId, deleted });
    },
  };

  /** Émis par le moteur après la réponse de la commande. */
  function emitLater(courseId: string, deleted = false) {
    setTimeout(() => {
      if (!simulator.disposed) void fake.emit(courseId, deleted);
    }, 0);
  }

  function failure(command: string) {
    const error = fake.failures.get(command);
    if (error) throw error;
  }

  function writePlan(course: Course) {
    const version = (planVersions.get(course.id) ?? 0) + 1;
    planVersions.set(course.id, version);
    const count = course.minutes <= 10 ? 2 : course.minutes <= 20 ? 4 : 6;
    course.plan = makePlan(course.topic, count, version);
    course.status = "planned";
    course.error = null;
    course.clarify = null;
    course.levels = Array.from({ length: count }, (_, i) => emptyLevel(i));
    // Comme le moteur : le bilan attend dès que le plan est prêt.
    course.finalQuiz = {
      state: "pending",
      error: null,
      questions: [],
      estimatedSeconds: 0,
    };
    if (course.web) {
      course.sources = [
        ...course.sources.filter((source) => source.kind !== "web"),
        { kind: "web", title: "A web page", url: "https://example.com/page" },
      ];
    }
  }

  function question(id: string, type: QuestionType): Question {
    const [made, solution] = makeQuestion(id, type);
    solutions.set(id, solution);
    return made;
  }

  function attemptOf(level: number, j: number): Attempt {
    return {
      explanation: j === 0 ? null : `Another way to see level ${level + 1}.`,
      questions: [0, 1, 2].map((n) =>
        question(`L${level}-A${j}-Q${n}`, MIX[(level + j + n) % MIX.length]),
      ),
      passMark: 2,
    };
  }

  function writeLevel(course: Course, i: number) {
    const level = course.levels[i];
    level.state = "ready";
    level.error = null;
    level.lesson = makeLesson(i);
    level.check = question(`L${i}-check`, "choice");
    level.attempts = [attemptOf(i, 0)];
    level.remedy = null;
    level.remedyError = null;
    level.estimatedSeconds = 180;
  }

  function addAttempt(course: Course, i: number) {
    const level = course.levels[i];
    level.attempts.push(attemptOf(i, level.attempts.length));
    level.remedy = null;
    level.remedyError = null;
  }

  function writeFinal(course: Course) {
    const count = course.minutes <= 10 ? 3 : course.minutes <= 20 ? 5 : 6;
    const types: QuestionType[] = [
      "choice",
      "trueFalse",
      "blank",
      "choice",
      "trueFalse",
      "choice",
    ];
    course.finalQuiz = {
      state: "ready",
      error: null,
      questions: types
        .slice(0, count)
        .map((type, n) => question(`F-Q${n}`, type)),
      estimatedSeconds: count * 25,
    };
  }

  function record(course: Course, questionId: string, given: AnswerValue) {
    const solution = solutions.get(questionId);
    if (!solution) throw invalid(`unknown question ${questionId}`);
    course.answers[questionId] ??= grade(given, solution);
    return course.answers[questionId];
  }

  /** Statuts des paliers et fin du cours, comme le moteur. */
  function refresh(course: Course) {
    const skip = skipped.get(course.id) ?? new Set<number>();
    let currentFound = false;
    for (const level of course.levels) {
      let passed = false;
      let failed = 0;
      for (const attempt of level.attempts) {
        const answers = attempt.questions.map((q) => course.answers[q.id]);
        if (answers.some((answer) => !answer)) continue;
        const correct = answers.filter((answer) => answer.correct).length;
        if (correct >= attempt.passMark) passed = true;
        else failed += 1;
      }
      level.failedAttempts = failed;
      if (passed) level.status = "passed";
      else if (skip.has(level.index)) level.status = "review";
      else if (!currentFound) {
        level.status = "current";
        currentFound = true;
      } else level.status = "locked";
    }
    const done = course.levels.every(
      (level) => level.status === "passed" || level.status === "review",
    );
    const final = course.finalQuiz;
    if (
      course.status === "active" &&
      done &&
      final?.state === "ready" &&
      final.questions.every((q) => course.answers[q.id])
    ) {
      course.status = "finished";
      course.finishedAt = now();
    }
  }

  /** Génération au démarrage (auto). */
  function generateAll(course: Course) {
    course.levels.forEach((level, i) => {
      if (level.state !== "ready") writeLevel(course, i);
    });
    writeFinal(course);
  }

  simulator.handle("study_list", () => {
    failure("study_list");
    return [...fake.courses.values()]
      .sort((a, b) => b.updatedAt - a.updatedAt)
      .map(summaryOf);
  });

  simulator.handle("study_get", (args: Args) => {
    failure("study_get");
    const course = fake.courses.get(args.courseId as string);
    if (!course) throw { kind: "notFound", message: "course not found" };
    return course;
  });

  simulator.handle("study_create", (args: Args) => {
    failure("study_create");
    const request = args.course as NewCourse;
    const topic = request.topic.trim();
    if (!topic && request.documentIds.length === 0) {
      throw invalid("Type a topic or add a document");
    }
    if ([...topic].length > 300) throw invalid("The topic is too long");
    if (![10, 20, 30].includes(request.minutes)) {
      throw invalid("unsupported duration");
    }
    if (request.documentIds.length > 3) throw invalid("Too many documents");
    const documents = request.documentIds.map((id) => {
      const file = backend.staged.get(id);
      if (!file) throw invalid(`unknown document ${id}`);
      if (file.kind === "image") throw invalid("Images can't be used");
      return file;
    });
    if (!(backend.settings.studyModel || backend.settings.selectedModel)) {
      throw invalid("Choose a model for your courses first");
    }
    for (const file of documents) backend.staged.delete(file.id);
    const course = fake.seed({
      topic,
      status: "planning",
      minutes: request.minutes,
      level: request.level,
      web: request.web,
      model: backend.settings.studyModel || backend.settings.selectedModel,
      sources: documents.map((file) => ({
        kind: "document",
        title: file.filename,
        url: "",
      })),
    });
    if (fake.auto) {
      if (fake.nextPlan === "unclear") {
        course.status = "unclear";
        course.clarify = {
          question: "Which meaning do you want?",
          options: ["The planet", "The metal"],
        };
      } else {
        writePlan(course);
        writeLevel(course, 0);
        refresh(course);
      }
    }
    emitLater(course.id);
    return course.id;
  });

  /** Cours d'une commande ; `NotFound` s'il n'existe pas. */
  function target(args: Args): Course {
    const course = fake.courses.get(args.courseId as string);
    if (!course) throw { kind: "notFound", message: "course not found" };
    return course;
  }

  /** Change un cours (date d'activité comprise) puis émet. */
  function change(course: Course, apply: () => void) {
    apply();
    refresh(course);
    course.updatedAt = now();
    emitLater(course.id);
    return null;
  }

  simulator.handle("study_clarify", (args: Args) => {
    failure("study_clarify");
    const course = target(args);
    if (course.status !== "unclear") throw invalid("not unclear");
    const choice = args.choice as string | null;
    return change(course, () => {
      if (choice) course.topic = `${course.topic} (${choice})`;
      course.clarify = null;
      course.status = "planning";
      course.paused = false;
      if (fake.auto) {
        writePlan(course);
        writeLevel(course, 0);
      }
    });
  });

  simulator.handle("study_replan", (args: Args) => {
    failure("study_replan");
    const course = target(args);
    if (!["planned", "unclear", "failed"].includes(course.status)) {
      throw invalid("the course has started");
    }
    return change(course, () => {
      course.status = "planning";
      course.plan = null;
      course.levels = [];
      course.paused = false;
      if (fake.auto) {
        writePlan(course);
        writeLevel(course, 0);
      }
    });
  });

  simulator.handle("study_start", (args: Args) => {
    failure("study_start");
    const course = target(args);
    if (course.status !== "planned") throw invalid("not planned");
    return change(course, () => {
      course.status = "active";
      if (fake.auto) generateAll(course);
    });
  });

  simulator.handle("study_pause", (args: Args) => {
    failure("study_pause");
    const course = target(args);
    const paused = args.paused as boolean;
    if (typeof paused !== "boolean") {
      throw simulator.fail(
        `study_pause: bad arguments ${JSON.stringify(args)}`,
      );
    }
    return change(course, () => {
      course.paused = paused;
      if (!paused) return;
      // La partie en cours est arrêtée : elle sera refaite à la reprise.
      for (const level of course.levels) {
        if (level.state === "generating") level.state = "pending";
        if (level.remedy === "generating") level.remedy = "pending";
      }
      if (course.finalQuiz?.state === "generating") {
        course.finalQuiz = { ...course.finalQuiz, state: "pending" };
      }
    });
  });

  simulator.handle("study_answer", (args: Args) => {
    failure("study_answer");
    const course = target(args);
    const questionId = args.questionId as string;
    const levelMatch = /^L(\d+)-/.exec(questionId);
    if (levelMatch) {
      const level = course.levels[Number(levelMatch[1])];
      if (!level || level.status === "locked") throw invalid("level locked");
    } else if (
      !course.levels.every(
        (level) => level.status === "passed" || level.status === "review",
      )
    ) {
      throw invalid("final review locked");
    }
    const answered = record(course, questionId, args.answer as AnswerValue);
    change(course, () => {});
    return answered;
  });

  simulator.handle("study_progress", (args: Args) => {
    failure("study_progress");
    const course = target(args);
    course.position = args.position as string;
    course.elapsedMs = args.elapsedMs as number;
    return null;
  });

  simulator.handle("study_remedy", (args: Args) => {
    failure("study_remedy");
    const course = target(args);
    const i = args.level as number;
    const level = course.levels[i];
    const last = level?.attempts.length ? level.attempts.length - 1 : -1;
    const lastAttempt = level?.attempts[last];
    const answers = lastAttempt?.questions.map((q) => course.answers[q.id]);
    const failedLast =
      lastAttempt &&
      answers?.every(Boolean) &&
      answers.filter((answer) => answer?.correct).length < lastAttempt.passMark;
    // Comme le moteur : demande en file (« pending ») puis écrite.
    if (
      !failedLast ||
      level.remedy === "pending" ||
      level.remedy === "generating"
    ) {
      throw invalid("no failed attempt");
    }
    return change(course, () => {
      course.paused = false;
      if (fake.auto) addAttempt(course, i);
      else level.remedy = "pending";
    });
  });

  simulator.handle("study_skip", (args: Args) => {
    failure("study_skip");
    const course = target(args);
    const i = args.level as number;
    const level = course.levels[i];
    if (
      !level ||
      level.failedAttempts < 2 ||
      level.status === "passed" ||
      level.status === "review"
    ) {
      throw invalid("cannot skip");
    }
    return change(course, () => {
      const set = skipped.get(course.id) ?? new Set<number>();
      set.add(i);
      skipped.set(course.id, set);
    });
  });

  simulator.handle("study_regenerate", (args: Args) => {
    failure("study_regenerate");
    const course = target(args);
    const i = args.level as number;
    const level = course.levels[i];
    if (!level) throw invalid("unknown level");
    return change(course, () => {
      for (const id of Object.keys(course.answers)) {
        if (id.startsWith(`L${i}-`)) delete course.answers[id];
      }
      skipped.get(course.id)?.delete(i);
      course.levels[i] = emptyLevel(i);
      course.paused = false;
      if (fake.auto) writeLevel(course, i);
    });
  });

  simulator.handle("study_retry", (args: Args) => {
    failure("study_retry");
    const course = target(args);
    return change(course, () => {
      course.paused = false;
      if (args.withoutWeb) {
        course.web = false;
        course.sources = course.sources.filter((s) => s.kind !== "web");
      }
      if (course.status === "failed") {
        course.status = "planning";
        course.error = null;
        if (fake.auto) {
          writePlan(course);
          writeLevel(course, 0);
        }
      }
      course.levels.forEach((level, i) => {
        if (level.state === "failed") {
          level.state = "pending";
          level.error = null;
          if (fake.auto) writeLevel(course, i);
        }
        if (level.remedy === "failed") {
          level.remedy = "pending";
          level.remedyError = null;
          if (fake.auto) addAttempt(course, i);
        }
      });
      if (course.finalQuiz?.state === "failed") {
        course.finalQuiz = {
          ...course.finalQuiz,
          state: "pending",
          error: null,
        };
        if (fake.auto) writeFinal(course);
      }
    });
  });

  function startStream(args: Args, kind: "explain" | "ask") {
    const course = target(args);
    const level = course.levels[args.level as number];
    if (!level?.lesson) throw invalid("level not ready");
    const questionText = kind === "ask" ? (args.question as string) : null;
    if (
      kind === "ask" &&
      (!questionText?.trim() || questionText.length > 1000)
    ) {
      throw invalid("Type a question");
    }
    const channel: FakeChannel = simulator.channel(args.onEvent);
    const section = kind === "explain" ? (args.section as number) : null;
    const text =
      kind === "explain"
        ? `Simpler words for section ${(section ?? 0) + 1}.`
        : `Answer to: ${questionText}`;
    const half = Math.ceil(text.length / 2);
    const planned: StudyStreamEvent[] = [
      { event: "delta", text: text.slice(0, half) },
      { event: "delta", text: text.slice(half) },
      { event: "done", text, stopped: false },
    ];
    let resolve: () => void = () => {};
    const answered = new Promise<null>((ok) => {
      resolve = () => ok(null);
    });
    const stream: FakeStudyStream = {
      kind,
      streamId: args.streamId as string,
      courseId: course.id,
      level: level.index,
      section,
      question: questionText,
      planned,
      delivered: [],
      cancelled: false,
      finished: false,
      async step(count = 1) {
        for (let i = 0; i < count; i += 1) {
          const event = planned[stream.delivered.length];
          if (!event || stream.finished) {
            throw simulator.fail(
              `study stream ${stream.streamId}: no event left`,
            );
          }
          await deliver(event);
        }
      },
      async finish() {
        if (stream.finished) return;
        if (stream.cancelled) {
          const partial = stream.delivered
            .map((event) => (event.event === "delta" ? event.text : ""))
            .join("");
          await deliver({ event: "done", text: partial, stopped: true });
          return;
        }
        while (!stream.finished)
          await deliver(planned[stream.delivered.length]);
      },
      async fail(error) {
        await deliver({
          event: "error",
          code: error.code,
          message: error.message,
        });
      },
    };
    async function deliver(event: StudyStreamEvent) {
      stream.delivered.push(event);
      await channel.send(event);
      if (event.event === "done" && !event.stopped) {
        if (kind === "explain") {
          level.explains.push({ section: section ?? 0, text: event.text });
        } else {
          level.asks.push({ question: questionText ?? "", answer: event.text });
        }
        course.updatedAt = now();
        await fake.emit(course.id);
      }
      if (event.event === "done" || event.event === "error") {
        stream.finished = true;
        await act(async () => resolve());
      }
    }
    fake.streams.push(stream);
    return answered;
  }

  simulator.handle("study_explain", (args: Args) => {
    failure("study_explain");
    return startStream(args, "explain");
  });
  simulator.handle("study_ask", (args: Args) => {
    failure("study_ask");
    return startStream(args, "ask");
  });
  simulator.handle("study_stream_cancel", (args: Args) => {
    failure("study_stream_cancel");
    const stream = fake.streams.find((s) => s.streamId === args.streamId);
    if (stream && !stream.finished) stream.cancelled = true;
    return null;
  });

  simulator.handle("study_notes", (args: Args) => {
    failure("study_notes");
    const course = target(args);
    const points = course.levels.flatMap(
      (level) => level.lesson?.keyPoints ?? [],
    );
    return `# ${course.plan?.title ?? course.topic}\n\n${points
      .map((point) => `- ${point}`)
      .join("\n")}\n`;
  });
  simulator.handle("study_save_notes", (args: Args) => {
    failure("study_save_notes");
    target(args);
    return fake.saveResult;
  });

  simulator.handle("study_delete", (args: Args) => {
    failure("study_delete");
    const id = args.courseId as string;
    fake.courses.delete(id);
    emitLater(id, true);
    return null;
  });

  return fake;
}

/**
 * Faux moteur seul, sans simulateur ni évènements (essais des fonctions
 * pures de src/lib/study.ts) : cours fabriqués par `seed`, réponses par
 * `answer`.
 */
export function standaloneStudy(): FakeStudy {
  const stub = {
    handle() {},
    emitEvent: async () => {},
    fail: (problem: string) => new Error(problem),
    disposed: false,
  } as unknown as Simulator;
  return fakeStudy(stub, {
    settings: { studyModel: "", selectedModel: "qwen3:0.6b" } as Settings,
    staged: new Map(),
  });
}
