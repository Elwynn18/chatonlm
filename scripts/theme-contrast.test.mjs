// Thème central de l'app (src/index.css) : chaque couple texte/fond que les composants
// emploient garde un rapport de contraste d'au moins 4,5:1 (WCAG 2), en clair
// et en sombre. Les valeurs sont lues dans la feuille de style : une couleur
// changée sans égard pour la lisibilité fait échouer l'essai. Les écrans
// réels sont mesurés à part (e2e/theme.test.mjs).
import { readFileSync } from "node:fs";
import path from "node:path";
import { describe, expect, it } from "vitest";

// Par le disque : vitest vide les feuilles de style importées.
const css = readFileSync(
  path.resolve(import.meta.dirname, "../src/index.css"),
  "utf8",
);

/** Déclarations `--color-*` du bloc qui va de `opening` à `closing`. */
function colorsOf(opening, closing) {
  const start = css.indexOf(opening);
  const end = css.indexOf(closing, start);
  expect(start, opening).toBeGreaterThanOrEqual(0);
  expect(end, opening).toBeGreaterThan(start);
  const colors = {};
  for (const [, name, value] of css
    .slice(start, end)
    .matchAll(/--color-([a-z0-9-]+):\s*(#[0-9a-f]{6})\b/g)) {
    colors[name] = value;
  }
  return colors;
}

const light = colorsOf("@theme {", "\n}");
// Thème sombre : les mêmes noms, redéfinis par la règle « .dark ».
const dark = { ...light, ...colorsOf("\n  .dark {", "\n  }") };

const channels = (hex) =>
  [1, 3, 5].map((i) => Number.parseInt(hex.slice(i, i + 2), 16));

function luminance(hex) {
  const [r, g, b] = channels(hex).map((value) => {
    const c = value / 255;
    return c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
  });
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}

function contrast(a, b) {
  const [high, low] = [luminance(a), luminance(b)].sort((x, y) => y - x);
  return (high + 0.05) / (low + 0.05);
}

/** `top` à l'opacité `alpha` sur `bottom` (fonds translucides). */
function over(top, bottom, alpha) {
  const under = channels(bottom);
  return `#${channels(top)
    .map((c, i) =>
      Math.round(c * alpha + under[i] * (1 - alpha))
        .toString(16)
        .padStart(2, "0"),
    )
    .join("")}`;
}

/** Couples sous le rapport demandé, lisibles dans le message d'échec. */
function failing(palette, texts, backgrounds, min) {
  const failures = [];
  for (const text of texts) {
    for (const background of backgrounds) {
      const [label, color] =
        typeof background === "string"
          ? [background, palette[background]]
          : background;
      const ratio = contrast(palette[text], color);
      if (ratio < min) {
        failures.push(`${text} sur ${label} : ${ratio.toFixed(2)}`);
      }
    }
  }
  return failures;
}

describe("thème central", () => {
  it("définit toutes les couleurs attendues, zinc identique à neutral", () => {
    const steps = [50, 100, 200, 300, 400, 500, 600, 700, 800, 900, 950];
    for (const palette of [light, dark]) {
      for (const step of steps) {
        expect(palette[`neutral-${step}`]).toMatch(/^#[0-9a-f]{6}$/);
        expect(palette[`zinc-${step}`]).toBe(palette[`neutral-${step}`]);
      }
      for (const name of ["white", "black", "accent", "accent-fg"]) {
        expect(palette[name], name).toMatch(/^#[0-9a-f]{6}$/);
      }
    }
  });

  it("clair : textes à 4,5:1 au moins sur les surfaces", () => {
    // Surface, surface 2 (barre latérale, survol), sable (bulle, pastilles).
    const surfaces = ["white", "neutral-50", "neutral-100", "neutral-200"];
    expect(
      failing(
        light,
        [
          "black",
          "neutral-900",
          "neutral-800",
          "neutral-700",
          "neutral-600",
          "neutral-500",
        ],
        surfaces,
        4.5,
      ),
    ).toEqual([]);
    // Roux lisible (liens, icône active) : jamais sur le sable.
    expect(
      failing(
        light,
        ["accent-fg"],
        ["white", "neutral-50", "neutral-100"],
        4.5,
      ),
    ).toEqual([]);
  });

  it("sombre : textes à 4,5:1 au moins sur les surfaces", () => {
    // Fond, surface 2 (zone de saisie, cartes), barre latérale (950 à 40 %).
    const sidebar = [
      "neutral-950/40",
      over(dark["neutral-950"], dark["neutral-900"], 0.4),
    ];
    const surfaces = ["neutral-950", "neutral-900", "neutral-800", sidebar];
    expect(
      failing(
        dark,
        [
          "white",
          "neutral-100",
          "neutral-200",
          "neutral-300",
          "neutral-400",
          "neutral-500",
          "accent-fg",
        ],
        surfaces,
        4.5,
      ),
    ).toEqual([]);
    // Élément relevé (bulle de l'utilisateur, pastilles, survol).
    expect(
      failing(
        dark,
        ["white", "neutral-100", "neutral-200", "neutral-300", "neutral-400"],
        ["neutral-700"],
        4.5,
      ),
    ).toEqual([]);
  });

  it("aplats : encre sur roux, crème sur encre, dans les deux thèmes", () => {
    for (const palette of [light, dark]) {
      // Élément actif (barre latérale, onglet, bascule) : texte neutral-900.
      expect(failing(palette, ["neutral-900"], ["accent"], 4.5)).toEqual([]);
      // Bouton principal : clair = encre, texte blanc ; sombre = l'inverse.
      expect(failing(palette, ["white"], ["neutral-900"], 4.5)).toEqual([]);
    }
  });

  it("erreurs et réussites (clair) : rouges du thème, vert doux et son texte", () => {
    expect(
      failing(
        light,
        ["red-600", "red-700", "red-800"],
        ["white", "neutral-100", "neutral-200"],
        4.5,
      ),
    ).toEqual([]);
    expect(failing(light, ["green-800"], ["green-100"], 4.5)).toEqual([]);
  });

  it("estompé (invites des champs, icônes) : 3:1 au moins", () => {
    expect(
      failing(light, ["neutral-400"], ["white", "neutral-100"], 3),
    ).toEqual([]);
  });
});
