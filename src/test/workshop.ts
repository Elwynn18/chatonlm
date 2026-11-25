// Faux moteur de l'Atelier (moteur de tâches longues) pour le simulateur
// (tests vitest seulement), installé par fakeBackend (backend.workshop).
//
// Projets en mémoire, comme le moteur (`workshop::service`) : liste du plus
// récent au plus ancien avec le rang dans la file, projet complet (question
// en attente, dernier numéro d'événement, rapport), journal numéroté lu après
// un numéro, parties (`workshop_parts`, `workshop_part`), spécification
// (`workshop_spec`), ouverture du livrable (refusée avant la livraison),
// réglages (dossier accepté seulement s'il vient du sélecteur), vérification
// du modèle. Chaque
// changement émet « workshop-event » (`WorkshopNotice`) à la tâche suivante,
// comme le moteur après sa réponse.
//
// Le test fait avancer un projet par `emit` (événement), `update` (état,
// phase…), `ask` (question en attente), `setParts`, `finish` (rapport) ; ou
// par le projet de démonstration (`demo`, puis `advance` : une étape du
// déroulé à chaque appel, de la collecte au rapport, avec une question du
// plan à approuver au milieu). Les réponses (`workshop_answer`) sont vérifiées
// (question en attente, type de la réponse) comme le moteur, puis la
// question est retirée et le projet repart.
// Pour les tests seulement.

import type { AppError } from "@/bindings/AppError";
import type { Check } from "@/bindings/Check";
import type { ModelCheck } from "@/bindings/ModelCheck";
import type { NewProjectRequest } from "@/bindings/NewProjectRequest";
import type { PendingQuestion } from "@/bindings/PendingQuestion";
import type { ProjectDetail } from "@/bindings/ProjectDetail";
import type { ProjectState } from "@/bindings/ProjectState";
import type { ProjectSummary } from "@/bindings/ProjectSummary";
import type { Report } from "@/bindings/Report";
import type { StagedFile } from "@/bindings/StagedFile";
import type { WorkshopAnswer } from "@/bindings/WorkshopAnswer";
import type { WorkshopEvent } from "@/bindings/WorkshopEvent";
import type { WorkshopEventKind } from "@/bindings/WorkshopEventKind";
import type { WorkshopNotice } from "@/bindings/WorkshopNotice";
import type { WorkshopPart } from "@/bindings/WorkshopPart";
import type { WorkshopPartSummary } from "@/bindings/WorkshopPartSummary";
import type { WorkshopPlan } from "@/bindings/WorkshopPlan";
import type { WorkshopQuestion } from "@/bindings/WorkshopQuestion";
import type { WorkshopSettings } from "@/bindings/WorkshopSettings";
import type { WorkshopSettingsPatch } from "@/bindings/WorkshopSettingsPatch";
import type { WorkshopSpec } from "@/bindings/WorkshopSpec";
import {
  MAX_ATTACHMENTS,
  MAX_TASK_CHARS,
  proposedFolderName,
  strategyAvailable,
  titleOf,
  folderNameOf,
} from "@/lib/workshop";
import type { Args, Simulator } from "@/test/simulator";

/** Dossier des projets par défaut du faux moteur. */
export const DEFAULT_DIR = "/home/user/Documents/ChatonLM";
/** Heure du premier événement d'un projet semé (ms). */
export const WORKSHOP_TIME = 1_700_000_000_000;

/** Un projet du faux moteur. */
export interface FakeProject {
  summary: ProjectSummary;
  task: string;
  attachments: string[];
  question: PendingQuestion | null;
  report: Report | null;
  events: WorkshopEvent[];
  parts: WorkshopPart[];
  /** Spécification retenue (workshop_spec), null avant. */
  spec: WorkshopSpec | null;
}

export interface FakeWorkshop {
  projects: Map<number, FakeProject>;
  settings: WorkshopSettings;
  /** Dossier rendu par workshop_pick_dir (null : annulé). */
  pickedDir: string | null;
  /** Réponse de workshop_model_check, selon le modèle demandé ("" : Auto). */
  modelCheck: (model: string) => ModelCheck;
  /** Erreur rendue par une commande, par nom. */
  failures: Map<string, AppError>;
  /** Projet ajouté (en file par défaut), sans avis. */
  seed(patch?: Partial<ProjectSummary> & { task?: string }): FakeProject;
  project(id: number): FakeProject;
  /** Ajoute un événement au journal et prévient la page. */
  emit(id: number, kind: WorkshopEventKind, atMs?: number): Promise<void>;
  /** Change le résumé (état, phase…) et prévient la page. */
  update(id: number, patch: Partial<ProjectSummary>): Promise<void>;
  /** Pose une question (état « waiting ») et prévient la page. */
  ask(id: number, question: WorkshopQuestion): Promise<void>;
  /** Remplace les parties et prévient la page. */
  setParts(id: number, parts: WorkshopPart[]): Promise<void>;
  /** Livre le projet avec ce rapport et prévient la page. */
  finish(id: number, report: Report): Promise<void>;
  /** Projet de démonstration (« Guide de la permaculture »), en file. */
  demo(): FakeProject;
  /** Étape suivante du projet de démonstration ; faux à la fin. */
  advance(id: number): Promise<boolean>;
  /**
   * Retient la prochaine réponse de `command` (workshop_get, workshop_events,
   * workshop_parts, workshop_set_settings, workshop_pause) : ses données sont lues à l'appel, la réponse part quand
   * la fonction rendue est appelée (une lecture partie avant un avis).
   */
  hold(command: string): () => void;
  /** Prévient la page (avis du moteur). */
  notify(
    notice: Partial<WorkshopNotice> & { projectId: number },
  ): Promise<void>;
}

/** Une vérification. */
export function check(
  criterion: string,
  passed: boolean,
  patch: Partial<Check> = {},
): Check {
  return {
    criterion,
    method: "program",
    passed,
    evidence: "",
    where: null,
    blocking: !passed,
    ...patch,
  };
}

/** Une partie (pas commencée par défaut). */
export function part(
  id: number,
  patch: Partial<WorkshopPart> = {},
): WorkshopPart {
  return {
    id,
    parent: id >= 100 ? Math.floor(id / 100) : null,
    title: `Part ${id}`,
    kind: "text",
    targetWords: 500,
    state: "pending",
    attempt: 0,
    words: 0,
    text: "",
    truncated: false,
    checks: [],
    ...patch,
  };
}

/** Plan de démonstration : intro, trois parties, conclusion. */
export const DEMO_PLAN: WorkshopPlan = {
  title: "Urban permaculture",
  bricks: [
    {
      id: 1,
      title: "Introduction",
      goal: "Say what the guide covers",
      kind: "intro",
      words: 300,
      dependsOn: [],
      criteria: [],
      forbidden: [],
    },
    {
      id: 2,
      title: "The soil",
      goal: "Explain how to build a living soil",
      kind: "text",
      words: 800,
      dependsOn: [],
      criteria: ["C1"],
      forbidden: [],
    },
    {
      id: 3,
      title: "Water",
      goal: "Explain how to save water",
      kind: "text",
      words: 800,
      dependsOn: [],
      criteria: ["C2"],
      forbidden: [],
    },
    {
      id: 4,
      title: "Compost",
      goal: "Explain how to compost on a balcony",
      kind: "text",
      words: 800,
      dependsOn: [2],
      criteria: ["C3"],
      forbidden: [],
    },
    {
      id: 5,
      title: "Conclusion",
      goal: "Sum up",
      kind: "conclusion",
      words: 300,
      dependsOn: [],
      criteria: [],
      forbidden: [],
    },
  ],
};

/** Spécification de démonstration. */
export const DEMO_SPEC: WorkshopSpec = {
  goal: "A guide to urban permaculture for beginners",
  deliverables: ["guide"],
  criteria: [
    { id: "C1", text: "Explains how to build a living soil", scope: "section" },
    { id: "C2", text: "Gives 3 ways to save water", scope: "section" },
    { id: "C3", text: "Cites 2 sources on compost", scope: "section" },
  ],
  audience: "beginners",
  language: "en",
  register: "informal",
  units: "metric",
  lengthWords: 3000,
  glossary: [],
  assumptions: [],
  constraints: [],
};

/** Rapport de démonstration. */
export const DEMO_REPORT: Report = {
  strategy: "longForm",
  route: "parts",
  file: "urban-permaculture.md",
  words: 3020,
  targetWords: 3000,
  checks: [
    check("C1", true, { method: "formJudge", evidence: "The soil section" }),
    check("length", true, { evidence: "3020/2700-3300 words" }),
  ],
  judgedOnOutline: false,
  bricks: [
    {
      id: 2,
      title: "The soil",
      state: "done",
      fixed: false,
      words: 810,
      checks: [check("C1", true, { method: "formJudge" })],
    },
    {
      id: 4,
      title: "Compost",
      state: "toReview",
      fixed: true,
      words: 790,
      checks: [
        check("200 g of manure per square metre", false, {
          method: "factJudge",
          blocking: true,
        }),
        check("Worms like coffee grounds", false, {
          method: "factJudge",
          blocking: false,
        }),
        check("engineWord", false, { blocking: false }),
      ],
    },
  ],
  notChecked: ["sources"],
  warnings: ["planBestOfThree", "planDefect.overlap"],
  doubtful: 1,
  toReview: 1,
  proofreadBlocks: 10,
  proofreadChanged: 3,
  proofreadRejected: 1,
};

function invalid(message: string): AppError {
  return { kind: "invalid", message };
}

function notFound(): AppError {
  return { kind: "notFound", message: "project not found" };
}

/** Le type de réponse attendu par une question. */
const ANSWER_FOR: Record<WorkshopQuestion["type"], WorkshopAnswer["type"]> = {
  strategy: "strategy",
  framing: "framing",
  approvePlan: "plan",
  approveAction: "action",
};

export function fakeWorkshop(
  simulator: Simulator,
  backend: {
    staged: Map<string, StagedFile>;
    settings: { selectedModel: string };
  },
): FakeWorkshop {
  let lastId = 0;
  let lastQuestion = 0;
  let clock = WORKSHOP_TIME;
  const steps = new Map<number, number>();
  /** Dossiers rendus par le sélecteur (seuls acceptés ensuite). */
  const granted = new Set<string>();
  /** Réponses retenues (hold), par commande. */
  const holds = new Map<string, Promise<void>>();

  const fake: FakeWorkshop = {
    projects: new Map(),
    settings: { dir: DEFAULT_DIR, custom: false, web: true, autonomy: "ask" },
    pickedDir: null,
    modelCheck: (model) => ({
      model: model === "" ? backend.settings.selectedModel : model,
      auto: model === "",
      warnings: [],
      error: null,
    }),
    failures: new Map(),
    seed(patch = {}) {
      lastId += 1;
      const { task = "Write a guide", ...summary } = patch;
      const project: FakeProject = {
        summary: {
          id: lastId,
          title: titleOf(task),
          strategy: null,
          strategyForced: false,
          autonomy: "ask",
          state: "queued",
          phase: null,
          model: "",
          modelUsed: "",
          position: null,
          error: null,
          folder: `${fake.settings.dir}/${proposedFolderName(task) || "Project"}`,
          createdAt: WORKSHOP_TIME + lastId * 1000,
          updatedAt: WORKSHOP_TIME + lastId * 1000,
          finishedAt: null,
          ...summary,
        },
        task,
        attachments: [],
        question: null,
        report: null,
        events: [],
        parts: [],
        spec: null,
      };
      fake.projects.set(project.summary.id, project);
      return project;
    },
    project(id) {
      const project = fake.projects.get(id);
      if (!project) throw new Error(`unknown workshop project ${id}`);
      return project;
    },
    async emit(id, kind, atMs) {
      const project = fake.project(id);
      clock = Math.max(clock + 60_000, atMs ?? 0);
      const seq = project.events.length + 1;
      project.events.push({ seq, atMs: atMs ?? clock, kind });
      if (kind.type === "phaseStarted") project.summary.phase = kind.phase;
      await fake.notify({ projectId: id, lastSeq: seq });
    },
    async update(id, patch) {
      const project = fake.project(id);
      Object.assign(project.summary, patch);
      await fake.notify({ projectId: id, state: project.summary.state });
    },
    async ask(id, question) {
      const project = fake.project(id);
      lastQuestion += 1;
      project.question = { id: lastQuestion, question, askedAt: clock };
      project.summary.state = "waiting";
      await fake.emit(id, { type: "questionAsked", question });
    },
    async setParts(id, parts) {
      fake.project(id).parts = parts;
      await fake.notify({
        projectId: id,
        lastSeq: fake.project(id).events.length,
      });
    },
    async finish(id, report) {
      const project = fake.project(id);
      project.report = report;
      project.summary.state = "done";
      project.summary.finishedAt = clock;
      await fake.emit(id, { type: "finished", state: "done" });
    },
    demo() {
      return fake.seed({
        task: "Write a 3,000-word guide to urban permaculture for beginners.",
        autonomy: "ask",
      });
    },
    async advance(id) {
      const step = steps.get(id) ?? 0;
      steps.set(id, step + 1);
      const project = fake.project(id);
      const parts = (state: (id: number) => Partial<WorkshopPart>) =>
        DEMO_PLAN.bricks.map((brick) =>
          part(brick.id, {
            title: brick.title,
            kind: brick.kind,
            targetWords: brick.words,
            ...state(brick.id),
          }),
        );
      switch (step) {
        case 0:
          project.summary.state = "running";
          project.summary.modelUsed = backend.settings.selectedModel;
          await fake.emit(id, { type: "phaseStarted", phase: "collect" });
          await fake.emit(id, { type: "phaseDone", phase: "collect" });
          await fake.emit(id, { type: "phaseStarted", phase: "strategy" });
          await fake.emit(id, {
            type: "activity",
            activity: { code: "choosingStrategy", draw: 2 },
          });
          return true;
        case 1:
          project.summary.strategy = "longForm";
          project.spec = DEMO_SPEC;
          await fake.emit(id, {
            type: "strategyChosen",
            strategy: "longForm",
            unanimous: true,
          });
          await fake.emit(id, { type: "phaseDone", phase: "strategy" });
          await fake.emit(id, { type: "phaseStarted", phase: "plan" });
          await fake.emit(id, {
            type: "activity",
            activity: { code: "writingPlan", attempt: 1 },
          });
          return true;
        case 2:
          await fake.ask(id, { type: "approvePlan", value: DEMO_PLAN });
          return true;
        case 3:
          await fake.emit(id, { type: "planReady", plan: DEMO_PLAN });
          await fake.emit(id, { type: "phaseDone", phase: "plan" });
          await fake.emit(id, { type: "phaseStarted", phase: "build" });
          project.parts = parts((brickId) =>
            brickId === 2 ? { state: "running" } : {},
          );
          await fake.emit(id, {
            type: "brick",
            id: 2,
            state: "running",
            attempt: 0,
          });
          await fake.emit(id, {
            type: "activity",
            activity: { code: "writingBrick", id: 2 },
          });
          return true;
        case 4:
          project.parts = parts((brickId) =>
            brickId === 2
              ? {
                  state: "done",
                  words: 810,
                  text: "## The soil\n\nA living soil feeds the plants.",
                  checks: [
                    check("C1", true, { method: "formJudge" }),
                    check("length", true, { evidence: "810/600-1000 words" }),
                  ],
                }
              : brickId === 4
                ? { state: "fixing", attempt: 1, words: 700 }
                : {},
          );
          await fake.emit(id, {
            type: "brick",
            id: 2,
            state: "done",
            attempt: 0,
          });
          await fake.emit(id, {
            type: "brickChecks",
            id: 4,
            attempt: 0,
            checks: [check("C3", false, { method: "formJudge" })],
          });
          await fake.emit(id, {
            type: "activity",
            activity: { code: "fixingBrick", id: 4 },
          });
          return true;
        case 5:
          project.parts = parts((brickId) => ({
            state: brickId === 4 ? "toReview" : "done",
            words: 600,
          }));
          await fake.emit(id, { type: "phaseDone", phase: "build" });
          for (const phase of ["assemble", "verify", "deliver"] as const) {
            await fake.emit(id, { type: "phaseStarted", phase });
            await fake.emit(id, { type: "phaseDone", phase });
          }
          await fake.finish(id, DEMO_REPORT);
          return true;
        default:
          return false;
      }
    },
    hold(command) {
      let release = () => {};
      holds.set(
        command,
        new Promise<void>((resolve) => {
          release = resolve;
        }),
      );
      return () => release();
    },
    async notify(notice) {
      await simulator.emitEvent("workshop-event", {
        state: null,
        lastSeq: null,
        deleted: false,
        ...notice,
      } satisfies WorkshopNotice);
    },
  };

  /** Émis par le moteur après la réponse de la commande. */
  function notifyLater(
    notice: Partial<WorkshopNotice> & { projectId: number },
  ) {
    setTimeout(() => {
      if (!simulator.disposed) void fake.notify(notice);
    }, 0);
  }

  /** Rend `value` (lue à l'appel) après la retenue de `command`, s'il y en a une. */
  async function answer<T>(command: string, value: T): Promise<T> {
    const held = holds.get(command);
    if (held) {
      holds.delete(command);
      await held;
    }
    return value;
  }

  function failure(command: string) {
    const error = fake.failures.get(command);
    if (error) throw error;
  }

  function found(args: Args): FakeProject {
    const project = fake.projects.get(args.id as number);
    if (!project) throw notFound();
    return project;
  }

  /** Rangs dans la file : les projets en file, du plus ancien au plus récent. */
  function positions(): Map<number, number> {
    const queued = [...fake.projects.values()]
      .filter((project) => project.summary.state === "queued")
      .sort((a, b) => a.summary.createdAt - b.summary.createdAt);
    return new Map(
      queued.map((project, index) => [project.summary.id, index + 1]),
    );
  }

  function summaryOf(project: FakeProject): ProjectSummary {
    const position = positions().get(project.summary.id) ?? null;
    return { ...project.summary, position };
  }

  function partSummary(value: WorkshopPart): WorkshopPartSummary {
    return {
      id: value.id,
      parent: value.parent,
      title: value.title,
      kind: value.kind,
      targetWords: value.targetWords,
      state: value.state,
      attempt: value.attempt,
      words: value.words,
      dependsOn: [],
      criteria: value.checks
        .map((item) => item.criterion)
        .filter((criterion) => /^C\d+$/u.test(criterion)),
    };
  }

  function setState(project: FakeProject, state: ProjectState) {
    project.summary.state = state;
    project.summary.updatedAt = clock;
    notifyLater({ projectId: project.summary.id, state });
  }

  simulator.handle("workshop_list", () => {
    failure("workshop_list");
    return [...fake.projects.values()]
      .sort((a, b) => b.summary.createdAt - a.summary.createdAt)
      .map(summaryOf);
  });

  simulator.handle("workshop_get", (args: Args): Promise<ProjectDetail> => {
    failure("workshop_get");
    const project = found(args);
    return answer("workshop_get", {
      project: summaryOf(project),
      task: project.task,
      attachments: [...project.attachments],
      question: project.question,
      lastSeq: project.events.length,
      report: project.report,
    });
  });

  simulator.handle("workshop_events", (args: Args) => {
    failure("workshop_events");
    const project = found(args);
    const since = args.sinceSeq as number;
    return answer(
      "workshop_events",
      project.events.filter((event) => event.seq > since).slice(0, 1000),
    );
  });

  simulator.handle("workshop_parts", (args: Args) => {
    failure("workshop_parts");
    return answer("workshop_parts", found(args).parts.map(partSummary));
  });

  simulator.handle("workshop_part", (args: Args) => {
    failure("workshop_part");
    const value = found(args).parts.find((item) => item.id === args.brickId);
    if (!value) throw notFound();
    return value;
  });

  simulator.handle("workshop_create", (args: Args) => {
    failure("workshop_create");
    const request = args.project as NewProjectRequest;
    const task = request.task.trim();
    if (task === "") throw invalid("Describe the task first");
    if ([...task].length > MAX_TASK_CHARS)
      throw invalid("The task is too long");
    if (request.attachmentIds.length > MAX_ATTACHMENTS) {
      throw invalid("Too many attachments");
    }
    if (request.strategy && !strategyAvailable(request.strategy)) {
      throw invalid("This strategy isn't available yet");
    }
    const checked = fake.modelCheck(request.model ?? "");
    if (checked.error) throw invalid("This model can't be used");
    const files = request.attachmentIds.map((id) => {
      const file = backend.staged.get(id);
      if (!file) throw invalid(`unknown attachment ${id}`);
      if (file.kind === "image") throw invalid("Images can't be used");
      return file;
    });
    for (const file of files) backend.staged.delete(file.id);
    const name =
      folderNameOf(request.folderName ?? "") ||
      proposedFolderName(task) ||
      "Project";
    const project = fake.seed({
      task,
      strategy: request.strategy ?? null,
      strategyForced: request.strategy !== undefined,
      autonomy: request.autonomy,
      model: request.model ?? "",
      folder: `${fake.settings.dir}/${name}`,
    });
    project.attachments = files.map((file) => file.filename);
    notifyLater({ projectId: project.summary.id, state: "queued" });
    return project.summary.id;
  });

  simulator.handle("workshop_answer", (args: Args) => {
    failure("workshop_answer");
    const project = found(args);
    const request = args.answer as {
      questionId: number;
      answer: WorkshopAnswer;
    };
    const pending = project.question;
    if (!pending || pending.id !== request.questionId) {
      throw invalid("This question no longer waits for an answer");
    }
    if (ANSWER_FOR[pending.question.type] !== request.answer.type) {
      throw invalid("This answer doesn't match the question");
    }
    project.question = null;
    project.summary.state = "running";
    const seq = project.events.length + 1;
    clock += 60_000;
    project.events.push({
      seq,
      atMs: clock,
      kind: { type: "answerReceived", answer: request.answer },
    });
    notifyLater({
      projectId: project.summary.id,
      state: "running",
      lastSeq: seq,
    });
    return null;
  });

  simulator.handle("workshop_pause", (args: Args) => {
    failure("workshop_pause");
    const project = found(args);
    if (
      ["queued", "running", "waiting", "yielding"].includes(
        project.summary.state,
      )
    ) {
      setState(project, "paused");
    }
    return answer("workshop_pause", null);
  });

  simulator.handle("workshop_resume", (args: Args) => {
    failure("workshop_resume");
    const project = found(args);
    if (["paused", "failed"].includes(project.summary.state)) {
      project.summary.error = null;
      setState(project, project.question ? "waiting" : "queued");
    }
    return null;
  });

  simulator.handle("workshop_cancel", (args: Args) => {
    failure("workshop_cancel");
    const project = found(args);
    project.question = null;
    setState(project, "cancelled");
    return null;
  });

  simulator.handle("workshop_delete", (args: Args) => {
    failure("workshop_delete");
    const project = found(args);
    fake.projects.delete(project.summary.id);
    notifyLater({ projectId: project.summary.id, deleted: true });
    return null;
  });

  simulator.handle("workshop_open_folder", (args: Args) => {
    failure("workshop_open_folder");
    found(args);
    return null;
  });

  simulator.handle("workshop_open_file", (args: Args) => {
    failure("workshop_open_file");
    if (!found(args).report) throw notFound();
    return null;
  });

  simulator.handle("workshop_spec", (args: Args) => {
    failure("workshop_spec");
    return found(args).spec;
  });

  simulator.handle("workshop_pick_dir", () => {
    failure("workshop_pick_dir");
    if (fake.pickedDir !== null) granted.add(fake.pickedDir);
    return fake.pickedDir;
  });

  simulator.handle("workshop_settings", () => {
    failure("workshop_settings");
    return { ...fake.settings };
  });

  simulator.handle("workshop_set_settings", (args: Args) => {
    failure("workshop_set_settings");
    const patch = args.patch as WorkshopSettingsPatch;
    if (patch.dir !== undefined) {
      if (patch.dir === "") {
        fake.settings.dir = DEFAULT_DIR;
        fake.settings.custom = false;
      } else if (granted.has(patch.dir)) {
        fake.settings.dir = patch.dir;
        fake.settings.custom = true;
      } else {
        throw invalid("Choose the folder with the folder picker");
      }
    }
    if (patch.web !== undefined) fake.settings.web = patch.web;
    if (patch.autonomy !== undefined) fake.settings.autonomy = patch.autonomy;
    return answer("workshop_set_settings", { ...fake.settings });
  });

  simulator.handle("workshop_model_check", (args: Args) => {
    failure("workshop_model_check");
    return fake.modelCheck((args.model as string | null) ?? "");
  });

  return fake;
}
