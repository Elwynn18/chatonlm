// @vitest-environment node
import { describe, expect, it } from "vitest";
import { bridgeDirSetting, unpairedText } from "./extension";

describe("extension", () => {
  it("reads the hidden bridge folder only from the user's settings", () => {
    // Un dépôt ouvert (.vscode/settings.json) ne redirige pas l'extension
    // vers un faux pont.
    const inspect = (values: Record<string, unknown>) => ({
      inspect: () => values as never,
    });
    expect(
      bridgeDirSetting(
        inspect({ globalValue: "/run/user/1000/x", workspaceValue: "/repo" }),
      ),
    ).toBe("/run/user/1000/x");
    expect(
      bridgeDirSetting(
        inspect({ workspaceValue: ".", workspaceFolderValue: "." }),
      ),
    ).toBeUndefined();
    expect(bridgeDirSetting(inspect({ globalValue: 42 }))).toBeUndefined();
  });

  it("says where else to remove the editor only when the app did not (R16)", () => {
    expect(unpairedText(true)).toBe("This editor is no longer paired.");
    expect(unpairedText(false)).toBe(
      "This editor is no longer paired. You can also remove it in ChatonLM, under Settings › VS Code.",
    );
  });
});
