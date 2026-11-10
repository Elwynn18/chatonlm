// Mise en page du panneau : la page ne défile jamais d'un bloc, les cartes
// ont leur zone bornée qui défile seule, la conversation garde le reste ; et
// la feuille du panneau (menu des modèles lisible en thème sombre).
// jsdom ne calcule pas la mise en page : la structure et les classes sont
// essayées ici, le rendu réel par les captures de VSCodium.
import { act, cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type {
  HostMessage,
  ImageView,
  PanelMessage,
  PanelState,
  StudyView,
  TourView,
} from "../shared/protocol";
import { Panel } from "./Panel";

const CONNECTED: PanelState = {
  connection: "connected",
  models: ["qwen3:0.6b"],
  remoteModels: [],
  model: "qwen3:0.6b",
  ephemeral: false,
  chatId: null,
  busy: false,
  generation: 0,
  notice: null,
};

const TOUR: TourView = {
  id: 1,
  kind: "file",
  path: "src/app.ts",
  status: "playing",
  segments: [
    { lines: [1, 3], text: "The imports." },
    { lines: [4, 9], text: "The main loop." },
  ],
  current: 0,
  receipt: null,
};

const STUDY: StudyView = {
  id: 1,
  kind: "project",
  topic: "How the work project works",
  status: "planning",
  courseId: "course-1",
  levelsReady: 0,
  levelsTotal: 0,
  error: null,
  files: ["README.md"],
  blocked: [],
};

const IMAGE: ImageView = {
  id: 1,
  target: "site/img/hero.png",
  prompt: "A red kitten",
  status: "ready",
  stage: null,
  step: null,
  total: null,
  cpu: false,
  preview: null,
  width: 512,
  height: 512,
  exists: false,
  error: null,
};

afterEach(() => cleanup());

function fromHost(data: HostMessage) {
  act(() => {
    window.dispatchEvent(new MessageEvent("message", { data }));
  });
}

function renderPanel() {
  const post = vi.fn<(message: PanelMessage) => void>();
  render(<Panel post={post} />);
  fromHost({ type: "init", locale: "en", theme: "light", state: CONNECTED });
}

const classes = (element: Element) => element.className.split(/\s+/);

describe("Panel layout", () => {
  it("keeps the cards in a bounded zone that scrolls by itself", () => {
    renderPanel();
    fromHost({ type: "tour", tour: TOUR });
    fromHost({ type: "study", study: STUDY });
    fromHost({ type: "image", image: IMAGE });

    const main = screen.getByRole("main");
    // La page ne défile jamais d'un bloc : ni la racine ni « main ».
    expect(classes(main)).toEqual(
      expect.arrayContaining(["min-h-0", "flex-1", "overflow-hidden"]),
    );
    expect(classes(main.parentElement!)).toContain("overflow-hidden");

    // Les cartes sont toutes dans leur zone, bornée en hauteur et qui défile.
    const cards = screen.getByTestId("cards");
    expect(cards.parentElement).toBe(main);
    expect(classes(cards)).toContain("overflow-y-auto");
    expect(cards.className).toMatch(/(^|\s)max-h-\[\d+%\](\s|$)/);
    for (const id of ["tour", "study", "image"]) {
      expect(cards.contains(screen.getByTestId(id))).toBe(true);
    }

    // Le corps est hors de cette zone, et prend le reste sans déborder.
    const body = screen.getByTestId("panel-body");
    expect(cards.contains(body)).toBe(false);
    expect(body.parentElement).toBe(main);
    expect(classes(body)).toEqual(
      expect.arrayContaining(["min-h-0", "flex-1"]),
    );
    expect(body.contains(screen.getByTestId("screen-empty"))).toBe(true);

    // Actions rapides et zone de saisie restent hors de « main » (fixes).
    const box = screen.getByRole("textbox", { name: "Message to ChatonLM" });
    expect(main.contains(box)).toBe(false);
    expect(
      main.contains(screen.getByRole("group", { name: "Quick actions" })),
    ).toBe(false);
  });

  it("gives everything to the conversation when no card is open", () => {
    renderPanel();
    expect(screen.queryByTestId("cards")).toBeNull();
    expect(screen.getByTestId("panel-body").parentElement).toBe(
      screen.getByRole("main"),
    );
  });
});

/** Feuilles lues par node:fs (vitest vide les feuilles importées). Le
 * panneau est compilé avec les types du navigateur, sans ceux de Node : le
 * module est chargé sans ses types, pour ce seul essai (une référence aux
 * types de Node les ouvrirait à tout le code du panneau). */
const { readFileSync } = (await import(
  /* @vite-ignore */ "node:fs" as string
)) as { readFileSync: (file: string, encoding: "utf8") => string };
const here = (import.meta as ImportMeta & { dirname: string }).dirname;
const read = (file: string) => readFileSync(`${here}/${file}`, "utf8");
const PANEL_CSS = read("index.css");
const APP_CSS = read("../../src/index.css");

/** Déclarations de la règle dont le sélecteur est exactement `selector`. */
function declarations(css: string, selector: string): Record<string, string> {
  const escaped = selector.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  const match = new RegExp(`(?:^|\\n)${escaped}\\s*\\{([^}]*)\\}`).exec(
    css.replace(/\/\*[\s\S]*?\*\//g, ""),
  );
  if (!match) throw new Error(`no rule for ${selector}`);
  return Object.fromEntries(
    match[1]
      .split(";")
      .map((line) => line.trim())
      .filter(Boolean)
      .map((line) => {
        const at = line.indexOf(":");
        return [line.slice(0, at).trim(), line.slice(at + 1).trim()];
      }),
  );
}

/** Valeur d'un jeton du thème, en clair (@theme) ou en sombre (« .dark »). */
function token(name: string, dark: boolean): string {
  const scope = dark ? /\n {2}\.dark \{([^}]*)\}/ : /@theme \{([^}]*)\}/;
  const block = scope.exec(APP_CSS)?.[1] ?? "";
  const value = new RegExp(`--color-${name}:\\s*(#[0-9a-f]{6})`).exec(block);
  return value ? value[1] : token(name, false);
}

function luminance(hex: string): number {
  const [r, g, b] = [1, 3, 5].map((at) => {
    const c = parseInt(hex.slice(at, at + 2), 16) / 255;
    return c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
  });
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}

function contrast(a: string, b: string): number {
  const [hi, lo] = [luminance(a), luminance(b)].sort((x, y) => y - x);
  return (hi + 0.05) / (lo + 0.05);
}

/** `var(--color-x)` → valeur du jeton x dans le thème. */
function resolve(value: string, dark: boolean): string {
  const name = /^var\(--color-([a-z0-9-]+)\)$/.exec(value)?.[1];
  if (!name) throw new Error(`not a theme color: ${value}`);
  return token(name, dark);
}

describe("Panel stylesheet", () => {
  it("never lets the whole page scroll", () => {
    const page = declarations(PANEL_CSS, "html,\nbody");
    expect(page.height).toBe("100%");
    expect(page.overflow).toBe("hidden");
  });

  it("draws the native menus in the theme of the panel", () => {
    // Menu natif d'Electron : clair ou sombre selon color-scheme, pas
    // selon le système (sinon fond blanc et texte crème en thème sombre).
    expect(declarations(PANEL_CSS, "html")["color-scheme"]).toBe("light");
    expect(declarations(PANEL_CSS, "html.dark")["color-scheme"]).toBe("dark");

    for (const [selector, dark] of [
      ["option", false],
      [".dark option", true],
    ] as const) {
      const rule = declarations(PANEL_CSS, selector);
      const text = resolve(rule.color, dark);
      const background = resolve(rule["background-color"], dark);
      expect(contrast(text, background)).toBeGreaterThanOrEqual(4.5);
    }
    // Les fonds suivent le thème : clair en clair, sombre en sombre.
    expect(
      luminance(
        resolve(declarations(PANEL_CSS, "option")["background-color"], false),
      ),
    ).toBeGreaterThan(0.5);
    expect(
      luminance(
        resolve(
          declarations(PANEL_CSS, ".dark option")["background-color"],
          true,
        ),
      ),
    ).toBeLessThan(0.1);
  });
});
