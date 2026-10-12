import { act, cleanup, render, screen } from "@testing-library/react";
import {
  createMemoryHistory,
  createRootRoute,
  createRouter,
  RouterProvider,
} from "@tanstack/react-router";
import {
  afterEach,
  beforeEach,
  describe,
  expect,
  it,
  onTestFinished,
  vi,
} from "vitest";

const openExternal = vi.hoisted(() => vi.fn());
vi.mock("@/lib/ipc", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/lib/ipc")>()),
  openExternal,
}));

import { openLink, resetLinks } from "@/lib/openLink";
import { SidebarLayout } from "./layout";

beforeEach(() => {
  openExternal.mockReset();
  openExternal.mockRejectedValue({ kind: "system", message: "no browser" });
});

afterEach(() => {
  cleanup();
  resetLinks();
});

/** Gabarit seul, dans un routeur minimal (ses liens en ont besoin). */
async function renderLayout() {
  const router = createRouter({
    routeTree: createRootRoute({
      component: () => (
        <SidebarLayout sidebar={null}>
          <div data-testid="page" className="flex-1" />
        </SidebarLayout>
      ),
    }),
    history: createMemoryHistory({ initialEntries: ["/"] }),
  });
  // jsdom n'a pas window.scrollTo, que le routeur appelle à chaque navigation.
  const scrollTo = vi.spyOn(window, "scrollTo").mockImplementation(() => {});
  onTestFinished(() => scrollTo.mockRestore());
  render(<RouterProvider router={router} />);
  await screen.findByTestId("page");
}

describe("SidebarLayout", () => {
  it("shows a link failure at the top of the content area", async () => {
    await renderLayout();
    await act(() => openLink("https://ollama.com/download"));

    const alert = screen.getByRole("alert");
    // Dans <main>, son repère (relative) : centré sur la zone de contenu,
    // barre latérale ouverte ou non, et loin de la zone de saisie.
    const main = alert.closest("main");
    expect(main).not.toBeNull();
    expect(main!.className.split(" ")).toContain("relative");
    expect(alert.parentElement!.className.split(" ")).toEqual(
      expect.arrayContaining(["absolute", "top-14"]),
    );
  });
});
