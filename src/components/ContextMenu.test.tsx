import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { StrictMode, useEffect, useRef, useState } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { ContextMenu, type MenuItem } from "./ContextMenu";

afterEach(cleanup);

function renderMenu() {
  const onClose = vi.fn();
  const rename = vi.fn();
  const remove = vi.fn();
  render(
    <ContextMenu
      x={10}
      y={20}
      onClose={onClose}
      items={[
        { label: "Rename", onSelect: rename },
        { label: "Delete", onSelect: remove },
      ]}
    />,
  );
  return { onClose, rename, remove };
}

describe("ContextMenu", () => {
  it("focuses the first item and moves with the arrow keys", async () => {
    renderMenu();
    const [rename, remove] = screen.getAllByRole("menuitem");
    expect(document.activeElement).toBe(rename);
    await userEvent.keyboard("{ArrowDown}");
    expect(document.activeElement).toBe(remove);
    await userEvent.keyboard("{ArrowDown}");
    expect(document.activeElement).toBe(rename);
    await userEvent.keyboard("{ArrowUp}");
    expect(document.activeElement).toBe(remove);
  });

  it("closes, then runs the chosen item", async () => {
    const { onClose, remove } = renderMenu();
    await userEvent.click(screen.getByRole("menuitem", { name: "Delete" }));
    expect(onClose).toHaveBeenCalled();
    expect(remove).toHaveBeenCalledOnce();
  });

  it("closes on Escape, outside click and window blur", () => {
    const { onClose, rename, remove } = renderMenu();
    fireEvent.keyDown(document, { key: "Escape" });
    fireEvent.mouseDown(document.body);
    fireEvent.blur(window);
    expect(onClose).toHaveBeenCalledTimes(3);
    expect(rename).not.toHaveBeenCalled();
    expect(remove).not.toHaveBeenCalled();
  });

  describe("gives the focus back when it closes", () => {
    /**
     * Comme la barre latérale et les liens : une cible focalisable ouvre le
     * menu (clic droit ou touche Menu) ; un choix peut afficher un champ qui
     * prend le focus dans son propre effet (Rename).
     */
    function Harness() {
      const [open, setOpen] = useState(false);
      const [editing, setEditing] = useState(false);
      const items: MenuItem[] = [
        { label: "Rename", onSelect: () => setEditing(true) },
        { label: "Delete", onSelect: () => {} },
      ];
      return (
        <>
          <button type="button" onContextMenu={() => setOpen(true)}>
            Target
          </button>
          <button type="button">Elsewhere</button>
          {editing && <EditField />}
          {open && (
            <ContextMenu
              x={10}
              y={20}
              onClose={() => setOpen(false)}
              items={items}
            />
          )}
        </>
      );
    }

    function EditField() {
      const ref = useRef<HTMLInputElement>(null);
      useEffect(() => ref.current?.focus(), []);
      return <input ref={ref} aria-label="Title" />;
    }

    function openFrom(target: HTMLElement) {
      target.focus();
      fireEvent.contextMenu(target);
      expect(document.activeElement?.getAttribute("role")).toBe("menuitem");
    }

    it("after Escape", () => {
      render(<Harness />);
      const target = screen.getByRole("button", { name: "Target" });
      openFrom(target);
      fireEvent.keyDown(document.activeElement!, { key: "Escape" });
      expect(screen.queryByRole("menu")).toBeNull();
      expect(document.activeElement).toBe(target);
    });

    it("after a choice", async () => {
      render(<Harness />);
      const target = screen.getByRole("button", { name: "Target" });
      openFrom(target);
      await userEvent.keyboard("{ArrowDown}{Enter}");
      expect(screen.queryByRole("menu")).toBeNull();
      expect(document.activeElement).toBe(target);
    });

    it("after a click outside", () => {
      render(<Harness />);
      const target = screen.getByRole("button", { name: "Target" });
      openFrom(target);
      fireEvent.mouseDown(document.body);
      expect(screen.queryByRole("menu")).toBeNull();
      expect(document.activeElement).toBe(target);
    });

    it("but lets a choice move it (Rename's field)", async () => {
      render(<Harness />);
      openFrom(screen.getByRole("button", { name: "Target" }));
      await userEvent.click(screen.getByRole("menuitem", { name: "Rename" }));
      expect(document.activeElement).toBe(screen.getByLabelText("Title"));
    });

    it("but not when the focus was moved elsewhere meanwhile", () => {
      render(<Harness />);
      openFrom(screen.getByRole("button", { name: "Target" }));
      const elsewhere = screen.getByRole("button", { name: "Elsewhere" });
      elsewhere.focus();
      fireEvent.keyDown(document, { key: "Escape" });
      expect(screen.queryByRole("menu")).toBeNull();
      expect(document.activeElement).toBe(elsewhere);
    });

    it("after the window loses the focus", () => {
      render(<Harness />);
      const target = screen.getByRole("button", { name: "Target" });
      openFrom(target);
      fireEvent.blur(window);
      expect(screen.queryByRole("menu")).toBeNull();
      expect(document.activeElement).toBe(target);
    });

    it("under StrictMode, whose replayed effects see the menu focused", () => {
      render(
        <StrictMode>
          <Harness />
        </StrictMode>,
      );
      const target = screen.getByRole("button", { name: "Target" });
      openFrom(target);
      fireEvent.keyDown(document.activeElement!, { key: "Escape" });
      expect(document.activeElement).toBe(target);
    });

    it("and leaves it alone when nothing had it", () => {
      render(<Harness />);
      fireEvent.contextMenu(screen.getByRole("button", { name: "Target" }));
      expect(document.activeElement?.getAttribute("role")).toBe("menuitem");
      fireEvent.keyDown(document, { key: "Escape" });
      expect(document.activeElement).toBe(document.body);
    });
  });
});
