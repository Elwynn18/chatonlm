import { cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { DatabaseProblem } from "@/bindings/DatabaseProblem";
import type { DatabaseProblemKind } from "@/bindings/DatabaseProblemKind";
import { DatabaseProblemScreen } from "./DatabaseProblemScreen";

afterEach(cleanup);

const PATH = "/home/user/.local/share/com.chatonlm.chatonlm/db/chatonlm.sqlite";

function problem(kind: DatabaseProblemKind, message = "database error") {
  return { kind, message, path: PATH } satisfies DatabaseProblem;
}

function renderScreen(value: DatabaseProblem) {
  const onRecover = vi.fn<(fresh: boolean) => Promise<DatabaseProblem | null>>(
    () => Promise.resolve(null),
  );
  const onRecovered = vi.fn();
  render(
    <DatabaseProblemScreen
      problem={value}
      onRecover={onRecover}
      onRecovered={onRecovered}
    />,
  );
  return { onRecover, onRecovered };
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
