// Mesure des contrastes de la page affichée (WCAG 2 : rapport de luminance
// entre un texte et son fond), dans l'app réelle. Sert à l'essai du thème
// (e2e/theme.test.mjs) et au relevé des deux thèmes pris à chaque capture
// quand E2E_THEME_AUDIT désigne un dossier (voir `auditThemes`).
import { appendFileSync, mkdirSync, writeFileSync } from "node:fs";
import { basename } from "node:path";
import { setTimeout as delay } from "node:timers/promises";

/** Rapport minimal d'un texte à lire. */
export const MIN_CONTRAST = 4.5;

/**
 * Script de page : chaque texte visible, avec sa couleur et celle du fond
 * réellement peint dessous (fonds translucides et opacités composés), et leur
 * rapport de contraste. Un texte par couple (classes, couleurs). `kind` :
 * "text", "placeholder" (texte d'invite d'un champ vide) ou "disabled" (dans
 * un contrôle désactivé, ou estompé à 60 % ou moins : la zone de saisie sous
 * son voile, les états disabled:opacity-60 et en dessous) ; `gradient` : un fond en dégradé ou en image est
 * sous le texte (rapport calculé sur la couleur de fond seule).
 */
const PROBE = `
  const canvas = document.createElement("canvas");
  canvas.width = canvas.height = 1;
  const context = canvas.getContext("2d", { willReadFrequently: true });
  const parsed = new Map();
  // Toute couleur CSS (oklch, color-mix...) en sRGB, par le canevas.
  const parse = (color) => {
    if (!parsed.has(color)) {
      context.clearRect(0, 0, 1, 1);
      context.fillStyle = "#000";
      context.fillStyle = color;
      context.fillRect(0, 0, 1, 1);
      const [r, g, b, a] = context.getImageData(0, 0, 1, 1).data;
      parsed.set(color, [r, g, b, a / 255]);
    }
    return parsed.get(color);
  };
  const over = (top, bottom) => {
    const a = top[3];
    return [0, 1, 2].map((i) => top[i] * a + bottom[i] * (1 - a)).concat(1);
  };
  const linear = (c) => {
    c /= 255;
    return c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
  };
  const luminance = ([r, g, b]) =>
    0.2126 * linear(r) + 0.7152 * linear(g) + 0.0722 * linear(b);
  const ratio = (a, b) => {
    const [high, low] = [luminance(a), luminance(b)].sort((x, y) => y - x);
    return (high + 0.05) / (low + 0.05);
  };
  const hex = (color) =>
    "#" + color.slice(0, 3).map((c) => Math.round(c).toString(16).padStart(2, "0")).join("");

  // Fond peint sous un élément : ses ancêtres, du plus proche au plus lointain,
  // jusqu'au premier fond opaque ; à défaut, le fond de la page.
  const backgroundOf = (element) => {
    const layers = [];
    let gradient = false;
    for (let node = element; node; node = node.parentElement) {
      const style = getComputedStyle(node);
      if (style.backgroundImage !== "none") gradient = true;
      const color = parse(style.backgroundColor);
      if (color[3] > 0) layers.push(color);
      if (color[3] >= 0.999) break;
    }
    let result = [255, 255, 255, 1];
    for (const layer of layers.reverse()) result = over(layer, result);
    return { color: result, gradient };
  };
  const opacityOf = (element) => {
    let opacity = 1;
    for (let node = element; node; node = node.parentElement) {
      opacity *= Number(getComputedStyle(node).opacity);
    }
    return opacity;
  };
  const visible = (element) => {
    const style = getComputedStyle(element);
    if (style.visibility !== "visible" || style.display === "none") return false;
    const box = element.getBoundingClientRect();
    if (box.width < 1 || box.height < 1) return false;
    // Texte réservé aux lecteurs d'écran (sr-only) : rien n'est peint.
    if (box.width <= 1 && box.height <= 1) return false;
    return box.bottom > 0 && box.right > 0 &&
      box.top < innerHeight && box.left < innerWidth;
  };

  const found = new Map();
  const record = (element, kind, text, colorText) => {
    const opacity = opacityOf(element);
    if (opacity < 0.05) return;
    const background = backgroundOf(element);
    const color = parse(colorText);
    const painted = over([color[0], color[1], color[2], color[3] * opacity], background.color);
    const style = getComputedStyle(element);
    const disabled = element.closest(
      "[disabled], [aria-disabled=true], [inert]",
    ) !== null || opacity <= 0.6;
    const entry = {
      kind: disabled ? "disabled" : kind,
      ratio: Math.round(ratio(painted, background.color) * 100) / 100,
      color: hex(painted),
      background: hex(background.color),
      gradient: background.gradient,
      size: Math.round(parseFloat(style.fontSize) * 10) / 10,
      weight: Number(style.fontWeight),
      tag: element.tagName.toLowerCase(),
      classes: String(element.getAttribute("class") ?? "").slice(0, 400),
      text: text.trim().replace(/\\s+/g, " ").slice(0, 60),
    };
    const key = [entry.kind, entry.classes, entry.color, entry.background].join("|");
    const known = found.get(key);
    if (!known || known.ratio > entry.ratio) found.set(key, entry);
  };

  const walker = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT);
  for (let node = walker.nextNode(); node; node = walker.nextNode()) {
    if (!node.nodeValue.trim()) continue;
    const element = node.parentElement;
    if (!element || element.closest("svg, option, script, style")) continue;
    if (!visible(element)) continue;
    // Texte découpé hors de sa boîte (clip, hauteur nulle d'un parent).
    const range = document.createRange();
    range.selectNodeContents(node);
    if (![...range.getClientRects()].some((box) => box.width >= 1 && box.height >= 1)) continue;
    record(element, "text", node.nodeValue, getComputedStyle(element).color);
  }
  for (const field of document.querySelectorAll("input, textarea")) {
    if (field.type === "hidden" || !visible(field)) continue;
    if (field.value) {
      record(field, "text", field.value, getComputedStyle(field).color);
    } else if (field.placeholder) {
      record(field, "placeholder", field.placeholder,
        getComputedStyle(field, "::placeholder").color);
    }
  }
  return [...found.values()];
`;

/** Textes de la page avec leur contraste (voir `PROBE`). */
export const measureContrast = (driver) => driver.execute(PROBE);

/**
 * Textes à lire sous le rapport minimal : ni invites de champs vides, ni
 * contrôles désactivés (hors du critère de WCAG, relevés à part).
 */
export const unreadable = (entries, min = MIN_CONTRAST) =>
  entries.filter((entry) => entry.kind === "text" && entry.ratio < min);

/** Impose un thème à la page, sans toucher au réglage ; rend l'état d'avant. */
export async function forceTheme(driver, dark) {
  const before = await driver.execute(
    `const root = document.documentElement;
     const before = { dark: root.classList.contains("dark"), scheme: root.style.colorScheme };
     root.classList.toggle("dark", arguments[0]);
     root.style.colorScheme = arguments[0] ? "dark" : "light";
     return before;`,
    [dark],
  );
  // Fin des transitions de couleur (150 à 300 ms).
  await delay(400);
  return before;
}

export const restoreTheme = (driver, before) =>
  driver.execute(
    `const root = document.documentElement;
     root.classList.toggle("dark", arguments[0].dark);
     root.style.colorScheme = arguments[0].scheme;`,
    [before],
  );

/**
 * Relevé des deux thèmes pour une capture `file` d'un essai : la page est
 * passée en clair puis en sombre, capturée dans `dir`
 * (`<nom>-light.png`, `<nom>-dark.png`) et mesurée (une ligne JSON par texte
 * dans `dir/contrast.jsonl`), puis rendue à son thème. Lancé par
 * `WebDriver.screenshot` quand E2E_THEME_AUDIT désigne `dir` : tous les écrans
 * que les essais capturent déjà sont ainsi vus et mesurés dans les deux thèmes.
 */
export async function auditThemes(driver, file, dir) {
  mkdirSync(dir, { recursive: true });
  const name = basename(file).replace(/\.png$/, "");
  let before = null;
  try {
    for (const theme of ["light", "dark"]) {
      const state = await forceTheme(driver, theme === "dark");
      before ??= state;
      const entries = await measureContrast(driver);
      for (const entry of entries) {
        appendFileSync(
          `${dir}/contrast.jsonl`,
          `${JSON.stringify({ shot: name, theme, ...entry })}\n`,
        );
      }
      const base64 = await driver.request("GET", driver.session("/screenshot"));
      writeFileSync(
        `${dir}/${name}-${theme}.png`,
        Buffer.from(base64, "base64"),
      );
    }
  } finally {
    if (before) {
      await restoreTheme(driver, before);
      await delay(400);
    }
  }
}
