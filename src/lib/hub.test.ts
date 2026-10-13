import { describe, expect, it } from "vitest";
import type { HubDownload } from "@/bindings/HubDownload";
import type { HubHardware } from "@/bindings/HubHardware";
import { translator } from "@/i18n";
import {
  activeText,
  downloadPercent,
  isActiveDownload,
  isPendingDownload,
  familyOf,
  filtersFromParams,
  fitLabel,
  formatContext,
  formatCount,
  formatParameters,
  formatTimeLeft,
  GRADIENT_CLASS,
  GRADIENTS,
  gradientFor,
  gradientStyle,
  gpuName,
  initials,
  machineHint,
  machineLabel,
  progressText,
  searchText,
  usesFromParam,
  usesParam,
  validateHubParams,
} from "./hub";

const GIB = 1024 ** 3;
const MIB = 1024 ** 2;

const en = translator("en");
const fr = translator("fr");
const es = translator("es");
/** Espace insécable (français et espagnol : « 5,0 Go », « 3 min »). */
const NBSP = " ";

function download(fields: Partial<HubDownload>): HubDownload {
  return {
    model: "hf.co/a/b:Q4_K_M",
    repo: "a/b",
    label: "Q4_K_M",
    state: "downloading",
    completed: 0,
    total: 0,
    bytesPerSecond: null,
    error: null,
    startedAt: 0,
    finishedAt: null,
    ...fields,
  };
}

// Tailles et dates relatives : formatBytes et formatTimeAgo, essayées dans
// src/i18n/i18n.test.tsx.
describe("formats", () => {
  it("counts: 999, 1.2K, 12K, 1.2M, never 1000K", () => {
    expect(formatCount(en, 999)).toBe("999");
    expect(formatCount(en, 1234)).toBe("1.2K");
    expect(formatCount(en, 12_345)).toBe("12K");
    expect(formatCount(en, 1_234_567)).toBe("1.2M");
    expect(formatCount(en, 999_950)).toBe("1M");
    expect(formatCount(en, 3_000_000)).toBe("3M");
    expect(formatCount(en, 0)).toBe("0");
  });

  it("counts, parameters and context in the way of the language", () => {
    // Suffixes et séparateur décimal de la langue (espace étroite en
    // français, d'Intl).
    expect(formatCount(fr, 1234)).toMatch(/^1,2\s?k$/u);
    expect(formatCount(es, 12_345)).toMatch(/^12\s?mil$/u);
    expect(formatParameters(fr, 8.19e9)).toBe("8,2B");
    expect(formatParameters(es, 27.2e9)).toBe("27B");
    expect(formatContext(fr, 1536)).toBe("1,5K");
  });

  it("parameters and context", () => {
    expect(formatParameters(en, 8.19e9)).toBe("8.2B");
    expect(formatParameters(en, 8e9)).toBe("8B");
    expect(formatParameters(en, 27.2e9)).toBe("27B");
    expect(formatParameters(en, 135e6)).toBe("135M");
    expect(formatContext(en, 131072)).toBe("128K");
    expect(formatContext(en, 40960)).toBe("40K");
    expect(formatContext(en, 1048576)).toBe("1M");
  });

  it("time left", () => {
    expect(formatTimeLeft(en, 42.2)).toBe("43 s left");
    expect(formatTimeLeft(en, 70)).toBe("1 min left");
    expect(formatTimeLeft(en, 3900)).toBe("1 h 5 min left");
    expect(formatTimeLeft(en, 7200)).toBe("2 h left");
  });

  it("time left in French and Spanish: singular and plural", () => {
    expect(formatTimeLeft(fr, 70)).toBe(`1${NBSP}min restante`);
    expect(formatTimeLeft(fr, 180)).toBe(`3${NBSP}min restantes`);
    expect(formatTimeLeft(fr, 3900)).toBe(`1${NBSP}h 5${NBSP}min restantes`);
    expect(formatTimeLeft(es, 0.4)).toBe(`queda 1${NBSP}s`);
    expect(formatTimeLeft(es, 7200)).toBe(`quedan 2${NBSP}h`);
  });

  it("progress line: sizes, speed, time left; unknown total: received only", () => {
    const running = download({
      completed: 2.1 * GIB,
      total: 5 * GIB,
      bytesPerSecond: 48 * MIB,
    });
    expect(progressText(en, running)).toBe(
      "2.1 GB of 5.0 GB · 48 MB/s · 1 min left",
    );
    expect(progressText(en, download({ completed: 300 * MIB }))).toBe("300 MB");
    // Tailles, débit et temps restant de la langue.
    expect(progressText(fr, running)).toBe(
      `2,1${NBSP}Go sur 5,0${NBSP}Go · 48${NBSP}Mo/s · 1${NBSP}min restante`,
    );
    expect(progressText(es, running)).toBe(
      `2,1${NBSP}GB de 5,0${NBSP}GB · 48${NBSP}MB/s · queda 1${NBSP}min`,
    );
    expect(downloadPercent(1, 3)).toBe(33);
    expect(downloadPercent(5, 0)).toBe(0);
  });

  it("active states have their own text", () => {
    expect(activeText(en, download({ state: "queued" }))).toBe("Waiting…");
    // En pause : ce qui est reçu est dit, sans débit ni temps restant.
    expect(
      activeText(
        en,
        download({
          state: "paused",
          completed: 2 * 1024 ** 3,
          total: 5 * 1024 ** 3,
          bytesPerSecond: 50 * 1024 ** 2,
        }),
      ),
    ).toBe("Paused · 2.0 GB of 5.0 GB");
    expect(activeText(en, download({ state: "paused", total: 0 }))).toBe(
      "Paused",
    );
    expect(activeText(fr, download({ state: "paused", total: 0 }))).toBe(
      "En pause",
    );
    expect(activeText(es, download({ state: "paused", total: 0 }))).toBe(
      "En pausa",
    );
    expect(isPendingDownload(download({ state: "paused" }))).toBe(true);
    expect(isActiveDownload(download({ state: "paused" }))).toBe(false);
    for (const state of [
      "queued",
      "starting",
      "downloading",
      "finishing",
    ] as const) {
      expect(isPendingDownload(download({ state }))).toBe(true);
    }
    for (const state of ["done", "failed", "cancelled"] as const) {
      expect(isPendingDownload(download({ state }))).toBe(false);
    }
    expect(activeText(en, download({ state: "starting" }))).toBe("Starting…");
    expect(activeText(en, download({ state: "finishing" }))).toBe("Finishing…");
    expect(activeText(fr, download({ state: "queued" }))).toBe("En attente…");
    expect(activeText(es, download({ state: "finishing" }))).toBe(
      "Finalizando…",
    );
  });
});

describe("machine", () => {
  const machine: HubHardware = {
    gpus: [
      { name: "NVIDIA GeForce RTX 3090", vramTotal: 24 * GIB, vramFree: null },
    ],
    ramTotal: 32 * GIB,
    ramAvailable: 20 * GIB,
    context: 32768,
  };

  it("names the GPU briefly, adds up VRAM", () => {
    expect(gpuName("NVIDIA GeForce RTX 3090")).toBe("RTX 3090");
    expect(gpuName("AMD Radeon RX 7900 XTX")).toBe("RX 7900 XTX");
    expect(machineLabel(en, machine)).toBe("RTX 3090 · 24 GB VRAM · 32 GB RAM");
    expect(
      machineLabel(en, {
        ...machine,
        gpus: [machine.gpus[0], machine.gpus[0]],
      }),
    ).toBe("2 × RTX 3090 · 48 GB VRAM · 32 GB RAM");
    expect(machineLabel(en, { ...machine, gpus: [] })).toBe(
      "CPU only · 32 GB RAM",
    );
    expect(machineLabel(fr, { ...machine, gpus: [] })).toBe(
      `CPU seul · 32${NBSP}Go RAM`,
    );
  });

  it("the hint explains the chips of this machine", () => {
    expect(machineHint(en, machine).split("\n")).toEqual([
      "Estimates for a 32K context.",
      "Fits your GPU: runs entirely on the graphics card.",
      "Runs partly on CPU: works, but slower.",
      "Too large: needs more memory than this machine has.",
    ]);
    expect(machineHint(en, { ...machine, gpus: [] }).split("\n")).toEqual([
      "Estimates for a 32K context, without a graphics card.",
      "Fits in memory: runs on the processor.",
      "Too large: needs more memory than this machine has.",
    ]);
    expect(machineHint(es, machine).split("\n")[0]).toBe(
      "Estimaciones para un contexto de 32K.",
    );
  });

  it("fit texts depend on the GPU", () => {
    expect(fitLabel(en, "fits", true)).toBe("Fits your GPU");
    expect(fitLabel(en, "fits", false)).toBe("Fits in memory");
    expect(fitLabel(en, "partial", true)).toBe("Runs partly on CPU");
    expect(fitLabel(en, "tooLarge", true)).toBe("Too large");
    expect(fitLabel(fr, "fits", true)).toBe("Tient sur votre GPU");
    expect(fitLabel(es, "tooLarge", false)).toBe("Demasiado grande");
  });
});

describe("avatars", () => {
  it("initials: first letter, then a version digit or the second word", () => {
    expect(initials("Qwen3-8B")).toBe("Q3");
    expect(initials("gemma-4-12B-it")).toBe("G4");
    expect(initials("Llama-3.3-70B-Instruct")).toBe("L3");
    expect(initials("gpt-oss-20b")).toBe("GO");
    expect(initials("DeepSeek-R1-Distill")).toBe("DR");
    expect(initials("Mistral")).toBe("Mi");
    expect(initials("SmolLM2-135M-Instruct-GGUF")).toBe("S2");
  });

  it("gradients are deterministic, case-insensitive, from the palette", () => {
    expect(gradientFor("bartowski")).toBe(gradientFor("Bartowski"));
    expect(GRADIENTS).toContain(gradientFor("someone-new"));
    expect(gradientFor("qwen")).not.toBe(gradientFor("gemma"));
    // Variables CSS lues par les classes communes, sombre compris.
    expect(gradientStyle("qwen")).toEqual({
      "--hub-from": GRADIENTS[0].from,
      "--hub-to": GRADIENTS[0].to,
      "--hub-text": GRADIENTS[0].text,
      "--hub-from-dark": GRADIENTS[0].darkFrom,
      "--hub-to-dark": GRADIENTS[0].darkTo,
      "--hub-text-dark": GRADIENTS[0].darkText,
    });
    // Aplat (pas de dégradé) : la teinte de fin, claire et sombre.
    expect(GRADIENT_CLASS.split(" ")).toEqual([
      "bg-(--hub-to)",
      "text-(--hub-text)",
      "dark:bg-(--hub-to-dark)",
      "dark:text-(--hub-text-dark)",
    ]);
    for (const gradient of GRADIENTS) {
      for (const color of Object.values(gradient)) {
        expect(color).toMatch(/^oklch\([\d.% ]+( \/ 0\.\d+)?\)$/);
      }
    }
  });

  it("the same family gives the same colour on every page", () => {
    // Carte (famille du catalogue), installé (`details.family` d'Ollama),
    // téléchargement (nom du dépôt) : même famille, donc même dégradé.
    expect(familyOf("qwen3")).toBe("qwen");
    expect(familyOf("Qwen3-8B")).toBe("qwen");
    expect(familyOf("gemma3")).toBe("gemma");
    expect(familyOf("DeepSeek-R1-Distill-Qwen-7B")).toBe("deepseek");
    expect(familyOf("Phi-4-mini-instruct")).toBe("phi");
    expect(familyOf("graphite")).toBeNull();
    // Premier nom qui contient une famille.
    expect(familyOf(undefined, "", "Meta-Llama-3.1-8B")).toBe("llama");
    expect(familyOf("Tiny-Chat", null)).toBeNull();
  });
});

describe("address parameters", () => {
  it("keeps valid values only", () => {
    expect(
      validateHubParams({
        q: "  qwen coder ",
        family: "qwen",
        section: "trending",
        uses: "vision,code,bogus,code",
        size: "small",
        license: "open",
        verified: true,
      }),
    ).toEqual({
      q: "qwen coder",
      family: "qwen",
      section: "trending",
      uses: "code,vision",
      size: "small",
      license: "open",
      verified: true,
    });
    expect(
      validateHubParams({
        q: "",
        family: "../etc",
        section: "nope",
        uses: "",
        size: "huge",
        license: "mit",
        verified: "yes",
      }),
    ).toEqual({});
    // Le routeur lit « q=8 » comme un nombre.
    expect(validateHubParams({ q: 8 })).toEqual({ q: "8" });
  });

  it("uses round-trip in a fixed order", () => {
    expect(usesFromParam("tools,code")).toEqual(["code", "tools"]);
    expect(usesFromParam(["reasoning"])).toEqual(["reasoning"]);
    expect(usesParam(["tools", "vision"])).toBe("vision,tools");
    expect(usesParam([])).toBeUndefined();
  });

  it("filters: verified only counts in a search", () => {
    const params = {
      uses: "code",
      size: "fits" as const,
      license: "open" as const,
      verified: true,
    };
    expect(filtersFromParams(params, false)).toEqual({
      uses: ["code"],
      size: "fits",
      openLicense: true,
      verifiedOnly: false,
    });
    expect(filtersFromParams(params, true).verifiedOnly).toBe(true);
    expect(filtersFromParams({}, true)).toEqual({
      uses: [],
      size: null,
      openLicense: false,
      verifiedOnly: false,
    });
  });

  it("a search needs two characters", () => {
    expect(searchText(" q ")).toBe("");
    expect(searchText(" qw ")).toBe("qw");
  });
});
