import { cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { WebSearchErrorActions } from "./WebSearchErrorActions";

afterEach(() => cleanup());

const GENERATE = "Generate without web search";

describe("WebSearchErrorActions", () => {
  it("offers Sign In and Generate without web search", async () => {
    const onSignIn = vi.fn();
    const onRetry = vi.fn();
    render(<WebSearchErrorActions onSignIn={onSignIn} onRetry={onRetry} />);
    expect(screen.queryByRole("button", { name: "Retry" })).toBeNull();
    await userEvent.click(screen.getByRole("button", { name: "Sign In" }));
    await userEvent.click(screen.getByRole("button", { name: GENERATE }));
    expect(onSignIn).toHaveBeenCalledOnce();
    expect(onRetry).toHaveBeenCalledExactlyOnceWith(false);
  });

  it("signed in meanwhile: Retry with web search replaces Sign In", async () => {
    const onSignIn = vi.fn();
    const onRetry = vi.fn();
    const { rerender } = render(
      <WebSearchErrorActions onSignIn={onSignIn} onRetry={onRetry} signedIn />,
    );
    expect(screen.queryByRole("button", { name: "Sign In" })).toBeNull();
    await userEvent.click(screen.getByRole("button", { name: "Retry" }));
    expect(onRetry).toHaveBeenCalledExactlyOnceWith(true);
    await userEvent.click(screen.getByRole("button", { name: GENERATE }));
    expect(onRetry).toHaveBeenLastCalledWith(false);
    expect(onSignIn).not.toHaveBeenCalled();

    rerender(
      <WebSearchErrorActions
        onSignIn={onSignIn}
        onRetry={onRetry}
        signedIn
        retryDisabled
      />,
    );
    expect(
      screen.getByRole<HTMLButtonElement>("button", { name: "Retry" }).disabled,
    ).toBe(true);
    // Sans message à relancer : Sign In, même relié.
    rerender(<WebSearchErrorActions onSignIn={onSignIn} signedIn />);
    expect(screen.getByRole("button", { name: "Sign In" })).toBeTruthy();
  });

  it("Generate is inactive when asked, absent without a message to send", async () => {
    const onRetry = vi.fn();
    const { rerender } = render(
      <WebSearchErrorActions
        onSignIn={() => {}}
        onRetry={onRetry}
        retryDisabled
      />,
    );
    const generate = screen.getByRole<HTMLButtonElement>("button", {
      name: GENERATE,
    });
    expect(generate.disabled).toBe(true);
    await userEvent.click(generate);
    expect(onRetry).not.toHaveBeenCalled();

    rerender(<WebSearchErrorActions onSignIn={() => {}} />);
    expect(screen.queryByRole("button", { name: GENERATE })).toBeNull();
    expect(screen.getByRole("button", { name: "Sign In" })).toBeTruthy();
  });
});
