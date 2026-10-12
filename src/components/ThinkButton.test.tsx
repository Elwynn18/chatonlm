import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { type ReactNode, useState } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { Think } from "@/bindings/Think";
import { translator } from "@/i18n";
import { type ThinkControl, thinkControl } from "@/lib/think";
import { ThinkButton } from "./ThinkButton";

afterEach(() => cleanup());

const ACCENT = "bg-accent";
const GREY = "text-neutral-500";

/** Bouton d'un contrôle, dont la valeur suit les changements. */
function renderButton(
  initial: Exclude<ThinkControl, { kind: "none" }>,
  { aside = null as ReactNode, onDropdownToggle = vi.fn() } = {},
) {
  const onChange = vi.fn();
  function Harness() {
    const [control, setControl] = useState(initial);
    return (
      <>
        {aside}
        <ThinkButton
          control={control}
          onChange={(value: Think) => {
            onChange(value);
            if (control.kind === "toggle") {
              setControl({ ...control, value: value as boolean });
            } else if (control.kind === "levels") {
              setControl({ ...control, value });
            }
          }}
          onDropdownToggle={onDropdownToggle}
        />
      </>
    );
  }
  render(<Harness />);
  return { onChange, onDropdownToggle };
}

function levels(values: Think[], value: Think) {
  const control = thinkControl(
    translator("en"),
    {
      model: "m",
      capabilities: ["thinking"],
      thinking: { values, default: value },
      alwaysThinks: false,
    },
    null,
  );
  if (control.kind !== "levels") throw new Error("not levels");
  return control;
}

const GPT_OSS = levels(["low", "medium", "high"], "medium");
const WITH_OFF = levels([false, "low", "medium", "high"], "low");

const trigger = () => screen.getByRole("button", { name: /^Thinking level/ });
const items = () => screen.getAllByRole("menuitemradio");

describe("ThinkButton: toggle", () => {
  it("official tooltips, toggle button state and colors", async () => {
    const { onChange } = renderButton({ kind: "toggle", value: false });
    const button = screen.getByRole("button", { name: "Think" });
    expect(button.getAttribute("aria-pressed")).toBe("false");
    expect(button.title).toBe("Enable think mode");
    expect(button.className).toContain(GREY);

    await userEvent.click(button);
    expect(onChange).toHaveBeenLastCalledWith(true);
    expect(button.getAttribute("aria-pressed")).toBe("true");
    expect(button.title).toBe("Disable think mode");
    expect(button.className).toContain(ACCENT);

    await userEvent.click(button);
    expect(onChange).toHaveBeenLastCalledWith(false);
  });

  it("works from the keyboard and shows its focus", async () => {
    const { onChange } = renderButton({ kind: "toggle", value: true });
    const button = screen.getByRole("button", { name: "Think" });
    button.focus();
    await userEvent.keyboard("{Enter}");
    expect(onChange).toHaveBeenLastCalledWith(false);
    await userEvent.keyboard(" ");
    expect(onChange).toHaveBeenLastCalledWith(true);
    expect(button.className).toContain("focus:outline-2");
  });
});

describe("ThinkButton: fixed", () => {
  it("a model that always thinks has no button", () => {
    renderButton({ kind: "fixed" });
    expect(screen.queryByRole("button", { name: "Think" })).toBeNull();
    expect(screen.queryByRole("button")).toBeNull();
  });
});

describe("ThinkButton: levels", () => {
  it("pill with the current level, official tooltip, closed menu", () => {
    renderButton(GPT_OSS);
    const button = trigger();
    expect(button.title).toBe("Thinking level: Medium");
    expect(button.getAttribute("aria-label")).toBe("Thinking level: Medium");
    expect(button.getAttribute("aria-haspopup")).toBe("menu");
    expect(button.getAttribute("aria-expanded")).toBe("false");
    expect(button.textContent).toBe("Medium");
    expect(button.className).toContain(ACCENT);
    // Ampoule réduite à la largeur du svg officiel (w-3) : mêmes écarts.
    expect(button.querySelector("svg")?.getAttribute("class")).toContain(
      "-mx-1",
    );
    expect(screen.queryByRole("menu")).toBeNull();
  });

  it("opens upwards with the levels in order, the current one highlighted", async () => {
    const { onDropdownToggle } = renderButton(GPT_OSS);
    await userEvent.click(trigger());
    expect(onDropdownToggle).toHaveBeenLastCalledWith(true);
    const menu = screen.getByRole("menu");
    expect(menu.className).toContain("bottom-full");
    expect(menu.className).toContain("min-w-[120px]");
    expect(trigger().getAttribute("aria-expanded")).toBe("true");
    expect(trigger().getAttribute("aria-controls")).toBe(menu.id);
    expect(items().map((item) => item.textContent)).toEqual([
      "Low",
      "Medium",
      "High",
    ]);
    expect(items().map((item) => item.getAttribute("aria-checked"))).toEqual([
      "false",
      "true",
      "false",
    ]);
    expect(items()[1].className).toContain("bg-neutral-100");
    expect(items()[0].className).not.toMatch(/(^| )bg-neutral-100/);
    // Focus sur le niveau courant.
    await waitFor(() => expect(document.activeElement).toBe(items()[1]));
  });

  it("choosing a level closes the menu and gives the focus back", async () => {
    const { onChange } = renderButton(GPT_OSS);
    await userEvent.click(trigger());
    await userEvent.click(screen.getByRole("menuitemradio", { name: "High" }));
    expect(onChange).toHaveBeenCalledWith("high");
    expect(screen.queryByRole("menu")).toBeNull();
    expect(trigger().title).toBe("Thinking level: High");
    expect(document.activeElement).toBe(trigger());
  });

  it("the trigger closes the menu it opened", async () => {
    const { onDropdownToggle } = renderButton(GPT_OSS);
    await userEvent.click(trigger());
    await userEvent.click(trigger());
    expect(screen.queryByRole("menu")).toBeNull();
    expect(onDropdownToggle).toHaveBeenLastCalledWith(false);
  });

  it("Off where Ollama allows it, grey when chosen", async () => {
    const { onChange } = renderButton(WITH_OFF);
    await userEvent.click(trigger());
    expect(items().map((item) => item.textContent)).toEqual([
      "Off",
      "Low",
      "Medium",
      "High",
    ]);
    await userEvent.click(screen.getByRole("menuitemradio", { name: "Off" }));
    expect(onChange).toHaveBeenCalledWith(false);
    expect(trigger().title).toBe("Thinking level: Off");
    expect(trigger().className).toContain(GREY);
    expect(trigger().className).not.toContain(ACCENT);
  });

  it("On where Ollama allows it next to levels (Nemotron 3.5 Nano)", async () => {
    const { onChange } = renderButton(levels([false, true, "medium"], true));
    expect(trigger().title).toBe("Thinking level: On");
    expect(trigger().className).toContain(ACCENT);
    await userEvent.click(trigger());
    expect(items().map((item) => item.textContent)).toEqual([
      "Off",
      "On",
      "Medium",
    ]);
    expect(items().map((item) => item.getAttribute("aria-checked"))).toEqual([
      "false",
      "true",
      "false",
    ]);
    await userEvent.click(screen.getByRole("menuitemradio", { name: "Off" }));
    expect(onChange).toHaveBeenLastCalledWith(false);
    await userEvent.click(trigger());
    await userEvent.click(screen.getByRole("menuitemradio", { name: "On" }));
    expect(onChange).toHaveBeenLastCalledWith(true);
    expect(trigger().title).toBe("Thinking level: On");
  });

  it("keyboard: arrows open it, move around and wrap, Enter chooses", async () => {
    const { onChange } = renderButton(GPT_OSS);
    trigger().focus();
    await userEvent.keyboard("{ArrowDown}");
    await waitFor(() => expect(document.activeElement).toBe(items()[1]));
    await userEvent.keyboard("{ArrowDown}");
    expect(document.activeElement).toBe(items()[2]);
    await userEvent.keyboard("{ArrowDown}");
    expect(document.activeElement).toBe(items()[0]);
    await userEvent.keyboard("{ArrowUp}");
    expect(document.activeElement).toBe(items()[2]);
    await userEvent.keyboard("{Home}");
    expect(document.activeElement).toBe(items()[0]);
    await userEvent.keyboard("{End}");
    expect(document.activeElement).toBe(items()[2]);
    await userEvent.keyboard("{ArrowUp}{Enter}");
    expect(onChange).toHaveBeenCalledWith("medium");
    expect(screen.queryByRole("menu")).toBeNull();
    expect(document.activeElement).toBe(trigger());
  });

  it("Escape closes it, gives the focus back and is consumed", async () => {
    renderButton(GPT_OSS);
    trigger().focus();
    await userEvent.keyboard("{Enter}");
    await waitFor(() => expect(document.activeElement).toBe(items()[1]));
    // Écouteur de la zone de saisie (arrêt de la réponse), sur window.
    const seen: boolean[] = [];
    const listener = (event: KeyboardEvent) =>
      seen.push(event.defaultPrevented);
    window.addEventListener("keydown", listener);
    await userEvent.keyboard("{Escape}");
    window.removeEventListener("keydown", listener);
    expect(seen).toEqual([true]);
    expect(screen.queryByRole("menu")).toBeNull();
    expect(document.activeElement).toBe(trigger());
  });

  it("Escape is left to others when the menu is closed", () => {
    renderButton(GPT_OSS);
    trigger().focus();
    const event = new KeyboardEvent("keydown", {
      key: "Escape",
      bubbles: true,
      cancelable: true,
    });
    trigger().dispatchEvent(event);
    expect(event.defaultPrevented).toBe(false);
  });

  it("closes when the focus leaves it (Tab) or the window loses it", async () => {
    renderButton(GPT_OSS, {
      aside: <button type="button">Elsewhere</button>,
    });
    await userEvent.click(trigger());
    await waitFor(() => expect(document.activeElement).toBe(items()[1]));
    await userEvent.keyboard("{Tab}");
    expect(screen.queryByRole("menu")).toBeNull();

    await userEvent.click(trigger());
    act(() => screen.getByText("Elsewhere").focus());
    expect(screen.queryByRole("menu")).toBeNull();

    await userEvent.click(trigger());
    act(() => {
      window.dispatchEvent(new Event("blur"));
    });
    expect(screen.queryByRole("menu")).toBeNull();
  });

  it("stays open when the focus is lost to nowhere (click on WebKit)", async () => {
    renderButton(GPT_OSS);
    await userEvent.click(trigger());
    await waitFor(() => expect(document.activeElement).toBe(items()[1]));
    fireEvent.blur(items()[1], { relatedTarget: null });
    expect(screen.getByRole("menu")).toBeTruthy();
  });

  it("a click elsewhere closes it, without taking the focus back", async () => {
    renderButton(GPT_OSS, {
      aside: (
        <>
          <p>Earlier answer</p>
          <textarea aria-label="Message" />
        </>
      ),
    });
    // Sur du texte : le focus part dans le vide, seul le clic ferme le menu.
    await userEvent.click(trigger());
    await waitFor(() => expect(document.activeElement).toBe(items()[1]));
    await userEvent.click(screen.getByText("Earlier answer"));
    expect(screen.queryByRole("menu")).toBeNull();
    expect(document.activeElement).not.toBe(trigger());

    await userEvent.click(trigger());
    await userEvent.click(screen.getByLabelText("Message"));
    expect(screen.queryByRole("menu")).toBeNull();
    expect(document.activeElement).toBe(screen.getByLabelText("Message"));
  });

  it("a click on an entry does not move the focus before choosing", async () => {
    renderButton(GPT_OSS);
    await userEvent.click(trigger());
    const low = screen.getByRole("menuitemradio", { name: "Low" });
    const event = new MouseEvent("mousedown", {
      bubbles: true,
      cancelable: true,
    });
    low.dispatchEvent(event);
    expect(event.defaultPrevented).toBe(true);
  });
});
