// @vitest-environment node
// Visite guidée parlée (lot 2, X) : plan, lecture, événements → surlignage,
// barre d'état et panneau ; commandes ; filtrage hors machine ; fichier
// modifié ; diff ; erreurs.
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { deferred, RpcFailure, until } from "./test/fakeBridge";
import {
  cleanUp,
  ofType,
  openEditor,
  setupController,
  TEN_LINES,
} from "./test/setup";
import {
  Position,
  Range,
  resetTestState,
  Selection,
  TextEditorRevealType,
  testState,
  Uri,
  window,
} from "./test/vscode";
import { diffLineMap, diffToFile, shiftSegments, validSegments } from "./tour";

beforeEach(() => resetTestState());
afterEach(() => cleanUp());

const PLAN = {
  segments: [
    { lines: [1, 3], text: "Les trois premières lignes." },
    { lines: [4, 6], text: "Le milieu." },
    { lines: [7, 10], text: "La fin." },
  ],
};

/** Contrôleur, éditeur de dix lignes, plan et lecture « r1 » prêts. */
async function touring(
  options: { plan?: unknown; text?: string; file?: string } = {},
) {
  const env = await setupController();
  const editor = openEditor(
    options.file ?? "/work/src/app.ts",
    options.text ?? TEN_LINES,
  );
  env.fake.handlers["tour.plan"] = () => options.plan ?? PLAN;
  let reads = 0;
  env.fake.handlers["speech.read"] = () => ({ readId: `r${++reads}` });
  return { ...env, editor };
}

const tourOf = (posted: Parameters<typeof ofType>[0]) =>
  ofType(posted, "tour").at(-1)?.tour ?? null;

/** Lignes (0-based) surlignées dans l'éditeur. */
function highlighted(editor: { decorations: Map<unknown, Range[]> }) {
  const ranges = [...editor.decorations.values()].at(-1) ?? [];
  return ranges.map((r) => [r.start.line, r.end.line]);
}

function started(
  fake: { notify(m: string, p: unknown): void },
  readId: string,
  segmentId: string,
) {
  fake.notify("speech.event", { readId, type: "started", segmentId });
}

describe("tour plan, pure parts", () => {
  it("keeps only valid segments, inside the lines given", () => {
    expect(
      validSegments(
        {
          segments: [
            { lines: [3, 4], text: " ok " },
            { lines: [2, 4], text: "before the range" },
            { lines: [5, 4], text: "backwards" },
            { lines: [5, 12], text: "after the range" },
            { lines: [5.5, 6], text: "not a line" },
            { lines: [6, 7], text: "  " },
            { lines: [6], text: "one number" },
            { text: "no lines" },
            null,
            { lines: [10, 10], text: "last line" },
          ],
        },
        3,
        10,
      ),
    ).toEqual([
      { lines: [3, 4], text: "ok" },
      { lines: [10, 10], text: "last line" },
    ]);
    expect(validSegments({ segments: "no" }, 1, 3)).toEqual([]);
    expect(validSegments(null, 1, 3)).toEqual([]);
  });

  it("maps the lines of a diff to the lines of the changed file", () => {
    const diff = [
      "diff --git a/f.ts b/f.ts", // 1
      "--- a/f.ts", // 2
      "+++ b/f.ts", // 3
      "@@ -10,3 +10,4 @@ fn", // 4
      " ten", // 5 → 10
      "-old", // 6 → 11 (retirée : à la place de la suivante)
      "+new", // 7 → 11
      "+added", // 8 → 12
      " thirteen", // 9 → 13
      "@@ -40,2 +41,1 @@", // 10
      "-gone", // 11 → 41
      " kept", // 12 → 41
    ].join("\n");
    const map = diffLineMap(diff);
    expect(map).toEqual([
      null,
      null,
      null,
      null,
      10,
      11,
      11,
      12,
      13,
      null,
      41,
      41,
    ]);
    expect(diffToFile(map, [5, 9])).toEqual([10, 13]);
    expect(diffToFile(map, [7, 8])).toEqual([11, 12]);
    // Seulement des en-têtes : la ligne qui suit.
    expect(diffToFile(map, [1, 4])).toEqual([10, 10]);
    expect(diffToFile(map, [10, 10])).toEqual([41, 41]);
  });

  it("moves the segments after an edit, or gives up when one is cut", () => {
    const segments = [
      { lines: [1, 3] as [number, number], text: "a" },
      { lines: [5, 8] as [number, number], text: "b" },
    ];
    // Deux lignes ajoutées sur la ligne 4 (index 3), entre les segments.
    expect(
      shiftSegments(segments, { start: 3, end: 3, text: "x\ny\n" }),
    ).toEqual([
      { lines: [1, 3], text: "a" },
      { lines: [7, 10], text: "b" },
    ]);
    // Une ligne retirée dans le second segment : il raccourcit.
    expect(shiftSegments(segments, { start: 5, end: 6, text: "" })).toEqual([
      { lines: [1, 3], text: "a" },
      { lines: [5, 7], text: "b" },
    ]);
    // Une ligne ajoutée au tout début du premier segment : il glisse.
    expect(
      shiftSegments(segments, {
        start: 0,
        end: 0,
        endCharacter: 0,
        text: "top\n",
      }),
    ).toEqual([
      { lines: [2, 4], text: "a" },
      { lines: [6, 9], text: "b" },
    ]);
    // Deux lignes entières retirées juste avant le second segment.
    expect(
      shiftSegments(segments, {
        start: 2,
        end: 4,
        endCharacter: 0,
        text: "",
      }),
    ).toBeNull();
    expect(
      shiftSegments([{ lines: [5, 8] as [number, number], text: "b" }], {
        start: 2,
        end: 4,
        endCharacter: 0,
        text: "",
      }),
    ).toEqual([{ lines: [3, 6], text: "b" }]);
    // Dernière ligne du second segment retirée entière (Ctrl+Maj+K) : il
    // raccourcit, la visite continue.
    expect(
      shiftSegments(segments, {
        start: 7,
        end: 8,
        endCharacter: 0,
        text: "",
      }),
    ).toEqual([
      { lines: [1, 3], text: "a" },
      { lines: [5, 7], text: "b" },
    ]);
    // Une ligne changée sans retour à la ligne : rien ne bouge.
    expect(shiftSegments(segments, { start: 6, end: 6, text: "z" })).toEqual(
      segments,
    );
    // Une modification à cheval sur deux segments : impossible de suivre.
    expect(shiftSegments(segments, { start: 2, end: 5, text: "" })).toBeNull();
  });
});

describe("Tour", () => {
  it("plans the open file, reads it, and highlights each part as it is spoken", async () => {
    const { fake, controller, posted, editor } = await touring();
    await controller.tour.start(false);

    const [plan] = fake.calls("tour.plan");
    expect(plan).toEqual({
      locale: "fr",
      path: "src/app.ts",
      language: "typescript",
      code: TEN_LINES,
      firstLine: 1,
      kind: "file",
      model: "qwen3:0.6b",
    });
    const [read] = fake.calls("speech.read");
    expect(read).toEqual({
      segments: [
        { id: "1:0", text: "Les trois premières lignes." },
        { id: "1:1", text: "Le milieu." },
        { id: "1:2", text: "La fin." },
      ],
      language: "fr",
    });
    expect(testState.contexts["chatonlm.touring"]).toBe(true);
    // Reçu de ce qui est parti pour préparer la visite.
    expect(tourOf(posted)?.receipt?.items).toEqual([
      {
        kind: "file",
        label: "src/app.ts",
        path: "src/app.ts",
        lines: [1, 10],
        chars: TEN_LINES.length,
      },
    ]);

    started(fake, "r1", "1:1");
    await until(() => tourOf(posted)?.current === 1);
    expect(tourOf(posted)?.status).toBe("playing");
    // Lignes 4 à 6 du fichier : 3 à 5 en partant de 0.
    expect(highlighted(editor)).toEqual([[3, 5]]);
    const reveal = editor.revealed.at(-1);
    expect(reveal?.type).toBe(TextEditorRevealType.InCenter);
    expect([reveal?.range.start.line, reveal?.range.end.line]).toEqual([3, 5]);
    // Item de la visite (l'autocomplétion a le sien).
    const status = testState.statusItems.find(
      (item) => item.command === "chatonlm.tourTogglePause",
    );
    if (!status) throw new Error("no tour status item");
    expect(status.visible).toBe(true);
    expect(status.text).toContain("ChatonLM tour 2/3");
    expect(status.command).toBe("chatonlm.tourTogglePause");
    // Couleur de surlignage du thème de l'éditeur, bordure gauche.
    const options = testState.decorationTypes[0].options;
    expect(options.isWholeLine).toBe(true);
    expect((options.backgroundColor as { id: string }).id).toBe(
      "editor.rangeHighlightBackground",
    );
    expect(options.borderWidth).toBe("0 0 0 2px");

    // Dernier segment fini : la visite s'arrête d'elle-même.
    started(fake, "r1", "1:2");
    await until(() => tourOf(posted)?.current === 2);
    fake.notify("speech.event", {
      readId: "r1",
      type: "finished",
      segmentId: "1:2",
    });
    await until(() => tourOf(posted)?.status === "ended");
    expect(highlighted(editor)).toEqual([]);
    expect(status.visible).toBe(false);
    expect(testState.contexts["chatonlm.touring"]).toBe(false);
  });

  it("tours the selection, with its own line numbers", async () => {
    const { fake, controller, posted, editor } = await touring({
      plan: { segments: [{ lines: [3, 4], text: "Deux lignes." }] },
    });
    editor.selection = new Selection(new Position(2, 0), new Position(4, 0));
    await controller.tour.start(false);
    const [plan] = fake.calls("tour.plan");
    expect(plan.kind).toBe("selection");
    expect(plan.firstLine).toBe(3);
    expect(plan.code).toBe("line 3\nline 4");
    started(fake, "r1", "1:0");
    await until(() => tourOf(posted)?.current === 0);
    expect(highlighted(editor)).toEqual([[2, 3]]);
  });

  it("pauses, resumes the part again, and goes back and forth", async () => {
    const { fake, controller, posted } = await touring();
    await controller.tour.start(false);
    started(fake, "r1", "1:1");
    await until(() => tourOf(posted)?.current === 1);

    await controller.tourCommand("pause");
    expect(tourOf(posted)?.status).toBe("paused");
    await until(() => fake.calls("speech.stop").length === 1);
    expect(fake.calls("speech.stop")[0]).toEqual({ readId: "r1" });
    expect(
      testState.statusItems.find(
        (item) => item.command === "chatonlm.tourTogglePause",
      )?.text,
    ).toContain("2/3, paused");

    // Reprise : le segment en cours est relu depuis son début.
    await controller.tourCommand("resume");
    expect(fake.calls("speech.read")[1].segments).toEqual([
      { id: "1:1", text: "Le milieu." },
      { id: "1:2", text: "La fin." },
    ]);
    expect(tourOf(posted)?.status).toBe("playing");

    await controller.tourCommand("previous");
    expect(fake.calls("speech.read")[2].segments).toHaveLength(3);
    // La lecture précédente est arrêtée.
    await until(() => fake.calls("speech.stop").length === 2);
    expect(fake.calls("speech.stop")[1]).toEqual({ readId: "r2" });

    await controller.tourCommand("next");
    expect(
      (fake.calls("speech.read")[3].segments as { id: string }[])[0].id,
    ).toBe("1:1");
    // Un événement d'une lecture arrêtée ne change plus rien.
    started(fake, "r1", "1:2");
    started(fake, "r4", "1:1");
    await until(() => ofType(posted, "tour").length > 0);
    await new Promise((resolve) => setTimeout(resolve, 30));
    expect(tourOf(posted)?.current).toBe(1);
  });

  it("stops: reading stopped, highlight removed, segments still shown", async () => {
    const { fake, controller, posted, editor } = await touring();
    await controller.tour.start(false);
    started(fake, "r1", "1:0");
    await until(() => tourOf(posted)?.current === 0);
    expect(highlighted(editor)).toEqual([[0, 2]]);

    await controller.tourCommand("stop");
    expect(tourOf(posted)?.status).toBe("ended");
    expect(tourOf(posted)?.segments).toHaveLength(3);
    expect(highlighted(editor)).toEqual([]);
    expect(testState.contexts["chatonlm.touring"]).toBe(false);
    await until(() => fake.calls("speech.stop").length === 1);
    // Après l'arrêt, plus rien ne bouge.
    started(fake, "r1", "1:1");
    await new Promise((resolve) => setTimeout(resolve, 30));
    expect(tourOf(posted)?.status).toBe("ended");
    expect(highlighted(editor)).toEqual([]);

    // Un segment choisi dans le panneau relance la lecture à partir de lui.
    await controller.handle({ type: "tourGoto", index: 2 });
    expect(
      (fake.calls("speech.read").at(-1)?.segments as { id: string }[])[0].id,
    ).toBe("1:2");
    expect(testState.contexts["chatonlm.touring"]).toBe(true);
  });

  it("pauses while speech.read is still unanswered", async () => {
    const { fake, controller, posted } = await touring();
    const answer = deferred<unknown>();
    fake.handlers["speech.read"] = () => answer.promise;
    const starting = controller.tour.start(false);
    await until(() => fake.calls("speech.read").length === 1);
    expect(tourOf(posted)?.status).toBe("playing");
    await controller.tourCommand("pause");
    expect(tourOf(posted)?.status).toBe("paused");
    answer.resolve({ readId: "r1" });
    await starting;
    // La lecture répondue après la pause est arrêtée, et ne reprend pas.
    await until(() => fake.calls("speech.stop").length === 1);
    expect(fake.calls("speech.stop")[0]).toEqual({ readId: "r1" });
    started(fake, "r1", "1:0");
    await new Promise((resolve) => setTimeout(resolve, 30));
    expect(tourOf(posted)?.status).toBe("paused");
  });

  it("follows the edits made while the tour is prepared", async () => {
    const { fake, controller, posted, editor } = await touring();
    const plan = deferred<unknown>();
    fake.handlers["tour.plan"] = () => plan.promise;
    const starting = controller.tour.start(false);
    await until(() => tourOf(posted)?.status === "planning");
    // Deux lignes ajoutées en haut du fichier pendant la préparation.
    testState.events.documentChange.fire({
      document: editor.document,
      contentChanges: [{ range: new Range(0, 0, 0, 0), text: "a\nb\n" }],
    });
    plan.resolve(PLAN);
    await starting;
    expect(tourOf(posted)?.segments.map((s) => s.lines)).toEqual([
      [3, 5],
      [6, 8],
      [9, 12],
    ]);
    started(fake, "r1", "1:0");
    await until(() => tourOf(posted)?.current === 0);
    expect(highlighted(editor)).toEqual([[2, 4]]);
  });

  it("stops when the file changed too much while the tour was prepared", async () => {
    const { fake, controller, posted, editor } = await touring();
    const plan = deferred<unknown>();
    fake.handlers["tour.plan"] = () => plan.promise;
    const starting = controller.tour.start(false);
    await until(() => tourOf(posted)?.status === "planning");
    // Lignes retirées à cheval sur les deux premiers segments.
    testState.events.documentChange.fire({
      document: editor.document,
      contentChanges: [{ range: new Range(1, 0, 4, 0), text: "" }],
    });
    plan.resolve(PLAN);
    await starting;
    expect(fake.calls("speech.read")).toEqual([]);
    expect(tourOf(posted)).toBeNull();
    expect(testState.shown.at(-1)?.text).toMatch(/changed too much/);
    expect(testState.contexts["chatonlm.touring"]).toBe(false);
  });

  it("takes events that arrive before the answer of speech.read", async () => {
    const { fake, controller, posted, editor } = await touring();
    // Premier segment très court : fini, et le suivant commencé, avant même
    // la réponse de `speech.read`.
    fake.handlers["speech.read"] = (_params, { notify }) => {
      for (const [type, segmentId] of [
        ["started", "1:0"],
        ["finished", "1:0"],
        ["started", "1:1"],
      ]) {
        notify("speech.event", { readId: "early", type, segmentId });
      }
      return { readId: "early" };
    };
    await controller.tour.start(false);
    await until(() => tourOf(posted)?.current === 1);
    expect(highlighted(editor)).toEqual([[3, 5]]);
  });

  it("pauses when another reading takes the voice in the app", async () => {
    const { fake, controller, posted } = await touring();
    await controller.tour.start(false);
    started(fake, "r1", "1:0");
    await until(() => tourOf(posted)?.current === 0);
    fake.notify("speech.event", { readId: "r1", type: "stopped" });
    await until(() => tourOf(posted)?.status === "paused");
    fake.notify("speech.event", {
      readId: "r1",
      type: "started",
      segmentId: "1:1",
    });
    await new Promise((resolve) => setTimeout(resolve, 30));
    expect(tourOf(posted)?.current).toBe(0);
  });

  it("follows the lines when the file is edited, and stops when it can't", async () => {
    const { fake, controller, posted, editor } = await touring();
    await controller.tour.start(false);
    started(fake, "r1", "1:1");
    await until(() => tourOf(posted)?.current === 1);

    // Deux lignes ajoutées en haut du fichier.
    testState.events.documentChange.fire({
      document: editor.document,
      contentChanges: [{ range: new Range(0, 0, 0, 0), text: "a\nb\n" }],
    });
    expect(highlighted(editor)).toEqual([[5, 7]]);
    expect(tourOf(posted)?.segments.map((s) => s.lines)).toEqual([
      [3, 5],
      [6, 8],
      [9, 12],
    ]);

    // Un autre fichier : rien ne bouge.
    testState.events.documentChange.fire({
      document: openEditor("/work/other.ts", "x").document,
      contentChanges: [{ range: new Range(0, 0, 0, 0), text: "\n\n" }],
    });
    window.visibleTextEditors = [editor];
    expect(tourOf(posted)?.segments[0].lines).toEqual([3, 5]);

    // Lignes retirées à cheval sur deux segments : la visite s'arrête.
    testState.events.documentChange.fire({
      document: editor.document,
      contentChanges: [{ range: new Range(4, 0, 6, 0), text: "" }],
    });
    expect(tourOf(posted)?.status).toBe("ended");
    expect(highlighted(editor)).toEqual([]);
    expect(testState.shown.at(-1)?.text).toMatch(/changed too much/);
  });

  it("removes secrets from the code when the model is off this computer", async () => {
    const { fake, controller, posted } = await touring({
      text: 'const a = 1;\nconst password = "hunter22";\nconst b = 2;',
    });
    fake.local = false;
    await controller.tour.start(false);
    const [plan] = fake.calls("tour.plan");
    expect(plan.code).not.toContain("hunter22");
    expect(plan.code).toContain("[secret removed by ChatonLM]");
    // Les lignes ne bougent pas : les segments restent justes.
    expect((plan.code as string).split("\n")).toHaveLength(3);
    const receipt = tourOf(posted)?.receipt;
    expect(receipt?.destination).toBe("remote");
    expect(receipt?.blocked).toEqual([
      { label: "src/app.ts, line 2", reason: "secretValue" },
    ]);
  });

  it("sends nothing from a secret file to a model off this computer", async () => {
    const { fake, controller, posted } = await touring({
      file: "/work/.env",
      text: "TOKEN=abcdefgh",
    });
    fake.local = false;
    await controller.tour.start(false);
    expect(fake.calls("tour.plan")).toEqual([]);
    expect(ofType(posted, "tour")).toEqual([]);
    expect(testState.shown.at(-1)?.level).toBe("warning");
  });

  it("stops while the tour is planned: the request is cancelled", async () => {
    const { fake, controller, posted } = await touring();
    let answer!: (value: unknown) => void;
    fake.handlers["tour.plan"] = () =>
      new Promise((resolve) => (answer = resolve));
    const starting = controller.tour.start(false);
    await until(() => tourOf(posted)?.status === "planning");
    await controller.tourCommand("stop");
    await until(() => fake.cancelled.length === 1);
    answer(PLAN);
    await starting;
    expect(fake.calls("speech.read")).toEqual([]);
    expect(tourOf(posted)?.status).toBe("ended");
    expect(testState.shown).toEqual([]);
  });

  it("says why the tour couldn't be prepared", async () => {
    const { fake, controller, posted } = await touring();
    fake.handlers["tour.plan"] = () => {
      throw new RpcFailure(-32020, "refused", {
        message: "Le modèle n'a pas rendu de plan.",
      });
    };
    await controller.tour.start(false);
    expect(tourOf(posted)).toBeNull();
    expect(testState.shown.at(-1)).toEqual({
      level: "error",
      text: "Le modèle n'a pas rendu de plan.",
    });
    expect(testState.contexts["chatonlm.touring"]).toBe(false);

    // Plan sans segment valide.
    fake.handlers["tour.plan"] = () => ({ segments: [{ lines: [0, 99] }] });
    await controller.tour.start(false);
    expect(testState.shown.at(-1)?.text).toBe(
      "ChatonLM couldn't prepare the tour.",
    );
  });

  it("offers to open the app when no voice is installed", async () => {
    const { fake, controller, posted, deps } = await touring();
    fake.handlers["speech.read"] = () => {
      throw new RpcFailure(-32030, "no voice", { language: "fr" });
    };
    testState.choice = "Open ChatonLM";
    await controller.tour.start(false);
    await until(() => deps.launchApp.mock.calls.length === 1);
    expect(deps.launchApp).toHaveBeenCalledWith(false);
    expect(testState.shown.at(-1)?.text).toMatch(/no voice for this language/);
    // Les segments restent lisibles, la visite en pause.
    expect(tourOf(posted)?.status).toBe("paused");
  });

  it("tours the changes of a file, highlighted in the file itself", async () => {
    const { fake, controller, posted, editor } = await touring({
      plan: { segments: [{ lines: [6, 8], text: "Une ligne changée." }] },
    });
    const diff = [
      "diff --git a/src/app.ts b/src/app.ts",
      "--- a/src/app.ts",
      "+++ b/src/app.ts",
      "@@ -4,3 +4,3 @@",
      " line 4",
      "-old 5",
      "+line 5",
      " line 6",
    ].join("\n");
    const repository = {
      rootUri: Uri.file("/work"),
      inputBox: { value: "" },
      diff: async () => "",
      diffWithHEAD: async (path: string) =>
        path === "/work/src/app.ts" ? diff : "",
    };
    testState.extensions.set("vscode.git", {
      enabled: true,
      getAPI: () => ({
        repositories: [repository],
        getRepository: () => repository,
      }),
    });
    // Menu de la gestion de versions : l'argument désigne le fichier.
    await controller.tour.start(true, [
      { resourceUri: Uri.file("/work/src/app.ts") },
    ]);
    const [plan] = fake.calls("tour.plan");
    expect(plan).toMatchObject({ kind: "diff", code: diff, firstLine: 1 });
    expect(tourOf(posted)?.segments[0].lines).toEqual([5, 6]);
    started(fake, "r1", "1:0");
    await until(() => highlighted(editor).length > 0);
    expect(highlighted(editor)).toEqual([[4, 5]]);

    // Fichier sans modification.
    repository.diffWithHEAD = async () => "";
    await controller.tour.start(true, [Uri.file("/work/src/app.ts")]);
    expect(testState.shown.at(-1)?.text).toBe(
      "This file has no changes to walk through.",
    );
  });

  it("ends the tour when the link with the app is lost", async () => {
    const { fake, controller, posted, editor } = await touring();
    await controller.tour.start(false);
    started(fake, "r1", "1:0");
    await until(() => tourOf(posted)?.current === 0);
    fake.dropAll();
    await until(() => tourOf(posted)?.status === "ended");
    expect(highlighted(editor)).toEqual([]);
  });

  it("gives the tour back to a panel opened later", async () => {
    const { fake, controller, posted } = await touring();
    await controller.tour.start(false);
    started(fake, "r1", "1:0");
    await until(() => tourOf(posted)?.current === 0);
    posted.length = 0;
    await controller.handle({ type: "ready" });
    expect(tourOf(posted)?.current).toBe(0);
    expect(ofType(posted, "voice")).toEqual([{ type: "voice", voice: null }]);
  });
});
