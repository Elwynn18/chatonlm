// Icônes de l'app, toutes tirées du dessin source assets/logo.svg (tête de
// chaton rayée, roux sur crème) :
//
//   public/logo.svg                         copie (icône de la page)
//   src-tauri/icons/{32,64,128,256,512}.png paquets, zone de notification,
//                                           notifications, entrée de menu
//   src-tauri/icons/icon.ico                Windows : 16, 24, 32, 48, 64, 256
//
// Jusqu'à 32 px, les trois rayures du front (moins d'un pixel de large) ne
// donnent qu'une tache : ces tailles sont rendues sans elles.
//
// Rendu par rsvg-convert (librsvg, outil du système). Le résultat dépend de sa
// version : les icônes sont versionnées, ce script ne tourne qu'à la main,
// après un changement du dessin.
//
// Usage : node scripts/make-icons.mjs
import { execFileSync } from "node:child_process";
import { copyFileSync, readFileSync, writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = path.resolve(import.meta.dirname, "..");

/** Tailles des PNG de `src-tauri/icons`. */
export const PNG_SIZES = [32, 64, 128, 256, 512];
/** Tailles réunies dans `icon.ico`. */
export const ICO_SIZES = [16, 24, 32, 48, 64, 256];
/** Plus grande taille rendue sans les rayures. */
export const PLAIN_UP_TO = 32;

/** Le dessin sans les rayures du front, pour les petites tailles. */
export function withoutStripes(svg) {
  const out = svg.replace(/^[ \t]*<path id="stripes"[^>]*\/>\n?/m, "");
  if (out === svg) {
    throw new Error('élément <path id="stripes" …/> introuvable dans le logo');
  }
  return out;
}

/** Le dessin à rendre pour une icône de `size` pixels. */
export function drawingFor(svg, size) {
  return size <= PLAIN_UP_TO ? withoutStripes(svg) : svg;
}

/** Fichier ICO fait d'images PNG (`[{ size, png }]`, 32 bits par pixel). */
export function ico(images) {
  const header = Buffer.alloc(6);
  header.writeUInt16LE(1, 2); // type : icône
  header.writeUInt16LE(images.length, 4);
  let offset = header.length + 16 * images.length;
  const entries = images.map(({ size, png }) => {
    const entry = Buffer.alloc(16);
    entry.writeUInt8(size % 256, 0); // 0 = 256
    entry.writeUInt8(size % 256, 1);
    entry.writeUInt16LE(1, 4); // plans
    entry.writeUInt16LE(32, 6); // bits par pixel
    entry.writeUInt32LE(png.length, 8);
    entry.writeUInt32LE(offset, 12);
    offset += png.length;
    return entry;
  });
  return Buffer.concat([header, ...entries, ...images.map(({ png }) => png)]);
}

function render(svg, size) {
  return execFileSync(
    "rsvg-convert",
    ["--width", String(size), "--height", String(size), "--format", "png"],
    { input: drawingFor(svg, size), maxBuffer: 16 * 1024 * 1024 },
  );
}

function main() {
  const source = path.join(ROOT, "assets/logo.svg");
  const icons = path.join(ROOT, "src-tauri/icons");
  const svg = readFileSync(source, "utf8");
  copyFileSync(source, path.join(ROOT, "public/logo.svg"));
  for (const size of PNG_SIZES) {
    writeFileSync(path.join(icons, `${size}x${size}.png`), render(svg, size));
  }
  writeFileSync(
    path.join(icons, "icon.ico"),
    ico(ICO_SIZES.map((size) => ({ size, png: render(svg, size) }))),
  );
  console.log(
    `Icônes écrites : public/logo.svg, ${PNG_SIZES.length} PNG, icon.ico`,
  );
}

if (process.argv[1] === fileURLToPath(import.meta.url)) main();
