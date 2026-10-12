// Gabarit du ModelHub : la zone qui défile part du haut à chaque nouvelle
// page (carte, onglet, rubrique), Back retrouve la position quittée, même
// après un passage hors du ModelHub.

import { configure, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { renderHub } from "@/test/renderHub";

configure({ asyncUtilTimeout: 5000 });
vi.setConfig({ testTimeout: 30_000 });

const scrollArea = () => screen.getByTestId("hub-scroll");

async function home() {
  await screen.findByRole("heading", { name: "Trending" });
}

/** Ouvre la fiche de la première carte. */
async function openFirstCard(app: ReturnType<typeof renderHub>["app"]) {
  await userEvent.click(screen.getAllByTestId("model-card")[0]);
  await waitFor(() => expect(app.path()).toMatch(/^\/hub\/model\//));
  await screen.findByRole("heading", { level: 2 });
}

describe("ModelHub scrolling", () => {
  it("a card opens its page at the top; Back finds Discover where it was", async () => {
    const { app } = renderHub("/hub");
    await home();
    const scroll = scrollArea();
    scroll.scrollTop = 2000;
    await openFirstCard(app);
    // Même zone, gardée d'une page à l'autre, remise en haut.
    expect(scrollArea()).toBe(scroll);
    await waitFor(() => expect(scroll.scrollTop).toBe(0));
    scroll.scrollTop = 300;

    await userEvent.click(screen.getByRole("button", { name: "Back" }));
    await home();
    await waitFor(() => expect(scroll.scrollTop).toBe(2000));
    // Forward : la fiche là où on l'a quittée.
    app.router.history.forward();
    await screen.findByRole("heading", { level: 2 });
    await waitFor(() => expect(scroll.scrollTop).toBe(300));
  });

  it("tabs and See all start at the top", async () => {
    const { app } = renderHub("/hub");
    await home();
    scrollArea().scrollTop = 1500;
    await userEvent.click(screen.getByRole("link", { name: "Installed" }));
    await screen.findByRole("heading", { name: /models ·/ });
    await waitFor(() => expect(scrollArea().scrollTop).toBe(0));

    scrollArea().scrollTop = 400;
    await userEvent.click(screen.getByRole("link", { name: "Discover" }));
    await home();
    await waitFor(() => expect(scrollArea().scrollTop).toBe(0));

    scrollArea().scrollTop = 1200;
    await userEvent.click(
      screen.getByRole("link", { name: "See all Small and fast" }),
    );
    await screen.findByRole("heading", { name: "Small and fast", level: 3 });
    expect(app.router.state.location.search).toEqual({ section: "small" });
    await waitFor(() => expect(scrollArea().scrollTop).toBe(0));
  });

  it("Back from another page finds the ModelHub where it was", async () => {
    const { app } = renderHub("/hub");
    await home();
    scrollArea().scrollTop = 900;
    await app.router.navigate({ to: "/settings" });
    await waitFor(() => expect(screen.queryByTestId("hub-scroll")).toBeNull());
    app.router.history.back();
    await home();
    // Nouvelle zone (le gabarit a été remonté), position rendue.
    await waitFor(() => expect(scrollArea().scrollTop).toBe(900));
  });
});
