// @vitest-environment node
// Copie d'un dossier de modèles d'Ollama pour scripts/demo.sh
// --import-ollama (scripts/lib/ollama-copy.sh) : la reprise des modèles
// déplace puis efface les fichiers de la copie ; un `blobs` lié (vers un
// autre disque) ne doit jamais devenir un lien dans la copie, sinon la
// reprise travaillerait sur les vrais fichiers de l'utilisateur. Dossiers
// d'essai sous .tools/tmp.
import { spawnSync } from "node:child_process";
import {
  existsSync,
  lstatSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  symlinkSync,
  writeFileSync,
} from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { afterEach, describe, expect, it, vi } from "vitest";

// Écritures sur .tools (ntfs3) : lentes quand la suite entière charge le
// disque.
vi.setConfig({ testTimeout: 30_000 });

const ROOT = fileURLToPath(new URL("../", import.meta.url));
const LIB = join(ROOT, "scripts/lib/ollama-copy.sh");
const TMP = join(ROOT, ".tools/tmp");

const dirs = [];
afterEach(() => {
  for (const dir of dirs.splice(0))
    rmSync(dir, { recursive: true, force: true });
});

function tempDir() {
  mkdirSync(TMP, { recursive: true });
  const dir = mkdtempSync(join(TMP, "ollama-copy-"));
  dirs.push(dir);
  return dir;
}

/** `chatonlm_copy_ollama_models <from> <copy>` ; rend { status, stderr }. */
function copy(from, to) {
  return spawnSync(
    "bash",
    ["-c", 'source "$0"; chatonlm_copy_ollama_models "$1" "$2"', LIB, from, to],
    { encoding: "utf8" },
  );
}

function size(dir) {
  return spawnSync(
    "bash",
    ["-c", 'source "$0"; chatonlm_ollama_size "$1"', LIB, dir],
    { encoding: "utf8" },
  ).stdout.trim();
}

/**
 * Dossier de modèles d'Ollama dont `blobs` est un lien vers un autre
 * dossier (« autre disque »), et un manifeste lié lui aussi.
 */
function linkedOllama(base) {
  const disk = join(base, "other-disk");
  mkdirSync(join(disk, "blobs"), { recursive: true });
  writeFileSync(join(disk, "blobs/sha256-x"), "poids");
  writeFileSync(join(disk, "manifest"), '{"layers": []}');
  const models = join(base, "models");
  mkdirSync(join(models, "manifests/registry.ollama.ai/library/m"), {
    recursive: true,
  });
  symlinkSync(
    join(disk, "manifest"),
    join(models, "manifests/registry.ollama.ai/library/m/latest"),
  );
  symlinkSync(join(disk, "blobs"), join(models, "blobs"));
  return { disk, models };
}

describe("scripts/lib/ollama-copy.sh", () => {
  it("copies through symbolic links, never the links themselves", () => {
    const base = tempDir();
    const { disk, models } = linkedOllama(base);
    const to = join(base, "copy");
    const result = copy(models, to);
    expect(result.status, result.stderr).toBe(0);
    expect(lstatSync(join(to, "blobs")).isSymbolicLink()).toBe(false);
    expect(lstatSync(join(to, "blobs")).isDirectory()).toBe(true);
    const manifest = join(to, "manifests/registry.ollama.ai/library/m/latest");
    expect(lstatSync(manifest).isSymbolicLink()).toBe(false);
    expect(readFileSync(manifest, "utf8")).toBe('{"layers": []}');
    // Ce que la reprise ferait de la copie (déplacer, effacer) laisse
    // l'original en place.
    rmSync(join(to, "blobs/sha256-x"));
    rmSync(manifest);
    expect(readFileSync(join(disk, "blobs/sha256-x"), "utf8")).toBe("poids");
    expect(existsSync(join(disk, "manifest"))).toBe(true);
    expect(lstatSync(join(models, "blobs")).isSymbolicLink()).toBe(true);
  });

  it("counts the linked files in the size it shows", () => {
    const base = tempDir();
    const { models } = linkedOllama(base);
    writeFileSync(
      join(base, "other-disk/blobs/sha256-big"),
      Buffer.alloc(200 * 1024),
    );
    // Taille lue à travers le lien : au moins les 200 Kio du blob (sans
    // suivre le lien, quelques octets).
    const shown = size(models).match(/^(\d+)K$/);
    expect(shown, size(models)).not.toBeNull();
    expect(Number(shown[1])).toBeGreaterThanOrEqual(200);
  });

  it("refuses a copy that cannot follow a link, and leaves nothing", () => {
    const base = tempDir();
    const { models } = linkedOllama(base);
    symlinkSync(join(base, "nowhere"), join(base, "other-disk/blobs/dead"));
    const to = join(base, "copy");
    const result = copy(models, to);
    expect(result.status).not.toBe(0);
    expect(existsSync(to)).toBe(false);
    expect(existsSync(join(base, "other-disk/blobs/sha256-x"))).toBe(true);
  });

  it("refuses to overwrite an existing copy", () => {
    const base = tempDir();
    const { models } = linkedOllama(base);
    const to = join(base, "copy");
    mkdirSync(to);
    writeFileSync(join(to, "keep"), "x");
    expect(copy(models, to).status).not.toBe(0);
    expect(readFileSync(join(to, "keep"), "utf8")).toBe("x");
  });
});
