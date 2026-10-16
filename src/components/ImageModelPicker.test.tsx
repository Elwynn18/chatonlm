import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import {
  createMemoryHistory,
  createRootRoute,
  createRoute,
  createRouter,
  Outlet,
  RouterProvider,
} from "@tanstack/react-router";
import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";
import { createRef } from "react";
import { afterEach, describe, expect, it, onTestFinished, vi } from "vitest";
import type { ImageModelInfo } from "@/bindings/ImageModelInfo";
import type { ModelPickerHandle } from "@/components/ModelPicker";
import { untranslatedTexts, usePseudoLanguage } from "@/test/i18n";
import { KLEIN } from "@/test/images";
import { ImageModelPicker } from "./ImageModelPicker";

afterEach(() => {
  cleanup();
  clearMocks();
});

const KLEIN_Q4 = "leejet/FLUX.2-klein-4B-GGUF/flux-2-klein-4b-Q4_0.gguf";
const KLEIN_Q8 = "leejet/FLUX.2-klein-4B-GGUF/flux-2-klein-4b-Q8_0.gguf";
const Z_IMAGE = "leejet/Z-Image-Turbo-GGUF/z_image_turbo-Q4_K.gguf";
const LONG_NAME =
  "a-very-long-image-model-name-that-does-not-fit-in-the-button-Q8_0";

function info(
  id: string,
  name: string,
  fields: Partial<ImageModelInfo> = {},
): ImageModelInfo {
  const parts = id.split("/");
  return {
    id,
    name,
    repo: parts.slice(0, 2).join("/"),
    file: parts.slice(2).join("/"),
    revision: "r",
    family: KLEIN,
    license: "apache-2.0",
    openLicense: true,
    uncensored: false,
    recommended: false,
    installed: true,
    sizeBytes: 1,
    missingBytes: 0,
    selected: false,
    components: [],
    ...fields,
  };
}

/**
 * Faux moteur : modèles d'images (installés ou non), choix enregistré par
 * image_use_model (`refuse` : erreur renvoyée) ; `held` : image_models ne
 * répond qu'une fois la promesse tenue.
 */
function setup({
  models = [] as ImageModelInfo[],
  refuse = null as string | null,
  held = null as Promise<void> | null,
} = {}) {
  const state = { models: models.map((m) => ({ ...m })) };
  const commands: string[] = [];
  const uses: string[] = [];
  mockIPC((command, args) => {
    commands.push(command);
    switch (command) {
      case "image_models": {
        const list = () => state.models.map((m) => ({ ...m }));
        return held ? held.then(list) : list();
      }
      case "image_use_model": {
        const { id } = args as { id: string };
        uses.push(id);
        if (refuse !== null) throw { kind: "invalid", message: refuse };
        state.models = state.models.map((m) => ({
          ...m,
          selected: m.id === id,
        }));
        return null;
      }
    }
    throw new Error(`unexpected command ${command}`);
  });
  const onModelSelect = vi.fn();
  const onError = vi.fn();
  const handle = createRef<ModelPickerHandle>();
  const client = new QueryClient({
    defaultOptions: { queries: { networkMode: "always", retry: false } },
  });
  const rootRoute = createRootRoute({ component: () => <Outlet /> });
  const routeTree = rootRoute.addChildren([
    createRoute({
      getParentRoute: () => rootRoute,
      path: "/",
      component: () => (
        <>
          <p>Outside</p>
          <ImageModelPicker
            ref={handle}
            onModelSelect={onModelSelect}
            onError={onError}
          />
        </>
      ),
    }),
    createRoute({
      getParentRoute: () => rootRoute,
      path: "/hub/images",
      component: () => <div data-testid="hub-images" />,
    }),
  ]);
  const router = createRouter({
    routeTree,
    history: createMemoryHistory({ initialEntries: ["/"] }),
  });
  // jsdom n'a pas window.scrollTo, que le routeur appelle à chaque navigation.
  const scrollTo = vi.spyOn(window, "scrollTo").mockImplementation(() => {});
  onTestFinished(() => scrollTo.mockRestore());
  render(
    <QueryClientProvider client={client}>
      <RouterProvider router={router} />
    </QueryClientProvider>,
  );
  return { commands, uses, onModelSelect, onError, handle, router };
}

const button = () => screen.findByTitle("Select image model");
const list = () => screen.getByRole("listbox", { name: "Image models" });
const options = () => within(list()).getAllByRole("option");
const texts = () => options().map((o) => o.textContent);

/** Ouvre le menu une fois le modèle choisi affiché. */
async function open(expected: string) {
  const picker = await button();
  await waitFor(() => expect(picker.textContent).toBe(expected));
  await userEvent.click(picker);
  return picker;
}

const INSTALLED = [
  info(KLEIN_Q4, "FLUX.2 klein 4B", { selected: true, recommended: true }),
  info(KLEIN_Q8, "flux-2-klein-4b-Q8_0"),
  // En cours de téléchargement, et recommandation jamais téléchargée.
  info(Z_IMAGE, "Z-Image-Turbo", { installed: false, recommended: true }),
  info("someone/repo/other.gguf", "other", { installed: false }),
];

describe("ImageModelPicker", () => {
  it("shows the chosen image model, then lists the installed ones only, with a check", async () => {
    setup({ models: INSTALLED });
    const picker = await button();
    expect(picker.textContent).toBe("Loading...");
    await open("FLUX.2 klein 4B");
    expect(picker.getAttribute("aria-expanded")).toBe("true");
    expect(texts()).toEqual([
      "FLUX.2 klein 4B",
      "flux-2-klein-4b-Q8_0",
      "More image models",
    ]);
    const [chosen, other, more] = options();
    expect(chosen.getAttribute("aria-selected")).toBe("true");
    expect(chosen.querySelector("svg")).not.toBeNull();
    expect(other.getAttribute("aria-selected")).toBe("false");
    expect(other.querySelector("svg")).toBeNull();
    // Dernière ligne séparée des modèles.
    expect(more.className).toContain("border-t");
    // Le focus va à la liste (pas de recherche).
    expect(document.activeElement).toBe(list());
  });

  it("without a chosen model, the button asks to choose one", async () => {
    setup({
      models: [
        info(KLEIN_Q8, "flux-2-klein-4b-Q8_0"),
        // Choisi mais pas installé (téléchargement) : pas encore utilisable.
        info(Z_IMAGE, "Z-Image-Turbo", { selected: true, installed: false }),
      ],
    });
    await open("Choose an image model");
    expect(texts()).toEqual(["flux-2-klein-4b-Q8_0", "More image models"]);
    expect(
      options().some((o) => o.getAttribute("aria-selected") === "true"),
    ).toBe(false);
  });

  it("choosing a model does what Use does, then reads the image models again", async () => {
    const { uses, commands, onModelSelect, onError } = setup({
      models: INSTALLED,
    });
    const picker = await open("FLUX.2 klein 4B");
    const reads = commands.filter((c) => c === "image_models").length;
    await userEvent.click(screen.getByRole("option", { name: /Q8_0/ }));
    expect(screen.queryByRole("listbox")).toBeNull();
    expect(onModelSelect).toHaveBeenCalledTimes(1);
    await waitFor(() => expect(uses).toEqual([KLEIN_Q8]));
    await waitFor(() =>
      expect(commands.filter((c) => c === "image_models").length).toBe(
        reads + 1,
      ),
    );
    await waitFor(() =>
      expect(picker.textContent).toBe("flux-2-klein-4b-Q8_0"),
    );
    expect(onError).toHaveBeenLastCalledWith(null);
  });

  it("choosing the chosen model only closes the menu", async () => {
    const { uses, onModelSelect } = setup({ models: INSTALLED });
    await open("FLUX.2 klein 4B");
    await userEvent.click(
      screen.getByRole("option", { name: "FLUX.2 klein 4B" }),
    );
    expect(screen.queryByRole("listbox")).toBeNull();
    expect(onModelSelect).toHaveBeenCalledTimes(1);
    expect(uses).toEqual([]);
  });

  it("a refusal of the backend is given to the composer", async () => {
    const { onError } = setup({
      models: INSTALLED,
      refuse: "This model isn't installed",
    });
    const picker = await open("FLUX.2 klein 4B");
    await userEvent.click(screen.getByRole("option", { name: /Q8_0/ }));
    await waitFor(() =>
      expect(onError).toHaveBeenCalledWith("This model isn't installed"),
    );
    expect(picker.textContent).toBe("FLUX.2 klein 4B");
  });

  it("More image models opens the Images tab of the ModelHub", async () => {
    const { router } = setup({ models: INSTALLED });
    await open("FLUX.2 klein 4B");
    await userEvent.click(
      screen.getByRole("option", { name: "More image models" }),
    );
    await screen.findByTestId("hub-images");
    expect(router.state.location.pathname).toBe("/hub/images");
  });

  it("without an installed model: a short notice, and the way to the ModelHub", async () => {
    setup({ models: [INSTALLED[2]] });
    await open("Choose an image model");
    expect(list().textContent).toContain("No image models installed");
    expect(texts()).toEqual(["More image models"]);
  });

  it("keyboard: arrows go round all the rows, Enter picks, Enter on the last row opens the hub", async () => {
    const { uses } = setup({ models: INSTALLED });
    await open("FLUX.2 klein 4B");
    const highlighted = () =>
      options().findIndex(
        (o) => o.id === list().getAttribute("aria-activedescendant"),
      );
    await userEvent.keyboard("{ArrowDown}");
    expect(highlighted()).toBe(0);
    await userEvent.keyboard("{ArrowUp}");
    expect(highlighted()).toBe(2);
    await userEvent.keyboard("{ArrowDown}");
    expect(highlighted()).toBe(0);
    await userEvent.keyboard("{ArrowDown}{Enter}");
    await waitFor(() => expect(uses).toEqual([KLEIN_Q8]));
    expect(screen.queryByRole("listbox")).toBeNull();
    // Le bouton rouvre la liste (Entrée), la dernière ligne mène au Hub.
    const picker = await button();
    await waitFor(() =>
      expect(picker.textContent).toBe("flux-2-klein-4b-Q8_0"),
    );
    picker.focus();
    await userEvent.keyboard("{Enter}");
    expect(list()).toBeTruthy();
    await userEvent.keyboard("{ArrowUp}{Enter}");
    await screen.findByTestId("hub-images");
  });

  it("Escape closes the menu, is consumed, and gives the focus back to the button", async () => {
    setup({ models: INSTALLED });
    const picker = await open("FLUX.2 klein 4B");
    const seen: boolean[] = [];
    const listener = (e: KeyboardEvent) => seen.push(e.defaultPrevented);
    window.addEventListener("keydown", listener);
    onTestFinished(() => window.removeEventListener("keydown", listener));
    await userEvent.keyboard("{Escape}");
    expect(seen).toEqual([true]);
    expect(screen.queryByRole("listbox")).toBeNull();
    expect(document.activeElement).toBe(picker);
    // Menu fermé : Échap n'est plus consommée.
    await userEvent.keyboard("{Escape}");
    expect(seen).toEqual([true, false]);
  });

  it("closes on a click outside, and by its handle", async () => {
    const { handle } = setup({ models: INSTALLED });
    const picker = await open("FLUX.2 klein 4B");
    fireEvent.mouseDown(screen.getByText("Outside"));
    expect(screen.queryByRole("listbox")).toBeNull();
    await userEvent.click(picker);
    expect(list()).toBeTruthy();
    act(() => handle.current?.close());
    expect(screen.queryByRole("listbox")).toBeNull();
    // Le bouton lui-même ferme le menu ouvert.
    await userEvent.click(picker);
    await userEvent.click(picker);
    expect(screen.queryByRole("listbox")).toBeNull();
  });

  it("cuts a long name, kept whole as a tooltip, in a button of limited width", async () => {
    setup({ models: [info(KLEIN_Q8, LONG_NAME, { selected: true })] });
    const picker = await open(LONG_NAME);
    expect(picker.className).toContain("max-w-64");
    expect(picker.querySelector("span")?.className).toContain("truncate");
    const [option] = options();
    expect(option.getAttribute("title")).toBe(LONG_NAME);
    expect(option.querySelector("span")?.className).toContain("truncate");
  });

  it("waits for the image models before showing a name", async () => {
    let read = () => {};
    const held = new Promise<void>((resolve) => (read = resolve));
    setup({ models: INSTALLED, held });
    const picker = await button();
    expect(picker.textContent).toBe("Loading...");
    act(() => read());
    await waitFor(() => expect(picker.textContent).toBe("FLUX.2 klein 4B"));
  });

  it("has no untranslated text", async () => {
    usePseudoLanguage();
    setup({ models: INSTALLED });
    const picker = await screen.findByTitle("⟦Select image model⟧");
    await waitFor(() => expect(picker.textContent).toBe("FLUX.2 klein 4B"));
    await userEvent.click(picker);
    expect(
      untranslatedTexts(document.body, [
        "Outside",
        "FLUX.2 klein 4B",
        "flux-2-klein-4b-Q8_0",
      ]),
    ).toEqual([]);
    cleanup();
    setup({ models: [] });
    const empty = await screen.findByTitle("⟦Select image model⟧");
    await waitFor(() =>
      expect(empty.textContent).toBe("⟦Choose an image model⟧"),
    );
    await userEvent.click(empty);
    expect(untranslatedTexts(document.body, ["Outside"])).toEqual([]);
  });
});
