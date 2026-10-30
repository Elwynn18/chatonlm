// @vitest-environment node
// « Generate this image » (lot 4a, 3) : chemins d'image reconnus, description
// et taille déduites, action proposée seulement pour une image absente du
// dossier ouvert ; rien n'est écrit avant « Save », jamais hors du dossier
// (liens symboliques compris), jamais par-dessus un fichier sans accord ;
// PNG seulement ; aucun modèle d'images ; arrêt.
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import {
  decodePng,
  findImageRef,
  promptFor,
  realWithin,
  sizeFor,
  within,
} from "./image";
import { deferred, RpcFailure, until } from "./test/fakeBridge";
import { cleanUp, ofType, openEditor, setupController } from "./test/setup";
import { Range, resetTestState, testState, Uri } from "./test/vscode";

beforeEach(() => resetTestState());
afterEach(() => cleanUp());

/** Un PNG minimal (signature et un peu de contenu). */
const PNG = Buffer.from([
  0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 0, 1, 2, 3,
]).toString("base64");

describe("image paths", () => {
  it("finds the image path under the cursor in common forms", () => {
    const html = '<img src="img/hero.png" alt="A red kitten">';
    expect(findImageRef(html, 12)).toEqual({
      path: "img/hero.png",
      alt: "A red kitten",
      start: 10,
      end: 22,
    });
    expect(findImageRef("![Sleeping cat](./media/cat.png)", 20)).toMatchObject({
      path: "./media/cat.png",
      alt: "Sleeping cat",
    });
    expect(
      findImageRef("  background: url(../bg/stars.png);", 22),
    ).toMatchObject({ path: "../bg/stars.png", alt: null });
    expect(findImageRef('  "icon": "media/icon.png?v=2",', 14)).toMatchObject({
      path: "media/icon.png",
    });
  });

  it("ignores addresses, templates, other files and other places", () => {
    expect(findImageRef('<img src="https://x.org/a.png">', 12)).toBeNull();
    expect(
      findImageRef('<img src="data:image/png;base64,AAA">', 12),
    ).toBeNull();
    expect(findImageRef('<img src="${base}/a.png">', 12)).toBeNull();
    expect(findImageRef('const a = "notes.txt";', 13)).toBeNull();
    expect(findImageRef('x = 1; y = "a.png";', 2)).toBeNull();
  });

  it("proposes a description and a size", () => {
    expect(
      promptFor({
        path: "img/heroBanner_dark.png",
        alt: null,
        start: 0,
        end: 0,
      }),
    ).toBe("hero Banner dark");
    const ref = { path: "logo.png", alt: null, start: 0, end: 0 };
    expect(sizeFor(ref, '<img src="logo.png">')).toEqual({
      width: 512,
      height: 512,
    });
    expect(
      sizeFor(
        { path: "a.png", alt: null, start: 0, end: 0 },
        '<img src="a.png" width="1600" height="900">',
      ),
    ).toEqual({ width: 1024, height: 576 });
    expect(sizeFor({ path: "a.png", alt: null, start: 0, end: 0 }, "")).toEqual(
      {},
    );
    // Largeur CSS en pourcentage ou en em : pas des pixels (rapport 94, B5).
    expect(
      sizeFor(
        { path: "bg.png", alt: null, start: 0, end: 0 },
        '<img src="bg.png" style="width: 100%">',
      ),
    ).toEqual({ width: 1024, height: 576 });
    expect(
      sizeFor(
        { path: "a.png", alt: null, start: 0, end: 0 },
        '<img src="a.png" style="width: 20em">',
      ),
    ).toEqual({});
    expect(
      sizeFor(
        { path: "a.png", alt: null, start: 0, end: 0 },
        '<img src="a.png" style="width: 640px; height: 480px">',
      ),
    ).toEqual({ width: 640, height: 512 });
  });

  it("reads paths after an apostrophe, and decodes escapes", () => {
    // Rapport 94 (B6).
    const line = `<img alt="Bob's hero" src="hero.png">`;
    expect(findImageRef(line, line.indexOf("hero.png") + 2)).toMatchObject({
      path: "hero.png",
      alt: "Bob's hero",
    });
    expect(findImageRef("![](img/my%20cat.png)", 8)).toMatchObject({
      path: "img/my cat.png",
    });
    expect(
      findImageRef("  background: url(img/sky%20line.png);", 22),
    ).toMatchObject({ path: "img/sky line.png" });
    // Échappement invalide, ou qui cache une barre oblique inverse : rien.
    expect(findImageRef('<img src="a%E0%A4%A.png">', 12)).toBeNull();
    expect(findImageRef('<img src="..%5Cx.png">', 12)).toBeNull();
  });

  it("checks the folder, symbolic links followed", async () => {
    expect(within("/work", "/work/img/a.png")).toBe(true);
    expect(within("/work", "/workshop/a.png")).toBe(false);
    expect(within("/work", "/work")).toBe(false);
    expect(within("C:\\Work", "c:\\work\\a.png", true)).toBe(true);
    // /work/link → /etc : écrire /work/link/a.png sortirait du dossier.
    const realpath = async (p: string) => {
      if (p === "/work") return "/work";
      if (p === "/work/link") return "/etc";
      if (p === "/work/img") return "/work/img";
      throw new Error("ENOENT");
    };
    // Disque sans lien symbolique vu par lstat (le dossier relié est vu par
    // realpath seul).
    const noLinks = { realpath, isLink: async () => null };
    expect(await realWithin("/work", "/work/link/a.png", noLinks, false)).toBe(
      false,
    );
    expect(
      await realWithin("/work", "/work/img/new/a.png", noLinks, false),
    ).toBe(true);
  });

  it("refuses a dangling symbolic link, or a linked folder on the way", async () => {
    // Rapport 94 (S5) : img/hero.png → ../outside/planted.png (absent).
    // realpath échoue comme pour un fichier absent ; lstat dit « lien ».
    const realpath = async (p: string) => {
      if (p === "/work" || p === "/work/img") return p;
      throw new Error("ENOENT");
    };
    const links = new Set(["/work/img/hero.png", "/work/linked"]);
    const fs = {
      realpath,
      isLink: async (p: string) => (links.has(p) ? true : null),
    };
    expect(await realWithin("/work", "/work/img/hero.png", fs, false)).toBe(
      false,
    );
    expect(await realWithin("/work", "/work/linked/new/a.png", fs, false)).toBe(
      false,
    );
    expect(await realWithin("/work", "/work/img/other.png", fs, false)).toBe(
      true,
    );
    // lstat qui échoue autrement qu'« absent » : refus.
    const broken = {
      realpath,
      isLink: async () => {
        throw new Error("EACCES");
      },
    };
    expect(
      await realWithin("/work", "/work/img/other.png", broken, false),
    ).toBe(false);
  });

  it("accepts only PNG bytes", () => {
    expect(decodePng(PNG)?.length).toBe(15);
    expect(decodePng(Buffer.from("GIF89a....").toString("base64"))).toBeNull();
    expect(decodePng(42)).toBeNull();
  });
});

/** Contrôleur relié, page HTML qui cite une image absente. */
async function page(line = '<img src="img/hero.png" alt="A red kitten">') {
  const env = await setupController();
  openEditor("/work/site/index.html", line, undefined, "html");
  testState.files.set("/work/site/index.html", new TextEncoder().encode(line));
  testState.inputBox = "A red kitten on a sofa";
  return env;
}

const imageOf = (posted: Parameters<typeof ofType>[0]) =>
  ofType(posted, "image").at(-1)?.image ?? null;

const args = (column = 12) => [Uri.file("/work/site/index.html"), 0, column];

describe("Generate this image", () => {
  it("is offered only for a missing image inside the folder", async () => {
    const env = await page();
    const document = testState.documents.get("/work/site/index.html");
    const at = new Range(0, 12, 0, 12);
    const actions = await env.controller.image.provideCodeActions(
      document as never,
      at as never,
    );
    expect(actions.map((a) => a.title)).toEqual([
      "Generate this image with ChatonLM",
    ]);
    testState.files.set("/work/site/img/hero.png", new Uint8Array([1]));
    expect(
      await env.controller.image.provideCodeActions(
        document as never,
        at as never,
      ),
    ).toEqual([]);
  });

  it("writes nothing before Save, then the PNG inside the folder", async () => {
    const env = await page();
    env.fake.handlers["image.generate"] = (_params, { id, notify }) => {
      notify("image.event", {
        requestId: id,
        event: { stage: "generating", step: 3, total: 20, cpu: true },
      });
      return { pngBase64: PNG, seed: 1, model: "sd" };
    };
    await env.controller.image.generate(args());
    // Taille d'une bannière, d'après le nom « hero ».
    expect(env.fake.calls("image.generate")).toEqual([
      { prompt: "A red kitten on a sofa", width: 1024, height: 576 },
    ]);
    expect(imageOf(env.posted)).toMatchObject({
      status: "ready",
      target: "site/img/hero.png",
      preview: `data:image/png;base64,${PNG}`,
      exists: false,
    });
    expect(
      ofType(env.posted, "image").some(
        (m) => m.image?.step === 3 && m.image.cpu,
      ),
    ).toBe(true);
    expect(testState.written).toEqual([]);
    expect(await env.controller.image.save()).toBe(true);
    expect(testState.written).toEqual(["/work/site/img/hero.png"]);
    expect(
      Buffer.from(
        testState.files.get("/work/site/img/hero.png") ?? [],
      ).toString("base64"),
    ).toBe(PNG);
    expect(imageOf(env.posted)?.status).toBe("saved");
  });

  it("never writes outside the open folder", async () => {
    const env = await page('<img src="../../outside.png">');
    env.fake.handlers["image.generate"] = () => ({ pngBase64: PNG });
    await env.controller.image.generate(args(12));
    expect(testState.shown.at(-1)?.text).toContain("outside the open folder");
    expect(env.fake.calls("image.generate")).toEqual([]);
    expect(testState.written).toEqual([]);
  });

  it("never writes through a dangling symbolic link", async () => {
    const env = await page();
    env.fake.handlers["image.generate"] = () => ({ pngBase64: PNG });
    await env.controller.image.generate(args());
    expect(imageOf(env.posted)?.status).toBe("ready");
    // Lien posé avant « Save » : img/hero.png → hors du dossier (absent).
    testState.links.add("/work/site/img/hero.png");
    testState.choice = "Replace";
    expect(await env.controller.image.save()).toBe(false);
    expect(testState.written).toEqual([]);
    expect(imageOf(env.posted)).toMatchObject({
      status: "failed",
      error:
        "This path is outside the open folder: ChatonLM only writes images inside it.",
    });
  });

  it("asks before replacing a file, and keeps it if not confirmed", async () => {
    const env = await page();
    env.fake.handlers["image.generate"] = () => ({ pngBase64: PNG });
    await env.controller.image.generate(args());
    // Fichier apparu entre-temps (une autre écriture).
    testState.files.set("/work/site/img/hero.png", new Uint8Array([7]));
    await env.controller.handle({ type: "image", command: "save" });
    expect(testState.shown.at(-1)?.text).toContain("already exists");
    expect(testState.written).toEqual([]);
    expect(testState.files.get("/work/site/img/hero.png")).toEqual(
      new Uint8Array([7]),
    );
    testState.choice = "Replace";
    await env.controller.handle({ type: "image", command: "save" });
    expect(testState.written).toEqual(["/work/site/img/hero.png"]);
  });

  it("only makes PNG files", async () => {
    const env = await page('<img src="img/hero.jpg">');
    await env.controller.image.generate(args());
    expect(testState.shown.at(-1)?.text).toContain("only makes PNG images");
    expect(env.fake.calls("image.generate")).toEqual([]);
  });

  it("points to the image models when none is installed", async () => {
    const env = await page();
    env.fake.handlers["image.generate"] = () => {
      throw new RpcFailure(-32036, "no image model");
    };
    testState.choice = "Open ChatonLM";
    await env.controller.image.generate(args());
    expect(imageOf(env.posted)).toMatchObject({
      status: "failed",
      error: "No image model is installed in ChatonLM.",
    });
    expect(env.fake.shown).toEqual(["/hub/images"]);
  });

  it("stops a running image", async () => {
    const env = await page();
    const release = deferred();
    env.fake.handlers["image.generate"] = async () => {
      await release.promise;
      throw new RpcFailure(-32800, "cancelled");
    };
    const running = env.controller.image.generate(args());
    await until(() => imageOf(env.posted)?.status === "generating");
    await env.controller.handle({ type: "image", command: "cancel" });
    await until(() => env.fake.cancelled.length === 1);
    release.resolve();
    await running;
    expect(imageOf(env.posted)).toBeNull();
    expect(testState.written).toEqual([]);
  });
});
