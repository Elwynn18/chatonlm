// @vitest-environment node
// Essais de la fabrication des icônes (make-icons.mjs) : petites tailles sans
// les rayures, format du fichier ICO, et icônes versionnées : tailles
// attendues, couleurs du dessin source, rayures au-dessus de 32 px seulement.
import { readFileSync } from "node:fs";
import path from "node:path";
import { inflateSync } from "node:zlib";
import { describe, expect, it } from "vitest";
import {
  ICO_SIZES,
  PNG_SIZES,
  drawingFor,
  ico,
  withoutStripes,
} from "./make-icons.mjs";

const ROOT = path.resolve(import.meta.dirname, "..");
const ICONS = path.join(ROOT, "src-tauri/icons");
const LOGO = readFileSync(path.join(ROOT, "assets/logo.svg"), "utf8");

/** Largeur et hauteur lues dans l'en-tête IHDR d'un PNG. */
function pngSize(png) {
  expect(png.subarray(0, 8).toString("latin1")).toBe("\x89PNG\r\n\x1a\n");
  return [png.readUInt32BE(16), png.readUInt32BE(20)];
}

/** Images d'un fichier ICO : taille annoncée et octets. */
function icoImages(file) {
  expect([file.readUInt16LE(0), file.readUInt16LE(2)]).toEqual([0, 1]);
  return Array.from({ length: file.readUInt16LE(4) }, (_, index) => {
    const entry = file.subarray(6 + 16 * index, 22 + 16 * index);
    const length = entry.readUInt32LE(8);
    const offset = entry.readUInt32LE(12);
    return {
      size: entry.readUInt8(0) || 256,
      height: entry.readUInt8(1) || 256,
      bits: entry.readUInt16LE(6),
      png: file.subarray(offset, offset + length),
    };
  });
}

const CREAM = [0xff, 0xf4, 0xe8];
const GINGER = [0xf2, 0x8a, 0x38];

/** Lecture des pixels (R, V, B, A) d'un PNG RGBA 8 bits non entrelacé. */
function pixels(png) {
  const [width] = pngSize(png);
  expect([png.readUInt8(24), png.readUInt8(25), png.readUInt8(28)]).toEqual([
    8, 6, 0,
  ]);
  const data = [];
  for (let at = 8; at < png.length;) {
    const length = png.readUInt32BE(at);
    if (png.subarray(at + 4, at + 8).toString("latin1") === "IDAT") {
      data.push(png.subarray(at + 8, at + 8 + length));
    }
    at += 12 + length;
  }
  const raw = inflateSync(Buffer.concat(data));
  const stride = 4 * width;
  const out = Buffer.alloc(raw.length - raw.length / (stride + 1));
  for (let y = 0; y * (stride + 1) < raw.length; y++) {
    const filter = raw[y * (stride + 1)];
    for (let x = 0; x < stride; x++) {
      const left = x >= 4 ? out[y * stride + x - 4] : 0;
      const up = y > 0 ? out[(y - 1) * stride + x] : 0;
      const upLeft = x >= 4 && y > 0 ? out[(y - 1) * stride + x - 4] : 0;
      const paeth = () => {
        const p = left + up - upLeft;
        const [a, b, c] = [left, up, upLeft].map((v) => Math.abs(p - v));
        return a <= b && a <= c ? left : b <= c ? up : upLeft;
      };
      const predicted = [0, left, up, (left + up) >> 1, paeth()][filter];
      out[y * stride + x] = raw[y * (stride + 1) + 1 + x] + predicted;
    }
  }
  return (x, y) => [
    ...out.subarray(y * stride + 4 * x, y * stride + 4 * x + 4),
  ];
}

const distance = (pixel, color) =>
  Math.max(...color.map((value, index) => Math.abs(pixel[index] - value)));
const near = (pixel, color) => pixel[3] === 255 && distance(pixel, color) <= 3;
const closerToCream = (pixel) =>
  distance(pixel, CREAM) < distance(pixel, GINGER);

describe("drawing", () => {
  it("drops only the stripes for the small sizes", () => {
    const plain = withoutStripes(LOGO);
    expect(plain).not.toContain("stripes");
    expect(plain).toContain('id="head"');
    expect(plain.match(/<ellipse /g)).toHaveLength(2);
    expect(LOGO.split("\n").length - plain.split("\n").length).toBe(1);
  });

  it("refuses a drawing whose stripes it cannot find", () => {
    expect(() => withoutStripes("<svg><path id='head'/></svg>")).toThrow(
      /stripes/,
    );
  });

  it("keeps the stripes above 32 px", () => {
    expect(drawingFor(LOGO, 16)).toBe(withoutStripes(LOGO));
    expect(drawingFor(LOGO, 32)).toBe(withoutStripes(LOGO));
    expect(drawingFor(LOGO, 48)).toBe(LOGO);
    expect(drawingFor(LOGO, 512)).toBe(LOGO);
  });
});

describe("ico", () => {
  it("lists each PNG with its size, 256 written as 0", () => {
    const a = Buffer.from("premier");
    const b = Buffer.from("le second");
    const file = ico([
      { size: 16, png: a },
      { size: 256, png: b },
    ]);
    expect(file.readUInt8(6 + 16)).toBe(0);
    const images = icoImages(file);
    expect(
      images.map(({ size, height, bits }) => [size, height, bits]),
    ).toEqual([
      [16, 16, 32],
      [256, 256, 32],
    ]);
    expect(images.map(({ png }) => png.toString())).toEqual([
      "premier",
      "le second",
    ]);
    expect(file.length).toBe(6 + 2 * 16 + a.length + b.length);
  });
});

describe("committed icons", () => {
  it("has the page icon equal to the source drawing", () => {
    expect(readFileSync(path.join(ROOT, "public/logo.svg"), "utf8")).toBe(LOGO);
  });

  it("has each PNG at the size of its name", () => {
    for (const size of PNG_SIZES) {
      const png = readFileSync(path.join(ICONS, `${size}x${size}.png`));
      expect(pngSize(png)).toEqual([size, size]);
    }
  });

  it("has the Windows icon with its six PNG images", () => {
    const images = icoImages(readFileSync(path.join(ICONS, "icon.ico")));
    expect(images.map(({ size }) => size)).toEqual(ICO_SIZES);
    for (const { size, png } of images) {
      expect(pngSize(png)).toEqual([size, size]);
    }
  });

  it("shows the ginger head on its cream square, striped above 32 px only", () => {
    const large = pixels(readFileSync(path.join(ICONS, "64x64.png")));
    expect(large(0, 0)[3]).toBe(0); // coin du carré arrondi
    expect(near(large(32, 3), CREAM)).toBe(true);
    expect(near(large(32, 35), GINGER)).toBe(true);
    // Rayure du milieu (x de 249,5 à 262,5 sur 512, soit les pixels 31 et 32).
    expect(closerToCream(large(31, 27))).toBe(true);
    expect(closerToCream(large(32, 27))).toBe(true);

    const small = pixels(readFileSync(path.join(ICONS, "32x32.png")));
    expect(near(small(16, 1), CREAM)).toBe(true);
    expect(near(small(15, 13), GINGER)).toBe(true);
    expect(near(small(16, 13), GINGER)).toBe(true);
  });
});
