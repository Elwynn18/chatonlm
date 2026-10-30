import {
  existsSync,
  mkdirSync,
  mkdtempSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import path from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import viteConfig, { withoutNotProse } from "../vite.config.ts";
import {
  checkBundle,
  checkSources,
  checkWebviewBundle,
  EXPECTED_SCOPES,
  REQUIRED_PACKAGES,
  WEBVIEW_LIMITS,
} from "./check-bundle.mjs";

const tmp = path.resolve(import.meta.dirname, "../.tools/tmp");
const dirs = [];

afterEach(() => {
  for (const dir of dirs.splice(0)) rmSync(dir, { recursive: true });
});

/** Grammaire minifiée comme dans un build Vite (JSON dans un gabarit). */
function grammar(scope, name = "x") {
  return `Object.freeze(JSON.parse(\`{"displayName":"X","name":"${name}","patterns":[{"match":"a","name":"keyword.control.${name}"}],"scopeName":"${scope}"}\`))`;
}

const EMPTY_GLSL =
  "[{name:`glsl`,scopeName:`source.glsl`,patterns:[],repository:{}}]";
const GRAMMARS = [
  ...EXPECTED_SCOPES.filter((scope) => scope !== "source.glsl").map((scope) =>
    grammar(scope),
  ),
  EMPTY_GLSL,
].join(",");
const FONT_FACE =
  '@font-face{font-family:KaTeX_Main;font-style:normal;font-weight:400;src:url(/assets/KaTeX_Main-Regular-x.woff2)format("woff2")}';
// Règles de typography telles que les laisse withoutNotProse (vite.config.ts).
const PROSE =
  ".prose{color:var(--tw-prose-body)}.prose :where(p){margin-top:1.25em}.prose-headings\\:font-semibold :where(h1,h2,th){font-weight:600}";
const HTML = (head = "") =>
  `<!doctype html><html><head><script type="module" crossorigin src="/assets/index-a.js"></script><link rel="stylesheet" crossorigin href="/assets/index-b.css">${head}</head><body><div id="root"></div></body></html>`;
const license = (names) =>
  JSON.stringify(
    names.map((name) => ({ name, version: "1.0.0", identifier: "MIT" })),
  );

/** Build conforme à la cible, modifié par `overrides` (null retire un fichier). */
function build(overrides = {}) {
  return write({
    "index.html": HTML(),
    "assets/index-a.js":
      'const h=()=>import("./highlighterCore-c.js");console.log("app",h);',
    "assets/highlighterCore-c.js": `const g=[${GRAMMARS}];export{g};`,
    "assets/index-b.css": `${FONT_FACE}.space-y-4{margin:0}.font-semibold{font-weight:600}${PROSE}`,
    "assets/KaTeX_Main-Regular-x.woff2": "wOF2",
    ".vite/license.json": license(REQUIRED_PACKAGES),
    ...overrides,
  });
}

/** Build du panneau VS Code conforme, modifié par `overrides`. */
function webviewBuild(overrides = {}) {
  return write({
    "index.js":
      'const h=()=>import(`./chunks/highlighterCore-c.js`);const s="http://www.w3.org/2000/svg";console.log("panel",h,s);',
    "chunks/highlighterCore-c.js": `const g=[${GRAMMARS}];export{g};`,
    "index.css": `${FONT_FACE.replace("/assets/", "./assets/")}.font-semibold{font-weight:600}${PROSE}`,
    "assets/KaTeX_Main-Regular-x.woff2": "wOF2",
    ".vite/license.json": license(REQUIRED_PACKAGES),
    ...overrides,
  });
}

function write(files) {
  mkdirSync(tmp, { recursive: true });
  const dir = mkdtempSync(path.join(tmp, "check-bundle-"));
  dirs.push(dir);
  for (const [file, content] of Object.entries(files)) {
    if (content === null) continue;
    mkdirSync(path.dirname(path.join(dir, file)), { recursive: true });
    writeFileSync(path.join(dir, file), content);
  }
  return dir;
}

const errorsOf = (overrides, options) =>
  checkBundle(build(overrides), options).errors;

describe("checkBundle", () => {
  it("accepts a build that matches the step 8 target", () => {
    const result = checkBundle(build());
    expect(result.errors).toEqual([]);
    expect(result.entry).toBe("assets/index-a.js");
    expect(result.grammarFiles).toEqual(["assets/highlighterCore-c.js"]);
    expect([...result.scopes].sort()).toEqual([...EXPECTED_SCOPES].sort());
  });

  it("refuses forbidden packages, grouped by reason", () => {
    const errors = errorsOf({
      ".vite/license.json": license([
        ...REQUIRED_PACKAGES,
        "mermaid",
        "@mermaid-js/parser",
        "cytoscape-fcose",
        "d3",
        "d3-array",
        "dagre-d3-es",
        "khroma",
        "dompurify",
        "lodash-es",
        "@shikijs/engine-oniguruma",
        "micromark-extension-math",
        "rehype-raw",
        "parse5",
        "vitest",
        "@testing-library/react",
      ]),
    });
    expect(errors).toHaveLength(5);
    expect(errors[0]).toMatch(/^Paquets interdits \(arbre de Mermaid/);
    expect(errors[0]).toContain("mermaid@1.0.0, @mermaid-js/parser@1.0.0");
    expect(errors[0]).toContain("… (+1)");
    expect(errors[1]).toContain("@shikijs/engine-oniguruma@1.0.0");
    expect(errors[2]).toContain("micromark-extension-math@1.0.0");
    expect(errors[3]).toContain("rehype-raw@1.0.0, parse5@1.0.0");
    expect(errors[4]).toContain("vitest@1.0.0, @testing-library/react@1.0.0");
  });

  it("does not mistake allowed packages for forbidden ones", () => {
    const errors = errorsOf({
      ".vite/license.json": license([
        ...REQUIRED_PACKAGES,
        "shiki",
        "@shikijs/core",
        "d3x",
        "rehype-katex",
        "micromark-extension-math-extra",
      ]),
    });
    expect(errors).toEqual([]);
  });

  it("requires the Markdown, math and highlighting packages", () => {
    const errors = errorsOf({
      ".vite/license.json": license(["react", "katex"]),
    });
    expect(errors).toEqual([
      "Paquets attendus absents (rendu Markdown, maths ou coloration incomplets) : streamdown, micromark-extension-llm-math, @shikijs/engine-javascript, @shikijs/langs",
    ]);
  });

  it("refuses WASM and non-woff2 font files", () => {
    const errors = errorsOf({
      "assets/onig.wasm": "\0asm",
      "assets/KaTeX_Main-Regular-y.ttf": "ttf",
      "assets/KaTeX_Main-Regular-z.woff": "woff",
      "fonts/x.otf": "otf",
    });
    // Fichiers parcourus dans l'ordre alphabétique (sortie stable).
    expect(errors).toEqual([
      "Fichiers interdits (polices KaTeX en woff2 seulement ; greffon katexWoff2Only de vite.config.ts) : assets/KaTeX_Main-Regular-y.ttf, assets/KaTeX_Main-Regular-z.woff, fonts/x.otf",
      "Fichiers interdits (moteur WASM ; Shiki doit utiliser le moteur JavaScript) : assets/onig.wasm",
    ]);
  });

  it("refuses non-woff2 font formats and duplicated @font-face in the CSS", () => {
    const errors = errorsOf({
      "assets/index-b.css": `${FONT_FACE}${FONT_FACE}@font-face{font-family:KaTeX_AMS;src:url(data:font/ttf;base64,AA==)format("truetype")}`,
    });
    expect(errors).toHaveLength(2);
    expect(errors[0]).toContain('format("truetype")');
    expect(errors[1]).toMatch(/^@font-face en double.*KaTeX_Main normal 400$/);
  });

  it("requires exactly the expected grammars", () => {
    const grammars = GRAMMARS.replace(
      '"scopeName":"source.go"',
      '"scopeName":"source.ruby"',
    );
    const errors = errorsOf({
      "assets/highlighterCore-c.js": `const g=[${grammars},${grammar("source.kotlin")}];`,
    });
    expect(errors).toEqual([
      "Grammaires inattendues, 2 (seuls les langages de src/lib/highlighterCore.ts sont embarqués) : source.kotlin, source.ruby",
      "Grammaires manquantes : source.go",
    ]);
  });

  it("reads escaped grammar JSON too", () => {
    const escaped = GRAMMARS.replace(
      '"scopeName":"source.go"',
      '\\"scopeName\\":\\"source.go\\"',
    );
    expect(escaped).not.toBe(GRAMMARS);
    const errors = errorsOf({
      "assets/highlighterCore-c.js": `const g=[${escaped}];`,
    });
    expect(errors).toEqual([]);
  });

  it("refuses the real GLSL grammar", () => {
    const errors = errorsOf({
      "assets/highlighterCore-c.js": `const g=[${GRAMMARS.replace(
        EMPTY_GLSL,
        grammar("source.glsl", "glsl").replace('"X"', '"GLSL"'),
      )}];`,
    });
    expect(errors).toEqual([
      "Grammaire GLSL complète embarquée (licence inconnue) : le greffon emptyGlslGrammar de vite.config.ts doit la remplacer par une grammaire vide",
    ]);
  });

  it("refuses a GLSL grammar that has patterns", () => {
    const errors = errorsOf({
      "assets/highlighterCore-c.js": `const g=[${GRAMMARS.replace(
        EMPTY_GLSL,
        '[{name:"glsl",scopeName:"source.glsl",patterns:[{match:"x"}]}]',
      )}];`,
    });
    expect(errors).toHaveLength(1);
    expect(errors[0]).toMatch(/^Grammaire source\.glsl non vide/);
  });

  const EAGER_GRAMMARS = (files) =>
    `Grammaires Shiki chargées au démarrage, dans le JS d'entrée ou ses imports statiques (${files}) : src/lib/highlighterCore.ts doit rester chargé à part, par import() depuis src/lib/highlighter.ts`;

  it("requires the highlighter to stay out of the entry chunk", () => {
    const result = checkBundle(
      build({
        "assets/index-a.js": `const g=[${GRAMMARS}];console.log(g);`,
        "assets/highlighterCore-c.js": null,
      }),
    );
    expect(result.errors).toEqual([EAGER_GRAMMARS("assets/index-a.js")]);
    expect(result.measures.highlighter).toBe(0);
  });

  it("requires the highlighter chunk to be imported on demand only", () => {
    // Import statique depuis un morceau lui-même importé par l'entrée.
    const result = checkBundle(
      build({
        "assets/index-a.js": 'import"./vendor-d.js";console.log("app");',
        "assets/vendor-d.js":
          'import{g as h}from"./highlighterCore-c.js";export{h};',
      }),
    );
    expect(result.errors).toEqual([
      EAGER_GRAMMARS("assets/highlighterCore-c.js"),
    ]);
    expect(result.eager).toEqual([
      "assets/highlighterCore-c.js",
      "assets/index-a.js",
      "assets/vendor-d.js",
    ]);
    // Module préchargé par index.html.
    expect(
      errorsOf({
        "index.html": HTML(
          '<link rel="modulepreload" crossorigin href="/assets/highlighterCore-c.js">',
        ),
      }),
    ).toEqual([EAGER_GRAMMARS("assets/highlighterCore-c.js")]);
  });

  it("counts the static imports of the entry in its size", () => {
    const dir = build({
      "assets/index-a.js":
        'import{r}from"./react-d.js";const h=()=>import("./highlighterCore-c.js");r(h);',
      "assets/react-d.js": `export const r=()=>"${"x".repeat(100)}";`,
    });
    const { errors, eager, measures } = checkBundle(dir);
    expect(errors).toEqual([]);
    expect(eager).toEqual(["assets/index-a.js", "assets/react-d.js"]);
    expect(measures.jsEntry).toBeGreaterThan(100);
    expect(measures.jsEntry + measures.highlighter).toBe(measures.jsTotal);
  });

  it("refuses code that the CSP blocks, but not look-alikes", () => {
    const errors = errorsOf({
      "assets/csp-d.js":
        'new Function("return 1");window.eval("1");importScripts("w.js");new SharedWorker("w.js");WebAssembly.instantiate(b);',
      "assets/fine-e.js":
        'callFunction(a);parseFunction(b);retrieval(c);evaluate(d);const s="eval";new Workers();',
    });
    expect(errors).toHaveLength(5);
    expect(errors.map((e) => e.split(" : ")[0])).toEqual([
      "Motif refusé par la CSP, new Function(",
      "Motif refusé par la CSP, eval(",
      "Motif refusé par la CSP, importScripts(",
      "Motif refusé par la CSP, new Worker(",
      "Motif refusé par la CSP, WebAssembly.",
    ]);
    for (const error of errors) expect(error).toContain("assets/csp-d.js");
    expect(errors.join("\n")).not.toContain("fine-e.js");
  });

  it("refuses test code", () => {
    const errors = errorsOf({
      "assets/sim-f.js": 'const m="chatonlm-ipc-simulator";',
      "assets/mocks-g.js":
        "console.warn(`[TAURI] Couldn't find callback id ${id}.`);",
    });
    expect(errors).toEqual([
      "Code de test embarqué, « chatonlm-ipc-simulator » (simulateur IPC de src/test/simulator.ts) : assets/sim-f.js",
      "Code de test embarqué, « Couldn't find callback id » (mocks de @tauri-apps/api) : assets/mocks-g.js",
    ]);
  });

  it("refuses inline scripts and styles in index.html", () => {
    const errors = errorsOf({
      "index.html": HTML("<script>window.x=1</script><style>body{}</style>"),
    });
    expect(errors).toHaveLength(2);
    expect(errors[0]).toMatch(/^index\.html : <script> en ligne/);
    expect(errors[1]).toMatch(/^index\.html : <style> en ligne/);
  });

  it("requires an entry script that exists", () => {
    expect(
      errorsOf({ "index.html": "<!doctype html><div id=root></div>" }),
    ).toEqual([
      "index.html : script d'entrée introuvable dans le build (aucun <script src>)",
    ]);
    expect(errorsOf({ "index.html": null })).toEqual([
      "index.html introuvable dans le build",
    ]);
  });

  it("refuses the default Streamdown component classes, variants excepted", () => {
    const errors = errorsOf({
      "assets/index-b.css": `${FONT_FACE}.ml-4{margin-left:1rem}.list-outside{}.list-disc{}.list-decimal{}.wrap-anywhere{}.text-primary{}.border-border{}:where(.divide-y>:not(:last-child)){}.border-l-4{}.bg-muted\\/80{}`,
    });
    expect(errors).toEqual([
      "Classes Tailwind des composants par défaut de Streamdown générées dans assets/index-b.css (absentes chez l'officiel ; ne pas les écrire dans src/ ni ajouter d'@source vers streamdown) : .ml-4, .list-outside, .list-disc, .list-decimal, .wrap-anywhere, .text-primary, .border-border, .divide-y, .border-l-4, .bg-muted",
    ]);
    expect(
      errorsOf({
        "assets/index-b.css": `${FONT_FACE}.hover\\:ml-4:hover{}.-ml-4{}.ml-40{}.space-y-4{}.divide-y-2{}.md\\:list-disc{}.border-l-4x{}`,
      }),
    ).toEqual([]);
  });

  it("measures the chunks and enforces the size limits", () => {
    const dir = build();
    const { measures } = checkBundle(dir);
    expect(measures.jsEntry).toBeGreaterThan(0);
    expect(measures.highlighter).toBeGreaterThan(measures.jsEntry);
    expect(measures.woff2).toBe(4);
    expect(measures.jsTotal).toBe(measures.jsEntry + measures.highlighter);
    const errors = checkBundle(dir, {
      limits: { jsEntry: 10, highlighter: 10, css: 10, woff2: 3 },
    }).errors;
    expect(errors.map((e) => e.split(" : ")[0])).toEqual([
      "Taille dépassée, JS d'entrée",
      "Taille dépassée, morceau du surligneur",
      "Taille dépassée, CSS",
      "Taille dépassée, polices woff2",
    ]);
    expect(errors[3]).toContain("4 o > 3 o");
  });

  it("reports a missing package list", () => {
    expect(errorsOf({ ".vite/license.json": null })).toEqual([
      ".vite/license.json illisible : vite build pas lancé, ou build.license retiré de vite.config.ts",
    ]);
  });

  it("refuses the not-prose guard of typography left in the CSS", () => {
    const errors = errorsOf({
      "assets/index-b.css": `${FONT_FACE}.prose :where(p):not(:where([class~=not-prose],[class~=not-prose] *)){margin-top:1.25em}`,
    });
    expect(errors).toEqual([
      "Garde not-prose de @tailwindcss/typography restée dans assets/index-b.css (greffon withoutNotProse de vite.config.ts, sinon recalcul de style ~20 fois plus lent dans WebKitGTK) : « :where(p):not(:where([class~=not-prose],[class~=not-prose] *)){margin »",
    ]);
    // Toute autre forme aussi (mise à jour de typography).
    expect(
      errorsOf({ "assets/index-b.css": `${FONT_FACE}.not-prose{all:unset}` }),
    ).toHaveLength(1);
  });
});

describe("checkWebviewBundle (VS Code panel)", () => {
  const webviewErrors = (overrides, options) =>
    checkWebviewBundle(webviewBuild(overrides), options).errors;

  it("accepts a panel build without index.html, entry and sheet at fixed names", () => {
    const result = checkWebviewBundle(webviewBuild());
    expect(result.errors).toEqual([]);
    expect(result.entry).toBe("index.js");
    expect(result.grammarFiles).toEqual(["chunks/highlighterCore-c.js"]);
    // Le build de l'app, lui, exige toujours son index.html.
    expect(checkBundle(webviewBuild()).errors).toContain(
      "index.html introuvable dans le build",
    );
  });

  it("requires index.js and index.css", () => {
    expect(
      webviewErrors({
        "index.js": null,
        "main-x.js": 'import("./chunks/highlighterCore-c.js");',
      }),
    ).toContain("Script d'entrée index.js introuvable dans le build");
    expect(
      webviewErrors({ "index.css": null, "style.css": FONT_FACE }).join("\n"),
    ).toMatch(/^Feuille index\.css introuvable/m);
  });

  it("refuses Tauri code", () => {
    expect(
      webviewErrors({
        "index.js":
          'const h=()=>import("./chunks/highlighterCore-c.js");window.__TAURI_INTERNALS__.invoke("x");',
      }).join("\n"),
    ).toMatch(/^Code de Tauri dans le panneau .*: index\.js$/m);
  });

  it("refuses remote resources, not mere addresses in the code", () => {
    const errors = webviewErrors({
      "index.css": `${FONT_FACE}@font-face{font-family:X;src:url(https://fonts.example/x.woff2)format("woff2")}`,
      "chunks/extra.js": 'import"https://cdn.example/x.js";',
    });
    expect(errors.join("\n")).toMatch(
      /^Ressource distante dans le panneau .*: index\.css, chunks\/extra\.js$/m,
    );
    expect(
      webviewErrors({
        "index.css": `${FONT_FACE}@import url(//cdn.example/a.css);`,
      }),
    ).toHaveLength(1);
  });

  it("applies the rules of the app build too", () => {
    expect(
      webviewErrors({ "assets/KaTeX_Main-Regular-x.woff": "wOFF" }).join("\n"),
    ).toMatch(/^Fichiers interdits/m);
    expect(
      webviewErrors({
        "index.js": `const g=[${GRAMMARS}];export{g};`,
      }).join("\n"),
    ).toMatch(/^Grammaires Shiki chargées au démarrage/m);
  });

  it("enforces its own size limits", () => {
    expect(
      webviewErrors({}, { limits: { ...WEBVIEW_LIMITS, css: 10 } }).join("\n"),
    ).toMatch(/^Taille dépassée, CSS/m);
  });

  it("accepts the real panel build when there is one", () => {
    const dist = path.resolve(import.meta.dirname, "../vscode/dist/webview");
    if (!existsSync(dist)) return;
    expect(checkWebviewBundle(dist).errors).toEqual([]);
  });
});

describe("checkSources", () => {
  /** Dossier avec src/ et index.html ; null retire un fichier. */
  function sources(files) {
    mkdirSync(tmp, { recursive: true });
    const dir = mkdtempSync(path.join(tmp, "check-sources-"));
    dirs.push(dir);
    for (const [file, content] of Object.entries({
      "index.html": HTML(),
      "src/main.tsx": 'export const a = <div className="prose" />;',
      ...files,
    })) {
      if (content === null) continue;
      mkdirSync(path.dirname(path.join(dir, file)), { recursive: true });
      writeFileSync(path.join(dir, file), content);
    }
    return dir;
  }

  it("refuses the not-prose class anywhere in the app sources", () => {
    const dir = sources({
      "src/components/Box.tsx":
        'import clsx from "clsx";\nexport const b = clsx("x", "not-prose");',
      "src/index.css": "@layer base {\n}\n.not-prose { color: red; }",
      "index.html": HTML('<div class="not-prose"></div>'),
      // Pas une source de l'app.
      "src/notes.md": "not-prose",
    });
    expect(checkSources(dir)).toEqual([
      "Classe not-prose dans les sources, sans effet ici (greffon withoutNotProse de vite.config.ts, sinon recalcul de style ~20 fois plus lent dans WebKitGTK) : src/components/Box.tsx:2, src/index.css:3, index.html:1",
    ]);
    expect(checkSources(sources({}))).toEqual([]);
  });

  it("reads the sources of the VS Code panel too", () => {
    const dir = sources({
      "vscode/webview/Panel.tsx":
        'export const p = <div className="not-prose" />;',
    });
    expect(checkSources(dir)).toEqual([
      "Classe not-prose dans les sources, sans effet ici (greffon withoutNotProse de vite.config.ts, sinon recalcul de style ~20 fois plus lent dans WebKitGTK) : vscode/webview/Panel.tsx:1",
    ]);
  });

  it("finds no not-prose class in the real sources", () => {
    expect(checkSources(path.resolve(import.meta.dirname, ".."))).toEqual([]);
  });
});

describe("withoutNotProse (vite.config.ts)", () => {
  const plugin = withoutNotProse();
  const context = {
    error(message) {
      throw new Error(message);
    },
  };
  const transform = (code, id = "/app/src/index.css") =>
    plugin.transform.call(context, code, id);

  it("runs right after Tailwind generates the CSS", () => {
    expect(plugin.enforce).toBe("pre");
    // Même groupe « pre » que Tailwind : l'ordre de la liste fait foi.
    const names = viteConfig.plugins
      .flat(Infinity)
      .filter((p) => p && p.enforce === "pre")
      .map((p) => p.name);
    const own = names.indexOf("chatonlm-without-not-prose");
    expect(own).toBeGreaterThan(
      names.indexOf("@tailwindcss/vite:generate:serve"),
    );
    expect(own).toBeGreaterThan(
      names.indexOf("@tailwindcss/vite:generate:build"),
    );
    expect(names.indexOf("@tailwindcss/vite:generate:build")).toBeGreaterThan(
      -1,
    );
  });

  it("removes the guard from the CSS of the dev server", () => {
    // Forme du serveur de dev (CSS imbriquée, guillemets, espaces), relevée
    // sur src/index.css.
    const dev = `.prose {
  color: var(--tw-prose-body);
  :where(p):not(:where([class~="not-prose"], [class~="not-prose"] *)) {
    margin-top: 1.25em;
  }
  :where(ol > li):not(:where([class~="not-prose"], [class~="not-prose"] *))::marker {
    color: var(--tw-prose-counters);
  }
}
.prose-headings\\:font-semibold :is(:where(h1, h2, h3, h4, h5, h6, th):not(:where([class~="not-prose"], [class~="not-prose"] *))) {
  font-weight: var(--font-weight-semibold);
}
.prose-ul\\:marker\\:text-neutral-700 :is(:where(ul):not(:where([class~="not-prose"], [class~="not-prose"] *))) ::marker {
  color: var(--color-neutral-700);
}`;
    expect(transform(dev, "/app/src/index.css?direct")).toEqual({
      code: `.prose {
  color: var(--tw-prose-body);
  :where(p) {
    margin-top: 1.25em;
  }
  :where(ol > li)::marker {
    color: var(--tw-prose-counters);
  }
}
.prose-headings\\:font-semibold :is(:where(h1, h2, h3, h4, h5, h6, th)) {
  font-weight: var(--font-weight-semibold);
}
.prose-ul\\:marker\\:text-neutral-700 :is(:where(ul)) ::marker {
  color: var(--color-neutral-700);
}`,
      map: null,
    });
  });

  it("removes the guard from the optimized CSS of a build", () => {
    const built =
      ".prose :where(p):not(:where([class~=not-prose],[class~=not-prose] *)){margin-top:1.25em}.prose :where(ol):not(:where([class~=not-prose],[class~=not-prose] *)),.prose :where(ul):not(:where([class~=not-prose],[class~=not-prose] *)){padding:0}.dark\\:prose-pre\\:bg-none:where(.dark,.dark *) :where(pre):not(:where([class~=not-prose],[class~=not-prose] *)){background-image:none}";
    expect(transform(built).code).toBe(
      ".prose :where(p){margin-top:1.25em}.prose :where(ol),.prose :where(ul){padding:0}.dark\\:prose-pre\\:bg-none:where(.dark,.dark *) :where(pre){background-image:none}",
    );
  });

  it("leaves other files alone", () => {
    expect(transform(".prose :where(p){margin:0}")).toBeNull();
    expect(
      transform('const s = ":not(.not-prose)";', "/app/src/Note.tsx"),
    ).toBeNull();
  });

  it("stops the build on another form of the guard", () => {
    expect(() =>
      transform(
        ".prose :where(p):not(:where(.not-prose, .not-prose *)){margin:0}",
      ),
    ).toThrow("garde not-prose de forme inattendue dans /app/src/index.css");
  });
});
