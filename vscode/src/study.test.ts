// @vitest-environment node
// Cours de l'Étude sur le projet ou un langage (lot 4a, 2) : fichiers
// proposés (README, manifeste, points d'entrée, plus importés), jamais un
// fichier sensible ; filtrage hors machine toujours appliqué ; limites du
// moteur ; progression par `study.event` ; ouverture dans l'app.
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import {
  buildDocuments,
  courseLanguage,
  documentsChars,
  projectCandidates,
  rankImports,
  Study,
} from "./study";
import { RpcFailure, until } from "./test/fakeBridge";
import { cleanUp, ofType, openEditor, setupController } from "./test/setup";
import { env as vscodeEnv, resetTestState, testState } from "./test/vscode";

beforeEach(() => resetTestState());
afterEach(() => cleanUp());

const encode = (text: string) => new TextEncoder().encode(text);
const put = (files: Record<string, string>) => {
  for (const [file, text] of Object.entries(files)) {
    testState.files.set(`/work/${file}`, encode(text));
  }
};

describe("project files", () => {
  it("ranks the files most imported by the others", () => {
    expect(
      rankImports([
        { path: "src/a.ts", text: 'import { x } from "./util";' },
        { path: "src/b.ts", text: 'import y from "./util.js";\nimport "./c";' },
        { path: "src/c.ts", text: 'const u = require("./util");' },
        { path: "src/util.ts", text: "export const x = 1;" },
        { path: "pkg/m.py", text: "from .helpers import h" },
        { path: "pkg/n.py", text: "from .helpers import k" },
        { path: "pkg/helpers.py", text: "def h(): pass" },
        { path: "src/lib.rs", text: "mod net;\nmod net2;" },
        { path: "src/net.rs", text: "" },
      ]),
    ).toEqual(["src/util.ts", "pkg/helpers.py"]);
  });

  it("proposes README, manifest, entry points and imported files, never secrets", () => {
    const candidates = projectCandidates(
      [
        "README.md",
        "package.json",
        ".env",
        "src/index.ts",
        "src/util.ts",
        "secrets.json",
        "config/id_rsa",
      ],
      ["src/util.ts", ".env"],
    );
    expect(candidates).toEqual([
      { path: "README.md", reason: "readme", picked: true },
      { path: "package.json", reason: "manifest", picked: true },
      { path: "src/index.ts", reason: "entry", picked: true },
      { path: "src/util.ts", reason: "imported", picked: true },
    ]);
  });

  it("keeps the README alone and gathers the other files", () => {
    const documents = buildDocuments(
      [
        { kind: "file", label: "README.md", path: "README.md", text: "# App" },
        {
          kind: "file",
          label: "src/a.ts",
          path: "src/a.ts",
          text: "let a = 1;",
          language: "ts",
        },
      ],
      "work — project files",
    );
    expect(documents).toEqual([
      { name: "README.md", text: "# App" },
      {
        name: "work — project files",
        text: "### src/a.ts\n\n```ts\nlet a = 1;\n```",
      },
    ]);
    expect(documentsChars(documents)).toBe(
      "README.md# Appwork — project files".length + documents[1].text.length,
    );
  });
});

/** Contrôleur relié, dépôt simulé et cours créé par le faux pont. */
async function project(files: Record<string, string>) {
  const env = await setupController();
  put(files);
  env.fake.handlers["study.create"] = () => ({ courseId: "course-1" });
  env.fake.handlers["study.list"] = () => [
    {
      id: "course-1",
      status: "planning",
      levelsReady: 0,
      levelsTotal: 0,
    },
  ];
  testState.inputBox = "How the work project works";
  testState.pick = (items) => items[0];
  return env;
}

const studyOf = (env: Awaited<ReturnType<typeof project>>) =>
  ofType(env.posted, "study").at(-1)?.study ?? null;

describe("Make a course about this project", () => {
  it("sends the checked files, secrets removed even for a local model", async () => {
    const env = await project({
      "README.md": "# Work\nA small app.",
      "package.json": '{ "name": "work" }',
      ".env": "OPENAI_KEY=sk-abcdefghijklmnopqrstuvwx",
      "src/index.ts":
        'import { a } from "./util";\nconst apiKey = "sk-abcdefghijklmnopqrstuvwx";',
      "src/b.ts": 'import { a } from "./util";',
      "src/util.ts": "export const a = 1;",
    });
    await env.controller.study.fromProject();
    // Liste proposée : jamais le fichier sensible.
    const listed = testState.quickPicks[0].map((item) => item.label);
    expect(listed).not.toContain(".env");
    await until(() => env.fake.calls("study.create").length === 1);
    const params = env.fake.calls("study.create")[0];
    expect(params).toMatchObject({
      topic: "How the work project works",
      level: "beginner",
      duration: 20,
    });
    const sent = JSON.stringify(params.documents);
    expect(sent).toContain("A small app.");
    expect(sent).toContain("export const a = 1;");
    expect(sent).not.toContain("sk-abcdefghijklmnopqrstuvwx");
    expect(sent).toContain("[secret removed by ChatonLM]");
    await until(() => studyOf(env)?.status === "planning");
    expect(studyOf(env)).toMatchObject({
      courseId: "course-1",
      files: ["README.md", "package.json", "src/index.ts", "src/util.ts"],
      blocked: [{ label: "src/index.ts, line 2", reason: "secretValue" }],
    });
  });

  it("refuses clearly beyond the engine limit, without sending", async () => {
    // Trois fichiers proposés de 90 000 caractères : 270 000 en tout.
    const env = await project({
      "README.md": "w".repeat(90_000),
      "src/main.ts": "x".repeat(90_000),
      "src/app.ts": "y".repeat(90_000),
    });
    await env.controller.study.fromProject();
    expect(testState.shown.at(-1)).toMatchObject({ level: "warning" });
    expect(testState.shown.at(-1)?.text).toContain("at most 200,000");
    expect(env.fake.calls("study.create")).toEqual([]);
  });

  it("follows the course and opens it in the app", async () => {
    const env = await project({ "README.md": "# Work" });
    await env.controller.study.fromProject();
    await until(() => studyOf(env)?.status === "planning");
    env.fake.handlers["study.list"] = () => [
      { id: "other", status: "active", levelsReady: 9, levelsTotal: 9 },
      { id: "course-1", status: "active", levelsReady: 2, levelsTotal: 5 },
    ];
    env.fake.notify("study.event", { courseId: "course-1", deleted: false });
    await until(() => studyOf(env)?.status === "ready");
    expect(studyOf(env)).toMatchObject({ levelsReady: 2, levelsTotal: 5 });
    // Résumé joint à l'évènement (forme du pont) : appliqué sans relecture.
    const lists = env.fake.calls("study.list").length;
    env.fake.notify("study.event", {
      courseId: "course-1",
      course: {
        id: "course-1",
        status: "active",
        levelsReady: 3,
        levelsTotal: 5,
      },
    });
    await until(() => studyOf(env)?.levelsReady === 3);
    expect(env.fake.calls("study.list")).toHaveLength(lists);
    await env.controller.handle({ type: "study", command: "open" });
    expect(env.fake.shown).toEqual(["/study/course-1"]);
  });

  it("never reads a huge file to rank the imports", async () => {
    // Rapport 94 (B11) : taille lue d'abord, gros paquet minifié jamais lu.
    const env = await project({
      "README.md": "# Work",
      "src/a.ts": 'import { x } from "./util";',
      "src/b.ts": 'import { x } from "./util";',
      "src/util.ts": "export const x = 1;",
      "src/vendor.bundle.js": "x".repeat(300_000),
    });
    testState.pickMany = () => [];
    await env.controller.study.fromProject();
    expect(testState.reads).not.toContain("/work/src/vendor.bundle.js");
    expect(testState.reads).toContain("/work/src/a.ts");
    const listed = testState.quickPicks[0].map((item) => item.label);
    expect(listed).toContain("src/util.ts");
    expect(listed).not.toContain("src/vendor.bundle.js");
  });

  it("sends the language of VS Code when the app knows it", async () => {
    expect(courseLanguage("fr")).toBe("fr");
    expect(courseLanguage("es-419")).toBe("es");
    expect(courseLanguage("en-US")).toBe("en");
    expect(courseLanguage("de")).toBeUndefined();
    vscodeEnv.language = "fr";
    const env = await project({ "README.md": "# Work" });
    await env.controller.study.fromProject();
    await until(() => env.fake.calls("study.create").length === 1);
    expect(env.fake.calls("study.create")[0].language).toBe("fr");
  });

  it("sends no language the app does not know", async () => {
    vscodeEnv.language = "de";
    const env = await project({ "README.md": "# Work" });
    await env.controller.study.fromProject();
    await until(() => env.fake.calls("study.create").length === 1);
    expect(env.fake.calls("study.create")[0]).not.toHaveProperty("language");
  });

  it("reads the course again while it is planned, events lost or not", async () => {
    // Rapport 94 (M1) : sans `study.event`, la carte ne reste pas figée.
    const env = await project({ "README.md": "# Work" });
    const posted: { study: unknown }[] = [];
    const study = new Study({
      link: env.link,
      post: (message) => {
        if (message.type === "study") posted.push(message);
      },
      reveal: async () => {},
      show: async () => {},
      pollMs: 20,
    });
    try {
      await study.fromProject();
      await until(() => study.view?.status === "planning");
      env.fake.handlers["study.list"] = () => [
        { id: "course-1", status: "active", levelsReady: 1, levelsTotal: 4 },
      ];
      await until(() => study.view?.status === "ready");
      expect(study.view).toMatchObject({ levelsReady: 1, levelsTotal: 4 });
      // Prêt : plus de relecture.
      const lists = env.fake.calls("study.list").length;
      await new Promise((resolve) => setTimeout(resolve, 80));
      expect(env.fake.calls("study.list")).toHaveLength(lists);
    } finally {
      study.dispose();
    }
  });

  it("reads a planned course again after a reconnection", async () => {
    const env = await project({ "README.md": "# Work" });
    await env.controller.study.fromProject();
    await until(() => studyOf(env)?.status === "planning");
    env.fake.handlers["study.list"] = () => [
      { id: "course-1", status: "active", levelsReady: 3, levelsTotal: 4 },
    ];
    env.fake.dropAll();
    await until(() => !env.link.connected);
    await until(() => env.link.connected, 3000);
    await until(() => studyOf(env)?.status === "ready");
    expect(studyOf(env)?.levelsReady).toBe(3);
  });

  it("reads a planned course again when the panel opens", async () => {
    const env = await project({ "README.md": "# Work" });
    await env.controller.study.fromProject();
    await until(() => studyOf(env)?.status === "planning");
    env.fake.handlers["study.list"] = () => [
      { id: "course-1", status: "active", levelsReady: 2, levelsTotal: 4 },
    ];
    await env.controller.handle({ type: "ready" });
    await until(() => studyOf(env)?.status === "ready");
  });

  it("shows the app's refusal", async () => {
    const env = await project({ "README.md": "# Work" });
    env.fake.handlers["study.create"] = () => {
      throw new RpcFailure(-32020, "refused", {
        message: "No model for Study.",
      });
    };
    await env.controller.study.fromProject();
    await until(() => studyOf(env)?.status === "failed");
    expect(studyOf(env)?.error).toBe("No model for Study.");
  });
});

describe("Make a course about this language", () => {
  it("names the language and sends the active file as an example", async () => {
    const env = await project({});
    openEditor(
      "/work/src/lib.rs",
      'fn main() {\n    let password = "hunter2hunter2";\n}',
      undefined,
      "rust",
    );
    testState.inputBox = undefined;
    // Sujet proposé : le nom du langage (boîte validée telle quelle).
    const shownValues: string[] = [];
    const vscodeWindow = (await import("./test/vscode")).window;
    const original = vscodeWindow.showInputBox;
    vscodeWindow.showInputBox = async (options?: { value?: string }) => {
      shownValues.push(options?.value ?? "");
      return options?.value;
    };
    try {
      await env.controller.study.fromLanguage();
    } finally {
      vscodeWindow.showInputBox = original;
    }
    expect(shownValues).toEqual(["The Rust language"]);
    await until(() => env.fake.calls("study.create").length === 1);
    const params = env.fake.calls("study.create")[0];
    expect(params.topic).toBe("The Rust language");
    const documents = params.documents as { name: string; text: string }[];
    expect(documents).toHaveLength(1);
    expect(documents[0].name).toBe("Example in Rust");
    expect(documents[0].text).toContain("fn main()");
    expect(documents[0].text).not.toContain("hunter2hunter2");
  });

  it("asks for a part of a file too long for a course", async () => {
    // Rapport 94 (B7) : aucune liste de fichiers n'a été montrée.
    const env = await project({});
    openEditor("/work/src/big.ts", "x".repeat(250_000));
    testState.inputBox = "TypeScript";
    await env.controller.study.fromLanguage();
    expect(testState.shown.at(-1)).toMatchObject({ level: "warning" });
    expect(testState.shown.at(-1)?.text).toContain("Select a part of the file");
    expect(testState.shown.at(-1)?.text).not.toContain("Check fewer files");
    expect(env.fake.calls("study.create")).toEqual([]);
  });
});
