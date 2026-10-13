import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useRef, useState } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { ConfirmDialog } from "./ConfirmDialog";

afterEach(cleanup);

describe("ConfirmDialog", () => {
  function renderDialog() {
    const onConfirm = vi.fn();
    const onCancel = vi.fn();
    render(
      <ConfirmDialog
        message="Are you sure you want to remove this chat?"
        onConfirm={onConfirm}
        onCancel={onCancel}
      />,
    );
    return { onConfirm, onCancel };
  }

  it("OK has the focus, so Enter confirms", async () => {
    const { onConfirm, onCancel } = renderDialog();
    const dialog = screen.getByRole("alertdialog");
    expect(dialog.textContent).toContain(
      "Are you sure you want to remove this chat?",
    );
    expect(document.activeElement).toBe(
      screen.getByRole("button", { name: "OK" }),
    );
    await userEvent.keyboard("{Enter}");
    expect(onConfirm).toHaveBeenCalledOnce();
    expect(onCancel).not.toHaveBeenCalled();
  });

  it("Tab stays inside the dialog", async () => {
    renderDialog();
    const cancel = screen.getByRole("button", { name: "Cancel" });
    const ok = screen.getByRole("button", { name: "OK" });
    await userEvent.tab();
    expect(document.activeElement).toBe(cancel);
    await userEvent.tab();
    expect(document.activeElement).toBe(ok);
    await userEvent.tab({ shift: true });
    expect(document.activeElement).toBe(cancel);
  });

  it("keeps Shift+Tab inside the dialog as WebKitGTK sends it", () => {
    renderDialog();
    const cancel = screen.getByRole("button", { name: "Cancel" });
    const ok = screen.getByRole("button", { name: "OK" });
    expect(document.activeElement).toBe(ok);
    // Maj+Tab de WebKitGTK : touche « Unidentified », code « Tab ».
    const leftTab = { key: "Unidentified", code: "Tab", shiftKey: true };
    // Faux si la touche a été prise (action native empêchée).
    expect(fireEvent.keyDown(ok, leftTab)).toBe(false);
    expect(document.activeElement).toBe(cancel);
    expect(fireEvent.keyDown(cancel, leftTab)).toBe(false);
    expect(document.activeElement).toBe(ok);
  });

  it("offers several choices after Cancel, the last one focused", async () => {
    const leave = vi.fn();
    const move = vi.fn();
    const onCancel = vi.fn();
    render(
      <ConfirmDialog
        title="Move models to the new location?"
        message={<p>Ollama will restart.</p>}
        onCancel={onCancel}
        actions={[
          { label: "Leave them", onSelect: leave },
          { label: "Move models", onSelect: move },
        ]}
      />,
    );
    const dialog = screen.getByRole("alertdialog", {
      name: "Move models to the new location?",
    });
    expect(dialog.getAttribute("aria-describedby")).toBeTruthy();
    expect(
      screen.getAllByRole("button").map((button) => button.textContent),
    ).toEqual(["Cancel", "Leave them", "Move models"]);
    expect(screen.queryByRole("button", { name: "OK" })).toBeNull();
    expect(document.activeElement).toBe(
      screen.getByRole("button", { name: "Move models" }),
    );
    await userEvent.keyboard("{Enter}");
    expect(move).toHaveBeenCalledOnce();
    await userEvent.click(screen.getByRole("button", { name: "Leave them" }));
    expect(leave).toHaveBeenCalledOnce();
    expect(onCancel).not.toHaveBeenCalled();
  });

  it("Cancel, Escape and a click beside it cancel", async () => {
    const { onConfirm, onCancel } = renderDialog();
    await userEvent.click(screen.getByRole("button", { name: "Cancel" }));
    fireEvent.keyDown(document, { key: "Escape" });
    const backdrop = screen.getByRole("alertdialog").parentElement!;
    fireEvent.mouseDown(backdrop);
    fireEvent.mouseDown(screen.getByRole("alertdialog"));
    expect(onCancel).toHaveBeenCalledTimes(3);
    expect(onConfirm).not.toHaveBeenCalled();
  });

  it("can have a title and its own confirm label", async () => {
    const onConfirm = vi.fn();
    render(
      <ConfirmDialog
        title="Delete qwen3:0.6b?"
        message="This removes the model from Ollama. You can download it again later."
        confirmLabel="Delete"
        onConfirm={onConfirm}
        onCancel={() => {}}
      />,
    );
    const dialog = screen.getByRole("alertdialog", {
      name: "Delete qwen3:0.6b?",
    });
    expect(dialog.getAttribute("aria-describedby")).toBeTruthy();
    expect(document.activeElement).toBe(
      screen.getByRole("button", { name: "Delete" }),
    );
    expect(screen.queryByRole("button", { name: "OK" })).toBeNull();
    await userEvent.keyboard("{Enter}");
    expect(onConfirm).toHaveBeenCalledOnce();
  });

  /** Bouton qui ouvre la boîte ; OK met le focus ailleurs si `elsewhere`. */
  function Opener({ elsewhere = false }: { elsewhere?: boolean }) {
    const [open, setOpen] = useState(false);
    const other = useRef<HTMLInputElement>(null);
    return (
      <>
        <button type="button" onClick={() => setOpen(true)}>
          Delete chat
        </button>
        <input ref={other} aria-label="Elsewhere" />
        {open && (
          <ConfirmDialog
            message="Sure?"
            onConfirm={() => {
              if (elsewhere) other.current?.focus();
              setOpen(false);
            }}
            onCancel={() => setOpen(false)}
          />
        )}
      </>
    );
  }

  it("gives the focus back to the button that opened it", async () => {
    render(<Opener />);
    const opener = screen.getByRole("button", { name: "Delete chat" });
    act(() => opener.focus());
    await userEvent.keyboard("{Enter}");
    expect(document.activeElement).toBe(
      screen.getByRole("button", { name: "OK" }),
    );
    // Échap.
    await userEvent.keyboard("{Escape}");
    expect(screen.queryByRole("alertdialog")).toBeNull();
    expect(document.activeElement).toBe(opener);
    // Cancel.
    await userEvent.keyboard("{Enter}");
    await userEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(document.activeElement).toBe(opener);
    // OK.
    await userEvent.keyboard("{Enter}");
    await userEvent.keyboard("{Enter}");
    expect(screen.queryByRole("alertdialog")).toBeNull();
    expect(document.activeElement).toBe(opener);
  });

  it("leaves the focus where the action put it", async () => {
    render(<Opener elsewhere />);
    const opener = screen.getByRole("button", { name: "Delete chat" });
    act(() => opener.focus());
    await userEvent.keyboard("{Enter}");
    await userEvent.keyboard("{Enter}");
    expect(screen.queryByRole("alertdialog")).toBeNull();
    expect(document.activeElement).toBe(screen.getByLabelText("Elsewhere"));
  });
});
