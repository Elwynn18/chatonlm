/* global document, window, Element */
// Essais du fil de l'accueil (src/scripts/site.js). Son découpage en tronçons
// d'abord (tronconner, parcourue), puis le script entier dans une page réduite
// aux calques du fil (repris du gabarit) et à quatre repères : à chaque image,
// seul le tronçon du bout du fil est touché, sur un calque qui s'arrête à lui.
// Tracé d'un seul <path>, le fil faisait repeindre toute la hauteur de la page
// à chaque image ; c'est à la maison, où il avance 3,4 s durant, que Brave a
// montré des bandes de page périmées.
import { readFileSync } from "node:fs";
import path from "node:path";
import { afterAll, beforeAll, describe, expect, it, vi } from "vitest";

// Longueur d'un tronçon (TRONCON de site.js), plus une corde ; débord d'un calque.
const TRONCON = 320 + 8;
const DEBORD = 4;

// Points d'un tracé « Mx yLx y x y … », et sa longueur.
const points = (d) => {
  const n = d.match(/-?\d+(\.\d+)?/g).map(Number);
  return Array.from({ length: n.length / 2 }, (_, i) => [
    n[2 * i],
    n[2 * i + 1],
  ]);
};
const longueur = (d) =>
  points(d).reduce(
    (somme, [x, y], i, tous) =>
      i ? somme + Math.hypot(x - tous[i - 1][0], y - tous[i - 1][1]) : 0,
    0,
  );

let site;
let defile = 0;
const calque = (attribut) => document.querySelector(`[${attribut}]`);
const traces = () => [...calque("data-fil-toile").querySelectorAll("path")];
const auBout = () => [...calque("data-fil-bout").querySelectorAll("path")];
// Fait défiler la page, puis laisse passer des images (16 ms chacune).
const defiler = (y, images) => {
  defile = y;
  window.dispatchEvent(new Event("scroll"));
  for (let i = 0; i < images; i += 1) vi.advanceTimersByTime(16);
};

beforeAll(async () => {
  vi.useFakeTimers();
  // Les trois calques du fil, tels que le gabarit de l'accueil les écrit.
  const gabarit = readFileSync(
    path.join(import.meta.dirname, "src/pages/home.html"),
    "utf8",
  );
  const calques = gabarit.match(/<svg class="fil[^>]*>.*<\/svg>/g);
  expect(calques).toHaveLength(3);
  // Repères : « x,y,largeur,hauteur » dans la page (la maison est un cadre).
  document.body.innerHTML = `<main>${calques.join("")}
    <i data-fil data-essai="100,60,0,0"></i>
    <i data-fil="perle" data-essai="600,500,0,0"></i>
    <i data-fil="maison" data-essai="300,900,400,300"></i>
    <i data-fil data-essai="80,2000,0,0"></i>
  </main>`;
  const rect = (x, y, width, height) => ({
    left: x,
    top: y,
    right: x + width,
    bottom: y + height,
    width,
    height,
  });
  vi.spyOn(Element.prototype, "getBoundingClientRect").mockImplementation(
    function mesure() {
      if (this.tagName === "MAIN") return rect(0, -defile, 1200, 2400);
      if (!this.dataset?.essai) return rect(0, 0, 0, 0);
      const [x, y, width, height] = this.dataset.essai.split(",").map(Number);
      return rect(x, y - defile, width, height);
    },
  );
  Element.prototype.getClientRects = () => [{}];
  window.matchMedia = () => ({ matches: false });
  window.ResizeObserver = class {
    observe() {}
  };
  site = await import("./src/scripts/site.js");
  // La pelote roule d'abord (1,1 s), puis le fil part et se pose.
  vi.advanceTimersByTime(1200);
  defiler(0, 300);
});

afterAll(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});

describe("découpage du fil en tronçons", () => {
  const courbe = [
    { x: 100, y: 60 },
    { x: 100, y: 700 },
    { x: 900, y: 500 },
    { x: 900, y: 1400 },
  ];
  const mur = [courbe[3], { x: 900, y: 2400 }];

  it("ne laisse aucun tronçon couvrir plus que sa longueur", () => {
    const { troncons, total } = site.tronconner(courbe[0], [courbe, mur]);
    expect(troncons.length).toBeGreaterThan(6);
    let debut = 0;
    let fin = [100, 60];
    for (const troncon of troncons) {
      expect(troncon.long).toBeLessThanOrEqual(TRONCON);
      expect(troncon.bas - troncon.haut).toBeLessThanOrEqual(TRONCON);
      expect(troncon.droite - troncon.gauche).toBeLessThanOrEqual(TRONCON);
      // Les tronçons se suivent : même point, longueur cumulée.
      expect(troncon.debut).toBeCloseTo(debut, 6);
      expect(points(troncon.d)[0]).toEqual(fin);
      // Ses tirets ont la longueur du tracé écrit, et il tient dans son rectangle.
      expect(longueur(troncon.d)).toBeCloseTo(troncon.long, 1);
      for (const [x, y] of points(troncon.d)) {
        expect(x).toBeGreaterThanOrEqual(troncon.gauche);
        expect(x).toBeLessThanOrEqual(troncon.droite);
        expect(y).toBeGreaterThanOrEqual(troncon.haut);
        expect(y).toBeLessThanOrEqual(troncon.bas);
      }
      debut += troncon.long;
      fin = points(troncon.d).at(-1);
    }
    expect(debut).toBeCloseTo(total, 6);
    expect(fin).toEqual([900, 2400]);
  });

  it("garde une droite droite : ses bouts seulement", () => {
    const { troncons, total } = site.tronconner(mur[0], [mur]);
    expect(total).toBeCloseTo(1000, 6);
    expect(troncons).toHaveLength(4);
    for (const troncon of troncons) expect(points(troncon.d)).toHaveLength(2);
  });

  it("mesure le trajet : longueur parcourue et point le plus bas atteint", () => {
    const boucle = [mur[1], { x: 900, y: 2300 }];
    const { parcouru, atteint, fins, total } = site.tronconner(courbe[0], [
      courbe,
      mur,
      boucle,
    ]);
    expect(fins).toHaveLength(3);
    expect(fins[1] - fins[0]).toBeCloseTo(1000, 6);
    expect(fins[2]).toBeCloseTo(total, 6);
    expect(parcouru).toHaveLength(atteint.length);
    expect(parcouru.at(-1)).toBeCloseTo(total, 6);
    for (let i = 1; i < parcouru.length; i += 1) {
      expect(parcouru[i]).toBeGreaterThanOrEqual(parcouru[i - 1]);
      expect(atteint[i]).toBeGreaterThanOrEqual(atteint[i - 1]);
    }
    // Le fil qui remonte ne fait pas remonter le point le plus bas.
    expect(atteint.at(-1)).toBe(2400);
  });

  it("n'a jamais qu'un tronçon en cours de tracé", () => {
    const { troncons, total } = site.tronconner(courbe[0], [courbe, mur]);
    for (let courant = 0; courant <= total; courant += 37) {
      const parts = troncons.map((t) => site.parcourue(t, courant));
      const enCours = parts.filter((p, i) => p > 0 && p < troncons[i].long);
      expect(enCours.length).toBeLessThanOrEqual(1);
      // Avant le bout du fil, tout est entier ; après, rien n'est tracé.
      const bout = parts.findIndex((p, i) => p < troncons[i].long);
      for (const [i, p] of parts.entries()) {
        if (bout === -1 || i < bout) expect(p).toBe(troncons[i].long);
        else if (i > bout) expect(p).toBe(0);
      }
      expect(parts.reduce((a, b) => a + b, 0)).toBeCloseTo(courant, 6);
    }
  });
});

describe("fil de la page", () => {
  it("pose les tronçons tracés dans la page et celui du bout sur son calque", () => {
    // Écran de 768 px : le fil s'arrête entre la perle et la maison.
    expect(traces().length).toBeGreaterThanOrEqual(2);
    for (const trace of traces()) expect(trace.style.strokeDasharray).toBe("");
    expect(auBout()).toHaveLength(1);
    const [bout] = auBout();
    expect(Number(bout.style.strokeDashoffset)).toBeGreaterThan(0);
    expect(Number(bout.style.strokeDashoffset)).toBeLessThan(
      Number(bout.style.strokeDasharray),
    );
    // Ils se suivent sans trou, du premier repère au bout du fil.
    const suite = [...traces(), bout].map((trace) =>
      points(trace.getAttribute("d")),
    );
    expect(suite[0][0]).toEqual([100, 60]);
    for (let i = 1; i < suite.length; i += 1)
      expect(suite[i][0]).toEqual(suite[i - 1].at(-1));
  });

  it("réduit le calque du bout à son tronçon, et celui des perles aux perles", () => {
    const bout = calque("data-fil-bout");
    const [x, y, large, haute] = bout
      .getAttribute("viewBox")
      .split(" ")
      .map(Number);
    expect(large).toBeLessThanOrEqual(TRONCON + 2 * DEBORD + 2);
    expect(haute).toBeLessThanOrEqual(TRONCON + 2 * DEBORD + 2);
    expect(bout.style.left).toBe(`${x}px`);
    expect(bout.style.top).toBe(`${y}px`);
    expect(bout.style.width).toBe(`${large}px`);
    expect(bout.style.height).toBe(`${haute}px`);
    // Le tronçon y tient, débord du trait compris.
    for (const [px, py] of points(auBout()[0].getAttribute("d"))) {
      expect(px - x).toBeGreaterThanOrEqual(DEBORD);
      expect(x + large - px).toBeGreaterThanOrEqual(DEBORD);
      expect(py - y).toBeGreaterThanOrEqual(DEBORD);
      expect(y + haute - py).toBeGreaterThanOrEqual(DEBORD);
    }
    const perles = calque("data-fil-perles");
    expect(perles.querySelectorAll("circle")).toHaveLength(1);
    expect(perles.getAttribute("viewBox")).toBe("580 480 40 40");
    expect(perles.style.left).toBe("580px");
  });

  it("ne touche, d'une image à l'autre, que le tronçon du bout", () => {
    const etat = (trace) =>
      `${trace.getAttribute("d")}|${trace.style.strokeDasharray}|${trace.style.strokeDashoffset}`;
    const avant = traces().map(etat);
    const [bout] = auBout();
    const tirets = bout.style.strokeDashoffset;
    defiler(12, 1);
    // Le fil a avancé : dans son tronçon, ou dans le suivant s'il l'a fini.
    const apres = traces().map(etat);
    expect(apres.slice(0, avant.length)).toEqual(avant);
    expect(apres.length - avant.length).toBeLessThanOrEqual(1);
    expect(auBout()).toHaveLength(1);
    if (auBout()[0] === bout)
      expect(bout.style.strokeDashoffset).not.toBe(tirets);
    else expect(traces()).toContain(bout);
    defiler(0, 300);
    expect(traces().map(etat)).toEqual(avant);
  });

  it("trace la maison tronçon après tronçon, puis les retire en remontant", () => {
    const depart = traces().length;
    // La maison arrive à l'écran : son tracé prend 3,4 s, à l'allure d'un crayon.
    defiler(700, 30);
    const enRoute = traces().length;
    defiler(700, 400);
    expect(enRoute).toBeGreaterThan(depart);
    expect(traces().length).toBeGreaterThan(enRoute + 3);
    expect(auBout().length).toBeLessThanOrEqual(1);
    for (const trace of traces()) expect(trace.style.strokeDasharray).toBe("");
    // Le sol de la maison (y = 900 + 96 % de 300) est tracé en entier.
    const sol = traces()
      .flatMap((trace) => points(trace.getAttribute("d")))
      .filter(([, y]) => y === 1188)
      .map(([x]) => x);
    expect(Math.min(...sol)).toBe(324);
    expect(Math.max(...sol)).toBe(676);
    defiler(0, 400);
    expect(traces()).toHaveLength(depart);
    expect(auBout()).toHaveLength(1);
  });
});
