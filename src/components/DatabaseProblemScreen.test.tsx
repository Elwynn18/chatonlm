import { cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { DatabaseProblem } from "@/bindings/DatabaseProblem";
import type { DatabaseProblemKind } from "@/bindings/DatabaseProblemKind";
import { formatDateTime } from "@/i18n/format";
import { DatabaseProblemScreen } from "./DatabaseProblemScreen";

afterEach(cleanup);

const PATH = "/home/user/.local/share/com.chatonlm.chatonlm/db/chatonlm.sqlite";

function problem(kind: DatabaseProblemKind, message = "database error") {
  return { kind, message, path: PATH, backup: null } satisfies DatabaseProblem;
}

/** 6 oct. 2026, 18:40 (heure locale). */
const SAVED_AT = new Date(2026, 9, 6, 18, 40).getTime();

function renderScreen(value: DatabaseProblem) {
  const onRecover = vi.fn<(fresh: boolean) => Promise<DatabaseProblem | null>>(
    () => Promise.resolve(null),
  );
  const onRestore = vi.fn<() => Promise<DatabaseProblem | null>>(() =>
    Promise.resolve(null),
  );
  const onRecovered = vi.fn();
  render(
    <DatabaseProblemScreen
      problem={value}
      onRecover={onRecover}
      onRestore={onRestore}
      onRecovered={onRecovered}
    />,
  );
  return { onRecover, onRestore, onRecovered };
}

const buttons = () =>
  screen.queryAllByRole("button").map((button) => button.textContent);

describe("DatabaseProblemScreen", () => {
  it("newer database: asks for an update, offers nothing that could harm it", () => {
    renderScreen(problem("newer"));
    expect(screen.getByRole("heading").textContent).toBe(
      "ChatonLM needs an update",
    );
    expect(screen.getByText(PATH)).toBeTruthy();
    expect(buttons()).toEqual([]);
  });

  it("newer database with a backup: offers to restore it, and says where the rest stays", async () => {
    const { onRestore, onRecover, onRecovered } = renderScreen({
      ...problem("newer"),
      backup: { version: 36, savedAt: SAVED_AT },
    });
    const label = `Restore the backup from ${formatDateTime("en", SAVED_AT)}`;
    expect(label).toContain("2026");
    expect(buttons()).toEqual([label]);
    expect(
      screen.getByText(
        "Your current database is kept next to it, under another name: chats created since this backup stay in that file.",
      ),
    ).toBeTruthy();
    await userEvent.click(screen.getByText(label));
    expect(onRestore).toHaveBeenCalledOnce();
    expect(onRecover).not.toHaveBeenCalled();
    expect(onRecovered).toHaveBeenCalledOnce();
  });

  it("restoring a backup: shows it working, then what remains", async () => {
    const { onRestore, onRecovered } = renderScreen({
      ...problem("newer"),
      backup: { version: 36, savedAt: SAVED_AT },
    });
    let finish: (remaining: DatabaseProblem | null) => void = () => {};
    onRestore.mockReturnValueOnce(
      new Promise((resolve) => {
        finish = resolve;
      }),
    );
    await userEvent.click(screen.getByRole("button"));
    expect(screen.getByRole("button").textContent).toBe("Restoring…");
    expect((screen.getByRole("button") as HTMLButtonElement).disabled).toBe(
      true,
    );
    finish(problem("corrupt"));
    expect(await screen.findByText("Start with a new database")).toBeTruthy();
    expect(screen.getByRole("heading").textContent).toBe(
      "ChatonLM can't read its database",
    );
    expect(onRecovered).not.toHaveBeenCalled();
  });

  it("a backup is offered only for a newer database", () => {
    renderScreen({
      ...problem("corrupt"),
      backup: { version: 36, savedAt: SAVED_AT },
    });
    expect(buttons()).toEqual(["Start with a new database", "Retry"]);
  });

  it("damaged file: start over with a new database", async () => {
    const { onRecover, onRecovered } = renderScreen(problem("corrupt"));
    expect(screen.getByRole("heading").textContent).toBe(
      "ChatonLM can't read its database",
    );
    expect(buttons()).toEqual(["Start with a new database", "Retry"]);
    await userEvent.click(screen.getByText("Start with a new database"));
    expect(onRecover).toHaveBeenCalledWith(true);
    expect(onRecovered).toHaveBeenCalledOnce();
  });

  it("locked database: retry, and show what remains", async () => {
    const { onRecover, onRecovered } = renderScreen(problem("locked"));
    expect(buttons()).toEqual(["Retry"]);
    onRecover.mockResolvedValueOnce(
      problem("unavailable", "file error: Permission denied (os error 13)"),
    );
    await userEvent.click(screen.getByText("Retry"));
    expect(onRecover).toHaveBeenCalledWith(false);
    expect(onRecovered).not.toHaveBeenCalled();
    expect(screen.getByRole("heading").textContent).toBe(
      "ChatonLM can't open its database",
    );
    expect(
      screen.getByText(
        "file error: Permission denied (os error 13). Check that this folder exists and that you can write to it, then click Retry.",
      ),
    ).toBeTruthy();
  });

  it("shows a failed recovery", async () => {
    const { onRecover } = renderScreen(problem("corrupt"));
    onRecover.mockRejectedValueOnce({
      kind: "internal",
      message: "file error: Read-only file system (os error 30)",
    });
    await userEvent.click(screen.getByText("Start with a new database"));
    expect((await screen.findByRole("alert")).textContent).toBe(
      "file error: Read-only file system (os error 30)",
    );
  });
});
