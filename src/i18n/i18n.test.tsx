// Langue de l'interface : textes, valeurs, pluriels, balises, changement de
// langue sans rechargement, formats et repérage des textes en dur.

import { act, cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import {
  getLanguage,
  setLanguage,
  translate,
  translator,
  useLanguage,
  useT,
} from "@/i18n";
import {
  formatBytes,
  formatDecimal,
  formatSizeValue,
  formatTimeAgo,
  languageName,
  nativeLanguageName,
} from "@/i18n/format";
import { Trans } from "@/i18n/Trans";
import { untranslatedTexts, usePseudoLanguage } from "@/test/i18n";

afterEach(() => {
  cleanup();
});

describe("translate", () => {
  it("reads a text in each language, with its values", () => {
    expect(translate("en", "settings.general.language.label")).toBe("Language");
    expect(translate("fr", "settings.general.language.label")).toBe("Langue");
    expect(translate("es", "settings.general.language.label")).toBe("Idioma");
    expect(translate("fr", "common.size", { value: "5,0", unit: "Go" })).toBe(
      "5,0 Go",
    );
  });

  it("groups thousands from 10,000 only, like the official texts", () => {
    expect(translate("en", "common.size", { value: 1010, unit: "B" })).toBe(
      "1010 B",
    );
    expect(formatDecimal("en", 1010, 1)).toBe("1010.0");
    expect(formatDecimal("fr", 12345.5, 1)).toBe("12345,5");
  });

  it("writes numbers the way of the language", () => {
    expect(translate("en", "common.size", { value: 12345, unit: "B" })).toBe(
      "12,345 B",
    );
    expect(translate("fr", "common.size", { value: 12345, unit: "o" })).toBe(
      "12 345 o",
    );
  });

  it("refuses, in tests, a missing value", () => {
    expect(() => translate("en", "common.size", { value: "1" })).toThrow(
      /unit/,
    );
  });

  it("gives the same function for a language until it changes", () => {
    expect(translator("fr")).toBe(translator("fr"));
    expect(translator("fr")).not.toBe(translator("es"));
  });
});

describe("formats", () => {
  const GIB = 2 ** 30;
  const MIB = 2 ** 20;

  it("byte sizes: one decimal under 10, base 1024, never 1024 MB", () => {
    const en = translator("en");
    expect(formatBytes(en, 5 * GIB)).toBe("5.0 GB");
    expect(formatBytes(en, 18.2 * GIB)).toBe("18 GB");
    expect(formatBytes(en, 850 * MIB)).toBe("850 MB");
    expect(formatBytes(en, 1023.9 * MIB)).toBe("1.0 GB");
    expect(formatBytes(en, 512)).toBe("512 B");
    expect(formatBytes(en, 0)).toBe("0 B");
    expect(formatBytes(translator("fr"), 5 * GIB)).toBe("5,0\u00a0Go");
    expect(formatBytes(translator("es"), 850 * MIB)).toBe("850\u00a0MB");
  });

  it("relative dates: just now, then the words of the language", () => {
    const now = Date.parse("2026-10-05T12:00:00Z");
    const en = translator("en");
    expect(formatTimeAgo(en, "2026-10-05T11:59:30Z", now)).toBe("just now");
    expect(formatTimeAgo(en, "2026-10-04T12:00:00Z", now)).toBe("yesterday");
    expect(formatTimeAgo(en, "2026-10-02T12:00:00Z", now)).toBe("3 days ago");
    expect(formatTimeAgo(en, now - 3 * 3600 * 1000, now)).toBe("3 hours ago");
    expect(formatTimeAgo(en, "2026-06-01T12:00:00Z", now)).toBe("4 months ago");
    expect(formatTimeAgo(en, "not a date", now)).toBe("");
    const fr = translator("fr");
    expect(formatTimeAgo(fr, "2026-10-05T11:59:30Z", now)).toBe("à l'instant");
    expect(formatTimeAgo(fr, "2026-10-04T12:00:00Z", now)).toBe("hier");
    expect(formatTimeAgo(translator("es"), "2026-10-02T12:00:00Z", now)).toBe(
      "hace 3 días",
    );
  });

  it("sizes use the decimal separator and units of the language", () => {
    expect(formatSizeValue("en", 5, 1, "GB")).toBe("5.0 GB");
    expect(formatSizeValue("fr", 5, 1, "GB")).toBe("5,0 Go");
    expect(formatSizeValue("es", 1.5, 1, "MB")).toBe("1,5 MB");
    expect(formatDecimal("fr", 0.25, 2)).toBe("0,25");
  });

  it("languages are named in the displayed language, or in themselves", () => {
    expect(languageName("fr", "de")).toBe("Allemand");
    expect(languageName("es", "en")).toBe("Inglés");
    expect(
      ["en", "es", "fr"].map((code) => nativeLanguageName(code as never)),
    ).toEqual(["English", "Español", "Français"]);
  });
});

function Probe() {
  const t = useT();
  const lang = useLanguage();
  return (
    <p data-lang={lang}>
      {t("settings.general.language.label")}{" "}
      <Trans
        k="settings.general.language.description"
        tags={{ b: (chunks) => <b>{chunks}</b> }}
      />
    </p>
  );
}

describe("setLanguage", () => {
  it("renders every reader again, without remounting it", () => {
    const { container } = render(<Probe />);
    const paragraph = container.querySelector("p");
    expect(paragraph?.textContent).toContain("Language");
    act(() => setLanguage("es"));
    expect(container.querySelector("p")).toBe(paragraph);
    expect(paragraph?.getAttribute("data-lang")).toBe("es");
    expect(paragraph?.textContent).toContain("Idioma");
    expect(document.documentElement.lang).toBe("es");
    expect(getLanguage()).toBe("es");
  });
});

describe("Trans", () => {
  it("replaces the tags of a text by the given elements", () => {
    render(
      <Trans
        k="settings.general.language.description"
        tags={{ b: (chunks) => <b>{chunks}</b> }}
      />,
    );
    // Texte sans balise : rendu tel quel.
    expect(
      screen.getByText(
        "Follow the system language, or always use another one.",
      ),
    ).toBeTruthy();
  });
});

describe("untranslatedTexts", () => {
  it("marks the texts of a screen already shown", () => {
    render(<Probe />);
    usePseudoLanguage();
    expect(untranslatedTexts()).toEqual([]);
  });

  it("finds the texts that do not come from the catalogs", () => {
    usePseudoLanguage();
    render(
      <div>
        <Probe />
        <button type="button" aria-label="Close panel">
          qwen3:0.6b
        </button>
        <span>Hard-coded words</span>
      </div>,
    );
    expect(untranslatedTexts(document.body, ["qwen3:0.6b"])).toEqual([
      "Hard-coded words",
      "[aria-label] Close panel",
    ]);
  });
});
