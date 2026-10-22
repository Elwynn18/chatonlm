import { describe, expect, it } from "vitest";
import {
  isCorrectShortcut,
  isNewChatShortcut,
  isNewEphemeralChatShortcut,
  isQuitShortcut,
  isRephraseShortcut,
  isSendShortcut,
  isSettingsShortcut,
  isRedoShortcut,
  isRedoTypingShortcut,
  isUndoShortcut,
  isUndoTypingShortcut,
  isVoiceModeShortcut,
  isVoiceModeSpace,
  NAMED_KEYS,
  SHORTCUT_GROUPS,
  SHORTCUTS,
  type ShortcutCombo,
  keysText,
  recordedKey,
  textEditingCommand,
  zoomShortcut,
} from "./shortcuts";

const key = (init: KeyboardEventInit) => new KeyboardEvent("keydown", init);

describe("isSettingsShortcut", () => {
  it("accepts Ctrl+,", () => {
    expect(isSettingsShortcut(key({ key: ",", ctrlKey: true }))).toBe(true);
  });

  it("refuses other modifiers, other keys, repeats and compositions", () => {
    const refused: KeyboardEventInit[] = [
      { key: "," },
      { key: ",", ctrlKey: true, shiftKey: true },
      { key: ",", ctrlKey: true, altKey: true },
      { key: ",", ctrlKey: true, metaKey: true },
      { key: ",", metaKey: true },
      { key: ",", ctrlKey: true, repeat: true },
      { key: ",", ctrlKey: true, isComposing: true },
      // La touche, pas sa position : en AZERTY, « ; » est sur Comma.
      { key: ";", code: "Comma", ctrlKey: true },
      { key: ".", ctrlKey: true },
    ];
    for (const init of refused) {
      expect(isSettingsShortcut(key(init)), JSON.stringify(init)).toBe(false);
    }
  });

  it("follows the key wherever it is: Ctrl+, on the AZERTY M key", () => {
    expect(
      isSettingsShortcut(key({ key: ",", code: "KeyM", ctrlKey: true })),
    ).toBe(true);
  });
});

describe("isQuitShortcut", () => {
  it("accepts Ctrl+Q, also with Caps Lock, wherever Q is (AZERTY)", () => {
    expect(isQuitShortcut(key({ key: "q", ctrlKey: true }))).toBe(true);
    expect(isQuitShortcut(key({ key: "Q", ctrlKey: true }))).toBe(true);
    expect(isQuitShortcut(key({ key: "q", code: "KeyA", ctrlKey: true }))).toBe(
      true,
    );
  });

  it("refuses Ctrl+Shift+Q, other modifiers, repeats and other keys", () => {
    const refused: KeyboardEventInit[] = [
      { key: "q" },
      { key: "Q", ctrlKey: true, shiftKey: true },
      { key: "q", ctrlKey: true, altKey: true },
      { key: "q", ctrlKey: true, metaKey: true },
      { key: "q", ctrlKey: true, repeat: true },
      { key: "q", ctrlKey: true, isComposing: true },
      // En AZERTY, la touche Q du QWERTY donne « a ».
      { key: "a", code: "KeyQ", ctrlKey: true },
    ];
    for (const init of refused) {
      expect(isQuitShortcut(key(init)), JSON.stringify(init)).toBe(false);
    }
  });
});

describe("isNewChatShortcut", () => {
  it("accepts Ctrl+N, also with Caps Lock", () => {
    expect(isNewChatShortcut(key({ key: "n", ctrlKey: true }))).toBe(true);
    expect(isNewChatShortcut(key({ key: "N", ctrlKey: true }))).toBe(true);
  });

  it("refuses Ctrl+Shift+N, other modifiers, repeats and other keys", () => {
    const refused: KeyboardEventInit[] = [
      { key: "n" },
      { key: "N", ctrlKey: true, shiftKey: true },
      // Ctrl+Maj+N avec le verrouillage des majuscules.
      { key: "n", ctrlKey: true, shiftKey: true },
      { key: "n", ctrlKey: true, altKey: true },
      { key: "n", ctrlKey: true, metaKey: true },
      { key: "n", ctrlKey: true, repeat: true },
      { key: "n", ctrlKey: true, isComposing: true },
      { key: "m", ctrlKey: true },
    ];
    for (const init of refused) {
      expect(isNewChatShortcut(key(init)), JSON.stringify(init)).toBe(false);
    }
  });
});

describe("isNewEphemeralChatShortcut", () => {
  it("accepts Ctrl+Shift+N, also with Caps Lock", () => {
    for (const k of ["N", "n"]) {
      expect(
        isNewEphemeralChatShortcut(
          key({ key: k, ctrlKey: true, shiftKey: true }),
        ),
      ).toBe(true);
    }
  });

  it("refuses Ctrl+N, other modifiers, repeats and other keys", () => {
    const refused: KeyboardEventInit[] = [
      { key: "n", ctrlKey: true },
      { key: "N", shiftKey: true },
      { key: "N", ctrlKey: true, shiftKey: true, altKey: true },
      { key: "N", ctrlKey: true, shiftKey: true, metaKey: true },
      { key: "N", ctrlKey: true, shiftKey: true, repeat: true },
      { key: "N", ctrlKey: true, shiftKey: true, isComposing: true },
      { key: "M", ctrlKey: true, shiftKey: true },
    ];
    for (const init of refused) {
      expect(isNewEphemeralChatShortcut(key(init)), JSON.stringify(init)).toBe(
        false,
      );
    }
  });
});

describe("zoomShortcut", () => {
  it("reads the official keys, the keypad and the top-row 0 by its code", () => {
    const cases: [KeyboardEventInit, string][] = [
      [{ key: "+", ctrlKey: true, shiftKey: true }, "zoomIn"],
      [{ key: "=", ctrlKey: true }, "zoomIn"],
      [{ key: "+", code: "NumpadAdd", ctrlKey: true }, "zoomIn"],
      [{ key: "-", ctrlKey: true }, "zoomOut"],
      [{ key: "-", code: "NumpadSubtract", ctrlKey: true }, "zoomOut"],
      [{ key: "0", ctrlKey: true }, "zoomReset"],
      [{ key: "0", code: "Numpad0", ctrlKey: true }, "zoomReset"],
      // AZERTY : Ctrl et la touche « à 0 ».
      [{ key: "à", code: "Digit0", ctrlKey: true }, "zoomReset"],
      [{ key: "=", metaKey: true }, "zoomIn"],
    ];
    for (const [init, action] of cases) {
      expect(zoomShortcut(key(init)), JSON.stringify(init)).toBe(action);
    }
  });

  it("not without Ctrl, with Alt (AltGr) or for other keys", () => {
    const refused: KeyboardEventInit[] = [
      { key: "=" },
      { key: "-" },
      { key: "0", code: "Digit0" },
      { key: "}", code: "Equal", ctrlKey: true, altKey: true },
      { key: "0", ctrlKey: true, altKey: true },
      { key: "1", ctrlKey: true },
      { key: "=", ctrlKey: true, isComposing: true },
    ];
    for (const init of refused) {
      expect(zoomShortcut(key(init)), JSON.stringify(init)).toBeNull();
    }
  });
});

/** Événement d'une combinaison montrée (lettres en minuscules). */
function eventOf(keys: ShortcutCombo): KeyboardEventInit {
  const init: KeyboardEventInit = {};
  for (const name of keys) {
    if (name === "ctrl") init.ctrlKey = true;
    else if (name === "shift") init.shiftKey = true;
    else if (name === "enter") init.key = "Enter";
    else if (name === "esc") init.key = "Escape";
    else if (name === "space") init.key = " ";
    else init.key = name.length === 1 ? name.toLowerCase() : name;
  }
  return init;
}

describe("SHORTCUTS catalog", () => {
  it("every combination of an entry is recognised by its own predicate", () => {
    for (const entry of SHORTCUTS) {
      expect(entry.keys.length, entry.id).toBeGreaterThan(0);
      for (const combo of entry.keys) {
        expect(
          entry.matches(key(eventOf(combo))),
          `${entry.id}: ${combo.join("+")}`,
        ).toBe(true);
      }
      // Une valeur aria par combinaison.
      expect(entry.aria.split(" "), entry.id).toHaveLength(entry.keys.length);
    }
  });

  it("shows every combination the handlers accept: Ctrl+Y redoes, Ctrl+Enter sends", () => {
    const keysOf = (id: string) =>
      SHORTCUTS.find((entry) => entry.id === id)?.keys.map((combo) =>
        combo.join("+"),
      );
    expect(keysOf("redoTyping")).toEqual(["ctrl+shift+Z", "ctrl+Y"]);
    expect(keysOf("redoCorrection")).toEqual(["ctrl+shift+Z", "ctrl+Y"]);
    expect(keysOf("send")).toEqual(["enter", "ctrl+enter"]);
    expect(isSendShortcut(key({ key: "Enter", ctrlKey: true }))).toBe(true);
    expect(keysOf("pasteInReview")).toEqual(["ctrl+V"]);
  });

  it("in a group, no entry fires for the keys of another combination", () => {
    // Groupes différents : pages différentes (Ctrl+Entrée envoie aussi un
    // message dans le chat, comme l'officiel).
    for (const entry of SHORTCUTS) {
      const own = new Set(entry.keys.map((combo) => combo.join("+")));
      for (const other of SHORTCUTS) {
        if (other.group !== entry.group) continue;
        for (const combo of other.keys) {
          if (own.has(combo.join("+"))) continue;
          expect(
            entry.matches(key(eventOf(combo))),
            `${entry.id} for ${other.id} (${combo.join("+")})`,
          ).toBe(false);
        }
      }
    }
  });

  it("writes several combinations with the translated “or”", () => {
    const t = ((k: string) =>
      k === "settings.shortcuts.either"
        ? "<first/> ou <second/>"
        : k === "settings.shortcuts.keys.shift"
          ? "Maj"
          : "Ctrl") as Parameters<typeof keysText>[0];
    expect(
      keysText(t, [
        ["ctrl", "shift", "Z"],
        ["ctrl", "Y"],
      ]),
    ).toBe("Ctrl+Maj+Z ou Ctrl+Y");
    expect(keysText(t, [["ctrl", "Z"]])).toBe("Ctrl+Z");
  });

  it("has unique ids, known groups and named keys, and an aria value", () => {
    const ids = SHORTCUTS.map((entry) => entry.id);
    expect(new Set(ids).size).toBe(ids.length);
    for (const entry of SHORTCUTS) {
      expect(SHORTCUT_GROUPS).toContain(entry.group);
      expect(entry.aria).not.toBe("");
      for (const name of entry.keys.flat()) {
        // Une touche écrite en toutes lettres doit être une touche nommée
        // (traduite), sinon un seul caractère.
        if (name.length > 1) expect(NAMED_KEYS).toContain(name);
      }
    }
  });
});

describe("Proofread shortcuts", () => {
  it("Ctrl+Enter corrects, Ctrl+Shift+Enter rephrases, never both", () => {
    const correct = key({ key: "Enter", ctrlKey: true });
    const rephrase = key({ key: "Enter", ctrlKey: true, shiftKey: true });
    expect(isCorrectShortcut(correct)).toBe(true);
    expect(isRephraseShortcut(correct)).toBe(false);
    expect(isRephraseShortcut(rephrase)).toBe(true);
    expect(isCorrectShortcut(rephrase)).toBe(false);
    for (const init of [
      { key: "Enter" },
      { key: "Enter", ctrlKey: true, altKey: true },
      { key: "Enter", ctrlKey: true, metaKey: true },
      { key: "Enter", ctrlKey: true, repeat: true },
      { key: "Enter", ctrlKey: true, isComposing: true },
    ]) {
      expect(isCorrectShortcut(key(init)), JSON.stringify(init)).toBe(false);
    }
  });

  it("Ctrl+Z undoes, also with Caps Lock; not Ctrl+Shift+Z (redo)", () => {
    expect(isUndoShortcut(key({ key: "z", ctrlKey: true }))).toBe(true);
    expect(isUndoShortcut(key({ key: "Z", ctrlKey: true }))).toBe(true);
    expect(
      isUndoShortcut(key({ key: "Z", ctrlKey: true, shiftKey: true })),
    ).toBe(false);
    expect(isUndoShortcut(key({ key: "z" }))).toBe(false);
    expect(isUndoShortcut(key({ key: "z", ctrlKey: true, altKey: true }))).toBe(
      false,
    );
  });

  it("Ctrl+Shift+Z or Ctrl+Y redoes the review, not held or with Alt", () => {
    for (const init of [
      { key: "Z", ctrlKey: true, shiftKey: true },
      { key: "z", ctrlKey: true, shiftKey: true },
      { key: "y", ctrlKey: true },
      { key: "Y", ctrlKey: true },
    ]) {
      expect(isRedoShortcut(key(init)), JSON.stringify(init)).toBe(true);
    }
    for (const init of [
      { key: "z", ctrlKey: true },
      { key: "y", ctrlKey: true, shiftKey: true },
      { key: "y", ctrlKey: true, repeat: true },
      { key: "y", ctrlKey: true, altKey: true },
      { key: "y" },
    ]) {
      expect(isRedoShortcut(key(init)), JSON.stringify(init)).toBe(false);
    }
  });

  it("Enter sends without Shift or composition", () => {
    expect(isSendShortcut(key({ key: "Enter" }))).toBe(true);
    expect(isSendShortcut(key({ key: "Enter", shiftKey: true }))).toBe(false);
    expect(isSendShortcut(key({ key: "Enter", isComposing: true }))).toBe(
      false,
    );
  });
});

describe("undo and redo in a text field", () => {
  const on = (target: EventTarget, init: KeyboardEventInit) => {
    const event = key(init);
    Object.defineProperty(event, "target", { value: target });
    return event;
  };

  it("Ctrl+Z undoes, Ctrl+Shift+Z and Ctrl+Y redo, held keys too", () => {
    const area = document.createElement("textarea");
    expect(textEditingCommand(on(area, { key: "z", ctrlKey: true }))).toBe(
      "undo",
    );
    expect(
      textEditingCommand(on(area, { key: "z", ctrlKey: true, repeat: true })),
    ).toBe("undo");
    expect(
      textEditingCommand(on(area, { key: "Z", ctrlKey: true, shiftKey: true })),
    ).toBe("redo");
    expect(textEditingCommand(on(area, { key: "Y", ctrlKey: true }))).toBe(
      "redo",
    );
  });

  it("only in a text field, never with Alt (AltGr), Meta or a composition", () => {
    const area = document.createElement("textarea");
    const text = Object.assign(document.createElement("input"), {
      type: "text",
    });
    const box = Object.assign(document.createElement("input"), {
      type: "checkbox",
    });
    expect(textEditingCommand(on(text, { key: "z", ctrlKey: true }))).toBe(
      "undo",
    );
    for (const target of [box, document.body]) {
      expect(textEditingCommand(on(target, { key: "z", ctrlKey: true }))).toBe(
        null,
      );
    }
    for (const init of [
      { key: "z" },
      { key: "z", ctrlKey: true, altKey: true },
      { key: "z", ctrlKey: true, metaKey: true },
      { key: "z", ctrlKey: true, isComposing: true },
      { key: "y", ctrlKey: true, shiftKey: true },
      { key: "x", ctrlKey: true },
    ]) {
      expect(textEditingCommand(on(area, init)), JSON.stringify(init)).toBe(
        null,
      );
    }
    expect(isUndoTypingShortcut(key({ key: "Z", ctrlKey: true }))).toBe(true);
    expect(isRedoTypingShortcut(key({ key: "z", ctrlKey: true }))).toBe(false);
  });
});

describe("Voice mode shortcuts", () => {
  it("Ctrl+Shift+Space starts or ends voice mode, not held, not with Alt", () => {
    expect(
      isVoiceModeShortcut(key({ key: " ", ctrlKey: true, shiftKey: true })),
    ).toBe(true);
    for (const init of [
      { key: " ", ctrlKey: true },
      { key: " ", shiftKey: true },
      { key: " ", ctrlKey: true, shiftKey: true, altKey: true },
      { key: " ", ctrlKey: true, shiftKey: true, metaKey: true },
      { key: " ", ctrlKey: true, shiftKey: true, repeat: true },
      { key: " ", ctrlKey: true, shiftKey: true, isComposing: true },
      { key: "a", ctrlKey: true, shiftKey: true },
    ]) {
      expect(isVoiceModeShortcut(key(init)), JSON.stringify(init)).toBe(false);
    }
  });

  it("Space alone, held or not, for the voice bar", () => {
    expect(isVoiceModeSpace(key({ key: " " }))).toBe(true);
    expect(isVoiceModeSpace(key({ key: " ", repeat: true }))).toBe(true);
    for (const init of [
      { key: " ", ctrlKey: true },
      { key: " ", shiftKey: true },
      { key: " ", altKey: true },
      { key: " ", isComposing: true },
      { key: "Enter" },
    ]) {
      expect(isVoiceModeSpace(key(init)), JSON.stringify(init)).toBe(false);
    }
  });
});

describe("recordedKey (keys of a global shortcut chosen in the app)", () => {
  it("gives the modifiers and the virtual key code, not the physical key", () => {
    // AZERTY : la touche marquée A est KeyQ ; le moteur veut le code de « A ».
    expect(
      recordedKey(
        key({
          key: "a",
          code: "KeyQ",
          keyCode: 65,
          ctrlKey: true,
          altKey: true,
        }),
      ),
    ).toEqual({
      kind: "keys",
      keys: { ctrl: true, alt: true, shift: false, meta: false, keyCode: 65 },
    });
    expect(
      recordedKey(
        key({ key: "F9", keyCode: 120, shiftKey: true, metaKey: true }),
      ),
    ).toEqual({
      kind: "keys",
      keys: { ctrl: false, alt: false, shift: true, meta: true, keyCode: 120 },
    });
  });

  it("waits for more after a modifier alone or a repeated key", () => {
    for (const keyCode of [16, 17, 18, 91, 92]) {
      expect(
        recordedKey(key({ key: "Control", keyCode, ctrlKey: true })),
      ).toEqual({
        kind: "ignore",
      });
    }
    expect(
      recordedKey(key({ key: "k", keyCode: 75, ctrlKey: true, repeat: true })),
    ).toEqual({ kind: "ignore" });
  });

  it("cancels on Esc alone, and leaves Tab to the browser", () => {
    expect(recordedKey(key({ key: "Escape", keyCode: 27 }))).toEqual({
      kind: "cancel",
    });
    expect(
      recordedKey(key({ key: "Escape", keyCode: 27, ctrlKey: true })).kind,
    ).toBe("keys");
    expect(recordedKey(key({ key: "Tab", keyCode: 9 }))).toEqual({
      kind: "pass",
    });
    expect(
      recordedKey(key({ key: "Tab", keyCode: 9, shiftKey: true })),
    ).toEqual({ kind: "pass" });
    // Une touche refusée (Entrée) va au moteur, qui dit pourquoi.
    expect(recordedKey(key({ key: "Enter", keyCode: 13 })).kind).toBe("keys");
  });
});
