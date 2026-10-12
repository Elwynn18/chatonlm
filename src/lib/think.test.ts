import { describe, expect, it } from "vitest";
import type { ModelDetails } from "@/bindings/ModelDetails";
import type { Think } from "@/bindings/Think";
import { translator } from "@/i18n";
import {
  thinkControl,
  thinkLabel,
  thinkToSend,
  voiceThinkToSend,
} from "./think";

/** Modèle qui sait raisonner ; `thinking` : objet de /api/show. */
function thinker(
  values: Think[] | null,
  defaultValue: Think | null = null,
): ModelDetails {
  return {
    model: "m",
    capabilities: ["completion", "thinking"],
    thinking: values === null ? null : { values, default: defaultValue },
    alwaysThinks: false,
  };
}

const LEVELS: Think[] = ["low", "medium", "high"];

/** Libellés de l'interface en anglais (langue source). */
const en = translator("en");

describe("thinkControl", () => {
  it("no button without details or without the thinking capability", () => {
    expect(thinkControl(en, undefined, true)).toEqual({ kind: "none" });
    expect(thinkControl(en, null, true)).toEqual({ kind: "none" });
    // Même avec un objet thinking : la capacité décide.
    expect(
      thinkControl(
        en,
        {
          model: "m",
          capabilities: ["completion", "vision"],
          thinking: { values: [false, true], default: true },
          alwaysThinks: false,
        },
        true,
      ),
    ).toEqual({ kind: "none" });
  });

  it("without metadata: toggle, on by default, remembered choice kept", () => {
    expect(thinkControl(en, thinker(null), null)).toEqual({
      kind: "toggle",
      value: true,
    });
    expect(thinkControl(en, thinker(null), false)).toEqual({
      kind: "toggle",
      value: false,
    });
    // Un niveau n'est pas permis sans métadonnées : défaut.
    expect(thinkControl(en, thinker(null), "high")).toEqual({
      kind: "toggle",
      value: true,
    });
  });

  it("an empty list of values counts as no metadata", () => {
    expect(thinkControl(en, thinker([]), false)).toEqual({
      kind: "toggle",
      value: false,
    });
  });

  it("[false]: the model cannot think, no button", () => {
    expect(thinkControl(en, thinker([false], false), true)).toEqual({
      kind: "none",
    });
  });

  it("[true]: always thinks (no button), whatever was remembered", () => {
    expect(thinkControl(en, thinker([true], true), false)).toEqual({
      kind: "fixed",
    });
    expect(thinkControl(en, thinker([true]), null)).toEqual({ kind: "fixed" });
  });

  it("seen thinking with think off: always thinks, whatever Ollama says", () => {
    for (const details of [thinker(null), thinker([false, true], false)]) {
      expect(
        thinkControl(en, { ...details, alwaysThinks: true }, false),
      ).toEqual({
        kind: "fixed",
      });
    }
    // Sans la capacité, toujours rien.
    expect(
      thinkControl(
        en,
        { ...thinker(null), capabilities: ["completion"], alwaysThinks: true },
        null,
      ),
    ).toEqual({ kind: "none" });
  });

  it("[false, true]: toggle with Ollama's default, else on", () => {
    expect(thinkControl(en, thinker([false, true], true), null)).toEqual({
      kind: "toggle",
      value: true,
    });
    expect(thinkControl(en, thinker([false, true], false), null)).toEqual({
      kind: "toggle",
      value: false,
    });
    expect(thinkControl(en, thinker([true, false]), null)).toEqual({
      kind: "toggle",
      value: true,
    });
    // Défaut inattendu (niveau) : marche.
    expect(thinkControl(en, thinker([false, true], "high"), null)).toEqual({
      kind: "toggle",
      value: true,
    });
  });

  it("[false, true]: the remembered choice wins over the default", () => {
    expect(thinkControl(en, thinker([false, true], true), false)).toEqual({
      kind: "toggle",
      value: false,
    });
    expect(thinkControl(en, thinker([false, true], false), true)).toEqual({
      kind: "toggle",
      value: true,
    });
    // Choix périmé (le modèle avait des niveaux) : défaut.
    expect(thinkControl(en, thinker([false, true], false), "high")).toEqual({
      kind: "toggle",
      value: false,
    });
  });

  it("levels: menu in Ollama's order, default level", () => {
    expect(thinkControl(en, thinker(LEVELS, "medium"), null)).toEqual({
      kind: "levels",
      value: "medium",
      options: [
        { value: "low", label: "Low" },
        { value: "medium", label: "Medium" },
        { value: "high", label: "High" },
      ],
    });
    expect(
      thinkControl(en, thinker(["high", "low"], "low"), null),
    ).toMatchObject({
      value: "low",
      options: [
        { value: "high", label: "High" },
        { value: "low", label: "Low" },
      ],
    });
  });

  it("levels: Off where false is among the values", () => {
    expect(thinkControl(en, thinker([false, ...LEVELS], "low"), null)).toEqual({
      kind: "levels",
      value: "low",
      options: [
        { value: false, label: "Off" },
        { value: "low", label: "Low" },
        { value: "medium", label: "Medium" },
        { value: "high", label: "High" },
      ],
    });
    expect(
      thinkControl(en, thinker(["low", "high", false], false), null),
    ).toMatchObject({
      value: false,
      options: [{ value: "low" }, { value: "high" }, { value: false }],
    });
  });

  it("levels: On where true is among the values, in Ollama's order", () => {
    expect(thinkControl(en, thinker([true, false, "low"], true), null)).toEqual(
      {
        kind: "levels",
        value: true,
        options: [
          { value: true, label: "On" },
          { value: false, label: "Off" },
          { value: "low", label: "Low" },
        ],
      },
    );
    // Sans false : pas de Off.
    expect(
      thinkControl(en, thinker([true, "low", "high"], "high"), null),
    ).toEqual({
      kind: "levels",
      value: "high",
      options: [
        { value: true, label: "On" },
        { value: "low", label: "Low" },
        { value: "high", label: "High" },
      ],
    });
  });

  it("levels: Nemotron 3.5 Nano keeps its default, full thinking (true)", () => {
    // Ollama, model/renderers/nemotron3nano.go : "medium" réduit l'effort.
    const nemotron = thinker([false, true, "medium"], true);
    const control = thinkControl(en, nemotron, null);
    expect(control).toEqual({
      kind: "levels",
      value: true,
      options: [
        { value: false, label: "Off" },
        { value: true, label: "On" },
        { value: "medium", label: "Medium" },
      ],
    });
    // Envoyé explicitement, pas omis ni remplacé par "medium".
    expect(thinkToSend(control)).toBe(true);
  });

  it("levels: a remembered On is read back", () => {
    expect(
      thinkControl(en, thinker([false, true, "medium"], "medium"), true),
    ).toMatchObject({ value: true });
    expect(
      thinkControl(en, thinker([true, "low", "high"], "low"), true),
    ).toMatchObject({ value: true });
    // Les autres choix aussi, face au défaut true.
    expect(
      thinkControl(en, thinker([false, true, "medium"], true), "medium"),
    ).toMatchObject({ value: "medium" });
    expect(
      thinkControl(en, thinker([false, true, "medium"], true), false),
    ).toMatchObject({ value: false });
  });

  it("levels: a default that is not allowed falls back to the first level", () => {
    for (const fallback of [null, true, "max", false] as (Think | null)[]) {
      expect(thinkControl(en, thinker(LEVELS, fallback), null)).toMatchObject({
        kind: "levels",
        value: "low",
      });
    }
    // Off, ou On, en tête : premier niveau, pas la première option.
    for (const fallback of ["max", null] as (Think | null)[]) {
      expect(
        thinkControl(en, thinker([false, ...LEVELS], fallback), null),
      ).toMatchObject({ value: "low" });
      expect(
        thinkControl(en, thinker([true, false, "medium"], fallback), null),
      ).toMatchObject({ value: "medium" });
    }
  });

  it("levels: the remembered level wins, a stale one is ignored", () => {
    expect(thinkControl(en, thinker(LEVELS, "medium"), "high")).toMatchObject({
      value: "high",
    });
    expect(
      thinkControl(en, thinker([false, ...LEVELS], "low"), false),
    ).toMatchObject({ value: false });
    // Plus permis : Off sans false, On sans true, niveau disparu.
    for (const stale of [false, true, "max"] as Think[]) {
      expect(thinkControl(en, thinker(LEVELS, "medium"), stale)).toMatchObject({
        value: "medium",
      });
    }
  });

  it("other level names: known labels, else capitalized", () => {
    // Variante qwen3.8 d'Ollama (model/renderers/qwen35.go).
    expect(
      thinkControl(
        en,
        thinker([false, "low", "medium", "xhigh"], "xhigh"),
        null,
      ),
    ).toEqual({
      kind: "levels",
      value: "xhigh",
      options: [
        { value: false, label: "Off" },
        { value: "low", label: "Low" },
        { value: "medium", label: "Medium" },
        { value: "xhigh", label: "Extra high" },
      ],
    });
    expect(thinkControl(en, thinker(["minimal", "max"], "max"), null)).toEqual({
      kind: "levels",
      value: "max",
      options: [
        { value: "minimal", label: "Minimal" },
        { value: "max", label: "Max" },
      ],
    });
  });

  it("ignores empty names, duplicates and values of another type", () => {
    const odd = ["", "low", "low", 1, null, false, false] as unknown as Think[];
    expect(thinkControl(en, thinker(odd, "low"), null)).toEqual({
      kind: "levels",
      value: "low",
      options: [
        { value: "low", label: "Low" },
        { value: false, label: "Off" },
      ],
    });
  });
});

describe("thinkLabel", () => {
  it("official labels, Off, On, known and capitalized names", () => {
    expect(thinkLabel(en, "low")).toBe("Low");
    expect(thinkLabel(en, "medium")).toBe("Medium");
    expect(thinkLabel(en, "high")).toBe("High");
    expect(thinkLabel(en, false)).toBe("Off");
    expect(thinkLabel(en, true)).toBe("On");
    expect(thinkLabel(en, "xhigh")).toBe("Extra high");
    expect(thinkLabel(en, "max")).toBe("Max");
    expect(thinkLabel(en, "minimal")).toBe("Minimal");
  });

  it("in the language of t; an unknown level keeps Ollama's name", () => {
    const fr = translator("fr");
    expect(thinkLabel(fr, false)).toBe("Désactivé");
    expect(thinkLabel(fr, "xhigh")).toBe("Très élevé");
    expect(thinkLabel(translator("es"), "high")).toBe("Alto");
    expect(thinkLabel(fr, "minimal")).toBe("Minimal");
    // Nom d'Ollama qui serait une propriété d'un objet : pas un niveau connu.
    expect(thinkLabel(fr, "constructor")).toBe("Constructor");
  });
});

describe("thinkToSend", () => {
  it("sends the toggle and the level explicitly, nothing otherwise", () => {
    expect(thinkToSend({ kind: "toggle", value: false })).toBe(false);
    expect(thinkToSend({ kind: "toggle", value: true })).toBe(true);
    expect(thinkToSend({ kind: "levels", value: "high", options: [] })).toBe(
      "high",
    );
    expect(thinkToSend({ kind: "levels", value: false, options: [] })).toBe(
      false,
    );
    expect(thinkToSend({ kind: "levels", value: true, options: [] })).toBe(
      true,
    );
    expect(thinkToSend({ kind: "fixed" })).toBeUndefined();
    expect(thinkToSend({ kind: "none" })).toBeUndefined();
  });
});

describe("voiceThinkToSend (voice mode)", () => {
  it("turns thinking off when the model allows it", () => {
    // Bascule : false, même si le bouton est sur On.
    expect(voiceThinkToSend(thinkControl(en, thinker(null), true), false)).toBe(
      false,
    );
    expect(
      voiceThinkToSend(
        thinkControl(en, thinker([false, true], true), true),
        false,
      ),
    ).toBe(false);
    // Niveaux : le plus bas, « low ».
    expect(
      voiceThinkToSend(
        thinkControl(en, thinker(LEVELS, "high"), "high"),
        false,
      ),
    ).toBe("low");
    // Niveaux avec Off (Nemotron 3.5 Nano) : coupé.
    expect(
      voiceThinkToSend(
        thinkControl(en, thinker([false, true, "medium"], true), true),
        false,
      ),
    ).toBe(false);
    // Niveaux sans « low » : le premier proposé.
    expect(
      voiceThinkToSend(
        thinkControl(en, thinker(["medium", "high"], "high"), null),
        false,
      ),
    ).toBe("medium");
  });

  it("sends nothing for a model that always thinks or cannot think", () => {
    expect(
      voiceThinkToSend(thinkControl(en, thinker([true], true), null), false),
    ).toBeUndefined();
    expect(
      voiceThinkToSend(thinkControl(en, thinker([false]), null), false),
    ).toBeUndefined();
  });

  it("“Let models think”: the value of the Think button", () => {
    expect(
      voiceThinkToSend(
        thinkControl(en, thinker([false, true], false), true),
        true,
      ),
    ).toBe(true);
    expect(
      voiceThinkToSend(thinkControl(en, thinker(LEVELS, "low"), "high"), true),
    ).toBe("high");
    expect(
      voiceThinkToSend(thinkControl(en, thinker([true], true), null), true),
    ).toBeUndefined();
  });
});
