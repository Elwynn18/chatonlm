// Règles pures de la reprise des modèles d'Ollama : textes par code et par
// statut, cases cochées par défaut, octets nécessaires, copies et place,
// avancement.

import { describe, expect, it } from "vitest";
import type { ImportProblem } from "@/bindings/ImportProblem";
import type { ImportProgress } from "@/bindings/ImportProgress";
import { type TKey, translator } from "@/i18n";
import { candidate, source } from "@/test/modelImport";
import {
  candidateStatusText,
  copiesFiles,
  importPercent,
  isSelectable,
  lacksSpace,
  neededBytes,
  problemText,
  selectedNames,
  selectionKey,
} from "./modelImport";

const GIB = 1024 ** 3;

const t = translator("en");

const PROBLEMS: ImportProblem[] = [
  "ollamaFormat",
  "notGguf",
  "blobMissing",
  "cloud",
  "adapter",
  "invalidName",
  "unreadable",
  "embedding",
  "unsupported",
];

describe("modelImport", () => {
  it("gives each problem code its own text, in every language", () => {
    for (const lang of ["en", "es", "fr"] as const) {
      const texts = PROBLEMS.map((problem) =>
        translator(lang)(`settings.modelImport.problems.${problem}` as TKey),
      );
      expect(new Set(texts).size).toBe(PROBLEMS.length);
    }
    expect(problemText(t, "embedding")).toBe(
      "Embedding model: not used for chats.",
    );
  });

  it("says what happens to each model", () => {
    expect(candidateStatusText(t, candidate("a"))).toBeNull();
    expect(
      candidateStatusText(t, candidate("a", { status: "installed" })),
    ).toBe("Already in ChatonLM");
    expect(candidateStatusText(t, candidate("a", { status: "failed" }))).toBe(
      "Import failed twice in a row.",
    );
    // Raison du moteur d'abord, sinon le statut.
    expect(
      candidateStatusText(
        t,
        candidate("a", { status: "redownload", problem: "unsupported" }),
      ),
    ).toBe("llama.cpp can't run this kind of model.");
    expect(
      candidateStatusText(t, candidate("a", { status: "redownload" })),
    ).toBe("Download it again from Hugging Face.");
    expect(candidateStatusText(t, candidate("a", { status: "skipped" }))).toBe(
      "Not imported.",
    );
  });

  it("checks the ready models by default, minus those unchecked", () => {
    const folder = source("builtin", [
      candidate("a"),
      candidate("b", { status: "installed" }),
      candidate("c"),
      candidate("d", { status: "failed" }),
    ]);
    expect(folder.models.map(isSelectable)).toEqual([true, false, true, false]);
    expect(selectedNames(folder, new Set())).toEqual(["a", "c"]);
    expect(selectedNames(folder, new Set([selectionKey(folder, "a")]))).toEqual(
      ["c"],
    );
    // Même nom dans un autre dossier : sans effet ici.
    const other = { ...folder, dir: "/other" };
    expect(selectedNames(folder, new Set([selectionKey(other, "a")]))).toEqual([
      "a",
      "c",
    ]);
  });

  it("adds up the size of the named models", () => {
    const folder = source("machine", [
      candidate("a", { sizeBytes: 2 * GIB }),
      candidate("b", { sizeBytes: 3 * GIB }),
    ]);
    expect(neededBytes(folder, ["a", "b"])).toBe(5 * GIB);
    expect(neededBytes(folder, ["b"])).toBe(3 * GIB);
    expect(neededBytes(folder, [])).toBe(0);
  });

  it("copies from this computer's Ollama, a shared folder, or another drive", () => {
    expect(copiesFiles(source("builtin", []))).toBe(false);
    expect(copiesFiles(source("builtin", [], { sameDrive: false }))).toBe(true);
    expect(copiesFiles(source("builtin", [], { shared: true }))).toBe(true);
    expect(copiesFiles(source("machine", [], { sameDrive: true }))).toBe(true);
  });

  it("lacks space only for copies, when the free space is known", () => {
    const tight = { freeBytes: GIB };
    expect(lacksSpace(source("machine", [], tight), 2 * GIB)).toBe(true);
    expect(lacksSpace(source("machine", [], tight), GIB)).toBe(false);
    expect(lacksSpace(source("builtin", [], tight), 2 * GIB)).toBe(false);
    expect(
      lacksSpace(source("machine", [], { freeBytes: null }), 2 * GIB),
    ).toBe(false);
  });

  it("counts the progress in bytes, or in models when nothing is copied", () => {
    const progress: ImportProgress = {
      origin: "builtin",
      dir: "/d",
      model: null,
      doneModels: 1,
      totalModels: 4,
      doneBytes: 3 * GIB,
      totalBytes: 4 * GIB,
    };
    expect(importPercent(progress)).toBe(75);
    expect(importPercent({ ...progress, totalBytes: 0, doneBytes: 0 })).toBe(
      25,
    );
    expect(importPercent({ ...progress, totalBytes: 0, totalModels: 0 })).toBe(
      0,
    );
    expect(importPercent({ ...progress, doneBytes: 5 * GIB })).toBe(100);
  });
});
