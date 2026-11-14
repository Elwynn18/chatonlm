// Fixtures du décodage (scripts/make-audio-fixtures.sh) : réécrit en HE-AAC
// l'AudioSpecificConfig d'un M4A en AAC-LC produit par ffmpeg, comme le
// signalent les fichiers 139 de YouTube (signalisation explicite et
// hiérarchique : objet 5, ou 29 pour HE-AACv2, fréquence du cœur, fréquence de
// sortie double, puis objet 2 du cœur). Le son reste celui du cœur : le
// fichier vérifie la lecture de l'ASC, pas un décodage SBR.
//
// Usage : node scripts/make-audio-fixtures-heaac.mjs <entrée.m4a> <sortie.m4a> <5|29>
//
// Seules les tailles des boîtes qui contiennent l'ASC changent : l'entrée doit
// avoir sa boîte moov après les données, ou être fragmentée avec des
// décalages relatifs à chaque moof (default_base_moof, sans mfra), ce que
// produit le script appelant.
import { readFileSync, writeFileSync } from "node:fs";

const [input, output, objectArg] = process.argv.slice(2);
const object = Number(objectArg);
if (!input || !output || (object !== 5 && object !== 29)) {
  console.error(
    "usage: make-audio-fixtures-heaac.mjs <in.m4a> <out.m4a> <5|29>",
  );
  process.exit(2);
}

const RATES = [
  96000, 88200, 64000, 48000, 44100, 32000, 24000, 22050, 16000, 12000, 11025,
  8000, 7350,
];
// Boîtes qui en contiennent d'autres, et octets propres avant leurs enfants.
const CONTAINERS = { moov: 0, trak: 0, mdia: 0, minf: 0, stbl: 0, stsd: 8 };

const data = readFileSync(input);

/** Chemin [{start, type}] des boîtes qui mènent à `esds`, depuis la racine. */
function findEsds(start, end, path) {
  let at = start;
  while (at + 8 <= end) {
    const size = data.readUInt32BE(at);
    const type = data.toString("latin1", at + 4, at + 8);
    if (size < 8 || at + size > end)
      throw new Error(`boîte ${type} abîmée à ${at}`);
    const here = [...path, { start: at, type }];
    if (type === "esds") return here;
    if (type in CONTAINERS) {
      const found = findEsds(at + 8 + CONTAINERS[type], at + size, here);
      if (found) return found;
    } else if (type === "mp4a") {
      // Entrée audio : 28 octets (version 0), 16 de plus en version 1.
      const version = data.readUInt16BE(at + 16);
      const found = findEsds(
        at + 8 + 28 + (version === 1 ? 16 : 0),
        at + size,
        here,
      );
      if (found) return found;
    }
    at += size;
  }
  return null;
}

const path = findEsds(0, data.length, []);
if (!path) throw new Error("pas de boîte esds");
const esds = path[path.length - 1].start;

/** Descripteur MPEG-4 à `at` : étiquette, octets de longueur, contenu. */
function descriptor(at) {
  const tag = data[at];
  let length = 0;
  let bytes = 0;
  for (;;) {
    const byte = data[at + 1 + bytes];
    bytes += 1;
    length = (length << 7) | (byte & 0x7f);
    if (!(byte & 0x80)) break;
  }
  return { at, tag, bytes, length, body: at + 1 + bytes };
}

const es = descriptor(esds + 12);
if (es.tag !== 3) throw new Error("ES_Descriptor attendu");
const esFlags = data[es.body + 2];
let at = es.body + 3;
if (esFlags & 0x80) at += 2;
if (esFlags & 0x40) at += 1 + data[at];
if (esFlags & 0x20) at += 2;
const config = descriptor(at);
if (config.tag !== 4) throw new Error("DecoderConfigDescriptor attendu");
const specific = descriptor(config.body + 13);
if (specific.tag !== 5) throw new Error("DecoderSpecificInfo attendu");
const asc = data.subarray(specific.body, specific.body + specific.length);

// ASC d'AAC-LC sur 16 bits : objet (5), fréquence (4), canaux (4), GA (3).
const bits = (asc[0] << 8) | asc[1];
if (bits >> 11 !== 2) throw new Error("l'entrée doit être de l'AAC-LC");
const coreIndex = (bits >> 7) & 0xf;
const channels = object === 29 ? 1 : (bits >> 3) & 0xf;
const ga = bits & 0x7;
const outputIndex = RATES.indexOf(RATES[coreIndex] * 2);
if (outputIndex < 0) throw new Error("fréquence du cœur sans double");
// Objet (5) + cœur (4) + canaux (4) + sortie (4) + objet 2 (5) + GA (3) = 25 bits.
const packed =
  (((((object << 4) | coreIndex) << 4) | channels) << 4) | outputIndex;
const full = ((((packed << 5) | 2) << 3) | ga) << 7;
const newAsc = Buffer.from([
  full >>> 24,
  (full >>> 16) & 0xff,
  (full >>> 8) & 0xff,
  full & 0xff,
]);
const delta = newAsc.length - asc.length;

const patched = Buffer.concat([
  data.subarray(0, specific.body),
  newAsc,
  data.subarray(specific.body + specific.length),
]);
// Longueurs des descripteurs, sur le même nombre d'octets.
for (const d of [es, config, specific]) {
  let length = d.length + delta;
  if (length >= 1 << (7 * d.bytes))
    throw new Error("longueur de descripteur trop grande");
  for (let i = d.bytes - 1; i >= 0; i -= 1) {
    patched[d.at + 1 + i] = (length & 0x7f) | (i === d.bytes - 1 ? 0 : 0x80);
    length >>= 7;
  }
}
// Tailles des boîtes qui contiennent l'ASC.
for (const box of path) {
  patched.writeUInt32BE(patched.readUInt32BE(box.start) + delta, box.start);
}
// Entrée mp4a : fréquence et canaux de sortie, comme un vrai HE-AAC.
const entry = path.find((box) => box.type === "mp4a").start;
patched.writeUInt16BE(object === 29 ? 2 : channels, entry + 8 + 16);
patched.writeUInt32BE(RATES[outputIndex] * 65536, entry + 8 + 24);
writeFileSync(output, patched);
console.log(
  `${output} : ASC ${asc.toString("hex")} -> ${newAsc.toString("hex")}`,
);
