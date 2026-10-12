// @vitest-environment node
// Essais du manifeste des mises à jour d'une Release (release-manifest.mjs) :
// signatures vérifiées avec la clé publique (minisign : Ed25519, BLAKE2b-512,
// commentaire de confiance), version et nom de fichier signés, nouveautés et
// date lues dans le metainfo, plateformes et adresses, et le script entier
// sur un dossier fabriqué ici.
import { spawnSync } from "node:child_process";
import {
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import path from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import {
  assetUrl,
  checkSignature,
  configuredPublicKey,
  manifest,
  packageNames,
  releaseDate,
  releaseNotes,
  verifySignature,
  windowsInstallerName,
} from "./release-manifest.mjs";

const ROOT = path.resolve(import.meta.dirname, "..");
const FIXTURES = path.join(ROOT, "src-tauri/src/updater/fixtures");
const fixture = (name) => readFileSync(path.join(FIXTURES, name), "utf8");
// Clé de test et signatures de la version 9.9.9 (`tauri signer sign`), celles
// des essais du module updater (staging.rs).
const TEST_KEY = fixture("test.key.pub");
const PRODUCTION_KEY = fixture("production.key.pub");
const APPIMAGE_SIG = fixture("appimage-9.9.9.sig");
const DEB_SIG = fixture("deb-9.9.9.sig");
const APPIMAGE_AS_DEB_SIG = fixture("appimage-as-deb-9.9.9.sig");
const NSIS_SIG = fixture("nsis-9.9.9.sig");
const METAINFO = readFileSync(
  path.join(ROOT, "packaging/linux/com.chatonlm.chatonlm.metainfo.xml"),
  "utf8",
);
const NAMES = packageNames("9.9.9");

function pattern(len, seed) {
  const bytes = Buffer.alloc(len);
  for (let i = 0; i < len; i++) bytes[i] = (i * 31 + seed) % 256;
  return bytes;
}
/** Mêmes octets que test_appimage() de src-tauri/src/updater/staging.rs. */
function testAppImage() {
  const bytes = pattern(65536, 7);
  Buffer.from([0x7f, 0x45, 0x4c, 0x46, 2, 1, 1, 0, 0x41, 0x49, 2]).copy(bytes);
  bytes.writeBigUInt64LE(64n, 0x28);
  bytes.writeUInt16LE(64, 0x3a);
  bytes.writeUInt16LE(1, 0x3c);
  bytes.fill(0, 64 + 0x18, 64 + 0x28);
  Buffer.from("hsqs").copy(bytes, 128);
  return bytes;
}
/** Mêmes octets que test_deb(). */
function testDeb() {
  const bytes = pattern(65536, 11);
  Buffer.from("!<arch>\ndebian-binary   ").copy(bytes);
  return bytes;
}

/** Mêmes octets que test_nsis() de src-tauri/src/updater/install/format.rs. */
function testNsis() {
  const bytes = pattern(65536, 13);
  Buffer.from("MZ").copy(bytes);
  bytes.writeUInt32LE(128, 0x3c);
  Buffer.from([0x50, 0x45, 0, 0, 0x4c, 0x01]).copy(bytes, 128);
  bytes.fill(0, 1024, 1028);
  Buffer.from([0xef, 0xbe, 0xad, 0xde]).copy(bytes, 1028);
  Buffer.from("NullsoftInst").copy(bytes, 1032);
  return bytes;
}

/** Signature dont une ligne décodée est remplacée. */
function withLine(signature, index, line) {
  const lines = Buffer.from(signature.trim(), "base64")
    .toString("utf8")
    .split("\n");
  lines[index] = line;
  return Buffer.from(lines.join("\n")).toString("base64");
}

const dirs = [];
afterEach(() => {
  for (const dir of dirs.splice(0))
    rmSync(dir, { recursive: true, force: true });
});

describe("release-manifest", () => {
  it("vérifie une signature avec la clé publique, comme minisign", () => {
    expect(
      verifySignature(testAppImage(), APPIMAGE_SIG, TEST_KEY, "a"),
    ).toEqual({
      timestamp: expect.stringMatching(/^\d+$/),
      file: NAMES.appimage,
      version: "9.9.9",
    });
    // Autre clé (celle de production) : refusée avant tout calcul.
    expect(() =>
      verifySignature(testAppImage(), APPIMAGE_SIG, PRODUCTION_KEY, "a"),
    ).toThrow(/signée par une autre clé/);
    // Un octet du paquet changé.
    const tampered = testAppImage();
    tampered[5000] ^= 1;
    expect(() =>
      verifySignature(tampered, APPIMAGE_SIG, TEST_KEY, "a"),
    ).toThrow(/signature fausse/);
    // Signature d'un autre paquet.
    expect(() =>
      verifySignature(testAppImage(), DEB_SIG, TEST_KEY, "a"),
    ).toThrow(/signature fausse/);
    // Commentaire de confiance changé (autre version) : signature globale.
    const relabelled = withLine(
      APPIMAGE_SIG,
      2,
      `trusted comment: timestamp:1\tfile:${NAMES.appimage}\tversion:9.9.10`,
    );
    expect(() =>
      verifySignature(testAppImage(), relabelled, TEST_KEY, "a"),
    ).toThrow(/commentaire de confiance altéré/);
    expect(() =>
      verifySignature(testAppImage(), "pas du base64 !", TEST_KEY, "a"),
    ).toThrow(/illisible/);
    expect(() =>
      verifySignature(
        testAppImage(),
        withLine(APPIMAGE_SIG, 1, "AAAA"),
        TEST_KEY,
        "a",
      ),
    ).toThrow(/illisible/);
  });

  it("exige la version et le nom de fichier signés", () => {
    const check = (text, version, name, bytes) =>
      checkSignature(text, { version, name, bytes, pubkey: TEST_KEY });
    expect(
      check(`${APPIMAGE_SIG}\n`, "9.9.9", NAMES.appimage, testAppImage()),
    ).toBe(APPIMAGE_SIG.trim());
    expect(check(DEB_SIG, "9.9.9", NAMES.deb, testDeb())).toBe(DEB_SIG.trim());
    expect(() =>
      check(APPIMAGE_SIG, "9.9.10", NAMES.appimage, testAppImage()),
    ).toThrow(/signée pour la version 9\.9\.9, pas 9\.9\.10/);
    // Bons octets, signés sous le nom du .deb : refusés pour l'AppImage.
    expect(() =>
      check(APPIMAGE_AS_DEB_SIG, "9.9.9", NAMES.appimage, testAppImage()),
    ).toThrow(/signée pour le fichier chatonlm_9\.9\.9_amd64\.deb/);
    // Signature sans version (ancien format).
    expect(() =>
      check(
        fixture("payload-9.9.9.sig"),
        "9.9.9",
        "payload.bin",
        pattern(200000, 7),
      ),
    ).not.toThrow();
  });

  it("lit la clé publique de tauri.conf.json", () => {
    const config = JSON.parse(
      readFileSync(path.join(ROOT, "src-tauri/tauri.conf.json"), "utf8"),
    );
    expect(configuredPublicKey(config)).toBe(PRODUCTION_KEY.trim());
    expect(() => configuredPublicKey({})).toThrow(/pubkey absent/);
  });

  it("lit les nouveautés et la date de la version dans le metainfo", () => {
    expect(releaseNotes(METAINFO, "0.9.0")).toBe("First public beta.");
    expect(releaseDate(METAINFO, "0.9.0")).toMatch(
      /^\d{4}-\d{2}-\d{2}T00:00:00Z$/,
    );
    const metainfo = `<component><releases>
      <release version="1.0.0" date="2026-12-01">
        <description>
          <p>Stable  release &amp; more.</p>
          <ul><li>Faster <em>answers</em></li><li>Fixes</li></ul>
        </description>
      </release>
      <release version="1.0" date="2026-11-01"><description><p>Old</p></description></release>
      <release version="2.0.0"><description><p>No date</p></description></release>
      <release date="soon" version="3.0.0"><description><p>Bad</p></description></release>
    </releases></component>`;
    expect(releaseNotes(metainfo, "1.0.0")).toBe(
      "Stable release & more.\n\n- Faster answers\n- Fixes",
    );
    expect(releaseDate(metainfo, "1.0.0")).toBe("2026-12-01T00:00:00Z");
    expect(releaseDate(metainfo, "1.0")).toBe("2026-11-01T00:00:00Z");
    expect(() => releaseDate(metainfo, "2.0.0")).toThrow(/sans date/);
    expect(() => releaseDate(metainfo, "3.0.0")).toThrow(/sans date/);
    expect(() => releaseDate(metainfo, "1.0.1")).toThrow(/aucune <release/);
    expect(() => releaseNotes(metainfo, "1.0.1")).toThrow(/aucune <release/);
    expect(() =>
      releaseNotes('<release version="2.0.0"></release>', "2.0.0"),
    ).toThrow(/sans <description>/);
  });

  it("décrit l'AppImage, le .deb et le repli du greffon", () => {
    const result = manifest({
      version: "0.9.0",
      notes: "Notes",
      date: "2026-10-07T00:00:00Z",
      signatures: { appimage: "A", deb: "D" },
    });
    const appimage = {
      signature: "A",
      url: "https://github.com/Elwynn18/chatonlm/releases/download/v0.9.0/chatonlm_0.9.0_amd64.AppImage",
    };
    expect(result).toEqual({
      version: "0.9.0",
      notes: "Notes",
      pub_date: "2026-10-07T00:00:00Z",
      platforms: {
        "linux-x86_64-appimage": appimage,
        "linux-x86_64-deb": {
          signature: "D",
          url: "https://github.com/Elwynn18/chatonlm/releases/download/v0.9.0/chatonlm_0.9.0_amd64.deb",
        },
        "linux-x86_64": appimage,
      },
    });
    expect(packageNames("0.9.0").deb).toBe("chatonlm_0.9.0_amd64.deb");

    // Installateur Windows fourni : ses deux plateformes s'ajoutent, celles
    // de Linux ne changent pas.
    const both = manifest({
      version: "0.9.0",
      notes: "Notes",
      date: "2026-10-07T00:00:00Z",
      signatures: { appimage: "A", deb: "D", nsis: "N" },
    });
    const nsis = {
      signature: "N",
      url: "https://github.com/Elwynn18/chatonlm/releases/download/v0.9.0/chatonlm_0.9.0_x64-setup.exe",
    };
    expect(both).toEqual({
      ...result,
      platforms: {
        ...result.platforms,
        "windows-x86_64-nsis": nsis,
        "windows-x86_64": nsis,
      },
    });
    expect(windowsInstallerName("0.9.0")).toBe("chatonlm_0.9.0_x64-setup.exe");
    expect(assetUrl("1.2.3", "a b")).toMatch(/\/v1\.2\.3\/a%20b$/);
  });

  it("écrit latest.json après avoir vérifié chaque signature, ou échoue", () => {
    const tmp = path.join(ROOT, ".tools/tmp");
    mkdirSync(tmp, { recursive: true });
    const dir = mkdtempSync(path.join(tmp, "release-manifest-"));
    dirs.push(dir);
    const out = path.join(dir, "out");
    const signatures = path.join(dir, "signatures");
    mkdirSync(out);
    mkdirSync(signatures);
    const metainfo = path.join(dir, "metainfo.xml");
    writeFileSync(
      metainfo,
      '<releases><release version="9.9.9" date="2026-10-12"><description><p>Hello.</p></description></release></releases>',
    );
    const config = (key) => {
      const file = path.join(dir, "tauri.conf.json");
      writeFileSync(
        file,
        JSON.stringify({ plugins: { updater: { pubkey: key.trim() } } }),
      );
      return file;
    };
    writeFileSync(path.join(out, NAMES.appimage), testAppImage());
    writeFileSync(path.join(out, NAMES.deb), testDeb());
    writeFileSync(path.join(signatures, `${NAMES.appimage}.sig`), APPIMAGE_SIG);
    const run = (configFile) =>
      spawnSync(
        process.execPath,
        [
          path.join(ROOT, "scripts/release-manifest.mjs"),
          "--version",
          "9.9.9",
          "--dir",
          out,
          "--signatures",
          signatures,
          "--config",
          configFile,
          "--metainfo",
          metainfo,
          "--out",
          path.join(out, "latest.json"),
          "--notes-out",
          path.join(dir, "notes.md"),
        ],
        { encoding: "utf8" },
      );
    const missing = run(config(TEST_KEY));
    expect(missing.status).toBe(1);
    expect(missing.stderr).toMatch(/signature absente/);

    writeFileSync(path.join(signatures, `${NAMES.deb}.sig`), DEB_SIG);
    // Paquets signés par une autre clé que celle de l'app : échec.
    const otherKey = run(config(PRODUCTION_KEY));
    expect(otherKey.status).toBe(1);
    expect(otherKey.stderr).toMatch(/signée par une autre clé/);
    // Signature d'un paquet posée sur l'autre : échec.
    writeFileSync(path.join(signatures, `${NAMES.deb}.sig`), APPIMAGE_SIG);
    const swapped = run(config(TEST_KEY));
    expect(swapped.status).toBe(1);
    expect(swapped.stderr).toMatch(/signature fausse/);

    writeFileSync(path.join(signatures, `${NAMES.deb}.sig`), DEB_SIG);
    const ok = run(config(TEST_KEY));
    expect(ok.stderr).toBe("");
    expect(ok.status).toBe(0);
    const written = JSON.parse(
      readFileSync(path.join(out, "latest.json"), "utf8"),
    );
    expect(written.version).toBe("9.9.9");
    expect(written.notes).toBe("Hello.");
    expect(written.pub_date).toBe("2026-10-12T00:00:00Z");
    expect(written.platforms["linux-x86_64-deb"].signature).toBe(
      DEB_SIG.trim(),
    );
    expect(written.platforms["linux-x86_64-appimage"].signature).toBe(
      APPIMAGE_SIG.trim(),
    );
    expect(readFileSync(path.join(dir, "notes.md"), "utf8")).toBe("Hello.\n");
    // Sans installateur Windows : seulement Linux, comme avant.
    expect(Object.keys(written.platforms)).toEqual([
      "linux-x86_64-appimage",
      "linux-x86_64-deb",
      "linux-x86_64",
    ]);

    // Installateur Windows fourni : sa signature est exigée et vérifiée,
    // puis ses plateformes s'ajoutent sans changer celles de Linux.
    const nsis = windowsInstallerName("9.9.9");
    writeFileSync(path.join(out, nsis), testNsis());
    const unsigned = run(config(TEST_KEY));
    expect(unsigned.status).toBe(1);
    expect(unsigned.stderr).toMatch(/signature absente/);
    writeFileSync(path.join(signatures, `${nsis}.sig`), DEB_SIG);
    const wrong = run(config(TEST_KEY));
    expect(wrong.status).toBe(1);
    expect(wrong.stderr).toMatch(/signature fausse/);
    writeFileSync(path.join(signatures, `${nsis}.sig`), NSIS_SIG);
    const withWindows = run(config(TEST_KEY));
    expect(withWindows.stderr).toBe("");
    expect(withWindows.status).toBe(0);
    const both = JSON.parse(
      readFileSync(path.join(out, "latest.json"), "utf8"),
    );
    const installer = {
      signature: NSIS_SIG.trim(),
      url: `https://github.com/Elwynn18/chatonlm/releases/download/v9.9.9/${nsis}`,
    };
    expect(both.platforms).toEqual({
      ...written.platforms,
      "windows-x86_64-nsis": installer,
      "windows-x86_64": installer,
    });
  });
});
