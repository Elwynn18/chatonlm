#!/usr/bin/env node
// Manifeste des mises à jour d'une Release (`latest.json`, format statique de
// tauri-plugin-updater), et texte de la Release, à partir des paquets signés
// et de la description de la version dans le metainfo AppStream.
//
//   node scripts/release-manifest.mjs --version 0.9.0 --dir out \
//     --metainfo packaging/linux/com.chatonlm.chatonlm.metainfo.xml \
//     --out out/latest.json [--signatures <dossier des .sig>] \
//     [--config src-tauri/tauri.conf.json] [--notes-out release-notes.md]
//
// Plateformes : `linux-x86_64-appimage` et `linux-x86_64-deb` (choisies par le
// greffon selon le type de paquet installé), et `linux-x86_64` = AppImage
// (repli du greffon). Windows, seulement si l'installateur NSIS
// (`chatonlm_<version>_x64-setup.exe`) est dans --dir : `windows-x86_64-nsis`
// et `windows-x86_64`, vers cet installateur (sa signature est alors exigée
// et vérifiée comme les autres) ; sans lui, le manifeste reste celui de Linux
// seul. Attention : le greffon cherche sa plateforme à chaque vérification,
// même sans version plus récente ; dès qu'une version Windows est publiée,
// chaque Release suivante doit porter l'installateur, sinon ChatonLM pour
// Windows affiche une erreur. Adresses : assets de la Release `v<version>`
// (https://github.com/Elwynn18/chatonlm/releases/download/v<version>/…) ;
// l'app lit https://github.com/Elwynn18/chatonlm/releases/latest/download/latest.json,
// servi seulement pour la dernière Release publiée non « prerelease ».
// Signatures (`.sig` de `tauri signer sign`, à côté des paquets ou dans
// --signatures) : chacune VÉRIFIÉE sur son paquet avec la clé publique de
// l'app (`plugins.updater.pubkey` de --config) comme le fait minisign
// (Ed25519, préhachage BLAKE2b-512 pour les signatures « ED », signature
// globale du commentaire de confiance), puis version (`version:`, exigée par
// `requireSignedVersion`) et nom du paquet (`file:`) signés. Une signature
// d'une autre clé (clé de test, clé régénérée) fait échouer le script.
// Date (`pub_date`, « Released on » dans l'app) : celle du <release> de la
// version dans le metainfo, posée au jour de la publication.
import { createHash, createPublicKey, verify } from "node:crypto";
import { existsSync, readFileSync, writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

export const REPOSITORY = "Elwynn18/chatonlm";

/** Noms des paquets publiés pour `version` (sortie de package.sh). */
export function packageNames(version) {
  return {
    appimage: `chatonlm_${version}_amd64.AppImage`,
    deb: `chatonlm_${version}_amd64.deb`,
  };
}

/**
 * Nom de l'installateur Windows publié pour `version` (NSIS de Tauri :
 * `<productName>_<version>_x64-setup.exe`).
 */
export function windowsInstallerName(version) {
  return `chatonlm_${version}_x64-setup.exe`;
}

/** Adresse d'un asset de la Release `v<version>`. */
export function assetUrl(version, name) {
  return `https://github.com/${REPOSITORY}/releases/download/v${version}/${encodeURIComponent(name)}`;
}

/** Texte d'un fichier minisign donné en base64 (`.sig`, clé publique). */
function decodeMinisign(text, what) {
  const trimmed = text.trim();
  if (!/^[A-Za-z0-9+/=]+$/.test(trimmed)) {
    throw new Error(`${what} : base64 illisible`);
  }
  return Buffer.from(trimmed, "base64").toString("utf8").split("\n");
}

/** Ligne base64 d'un fichier minisign, de la taille attendue. */
function decodeLine(line, size, what) {
  const bytes = Buffer.from(line ?? "", "base64");
  if (!line || bytes.length !== size) {
    throw new Error(`${what} : ligne minisign illisible`);
  }
  return bytes;
}

/**
 * Clé publique minisign (fichier en base64, comme `plugins.updater.pubkey`) :
 * identifiant (8 octets) et clé Ed25519.
 */
export function parsePublicKey(text) {
  const lines = decodeMinisign(text, "clé publique");
  if (!lines[0]?.startsWith("untrusted comment:")) {
    throw new Error("clé publique : fichier minisign illisible");
  }
  const raw = decodeLine(lines[1], 42, "clé publique");
  if (raw.subarray(0, 2).toString("latin1") !== "Ed") {
    throw new Error("clé publique : algorithme inconnu");
  }
  return {
    keyId: raw.subarray(2, 10),
    key: createPublicKey({
      key: {
        kty: "OKP",
        crv: "Ed25519",
        x: raw.subarray(10).toString("base64url"),
      },
      format: "jwk",
    }),
  };
}

/**
 * Signature minisign (`.sig` de Tauri, en base64) : algorithme (« Ed » :
 * fichier signé tel quel ; « ED » : son BLAKE2b-512), identifiant de la clé,
 * signature, commentaire de confiance et signature globale.
 */
export function parseSignature(text, name) {
  const lines = decodeMinisign(text, `${name}.sig`);
  if (
    !lines[0]?.startsWith("untrusted comment:") ||
    !lines[2]?.startsWith("trusted comment: ")
  ) {
    throw new Error(`${name}.sig : signature minisign illisible`);
  }
  const raw = decodeLine(lines[1], 74, `${name}.sig`);
  const algorithm = raw.subarray(0, 2).toString("latin1");
  if (algorithm !== "Ed" && algorithm !== "ED") {
    throw new Error(`${name}.sig : algorithme inconnu`);
  }
  return {
    algorithm,
    keyId: raw.subarray(2, 10),
    signature: raw.subarray(10),
    trustedComment: lines[2].slice("trusted comment: ".length),
    globalSignature: decodeLine(lines[3], 64, `${name}.sig`),
  };
}

/**
 * Vérifie une signature minisign sur `bytes` avec la clé publique, comme
 * minisign et tauri-plugin-updater : même clé, signature du fichier (ou de
 * son BLAKE2b-512), puis signature globale (signature ‖ commentaire de
 * confiance). Renvoie les champs du commentaire de confiance
 * (`timestamp`, `file`, `version`), ou lève une erreur.
 */
export function verifySignature(bytes, signatureText, publicKeyText, name) {
  const publicKey = parsePublicKey(publicKeyText);
  const parsed = parseSignature(signatureText, name);
  if (!parsed.keyId.equals(publicKey.keyId)) {
    throw new Error(
      `${name}.sig : signée par une autre clé que celle de l'app (${parsed.keyId.toString("hex")})`,
    );
  }
  const message =
    parsed.algorithm === "ED"
      ? createHash("blake2b512").update(bytes).digest()
      : bytes;
  if (!verify(null, message, publicKey.key, parsed.signature)) {
    throw new Error(`${name}.sig : signature fausse pour ce fichier`);
  }
  const global = Buffer.concat([
    parsed.signature,
    Buffer.from(parsed.trustedComment, "utf8"),
  ]);
  if (!verify(null, global, publicKey.key, parsed.globalSignature)) {
    throw new Error(`${name}.sig : commentaire de confiance altéré`);
  }
  return Object.fromEntries(
    parsed.trustedComment.split("\t").map((field) => {
      const colon = field.indexOf(":");
      return colon < 0
        ? [field, ""]
        : [field.slice(0, colon), field.slice(colon + 1)];
    }),
  );
}

/**
 * Vérifie le contenu d'un `.sig` sur son paquet (`bytes`, nommé `name`)
 * avec la clé publique de l'app, puis la version et le nom de fichier
 * signés ; renvoie le texte tel quel ou lève une erreur.
 */
export function checkSignature(text, { version, name, bytes, pubkey }) {
  const fields = verifySignature(bytes, text, pubkey, name);
  if (fields.version !== version) {
    throw new Error(
      `${name}.sig : signée pour la version ${fields.version ?? "(aucune)"}, pas ${version}`,
    );
  }
  if (fields.file !== name) {
    throw new Error(
      `${name}.sig : signée pour le fichier ${fields.file ?? "(aucun)"}, pas ${name}`,
    );
  }
  return text.trim();
}

/** Décode les entités XML de base. */
function unescapeXml(text) {
  return text
    .replace(/&lt;/g, "<")
    .replace(/&gt;/g, ">")
    .replace(/&quot;/g, '"')
    .replace(/&apos;/g, "'")
    .replace(/&amp;/g, "&");
}

/**
 * Nouveautés de `version` dans le metainfo (élément <release> et sa
 * <description>), en texte brut : un paragraphe par <p>, « - » devant chaque
 * <li>. Erreur si la version n'y est pas décrite.
 */
export function releaseNotes(metainfo, version) {
  const releases = [
    ...metainfo.matchAll(/<release\b([^>]*)>([\s\S]*?)<\/release>/g),
  ];
  const release = releases.find(([, attributes]) =>
    new RegExp(`\\bversion="${version.replace(/\./g, "\\.")}"`).test(
      attributes,
    ),
  );
  if (!release) {
    throw new Error(`metainfo : aucune <release version="${version}">`);
  }
  const description = /<description>([\s\S]*?)<\/description>/.exec(release[2]);
  if (!description) {
    throw new Error(`metainfo : <release ${version}> sans <description>`);
  }
  const blocks = [];
  for (const [, tag, body] of description[1].matchAll(
    /<(p|ul|ol)\b[^>]*>([\s\S]*?)<\/\1>/g,
  )) {
    const clean = (text) =>
      unescapeXml(
        text
          .replace(/<[^>]+>/g, "")
          .replace(/\s+/g, " ")
          .trim(),
      );
    if (tag === "p") {
      blocks.push(clean(body));
    } else {
      const items = [...body.matchAll(/<li\b[^>]*>([\s\S]*?)<\/li>/g)].map(
        ([, item]) => `- ${clean(item)}`,
      );
      blocks.push(items.join("\n"));
    }
  }
  const notes = blocks.filter(Boolean).join("\n\n");
  if (!notes) {
    throw new Error(`metainfo : description vide pour ${version}`);
  }
  return notes;
}

/**
 * Date de publication de `version` (attribut `date` de son <release> dans le
 * metainfo), en RFC 3339 : `pub_date` de latest.json, « Released on » dans
 * l'app. Erreur si elle manque ou n'est pas une date.
 */
export function releaseDate(metainfo, version) {
  const release = [...metainfo.matchAll(/<release\b([^>]*)>/g)].find(
    ([, attributes]) =>
      new RegExp(`\\bversion="${version.replace(/\./g, "\\.")}"`).test(
        attributes,
      ),
  );
  if (!release) {
    throw new Error(`metainfo : aucune <release version="${version}">`);
  }
  const date = /\bdate="([^"]*)"/.exec(release[1])?.[1];
  if (
    !date ||
    !/^\d{4}-\d{2}-\d{2}$/.test(date) ||
    Number.isNaN(Date.parse(`${date}T00:00:00Z`))
  ) {
    throw new Error(
      `metainfo : <release ${version}> sans date de publication (date="AAAA-MM-JJ")`,
    );
  }
  return `${date}T00:00:00Z`;
}

/**
 * Manifeste `latest.json` à partir des signatures lues ; `signatures.nsis`
 * (installateur Windows fourni) ajoute les plateformes de Windows, après
 * celles de Linux, inchangées.
 */
export function manifest({ version, notes, date, signatures }) {
  const names = packageNames(version);
  const appimage = {
    signature: signatures.appimage,
    url: assetUrl(version, names.appimage),
  };
  const platforms = {
    "linux-x86_64-appimage": appimage,
    "linux-x86_64-deb": {
      signature: signatures.deb,
      url: assetUrl(version, names.deb),
    },
    "linux-x86_64": appimage,
  };
  if (signatures.nsis) {
    const nsis = {
      signature: signatures.nsis,
      url: assetUrl(version, windowsInstallerName(version)),
    };
    platforms["windows-x86_64-nsis"] = nsis;
    platforms["windows-x86_64"] = nsis;
  }
  return { version, notes, pub_date: date, platforms };
}

function parseArgs(argv) {
  const options = {};
  for (let i = 0; i < argv.length; i += 2) {
    const key = argv[i];
    const value = argv[i + 1];
    if (!key?.startsWith("--") || value === undefined) {
      throw new Error(`argument inattendu : ${key}`);
    }
    options[key.slice(2)] = value;
  }
  for (const required of ["version", "dir", "metainfo", "out"]) {
    if (!options[required]) throw new Error(`--${required} manquant`);
  }
  return options;
}

/** Clé publique de l'app (`plugins.updater.pubkey` de tauri.conf.json). */
export function configuredPublicKey(config) {
  const pubkey = config?.plugins?.updater?.pubkey;
  if (typeof pubkey !== "string" || !pubkey) {
    throw new Error("tauri.conf.json : plugins.updater.pubkey absent");
  }
  return pubkey;
}

function main() {
  const options = parseArgs(process.argv.slice(2));
  const { version, dir } = options;
  if (!/^\d+\.\d+\.\d+$/.test(version)) {
    throw new Error(`version invalide : ${version}`);
  }
  const config = options.config ?? "src-tauri/tauri.conf.json";
  const pubkey = configuredPublicKey(JSON.parse(readFileSync(config, "utf8")));
  const signaturesDir = options.signatures ?? dir;
  const names = packageNames(version);
  // Installateur Windows : seulement s'il est fourni.
  const windows = existsSync(path.join(dir, windowsInstallerName(version)));
  if (windows) names.nsis = windowsInstallerName(version);
  const signatures = {};
  for (const [kind, name] of Object.entries(names)) {
    const file = path.join(dir, name);
    if (!existsSync(file)) throw new Error(`paquet absent : ${file}`);
    const sig = path.join(signaturesDir, `${name}.sig`);
    if (!existsSync(sig)) throw new Error(`signature absente : ${sig}`);
    signatures[kind] = checkSignature(readFileSync(sig, "utf8"), {
      version,
      name,
      bytes: readFileSync(file),
      pubkey,
    });
  }
  const metainfo = readFileSync(options.metainfo, "utf8");
  const notes = releaseNotes(metainfo, version);
  const date = releaseDate(metainfo, version);
  const result = manifest({ version, notes, date, signatures });
  writeFileSync(options.out, `${JSON.stringify(result, null, 2)}\n`);
  if (options["notes-out"]) {
    writeFileSync(options["notes-out"], `${notes}\n`);
  }
  console.log(
    `${options.out} : ChatonLM ${version} (${date}), ${windows ? "Linux et Windows" : "Linux seul (pas d'installateur Windows)"}, signatures vérifiées`,
  );
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  try {
    main();
  } catch (error) {
    console.error(`release-manifest : ${error.message}`);
    process.exit(1);
  }
}
