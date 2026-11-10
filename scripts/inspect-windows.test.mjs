// Essais de l'empaquetage Windows, tous lancés sous Linux : inspection de
// l'installateur NSIS (inspect-windows.mjs, sur des fichiers PE et un dossier
// d'installation fabriqués ici), notices Windows
// (package-notices-windows.mjs), configuration du bundle
// (src-tauri/tauri.windows.conf.json, packaging/windows/), crochets NSIS et
// workflows de la CI (dont le moteur d'images, sd-server).
import { existsSync, readFileSync } from "node:fs";
import path from "node:path";
import { describe, expect, it } from "vitest";
import {
  contentProblems,
  executionLevel,
  formatReport,
  HOST_ONLY_DLLS,
  importProblems,
  installerProblems,
  isNsisInstaller,
  llamaHookPatterns,
  MAX_INSTALLER_BYTES,
  oldOllamaHookErrors,
  parsePe,
  REQUIRED_FILES,
  RETIRED_DIRS,
  SD_SERVER_EXE,
  versionString,
} from "./inspect-windows.mjs";
import { EXCLUDED_ENGINES } from "./inspect-packages.mjs";
import { parseEnv, renderSdServerReadme } from "./package-notices.mjs";
import {
  renderWindowsComponents,
  WINDOWS_COMPONENT_TEXTS,
  WINDOWS_TARGET,
  windowsAboutConfig,
} from "./package-notices-windows.mjs";
import { renderLlamaServerReadme } from "./package-notices-llama.mjs";

const ROOT = path.resolve(import.meta.dirname, "..");
const text = (file) => readFileSync(path.join(ROOT, file), "utf8");
const json = (file) => JSON.parse(text(file));
/** Version du moteur d'images épinglée, et celle d'une construction. */
const SD_PINNED = "master-964-1d0a58a";
const SD_VERSION = `${SD_PINNED}+chatonlm.0123456789ab`;
const LLAMA_PINNED = "v0.9.0";
const LLAMA_VERSION = `${LLAMA_PINNED}+chatonlm.0123456789ab`;

/**
 * Fichier PE32+ minimal : une section, avec la table des imports
 * (`imports`), celle des imports différés (`delay`) et, si `signed`, un
 * répertoire de sécurité non vide.
 */
function fakePe({
  machine = 0x8664,
  imports = [],
  delay = [],
  signed = false,
} = {}) {
  const rva = 0x1000;
  const raw = 0x200;
  const importTable = 20 * (imports.length + 1);
  const delayTable = 32 * (delay.length + 1);
  let cursor = importTable + delayTable;
  const names = [...imports, ...delay].map((name) => {
    const at = cursor;
    cursor += name.length + 1;
    return at;
  });
  const data = Buffer.alloc(cursor);
  imports.forEach((name, i) => {
    data.writeUInt32LE(rva + names[i], i * 20 + 12);
    data.write(name, names[i], "latin1");
  });
  delay.forEach((name, i) => {
    const at = names[imports.length + i];
    data.writeUInt32LE(rva + at, importTable + i * 32 + 4);
    data.write(name, at, "latin1");
  });

  const header = Buffer.alloc(raw);
  header.writeUInt16LE(0x5a4d, 0);
  const pe = 0x80;
  header.writeUInt32LE(pe, 0x3c);
  header.writeUInt32LE(0x00004550, pe);
  header.writeUInt16LE(machine, pe + 4);
  header.writeUInt16LE(1, pe + 6);
  header.writeUInt16LE(240, pe + 20);
  const optional = pe + 24;
  header.writeUInt16LE(0x20b, optional);
  header.writeUInt32LE(16, optional + 108);
  const directories = optional + 112;
  if (imports.length > 0) {
    header.writeUInt32LE(rva, directories + 1 * 8);
    header.writeUInt32LE(importTable, directories + 1 * 8 + 4);
  }
  if (signed) {
    header.writeUInt32LE(raw + data.length, directories + 4 * 8);
    header.writeUInt32LE(8, directories + 4 * 8 + 4);
  }
  if (delay.length > 0) {
    header.writeUInt32LE(rva + importTable, directories + 13 * 8);
    header.writeUInt32LE(delayTable, directories + 13 * 8 + 4);
  }
  const section = optional + 240;
  header.write(".rdata", section, "latin1");
  header.writeUInt32LE(data.length, section + 8);
  header.writeUInt32LE(rva, section + 12);
  header.writeUInt32LE(data.length, section + 16);
  header.writeUInt32LE(raw, section + 20);
  return Buffer.concat([header, data]);
}

describe("fichiers PE", () => {
  it("lit la machine, les imports, les imports différés et la signature", () => {
    const pe = parsePe(
      fakePe({
        imports: ["KERNEL32.dll", "VCRUNTIME140.dll", "MSVCP140.dll"],
        delay: ["dbghelp.dll"],
        signed: true,
      }),
    );
    expect(pe).toEqual({
      machine: 0x8664,
      imports: ["kernel32.dll", "msvcp140.dll", "vcruntime140.dll"],
      delayImports: ["dbghelp.dll"],
      signed: true,
    });
    expect(parsePe(fakePe({ machine: 0x14c })).machine).toBe(0x14c);
    expect(parsePe(fakePe())).toMatchObject({ imports: [], signed: false });
  });

  it("refuse ce qui n'est pas un PE", () => {
    expect(() => parsePe(Buffer.from("#!/bin/sh\n"))).toThrow(/PE invalide/);
    expect(() => parsePe(Buffer.alloc(256))).toThrow(/MZ/);
    const broken = fakePe();
    broken.writeUInt32LE(0, 0x80);
    expect(() => parsePe(broken)).toThrow(/signature PE/);
  });

  it("lit une chaîne de la ressource de version", () => {
    const resource = Buffer.concat([
      Buffer.from("xx"),
      Buffer.from("ProductVersion\0", "utf16le"),
      Buffer.alloc(2),
      Buffer.from("0.9.0\0", "utf16le"),
      Buffer.from("ProductName\0ChatonLM\0", "utf16le"),
    ]);
    expect(versionString(resource, "ProductVersion")).toBe("0.9.0");
    expect(versionString(resource, "ProductName")).toBe("ChatonLM");
    expect(versionString(resource, "FileVersion")).toBeNull();
    // Le même nom ailleurs dans le binaire, suivi d'octets quelconques.
    const noise = Buffer.concat([
      Buffer.from("ProductVersion\0", "utf16le"),
      Buffer.from([0x4c, 0x39, 0xa2, 0x0e, 0xe6, 0x96, 0x00, 0x00]),
      resource,
    ]);
    expect(versionString(noise, "ProductVersion")).toBe("0.9.0");
  });
});

describe("installateur", () => {
  const nsis = (level, extra = 0) =>
    Buffer.concat([
      fakePe({ machine: 0x14c }),
      Buffer.from(
        `<requestedExecutionLevel level="${level}" uiAccess="false"/>`,
      ),
      Buffer.from([0, 0, 0, 0, 0xef, 0xbe, 0xad, 0xde]),
      Buffer.from("NullsoftInst"),
      Buffer.alloc(extra),
    ]);

  it("reconnaît un installateur NSIS sans élévation", () => {
    const buffer = nsis("asInvoker");
    expect(isNsisInstaller(buffer)).toBe(true);
    expect(executionLevel(buffer)).toBe("asInvoker");
    expect(installerProblems({ buffer, size: buffer.length })).toEqual({
      errors: [],
      warnings: [],
    });
  });

  it("refuse un autre fichier, une élévation, une taille démesurée", () => {
    const plain = fakePe();
    expect(isNsisInstaller(plain)).toBe(false);
    expect(isNsisInstaller(Buffer.from("NullsoftInst"))).toBe(false);
    expect(installerProblems({ buffer: plain, size: 1 }).errors).toEqual([
      "Ce fichier n'est pas un installateur NSIS (PE + en-tête NSIS).",
    ]);
    const admin = nsis("requireAdministrator");
    expect(
      installerProblems({ buffer: admin, size: admin.length }).errors,
    ).toEqual([
      "Manifeste de l'installateur : niveau « requireAdministrator » au lieu de asInvoker (installation sans administrateur)",
    ]);
    const user = nsis("asInvoker");
    const huge = installerProblems({
      buffer: user,
      size: MAX_INSTALLER_BYTES + 1,
    });
    expect(huge.errors).toHaveLength(1);
    expect(huge.errors[0]).toMatch(/moteur CUDA embarqué/);
  });
});

/** Dossier d'installation fabriqué : chemin → { text?, pe? }. */
function installation(overrides = {}) {
  const entries = new Map();
  for (const file of REQUIRED_FILES) entries.set(file, {});
  const set = (file, value) =>
    entries.set(file, { ...entries.get(file), ...value });
  set("chatonlm.exe", {
    pe: {
      imports: [
        "kernel32.dll",
        "vcruntime140.dll",
        "api-ms-win-crt-heap-l1-1-0.dll",
      ],
      productVersion: "0.9.0",
    },
  });
  set("onnxruntime.dll", {
    pe: {
      imports: [
        "msvcp140.dll",
        "msvcp140_1.dll",
        "vcruntime140_1.dll",
        "dxgi.dll",
      ],
    },
  });
  set(SD_SERVER_EXE, {
    pe: { imports: ["kernel32.dll", "vulkan-1.dll", "advapi32.dll"] },
  });
  set("sd-server/fallback/vulkan-1.dll", {
    pe: { imports: ["kernel32.dll", "cfgmgr32.dll"] },
  });
  set("sd-server/VERSION", { text: `${SD_VERSION}\n` });
  set("llama-server/llama-server.exe", {
    pe: {
      imports: [
        "kernel32.dll",
        "llama-server-impl.dll",
        "llama.dll",
        "msvcp140.dll",
        "vcruntime140.dll",
        "vcruntime140_1.dll",
      ],
    },
  });
  set("llama-server/ggml-cpu-haswell.dll", {
    pe: { imports: ["kernel32.dll", "ggml-base.dll"] },
  });
  set("llama-server/ggml-vulkan.dll", {
    pe: { imports: ["kernel32.dll", "ggml-base.dll", "vulkan-1.dll"] },
  });
  set("llama-server/fallback/vulkan-1.dll", {
    pe: { imports: ["kernel32.dll", "cfgmgr32.dll"] },
  });
  set("llama-server/VERSION", { text: `${LLAMA_VERSION}\n` });
  set("licenses/llama-server/README.txt", {
    text: renderLlamaServerReadme({
      env: {
        LLAMA_VERSION: LLAMA_PINNED,
        LLAMA_BUILD: "b12110",
        LLAMA_REPO: "https://github.com/ggml-org/llama.cpp",
        LLAMA_COMMIT: "cd959bae9",
        LLAMA_GGML_VERSION: "0.29.0",
      },
      licenseFiles: [],
      windows: true,
    }),
  });
  set("licenses/sd-server/README.txt", {
    text: renderSdServerReadme({
      env: {
        SDCPP_VERSION: SD_PINNED,
        SDCPP_REPO: "https://github.com/leejet/stable-diffusion.cpp",
        SDCPP_COMMIT: "1d0a58a",
        SDCPP_GGML_COMMIT: "d25b121",
      },
      licenseFiles: [],
      windows: true,
    }),
  });
  set("licenses/THIRD_PARTY_LICENSES.txt", {
    text: "-- Silero VAD --\n== Part 9. Windows installer only ==\n",
  });
  set("licenses/rust-crates.txt", {
    text: "(target x86_64-pc-windows-msvc, without)\n  sonora 0.2.0 (BSD)\n  webview2-com-sys 0.39.1 (MIT)\n",
  });
  set("licenses/windows-components.txt", { text: renderWindowsComponents() });
  for (const [file, value] of Object.entries(overrides)) {
    if (value === null) entries.delete(file);
    else set(file, value);
  }
  const content = (file) => entries.get(file)?.text ?? `contenu de ${file}`;
  const sdServer = new Map(
    [...entries.keys()]
      .filter((f) => f.startsWith("sd-server/"))
      .map((f) => [f.slice("sd-server/".length), `sum:${content(f)}`]),
  );
  const llamaServer = new Map(
    [...entries.keys()]
      .filter((f) => f.startsWith("llama-server/"))
      .map((f) => [f.slice("llama-server/".length), `sum:${content(f)}`]),
  );
  return {
    files: new Map([...entries.keys()].map((f) => [f, 1])),
    read: content,
    bytes: (file) => Buffer.from(content(file)),
    hash: (file) => `sum:${content(file)}`,
    pe: (file) => ({
      machine: 0x8664,
      imports: [],
      delayImports: [],
      signed: false,
      productVersion: null,
      ...entries.get(file)?.pe,
    }),
    reference: {
      onnxruntime: "sum:contenu de onnxruntime.dll",
      sdServer,
      sdServerPinned: SD_PINNED,
      llamaServer,
      llamaServerPinned: LLAMA_PINNED,
    },
    version: "0.9.0",
  };
}

describe("contenu de l'installateur", () => {
  it("accepte une installation complète", () => {
    expect(contentProblems(installation())).toEqual({
      errors: [],
      warnings: [],
    });
  });

  it("refuse un exécutable construit avec le greffon WebDriver des essais", () => {
    const { errors } = contentProblems(
      installation({
        "chatonlm.exe": { text: "MZ… element not interactable …" },
      }),
    );
    expect(errors).toEqual([
      "chatonlm.exe contient le greffon WebDriver des essais (« element not interactable ») : build avec la fonctionnalité e2e",
    ]);
  });

  it("signale un fichier attendu absent", () => {
    const { errors } = contentProblems(
      installation({ "licenses/NSIS-COPYING.txt": null }),
    );
    expect(errors).toEqual(["Fichier absent : licenses/NSIS-COPYING.txt"]);
  });

  it("signale une DLL du runtime Visual C++ demandée et non livrée", () => {
    const { errors } = contentProblems(
      installation({ "msvcp140_1.dll": null }),
    );
    expect(errors).toEqual([
      "Fichier absent : msvcp140_1.dll",
      "onnxruntime.dll : DLL demandée et non livrée à côté : msvcp140_1.dll",
    ]);
  });

  it("cherche les DLL du moteur de texte dans son dossier, pas à la racine de l'app", () => {
    // msvcp140.dll est livré dans llama-server\ et à la racine : sans la
    // copie du moteur, celle de la racine de l'app ne compte pas.
    const { errors } = contentProblems(
      installation({ "llama-server/msvcp140.dll": null }),
    );
    expect(errors).toEqual([
      "Fichier absent : llama-server/msvcp140.dll",
      "llama-server/llama-server.exe : DLL demandée et non livrée à côté : msvcp140.dll",
    ]);
    expect(HOST_ONLY_DLLS.has("vulkan-1.dll")).toBe(true);
  });

  it("avertit seulement pour une DLL inconnue qui peut être de Windows", () => {
    const result = contentProblems(
      installation({
        "chatonlm.exe": {
          pe: { imports: ["mfplat.dll"], productVersion: "0.9.0" },
        },
      }),
    );
    expect(result.errors).toEqual([]);
    expect(result.warnings).toHaveLength(1);
    expect(result.warnings[0]).toMatch(
      /^chatonlm\.exe : DLL non répertoriée \(mfplat\.dll\)/,
    );
  });

  it("refuse un exécutable qui n'est pas x64, sauf le désinstalleur de NSIS", () => {
    const { errors } = contentProblems(
      installation({
        "onnxruntime.dll": { pe: { machine: 0xaa64 } },
        "uninstall.exe": { pe: { machine: 0x14c } },
      }),
    );
    expect(errors).toEqual([
      "onnxruntime.dll : machine 0xaa64 au lieu de x64 (0x8664)",
    ]);
  });

  it("refuse tout fichier CUDA, ROCm ou HIP, où qu'il soit, et l'ancien Ollama intégré", () => {
    const { errors } = contentProblems(
      installation({
        "ggml-cuda.dll": {},
        "cublas64_13.dll": {},
        "cuda/cudart64_13.dll": {},
        "rocm/amdhip64_6.dll": {},
      }),
    );
    expect(errors).toEqual([
      "Fichier CUDA, ROCm ou HIP livré (pack CUDA téléchargé à part) : cublas64_13.dll",
      "Fichier CUDA, ROCm ou HIP livré (pack CUDA téléchargé à part) : cuda/cudart64_13.dll",
      "Fichier CUDA, ROCm ou HIP livré (pack CUDA téléchargé à part) : ggml-cuda.dll",
      "Fichier CUDA, ROCm ou HIP livré (pack CUDA téléchargé à part) : rocm/amdhip64_6.dll",
    ]);
    // Dans le dossier du moteur de texte aussi (et quelle que soit la casse).
    const engine = contentProblems(
      installation({ "llama-server/GGML-CUDA.DLL": {} }),
    );
    expect(engine.errors).toEqual([
      "Fichier CUDA, ROCm ou HIP livré (pack CUDA téléchargé à part) : llama-server/GGML-CUDA.DLL",
    ]);
    // L'ancien Ollama intégré et ses notices ne sont plus livrés.
    expect(RETIRED_DIRS).toEqual(["ollama/", "licenses/ollama/"]);
    const old = contentProblems(
      installation({
        "ollama/ollama.exe": {},
        "Licenses/Ollama/README.txt": { text: "x" },
      }),
    );
    expect(old.errors).toEqual([
      "Fichier de l'ancien Ollama intégré livré : Licenses/Ollama/README.txt",
      "Fichier de l'ancien Ollama intégré livré : ollama/ollama.exe",
    ]);
    // Rien de l'installation ordinaire n'y ressemble.
    for (const file of REQUIRED_FILES) {
      expect(EXCLUDED_ENGINES.test(file), file).toBe(false);
    }
  });

  it("compare ONNX Runtime et la version de l'exécutable", () => {
    const ort = installation();
    ort.reference.onnxruntime = "autre";
    expect(contentProblems(ort).errors).toEqual([
      "onnxruntime.dll : différent de la copie vérifiée",
    ]);
    const exe = installation({
      "chatonlm.exe": { pe: { imports: [], productVersion: "0.8.0" } },
    });
    expect(contentProblems(exe).errors).toEqual([
      "chatonlm.exe : version 0.8.0 dans ses ressources au lieu de 0.9.0",
    ]);
  });

  it("exige les mentions des notices et avertit sans copie de référence", () => {
    const notices = installation({
      "licenses/rust-crates.txt": {
        text: "(target x86_64-unknown-linux-gnu)\n  sonora 0.2.0\n",
      },
    });
    expect(contentProblems(notices).errors).toEqual([
      "licenses/rust-crates.txt : mention absente (target x86_64-pc-windows-msvc)",
      "licenses/rust-crates.txt : mention absente (^\\s*webview2-com-sys \\d)",
    ]);
    const bare = installation();
    bare.reference = {
      onnxruntime: null,
      sdServer: null,
      sdServerPinned: null,
      llamaServer: null,
      llamaServerPinned: null,
    };
    const result = contentProblems(bare);
    expect(result.errors).toEqual([]);
    expect(result.warnings).toHaveLength(3);
    expect(result.warnings[0]).toMatch(
      /^Copie de référence d'ONNX Runtime absente/,
    );
    expect(result.warnings[1]).toMatch(
      /^Copie de référence du moteur d'images absente/,
    );
    expect(result.warnings[2]).toMatch(
      /^Copie de référence du moteur de texte absente/,
    );
  });

  it("exige le moteur d'images, identique au dossier construit, sans fichier en trop", () => {
    for (const file of [
      SD_SERVER_EXE,
      "sd-server/fallback/vulkan-1.dll",
      "sd-server/VERSION",
      "licenses/sd-server/README.txt",
    ]) {
      expect(REQUIRED_FILES).toContain(file);
    }
    // Absent : fichier exigé, et présent dans la copie de référence.
    const missing = installation({ [SD_SERVER_EXE]: null });
    missing.reference.sdServer.set("sd-server.exe", "sum");
    expect(contentProblems(missing).errors).toEqual([
      `Fichier absent : ${SD_SERVER_EXE}`,
      `Moteur d'images : fichier absent de l'installateur : ${SD_SERVER_EXE}`,
    ]);
    const base = installation();
    const changed = {
      ...base,
      hash: (file) => (file === SD_SERVER_EXE ? "autre" : base.hash(file)),
    };
    expect(contentProblems(changed).errors).toEqual([
      `Moteur d'images : fichier modifié : ${SD_SERVER_EXE}`,
    ]);
    const extra = {
      ...base,
      files: new Map([
        ...base.files,
        ["sd-server/sd-cli.exe", 1],
        ["sd-server/fallback/vulkan-1.pdb", 1],
      ]),
    };
    expect(contentProblems(extra).errors).toEqual([
      "Moteur d'images : fichier inattendu : sd-server/sd-cli.exe",
      "Moteur d'images : fichier inattendu : sd-server/fallback/vulkan-1.pdb",
    ]);
  });

  it("exige la version épinglée du moteur d'images", () => {
    const old = "master-963-0000000+chatonlm.0123456789ab";
    // Copie de référence reconstruite avec la même version : seule la forme
    // est fausse (packaging/sdcpp.env changé sans reconstruire).
    const stale = installation({ "sd-server/VERSION": { text: `${old}\n` } });
    expect(contentProblems(stale).errors).toEqual([
      `sd-server/VERSION : « ${old} » au lieu de ${SD_PINNED}+chatonlm.<empreinte> (packaging/sdcpp.env)`,
    ]);
    // Paquet construit avec une autre version que le dossier de référence.
    const built = installation();
    const other = {
      ...built,
      read: (file) =>
        file === "sd-server/VERSION" ? `${old}\n` : built.read(file),
      hash: (file) =>
        file === "sd-server/VERSION" ? `sum:${old}\n` : built.hash(file),
    };
    expect(contentProblems(other).errors).toEqual([
      "Moteur d'images : fichier modifié : sd-server/VERSION",
      `sd-server/VERSION : « ${old} » au lieu de ${SD_PINNED}+chatonlm.<empreinte> (packaging/sdcpp.env)`,
    ]);
  });

  it("refuse un sd-server.exe qui importe le runtime Visual C++ ou OpenMP", () => {
    const { errors } = contentProblems(
      installation({
        [SD_SERVER_EXE]: {
          pe: {
            imports: ["kernel32.dll", "vcruntime140.dll", "msvcp140.dll"],
            delayImports: ["vcomp140.dll"],
          },
        },
      }),
    );
    expect(errors).toEqual([
      `${SD_SERVER_EXE} : DLL du runtime Visual C++ ou d'OpenMP importée (CRT statique attendu) : vcruntime140.dll, msvcp140.dll, vcomp140.dll`,
      // Celles de la racine de l'app ne sont pas dans les dossiers de
      // recherche du serveur.
      `${SD_SERVER_EXE} : DLL demandée et non livrée à côté : vcruntime140.dll`,
      `${SD_SERVER_EXE} : DLL demandée et non livrée à côté : msvcp140.dll`,
    ]);
    const ucrt = contentProblems(
      installation({
        [SD_SERVER_EXE]: {
          pe: { imports: ["api-ms-win-crt-heap-l1-1-0.dll"] },
        },
      }),
    );
    expect(ucrt.errors).toEqual([
      `${SD_SERVER_EXE} : DLL du runtime Visual C++ ou d'OpenMP importée (CRT statique attendu) : api-ms-win-crt-heap-l1-1-0.dll`,
    ]);
  });

  it("trouve les DLL du chargeur de secours dans le dossier du serveur, le chargeur pouvant manquer", () => {
    // vulkan-1.dll : fourni par le pilote, sinon fallback/ (fin du PATH).
    expect(HOST_ONLY_DLLS.has("vulkan-1.dll")).toBe(true);
    const withoutFallback = installation({
      "sd-server/fallback/vulkan-1.dll": null,
    });
    withoutFallback.reference.sdServer.set("fallback/vulkan-1.dll", "sum");
    expect(contentProblems(withoutFallback).errors).toEqual([
      "Fichier absent : sd-server/fallback/vulkan-1.dll",
      "Moteur d'images : fichier absent de l'installateur : sd-server/fallback/vulkan-1.dll",
    ]);
    // Une DLL que le chargeur demande, livrée à côté de sd-server.exe.
    const helper = contentProblems(
      installation({
        "sd-server/fallback/vulkan-1.dll": {
          pe: { imports: ["kernel32.dll", "libhelper.dll"] },
        },
        "sd-server/libhelper.dll": {},
      }),
    );
    expect(helper.errors).toEqual([]);
    // Mais pas celle de la racine de l'app.
    const root = contentProblems(
      installation({
        "sd-server/fallback/vulkan-1.dll": {
          pe: { imports: ["kernel32.dll", "vcruntime140.dll"] },
        },
      }),
    );
    expect(root.errors).toEqual([
      "sd-server/fallback/vulkan-1.dll : DLL demandée et non livrée à côté : vcruntime140.dll",
    ]);
  });

  it("exige le moteur de texte, identique au dossier construit, avec son runtime Visual C++ et à la version épinglée", () => {
    // Fichier en trop (variante d'une autre version), fichier modifié.
    const extra = installation({
      "llama-server/ggml-cpu-old.dll": { pe: { imports: ["kernel32.dll"] } },
    });
    extra.reference.llamaServer.delete("ggml-cpu-old.dll");
    expect(contentProblems(extra).errors).toEqual([
      "Moteur de texte : fichier inattendu : llama-server/ggml-cpu-old.dll",
    ]);
    const changed = installation();
    changed.reference.llamaServer.set("llama.dll", "sum:autre");
    expect(contentProblems(changed).errors).toEqual([
      "Moteur de texte : fichier modifié : llama-server/llama.dll",
    ]);
    // Runtime Visual C++ en DLL : accepté parce qu'il est livré dans le
    // dossier du moteur, refusé sinon ; OpenMP jamais, pour aucune DLL.
    const crt = installation({ "llama-server/msvcp140.dll": null });
    crt.reference.llamaServer.delete("msvcp140.dll");
    expect(contentProblems(crt).errors).toEqual([
      "Fichier absent : llama-server/msvcp140.dll",
      "llama-server/llama-server.exe : DLL demandée et non livrée à côté : msvcp140.dll",
    ]);
    const omp = installation({
      "llama-server/ggml-cpu-haswell.dll": {
        pe: { imports: ["ggml-base.dll", "vcomp140.dll"] },
      },
    });
    expect(contentProblems(omp).errors).toEqual([
      "llama-server/ggml-cpu-haswell.dll : DLL d'OpenMP, de GCC ou d'OpenSSL importée : vcomp140.dll",
      "llama-server/ggml-cpu-haswell.dll : DLL demandée et non livrée à côté : vcomp140.dll",
    ]);
    const version = installation({
      "llama-server/VERSION": { text: "b12110\n" },
    });
    expect(contentProblems(version).errors).toEqual([
      `llama-server/VERSION : « b12110 » au lieu de ${LLAMA_PINNED}+chatonlm.<empreinte> (packaging/llama.env)`,
    ]);
    // Fichier livré que les crochets NSIS ne retireraient pas avant une mise
    // à jour (motif oublié dans CHATONLM_LLAMA_FILES).
    const uncovered = installation({
      "llama-server/libfoo.dll": { pe: { imports: ["kernel32.dll"] } },
    });
    expect(contentProblems(uncovered).errors).toEqual([
      "Moteur de texte : fichier que les crochets NSIS ne retirent pas avant une mise à jour : llama-server/libfoo.dll",
    ]);
    // Crochets vides : les contrôles de l'ancien Ollama intégré échouent
    // aussi (essai à part, « crochets NSIS »).
    expect(
      contentProblems({ ...installation(), hooks: "; vide\n" }).errors.filter(
        (error) => error.includes("CHATONLM_LLAMA_FILES"),
      ),
    ).toEqual([
      "packaging/windows/hooks.nsh : motifs de CHATONLM_LLAMA_FILES illisibles",
    ]);
    const missing = installation();
    missing.reference.llamaServer = null;
    expect(contentProblems(missing).warnings).toEqual([
      "Copie de référence du moteur de texte absente (.tools/runtimes/llama-server) : fichiers non comparés",
    ]);
  });

  it("exige la notice du moteur d'images pour Windows", () => {
    const linux = renderSdServerReadme({
      env: { SDCPP_VERSION: SD_PINNED },
      licenseFiles: [],
    });
    const { errors } = contentProblems(
      installation({ "licenses/sd-server/README.txt": { text: linux } }),
    );
    expect(errors).toEqual([
      "licenses/sd-server/README.txt : mention absente (fallback\\\\vulkan-1\\.dll)",
      "licenses/sd-server/README.txt : mention absente (Visual C\\+\\+)",
    ]);
  });

  it("donne la taille du moteur d'images dans le rapport", () => {
    const files = new Map([
      ["chatonlm.exe", 10],
      ["sd-server/sd-server.exe", 2048],
      ["sd-server/fallback/vulkan-1.dll", 1024],
      ["sd-server/VERSION", 41],
    ]);
    const report = formatReport({
      installer: "chatonlm_0.9.2_x64-setup.exe",
      size: 1,
      signed: false,
      files,
      pes: [],
      problems: { errors: [], warnings: [] },
    });
    expect(report).toContain(
      "    racine (exe, DLL) : 10 o\n    sd-server/ : 3.0 kio\n",
    );
    expect(report).not.toContain("ollama");
  });

  it("résout les imports sans tenir compte de la casse", () => {
    const files = new Map([
      ["app.exe", "App.exe"],
      ["vcruntime140.dll", "VCRUNTIME140.dll"],
    ]);
    const pe = (file) => ({
      machine: 0x8664,
      imports: file === "App.exe" ? ["vcruntime140.dll", "kernel32.dll"] : [],
    });
    expect(importProblems({ files, pe })).toEqual({ errors: [], warnings: [] });
  });
});

describe("notices Windows", () => {
  it("dérive la configuration de cargo-about de celle de Linux", () => {
    const linux = text("packaging/about.toml");
    const windows = windowsAboutConfig(linux);
    expect(windows).toContain(`targets = ["${WINDOWS_TARGET}"]`);
    expect(windows).not.toContain("x86_64-unknown-linux-gnu");
    // Rien d'autre ne change : même liste blanche.
    expect(windows.replace(WINDOWS_TARGET, "x86_64-unknown-linux-gnu")).toBe(
      linux,
    );
    expect(() => windowsAboutConfig("accepted = []\n")).toThrow(/targets/);
  });

  it("livre le texte de chaque licence citée", () => {
    for (const source of Object.values(WINDOWS_COMPONENT_TEXTS)) {
      expect(existsSync(path.join(ROOT, source)), source).toBe(true);
    }
    expect(
      text("packaging/windows/licenses/WebView2-SDK-LICENSE.txt"),
    ).toContain("Copyright (C) Microsoft Corporation");
    expect(text("packaging/windows/licenses/NSIS-COPYING.txt")).toContain(
      "SPECIAL EXCEPTION FOR LZMA COMPRESSION MODULE",
    );
    const components = renderWindowsComponents();
    for (const file of Object.keys(WINDOWS_COMPONENT_TEXTS)) {
      expect(components).toContain(file);
    }
    expect(components).toContain("not open-source software");
    expect(components).not.toMatch(/ollama/i);
  });

  it("décrit les composants Windows dans THIRD_PARTY_LICENSES", () => {
    const notices = text("THIRD_PARTY_LICENSES");
    expect(notices).toContain("  Part 9  Windows installer only\n");
    const part9 = notices.split("== Part 9. Windows installer only ==")[1];
    for (const word of [
      "-- WebView2 loader --",
      "-- NSIS --",
      "-- Microsoft Visual C++ runtime --",
      "WebView2-SDK-LICENSE.txt",
      "NSIS-COPYING.txt",
    ]) {
      expect(part9).toContain(word);
    }
    expect(part9.split("== Part 10.")[0]).not.toMatch(/ollama/i);
    expect(notices).toContain("onnxruntime.dll next to chatonlm.exe");
  });
});

describe("configuration Windows du bundle", () => {
  const common = json("src-tauri/tauri.conf.json");
  const windows = json("src-tauri/tauri.windows.conf.json");
  const resources = json("packaging/windows/bundle.conf.json");

  it("ne change la CSP que par l'adresse Windows du protocole chatonlm", () => {
    const source = "http://chatonlm.localhost";
    const strip = (csp) => csp.replace(` ${source}`, "");
    const csp = windows.app.security.csp;
    expect(csp).toContain(`img-src 'self' data: blob: chatonlm: ${source};`);
    expect(strip(csp)).toBe(strip(common.app.security.csp));
  });

  it("reprend la fenêtre commune, centrée, sans le remplissage automatique de WebView2", () => {
    // Le tableau `app.windows` est remplacé en entier par la fusion.
    expect(windows.app.windows).toEqual(
      common.app.windows.map((window) => ({
        ...window,
        center: true,
        generalAutofillEnabled: false,
      })),
    );
  });

  it("construit un installateur NSIS par utilisateur, avec les DLL du runtime", () => {
    expect(windows.productName).toBe("ChatonLM");
    expect(common.productName).toBe("chatonlm");
    expect(windows.bundle.targets).toEqual(["nsis"]);
    expect(windows.build.windows.staticVCRuntime).toBe(false);
    expect(windows.bundle.windows.bundleVCRuntime).toBe(true);
    expect(windows.bundle.windows.webviewInstallMode).toEqual({
      type: "downloadBootstrapper",
      silent: true,
    });
    const nsis = windows.bundle.windows.nsis;
    expect(nsis.installMode).toBe("currentUser");
    expect(nsis.languages).toEqual(["English", "French", "Spanish"]);
    expect(nsis.displayLanguageSelector).toBe(false);
    for (const file of [nsis.installerIcon, nsis.installerHooks]) {
      expect(existsSync(path.join(ROOT, "src-tauri", file)), file).toBe(true);
    }
  });

  it("garde les ressources hors des fichiers lus à chaque compilation", () => {
    // build.rs copie bundle.resources à chaque compilation : elles ne sont
    // données qu'à l'empaquetage (--config de scripts/package-windows.sh).
    expect(common.bundle.resources).toBeUndefined();
    expect(windows.bundle.resources).toBeUndefined();
    expect(resources.bundle.resources).toEqual({
      "../.tools/runtimes/onnxruntime/onnxruntime.dll": "onnxruntime.dll",
      "../.tools/runtimes/sd-server/sd-server.exe": "sd-server/sd-server.exe",
      "../.tools/runtimes/sd-server/fallback/": "sd-server/fallback/",
      "../.tools/runtimes/sd-server/VERSION": "sd-server/VERSION",
      "../.tools/runtimes/llama-server/": "llama-server/",
      "../.tools/package/notices/windows/": "licenses/",
    });
    expect(text("scripts/package-windows.sh")).toContain(
      "--config packaging/windows/bundle.conf.json",
    );
    expect(text("scripts/package-windows.sh")).toContain("--no-sign");
  });
});

describe("crochets NSIS", () => {
  const hooks = text("packaging/windows/hooks.nsh");
  // Lignes de code seulement (les commentaires citent ce qu'il ne faut pas faire).
  const code = hooks
    .split("\n")
    .filter((line) => !line.trim().startsWith(";"))
    .join("\n");

  it("arrête l'app puis ses moteurs, par leur chemin exact, dans une installation de ChatonLM seulement", () => {
    for (const file of [
      String.raw`"$INSTDIR\${MAINBINARYNAME}.exe"`,
      String.raw`"$INSTDIR\sd-server\sd-server.exe"`,
      String.raw`"$INSTDIR\llama-server\llama-server.exe"`,
    ]) {
      expect(code).toContain(`!insertmacro CHATONLM_STOP_FILE ${file}`);
    }
    const all = code.split("!macro CHATONLM_STOP_ALL")[1].split("!macroend")[0];
    expect(
      all
        .trim()
        .split("\n")
        .map((line) => line.trim()),
    ).toEqual([
      String.raw`!insertmacro CHATONLM_STOP_FILE "$INSTDIR\${MAINBINARYNAME}.exe"`,
      "!insertmacro CHATONLM_STOP_ENGINES",
    ]);
    // Rien n'est arrêté ni effacé si l'exécutable de ChatonLM n'est pas là :
    // le dossier choisi peut contenir un Ollama qui n'est pas le nôtre.
    const guard =
      "${If} ${FileExists} " + String.raw`"$INSTDIR\${MAINBINARYNAME}.exe"`;
    for (const hook of ["NSIS_HOOK_PREINSTALL", "NSIS_HOOK_PREUNINSTALL"]) {
      const body = code.split(`!macro ${hook}`)[1].split("!macroend")[0];
      expect(body).toContain("!insertmacro CHATONLM_STOP_ALL");
      const lines = body
        .trim()
        .split("\n")
        .map((line) => line.trim());
      expect(lines[0]).toBe(guard);
      expect(lines.at(-1)).toBe("${EndIf}");
    }
  });

  it("arrête aussi le moteur d'images par son chemin exact", () => {
    const builtin = code
      .split("!macro CHATONLM_STOP_ENGINES")[1]
      .split("!macroend")[0];
    expect(
      builtin
        .trim()
        .split("\n")
        .map((line) => line.trim()),
    ).toContain(
      String.raw`!insertmacro CHATONLM_STOP_FILE "$INSTDIR\sd-server\sd-server.exe"`,
    );
    // Aucune suppression de son dossier : le modèle de Tauri remplace ses
    // fichiers et les retire à la désinstallation.
    expect(code).not.toMatch(/RMDir .*sd-server/);
  });

  it("n'arrête et ne retire le moteur de texte que si son dossier porte notre marque, fichier par fichier", () => {
    const macro = (name) =>
      code
        .split(`!macro ${name}`)[1]
        .split("!macroend")[0]
        .trim()
        .split("\n")
        .map((line) => line.trim());
    // Marque : « +chatonlm. » cherchée dans la première ligne de VERSION.
    const ours = macro("CHATONLM_OUR_LLAMA");
    expect(ours[0]).toBe("StrCpy $R1 0");
    expect(ours).toContain(
      String.raw`FileOpen $R2 "$INSTDIR\llama-server\VERSION" r`,
    );
    expect(ours).toContain('${If} $R5 == "+chatonlm."');
    expect(ours).toContain("StrCpy $R5 $R3 10 $R2");
    expect(ours).toContain("StrCpy $R1 1");
    // Arrêt par le chemin exact, sous la garde de la marque.
    const builtin = macro("CHATONLM_STOP_ENGINES");
    const stop = builtin.indexOf(
      String.raw`!insertmacro CHATONLM_STOP_FILE "$INSTDIR\llama-server\llama-server.exe"`,
    );
    expect(builtin.slice(stop - 2, stop + 2)).toEqual([
      "!insertmacro CHATONLM_OUR_LLAMA",
      "${If} $R1 = 1",
      builtin[stop],
      "${EndIf}",
    ]);
    // Avant la copie, après l'arrêt, sous la même garde ; jamais à la
    // désinstallation (le modèle de Tauri y retire ses fichiers par nom).
    const pre = macro("NSIS_HOOK_PREINSTALL");
    const remove = pre.indexOf("!insertmacro CHATONLM_REMOVE_LLAMA");
    expect(pre.slice(remove - 2, remove + 2)).toEqual([
      "!insertmacro CHATONLM_OUR_LLAMA",
      "${If} $R1 = 1",
      "!insertmacro CHATONLM_REMOVE_LLAMA",
      "${EndIf}",
    ]);
    expect(pre.indexOf("!insertmacro CHATONLM_STOP_ALL")).toBeLessThan(remove);
    expect(macro("NSIS_HOOK_PREUNINSTALL").join("\n")).not.toMatch(
      /RMDir|REMOVE_LLAMA|Delete/,
    );
    // Retrait : nos motifs, puis jusqu'à dix nouveaux essais s'il en reste,
    // puis la marque, puis les dossiers seulement vides.
    expect(macro("CHATONLM_REMOVE_LLAMA")).toEqual([
      "!insertmacro CHATONLM_LLAMA_FILES CHATONLM_DELETE_PATTERN",
      "${For} $R3 1 10",
      "StrCpy $R2 0",
      "!insertmacro CHATONLM_LLAMA_FILES CHATONLM_COUNT_LEFT",
      "${If} $R2 = 0",
      "${ExitFor}",
      "${EndIf}",
      "Sleep 500",
      "!insertmacro CHATONLM_LLAMA_FILES CHATONLM_DELETE_PATTERN",
      "${Next}",
      String.raw`Delete "$INSTDIR\llama-server\VERSION"`,
      String.raw`RMDir "$INSTDIR\llama-server\fallback"`,
      String.raw`RMDir "$INSTDIR\llama-server"`,
    ]);
    expect(macro("CHATONLM_DELETE_PATTERN pattern")).toEqual([
      'Delete "${pattern}"',
    ]);
    expect(macro("CHATONLM_COUNT_LEFT pattern")).toEqual([
      '${If} ${FileExists} "${pattern}"',
      "IntOp $R2 $R2 + 1",
      "${EndIf}",
    ]);
  });

  it("ne retire de llama-server que des fichiers que nous livrons", () => {
    const patterns = llamaHookPatterns(hooks);
    const removed = (file) => patterns.some((pattern) => pattern.test(file));
    for (const file of [
      "llama-server.exe",
      "llama-server-impl.dll",
      "llama.dll",
      "llama-common.dll",
      "mtmd.dll",
      "ggml.dll",
      "ggml-base.dll",
      "ggml-cpu-haswell.dll",
      "GGML-CPU-ZEN4.DLL",
      "ggml-vulkan.dll",
      "vcruntime140.dll",
      "vcruntime140_1.dll",
      "msvcp140.dll",
      "fallback/vulkan-1.dll",
      "SHA256SUMS",
      "VERSION",
    ]) {
      expect(removed(file), file).toBe(true);
    }
    // Ce qu'un utilisateur range dans son propre dossier llama.cpp.
    for (const file of [
      "model.gguf",
      "models/qwen3.gguf",
      "llama-cli.exe",
      "notes.txt",
      "fallback/autre.dll",
      "sub/ggml-base.dll",
      "VERSION.txt",
    ]) {
      expect(removed(file), file).toBe(false);
    }
  });

  it("réessaie de supprimer uninstall.exe après la désinstallation, lui seul", () => {
    const again = code
      .split("!macro CHATONLM_DELETE_AGAIN filePath")[1]
      .split("!macroend")[0];
    expect(
      again
        .trim()
        .split("\n")
        .map((line) => line.trim()),
    ).toEqual([
      '${If} ${FileExists} "${filePath}"',
      "Sleep 500",
      'Delete "${filePath}"',
      "${EndIf}",
    ]);
    const lines = code
      .split("!macro NSIS_HOOK_POSTUNINSTALL")[1]
      .split("!macroend")[0]
      .trim()
      .split("\n")
      .map((line) => line.trim());
    const retryApp = String.raw`!insertmacro CHATONLM_DELETE_AGAIN "$INSTDIR\${MAINBINARYNAME}.exe"`;
    const retry = String.raw`!insertmacro CHATONLM_DELETE_AGAIN "$INSTDIR\uninstall.exe"`;
    expect(lines.slice(0, -1)).toEqual([
      ...Array(10).fill(retryApp),
      ...Array(10).fill(retry),
    ]);
    // Le dossier n'est retiré que vide.
    expect(lines.at(-1)).toBe('RMDir "$INSTDIR"');
    // Aucune autre suppression de fichier dans les crochets que celles du
    // moteur de texte (motifs de CHATONLM_LLAMA_FILES et sa marque) et de
    // l'ancien Ollama intégré (son serveur et sa marque).
    expect(code.match(/^\s*Delete .*/gm)).toEqual([
      '  Delete "${pattern}"',
      String.raw`  Delete "$INSTDIR\llama-server\VERSION"`,
      String.raw`    Delete "$INSTDIR\ollama\ollama.exe"`,
      String.raw`    Delete "$INSTDIR\ollama\VERSION"`,
      '    Delete "${filePath}"',
    ]);
  });

  it("n'arrête jamais un processus par son nom et ne touche pas aux données", () => {
    expect(code).not.toMatch(/taskkill|KillProcess|FindProcess/i);
    expect(code).not.toMatch(/\$(LOCAL)?APPDATA|\$PROFILE/);
    // Deux suppressions récursives seulement, de dossiers livrés par nous
    // seuls : les bibliothèques de l'ancien Ollama intégré et ses notices
    // (le moteur de texte est retiré par motifs).
    expect(code.match(/RMDir \/r .*/g)).toEqual([
      String.raw`RMDir /r "$INSTDIR\ollama\lib\ollama"`,
      String.raw`RMDir /r "$INSTDIR\licenses\ollama"`,
    ]);
    // Aucune ligne ne finit par « \ » (suite de ligne de NSIS).
    expect(hooks.split("\n").filter((line) => line.endsWith("\\"))).toEqual([]);
  });

  it("retire l'ancien Ollama intégré d'une 0.9.x, après l'arrêt, et rien d'autre", () => {
    expect(oldOllamaHookErrors(hooks)).toEqual([]);
    const macro = (name) =>
      code
        .split(new RegExp(`!macro ${name}\\b`))[1]
        .split("!macroend")[0]
        .trim()
        .split("\n")
        .map((line) => line.trim());
    // Reconnu par son serveur ET la marque VERSION (absente de l'officiel).
    expect(macro("CHATONLM_OLD_OLLAMA")).toEqual([
      "StrCpy $R1 0",
      '${If} ${FileExists} "$INSTDIR\\ollama\\ollama.exe"',
      '${AndIf} ${FileExists} "$INSTDIR\\ollama\\VERSION"',
      "StrCpy $R1 1",
      "${EndIf}",
    ]);
    // Arrêté avec les moteurs, sous la même garde.
    const engines = macro("CHATONLM_STOP_ENGINES");
    const stop = engines.indexOf("!insertmacro CHATONLM_STOP_OLD_OLLAMA");
    expect(engines.slice(stop - 2, stop + 2)).toEqual([
      "!insertmacro CHATONLM_OLD_OLLAMA",
      "${If} $R1 = 1",
      "!insertmacro CHATONLM_STOP_OLD_OLLAMA",
      "${EndIf}",
    ]);
    // Retiré avant la copie et avant la désinstallation, après l'arrêt.
    for (const hook of ["NSIS_HOOK_PREINSTALL", "NSIS_HOOK_PREUNINSTALL"]) {
      const body = macro(hook);
      expect(body.indexOf("!insertmacro CHATONLM_CLEAN_OLD_OLLAMA")).toBe(
        body.indexOf("!insertmacro CHATONLM_STOP_ALL") + 1,
      );
    }
    // La marque n'est retirée qu'avec le serveur : un reste encore tenu
    // reste reconnu à la désinstallation.
    expect(macro("CHATONLM_REMOVE_OLD_OLLAMA").slice(-6)).toEqual([
      "${Next}",
      '${IfNot} ${FileExists} "$INSTDIR\\ollama\\ollama.exe"',
      String.raw`Delete "$INSTDIR\ollama\VERSION"`,
      "${EndIf}",
      String.raw`RMDir "$INSTDIR\ollama\lib"`,
      String.raw`RMDir "$INSTDIR\ollama"`,
    ]);

    // Chaque écart est signalé, et refusé par l'inspection de l'installateur.
    const without = (line) => hooks.replace(line, "");
    const broken = [
      [
        without(
          "    !insertmacro CHATONLM_CLEAN_OLD_OLLAMA\n    ; Moteur de texte",
        ),
        "NSIS_HOOK_PREINSTALL : CHATONLM_CLEAN_OLD_OLLAMA attendue après CHATONLM_STOP_ALL",
      ],
      [
        without('  ${AndIf} ${FileExists} "$INSTDIR\\ollama\\VERSION"'),
        "CHATONLM_OLD_OLLAMA : VERSION non vérifié",
      ],
      [
        without(
          String.raw`  !insertmacro CHATONLM_STOP_FILE "$INSTDIR\ollama\lib\ollama\llama-server.exe"`,
        ),
        String.raw`CHATONLM_STOP_OLD_OLLAMA : $INSTDIR\ollama\lib\ollama\llama-server.exe non arrêté`,
      ],
      [
        hooks.replace(
          String.raw`RMDir /r "$INSTDIR\ollama\lib\ollama"`,
          String.raw`RMDir /r "$INSTDIR\ollama"`,
        ),
        String.raw`packaging/windows/hooks.nsh : RMDir /r sur tout un dossier : RMDir /r "$INSTDIR\ollama"`,
      ],
      [
        `${hooks}!macro CHATONLM_X\n  ExecWait 'taskkill /im ollama.exe'\n!macroend\n`,
        "packaging/windows/hooks.nsh : arrêt par nom (taskkill) : taskkill",
      ],
      [
        `${hooks}!macro CHATONLM_X\n  RMDir /r "$PROFILE\\.ollama"\n!macroend\n`,
        "packaging/windows/hooks.nsh : chemin hors du dossier d'installation : $PROFILE",
      ],
    ];
    for (const [text, error] of broken) {
      expect(text).not.toBe(hooks);
      expect(oldOllamaHookErrors(text)).toContain(error);
      expect(
        contentProblems({ ...installation(), hooks: text }).errors,
      ).toContain(error);
    }
  });
});

describe("workflow packages-windows.yml", () => {
  const workflow = text(".github/workflows/packages-windows.yml");
  const linux = text(".github/workflows/packages.yml");

  it("épingle ses actions aux mêmes révisions que packages.yml", () => {
    const pinned = (yaml) =>
      [...yaml.matchAll(/^\s*(?:- )?uses: (\S+)@(\S+)/gm)].map((m) => [
        m[1],
        m[2],
      ]);
    const known = new Map(pinned(linux));
    const used = pinned(workflow);
    expect(used.length).toBeGreaterThanOrEqual(5);
    for (const [action, revision] of used) {
      expect(revision, action).toMatch(/^[0-9a-f]{40}$/);
      expect(known.get(action), action).toBe(revision);
    }
  });

  it("tourne sur deux jobs Windows (installateur, pack CUDA), sans secret, artefacts de 2 jours", () => {
    expect(workflow.match(/runs-on: .*/g)).toEqual([
      "runs-on: windows-2025",
      "runs-on: windows-2025",
    ]);
    expect(workflow).toContain('- "winpkg/**"');
    expect(workflow).not.toMatch(/secrets\./);
    expect(workflow).toContain("permissions:\n  contents: read\n");
    expect(workflow.match(/actions\/upload-artifact@/g)).toHaveLength(2);
    expect(workflow.match(/retention-days: \d+/g)).toEqual([
      "retention-days: 2",
      "retention-days: 2",
    ]);
    expect(workflow).not.toContain("fetch-ollama");
    for (const step of [
      "scripts/fetch-onnxruntime.sh",
      "node scripts/fetch-cuda-toolkit.mjs",
      "scripts/build-llama-server.sh --cuda",
      "cargo test --manifest-path src-tauri/Cargo.toml --workspace --locked",
      "git diff --exit-code --stat -- src/bindings src/test/fixtures",
      "scripts/package-windows.sh",
      "node scripts/inspect-windows.mjs",
      "./packaging/ci/smoke-windows.ps1",
    ]) {
      expect(workflow, step).toContain(step);
    }
  });

  /** Corps d'une étape (de « - name: » à l'étape suivante), ou null. */
  const step = (yaml, name) => {
    const start = yaml.indexOf(`- name: ${name}\n`);
    if (start < 0) return null;
    const next = yaml.indexOf("\n      - name: ", start + 1);
    return yaml.slice(start, next < 0 ? undefined : next);
  };
  const SD_STEP = "SDK Vulkan et moteurs (sd-server, llama-server)";

  it("construit le moteur d'images dans les deux workflows, sans nouvelle action", () => {
    for (const [name, yaml] of [
      ["packages.yml", linux],
      ["packages-windows.yml", workflow],
    ]) {
      const body = step(yaml, SD_STEP);
      expect(body, name).not.toBeNull();
      expect(body, name).toContain("scripts/build-sd-server.sh");
      expect(body, name).toContain("packaging/sdcpp.env");
      // Le moteur de texte après le moteur d'images (chargeur de secours).
      expect(
        body.indexOf("scripts/build-llama-server.sh"),
        name,
      ).toBeGreaterThan(body.indexOf("scripts/build-sd-server.sh"));
      expect(body, name).not.toMatch(/^\s*(?:- )?uses:/m);
      // Avant l'empaquetage, qui l'exige.
      const pack =
        name === "packages.yml"
          ? "scripts/package.sh"
          : "scripts/package-windows.sh";
      expect(yaml.indexOf(`- name: ${SD_STEP}`), name).toBeLessThan(
        yaml.lastIndexOf(pack),
      );
    }
    // Toute action de packages.yml reste épinglée à une révision.
    for (const match of linux.matchAll(/^\s*(?:- )?uses: (\S+)@(\S+)/gm)) {
      expect(match[2], match[1]).toMatch(/^[0-9a-f]{40}$/);
    }
  });

  it("reconstruit le moteur d'images Windows quand sa version, ses correctifs ou son script changent", () => {
    const key =
      /^\s*key: packages-windows-\$\{\{ hashFiles\((.*)\) \}\}$/m.exec(
        workflow,
      );
    expect(key).not.toBeNull();
    const inputs = key[1].split(",").map((part) => part.trim());
    for (const input of [
      "'packaging/*.env'",
      "'packaging/sdcpp/*.patch'",
      "'scripts/build-sd-server.sh'",
      "'packaging/llama/*.patch'",
      "'scripts/build-llama-server.sh'",
    ]) {
      expect(inputs).toContain(input);
    }
    // Construit (ou repris du cache) puis exigé avant l'empaquetage.
    const pack = text("scripts/package-windows.sh");
    expect(pack).toMatch(
      /^step "Moteur d'images épinglé"\nscripts\/build-sd-server\.sh$/m,
    );
    expect(pack).toContain('".tools/runtimes/sd-server/sd-server.exe"');
    expect(pack.indexOf("scripts/build-sd-server.sh\n")).toBeLessThan(
      pack.indexOf("--config packaging/windows/bundle.conf.json"),
    );
  });

  it("essaie l'installation, le lancement et la désinstallation en silence", () => {
    const smoke = text("packaging/ci/smoke-windows.ps1");
    expect(smoke).toContain('Invoke-Silent $Installer @("/S")');
    expect(smoke).toContain('Invoke-Silent $uninstaller @("/S")');
    // Désinstallation : le dossier doit être vide, avec un délai pour les
    // nouveaux essais du crochet, et un diagnostic s'il reste un fichier.
    expect(smoke).toContain('Fail "Fichiers restants dans $install');
    expect(smoke).toContain("for ($i = 0; $i -lt 30; $i++)");
    expect(smoke).toContain("supprimable après coup");
    expect(smoke).toContain('$env:OLLAMA_HOST = "http://127.0.0.1:9"');
    expect(smoke).not.toMatch(/taskkill\.exe \/F \/IM|Stop-Process -Name/i);
    // Même réglage de l'app que la configuration du bundle.
    expect(smoke).toContain(
      `$ProductName = "${json("src-tauri/tauri.windows.conf.json").productName}"`,
    );
    expect(parseEnv("A=1\n").A).toBe("1");
  });
});
