// Onglet Images du ModelHub face au simulateur (faux moteur de
// src/test/images.ts) : onglet et badge, recommandations (taille à
// télécharger, licence, source), téléchargement (révision passée,
// progression, fin, Use, In use), suppression confirmée (focus), échec (Try
// again avec la révision du modèle, Dismiss), machine (moteur, processeur,
// carte graphique), recherche (q dans l'adresse après le délai, populaires,
// non censurés retirés ou marqués, Hugging Face injoignable), section de
// l'onglet Downloads ; textes en pseudo-langue et en français.

import {
  act,
  configure,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import type { SpeechDownload } from "@/bindings/SpeechDownload";
import { setLanguage } from "@/i18n";
import { initials } from "@/lib/hub";
import {
  imageDeleteModel,
  imageDownload,
  listenImageDownloads,
} from "@/lib/ipc";
import {
  GATED_REPO,
  KLEIN_FILE,
  KLEIN_ID,
  KLEIN_Q8_ID,
  KLEIN_REPO,
  KLEIN_REVISION,
  UNCENSORED_ID,
  UNCENSORED_REPO,
  UNKNOWN_REPO,
  Z_IMAGE_ID,
} from "@/test/images";

/** Encodeur de texte de klein 4B (pièce partagée de la famille). */
const KLEIN_ENCODER = "unsloth/Qwen3-4B-GGUF/Qwen3-4B-Q4_K_M.gguf";
import { untranslatedTexts, usePseudoLanguage } from "@/test/i18n";
import { renderHub } from "@/test/renderHub";

configure({ asyncUtilTimeout: 5000 });
vi.setConfig({ testTimeout: 30_000 });

const MIB = 1024 ** 2;

const card = (name: string) =>
  screen.getByRole("listitem", { name }) as HTMLElement;
const section = (name: string) => screen.queryByRole("list", { name });
const titlesIn = (name: string) =>
  within(section(name)!)
    .getAllByTestId("image-model")
    .map((element) => within(element).getByRole("heading").textContent);
const repoCards = () =>
  screen
    .getAllByTestId("image-repo-card")
    .map((element) =>
      decodeURIComponent(element.getAttribute("href") ?? "").replace(
        "/hub/images/",
        "",
      ),
    );

function download(id: string, fields: Partial<SpeechDownload>): SpeechDownload {
  return {
    id,
    name: id.split("/").at(-1)!,
    state: "downloading",
    completedBytes: 0,
    totalBytes: 400 * MIB,
    error: null,
    ...fields,
  };
}

describe("Images tab", () => {
  it("comes after Voices; the recommendations say what is left to download", async () => {
    renderHub("/hub/images");
    await screen.findAllByTestId("image-model");
    const tabs = within(screen.getByTestId("hub-tabs-browse")).getAllByRole(
      "link",
    );
    expect(tabs.map((tab) => tab.textContent)).toEqual([
      "Discover",
      "Transcription",
      "Polish",
      "Voices",
      "Images",
    ]);
    expect(
      screen.getByRole("link", { name: "Images" }).getAttribute("aria-current"),
    ).toBe("page");
    expect(
      screen.getByText(
        "Image models create pictures from your descriptions in a chat. They run inside ChatonLM, not in Ollama.",
      ),
    ).toBeTruthy();
    expect(section("Your image models")).toBeNull();
    expect(titlesIn("Recommended")).toEqual([
      "FLUX.2 klein 4B",
      "Z-Image-Turbo",
    ]);
    const klein = card("FLUX.2 klein 4B");
    // Modèle, encodeur et décodeur : rien n'est encore sur le disque.
    expect(klein.textContent).toContain("4.8 GB to download");
    expect(klein.textContent).toContain("License: apache-2.0");
    expect(
      within(klein)
        .getByRole("link", { name: KLEIN_REPO })
        .getAttribute("href"),
    ).toBe(`https://huggingface.co/${KLEIN_REPO}`);
    expect(
      within(klein)
        .getAllByRole("button")
        .map((button) => button.textContent),
    ).toEqual(["Download"]);
    // Aucun avertissement pour une machine avec carte graphique.
    expect(screen.queryByTestId("image-machine")).toBeNull();
  });

  it("Download passes the pinned revision; progress, then Use and In use", async () => {
    const { simulator, backend } = renderHub("/hub/images");
    await screen.findAllByTestId("image-model");
    await userEvent.click(
      within(card("FLUX.2 klein 4B")).getByRole("button", { name: "Download" }),
    );
    expect(simulator.callsTo("image_download")).toEqual([
      { repo: KLEIN_REPO, file: KLEIN_FILE, revision: KLEIN_REVISION },
    ]);
    // En cours : rangé dans « Your image models ».
    await waitFor(() =>
      expect(titlesIn("Your image models")).toEqual(["FLUX.2 klein 4B"]),
    );
    await within(card("FLUX.2 klein 4B")).findByText("Waiting…");
    expect(screen.getByTestId("active-downloads").textContent).toBe("1");

    await backend.images.progress(KLEIN_ID, 200 * MIB, 400 * MIB);
    const bar = await within(card("FLUX.2 klein 4B")).findByRole(
      "progressbar",
      { name: "Downloading FLUX.2 klein 4B" },
    );
    await waitFor(() => expect(bar.getAttribute("aria-valuenow")).toBe("50"));

    await backend.images.finish(KLEIN_ID);
    const use = await within(card("FLUX.2 klein 4B")).findByRole("button", {
      name: "Use",
    });
    expect(screen.queryByTestId("active-downloads")).toBeNull();
    expect(titlesIn("Recommended")).toEqual(["Z-Image-Turbo"]);
    await userEvent.click(use);
    expect(simulator.callsTo("image_use_model")).toEqual([{ id: KLEIN_ID }]);
    await within(card("FLUX.2 klein 4B")).findByText("In use");
    expect(
      within(card("FLUX.2 klein 4B")).queryByRole("button", { name: "Use" }),
    ).toBeNull();
  });

  it("an installed model of the same family makes its pieces count as present", async () => {
    renderHub("/hub/images", ({ backend }) => {
      backend.images.install(KLEIN_Q8_ID);
    });
    await screen.findAllByTestId("image-model");
    expect(titlesIn("Your image models")).toEqual(["flux-2-klein-4b-Q8_0"]);
    // Plus que le fichier du modèle (2,3 Go) : encodeur et décodeur partagés.
    expect(card("FLUX.2 klein 4B").textContent).toContain("2.3 GB to download");
    const installed = card("flux-2-klein-4b-Q8_0");
    expect(installed.textContent).toContain("FLUX.2 klein 4B");
    expect(installed.textContent).toContain("6.5 GB");
  });

  it("Delete asks first (in use: generation stops), then the focus goes to the card title", async () => {
    const { simulator } = renderHub("/hub/images", ({ backend }) => {
      backend.images.install(KLEIN_Q8_ID);
      backend.images.select(KLEIN_Q8_ID);
    });
    await screen.findAllByTestId("image-model");
    const installed = card("flux-2-klein-4b-Q8_0");
    expect(within(installed).getByText("In use")).toBeTruthy();
    await userEvent.click(
      within(installed).getByRole("button", {
        name: "Delete flux-2-klein-4b-Q8_0",
      }),
    );
    const dialog = await screen.findByRole("alertdialog", {
      name: "Delete flux-2-klein-4b-Q8_0?",
    });
    expect(dialog.textContent).toContain(
      "Image generation won't work until you download it again or choose another image model.",
    );
    await userEvent.click(
      within(dialog).getByRole("button", { name: "Cancel" }),
    );
    expect(simulator.callsTo("image_delete_model")).toEqual([]);

    await userEvent.click(
      within(installed).getByRole("button", {
        name: "Delete flux-2-klein-4b-Q8_0",
      }),
    );
    await userEvent.click(
      within(await screen.findByRole("alertdialog")).getByRole("button", {
        name: "Delete",
      }),
    );
    expect(simulator.callsTo("image_delete_model")).toEqual([
      { id: KLEIN_Q8_ID },
    ]);
    expect(document.activeElement).toBe(
      within(installed).getByRole("heading", { name: "flux-2-klein-4b-Q8_0" }),
    );
    // Pas une recommandation : retiré de la liste.
    await waitFor(() => expect(section("Your image models")).toBeNull());
  });

  it("a model that is not in use: the plain delete text", async () => {
    renderHub("/hub/images", ({ backend }) => {
      backend.images.install(KLEIN_ID);
    });
    await screen.findAllByTestId("image-model");
    await userEvent.click(
      within(card("FLUX.2 klein 4B")).getByRole("button", {
        name: "Delete FLUX.2 klein 4B",
      }),
    );
    expect((await screen.findByRole("alertdialog")).textContent).toContain(
      "This removes the model from this computer. Files it shares with your other image models are kept.",
    );
  });

  it("a failed download says why; Try again passes the model's revision; Dismiss", async () => {
    const { simulator, backend } = renderHub("/hub/images");
    await screen.findAllByTestId("image-model");
    await userEvent.click(
      within(card("FLUX.2 klein 4B")).getByRole("button", { name: "Download" }),
    );
    await within(card("FLUX.2 klein 4B")).findByText("Waiting…");
    await backend.images.fail(KLEIN_ID, "The file did not match its checksum.");
    expect(
      (await within(card("FLUX.2 klein 4B")).findByRole("alert")).textContent,
    ).toBe("The file did not match its checksum.");
    await userEvent.click(
      within(card("FLUX.2 klein 4B")).getByRole("button", {
        name: "Try again",
      }),
    );
    await waitFor(() =>
      expect(simulator.callsTo("image_download")).toEqual([
        { repo: KLEIN_REPO, file: KLEIN_FILE, revision: KLEIN_REVISION },
        { repo: KLEIN_REPO, file: KLEIN_FILE, revision: KLEIN_REVISION },
      ]),
    );
    await within(card("FLUX.2 klein 4B")).findByText("Waiting…");
    await backend.images.fail(KLEIN_ID, "Network error");
    await userEvent.click(
      await within(card("FLUX.2 klein 4B")).findByRole("button", {
        name: "Dismiss",
      }),
    );
    expect(simulator.callsTo("image_dismiss_download")).toEqual([
      { id: KLEIN_ID },
    ]);
    // Recommandation sans téléchargement : de retour avec Download.
    await waitFor(() => expect(section("Your image models")).toBeNull());
    expect(
      within(card("FLUX.2 klein 4B")).getByRole("button", { name: "Download" }),
    ).toBeTruthy();
  });

  it("a refused download shows the engine's reason on the card", async () => {
    const { backend } = renderHub("/hub/images");
    backend.images.machine.freeBytes = 1024 ** 3;
    await screen.findAllByTestId("image-model");
    await userEvent.click(
      within(card("Z-Image-Turbo")).getByRole("button", { name: "Download" }),
    );
    expect(
      (await within(card("Z-Image-Turbo")).findByRole("alert")).textContent,
    ).toMatch(/^Not enough disk space/);
  });

  it("a saved model never installed, not recommended: Dismiss forgets it, even when the processor blocks", async () => {
    const { simulator } = renderHub("/hub/images", ({ backend }) => {
      // Ligne d'avant un redémarrage : ni fichier, ni téléchargement.
      backend.images.record(KLEIN_Q8_ID);
      backend.images.machine.missingCpuFeatures = ["AVX2"];
    });
    await screen.findAllByTestId("image-model");
    const saved = card("flux-2-klein-4b-Q8_0");
    expect(
      within(saved)
        .getAllByRole("button")
        .map((button) => button.textContent),
    ).toEqual(["Dismiss"]);
    await userEvent.click(
      within(saved).getByRole("button", { name: "Dismiss" }),
    );
    expect(simulator.callsTo("image_dismiss_download")).toEqual([
      { id: KLEIN_Q8_ID },
    ]);
    await waitFor(() => expect(section("Your image models")).toBeNull());
  });

  it("a saved model never installed, on a machine that can: Download and Dismiss", async () => {
    renderHub("/hub/images", ({ backend }) => {
      backend.images.record(KLEIN_Q8_ID);
    });
    await screen.findAllByTestId("image-model");
    expect(
      within(card("flux-2-klein-4b-Q8_0"))
        .getAllByRole("button")
        .map((button) => button.textContent),
    ).toEqual(["Download", "Dismiss"]);
  });

  it("the error of an earlier action goes once another one succeeds", async () => {
    const { backend } = renderHub("/hub/images");
    await screen.findAllByTestId("image-model");
    await userEvent.click(
      within(card("FLUX.2 klein 4B")).getByRole("button", { name: "Download" }),
    );
    await within(card("FLUX.2 klein 4B")).findByText("Waiting…");
    backend.images.failures.set("image_cancel_download", {
      kind: "internal",
      message: "queue is busy",
    });
    await userEvent.click(
      within(card("FLUX.2 klein 4B")).getByRole("button", { name: "Cancel" }),
    );
    await within(card("FLUX.2 klein 4B")).findByText("queue is busy");
    backend.images.failures.delete("image_cancel_download");
    await backend.images.fail(KLEIN_ID, "Network error");
    await userEvent.click(
      await within(card("FLUX.2 klein 4B")).findByRole("button", {
        name: "Try again",
      }),
    );
    await within(card("FLUX.2 klein 4B")).findByText("Waiting…");
    expect(
      within(card("FLUX.2 klein 4B")).queryByText("queue is busy"),
    ).toBeNull();
  });

  it("pieces finished before a failure stay on the disk: less left to download", async () => {
    const { backend } = renderHub("/hub/images");
    await screen.findAllByTestId("image-model");
    await userEvent.click(
      within(card("FLUX.2 klein 4B")).getByRole("button", { name: "Download" }),
    );
    await within(card("FLUX.2 klein 4B")).findByText("Waiting…");
    await backend.images.fail(KLEIN_ID, "Network error", [KLEIN_ENCODER]);
    // Modèle (2,3 Go) et décodeur (160 Mo) : l'encodeur est déjà là.
    await waitFor(() =>
      expect(card("FLUX.2 klein 4B").textContent).toContain(
        "2.4 GB to download",
      ),
    );
  });

  it("a recommendation saved without its files comes before those never saved", async () => {
    renderHub("/hub/images", ({ backend }) => {
      backend.images.record(Z_IMAGE_ID);
    });
    await screen.findAllByTestId("image-model");
    expect(titlesIn("Recommended")).toEqual([
      "Z-Image-Turbo",
      "FLUX.2 klein 4B",
    ]);
  });

  it("Delete also removes the model's finished download", async () => {
    const { app } = renderHub("/hub/images", ({ backend }) => {
      backend.images.install(KLEIN_Q8_ID);
      backend.images.downloads.push(
        download(KLEIN_Q8_ID, { state: "done", completedBytes: 400 * MIB }),
      );
    });
    await screen.findAllByTestId("image-model");
    await userEvent.click(
      within(card("flux-2-klein-4b-Q8_0")).getByRole("button", {
        name: "Delete flux-2-klein-4b-Q8_0",
      }),
    );
    await userEvent.click(
      within(await screen.findByRole("alertdialog")).getByRole("button", {
        name: "Delete",
      }),
    );
    await waitFor(() => expect(section("Your image models")).toBeNull());
    await act(() => app.router.navigate({ to: "/hub/downloads" }));
    expect(await screen.findByText("No downloads yet.")).toBeTruthy();
  });
});

describe("Images: what the machine allows", () => {
  it("no graphics card: images by the processor, slow", async () => {
    renderHub("/hub/images", ({ backend }) => {
      backend.images.machine.gpu = false;
    });
    const notice = await screen.findByTestId("image-machine");
    expect(notice.textContent).toBe(
      "No graphics card found: images will be made by the processor, which takes several minutes each.",
    );
    expect(notice.querySelector("[data-note=gpu]")!.className).toContain(
      "amber",
    );
    // Permis : Download reste là.
    await screen.findAllByTestId("image-model");
    expect(
      within(card("FLUX.2 klein 4B")).getByRole("button", { name: "Download" }),
    ).toBeTruthy();
  });

  it("a processor without AVX2: generation impossible, in red, no Download nor Use", async () => {
    renderHub("/hub/images", ({ backend }) => {
      backend.images.machine.gpu = false;
      backend.images.machine.missingCpuFeatures = ["AVX2", "FMA"];
      backend.images.install(KLEIN_ID);
    });
    const notice = await screen.findByTestId("image-machine");
    expect(notice.textContent).toBe(
      "This computer can't create images: its processor lacks instructions the image engine needs (AVX2, FMA).",
    );
    expect(notice.querySelector("[data-note=cpu]")!.className).toContain(
      "red-600",
    );
    await screen.findAllByTestId("image-model");
    expect(
      within(card("Z-Image-Turbo")).queryByRole("button", { name: "Download" }),
    ).toBeNull();
    expect(
      within(card("FLUX.2 klein 4B")).queryByRole("button", { name: "Use" }),
    ).toBeNull();
    // Suppression gardée.
    expect(
      within(card("FLUX.2 klein 4B")).getByRole("button", {
        name: "Delete FLUX.2 klein 4B",
      }),
    ).toBeTruthy();
  });

  it("no image engine in this version", async () => {
    renderHub("/hub/images", ({ backend }) => {
      backend.images.machine.engineAvailable = false;
    });
    expect((await screen.findByTestId("image-machine")).textContent).toBe(
      "This version of ChatonLM doesn't include the image engine. You can download models, but not create images.",
    );
  });
});

describe("Images: search on Hugging Face", () => {
  it("without text: the most downloaded repos, uncensored ones left out; a card opens its page", async () => {
    const { simulator, app } = renderHub("/hub/images");
    const heading = await screen.findByRole("heading", {
      name: "Popular on Hugging Face",
    });
    // Région nommée par son titre (pas un aria-label en double).
    const region = screen.getByRole("region", {
      name: "Popular on Hugging Face",
    });
    expect(region.getAttribute("aria-labelledby")).toBe(heading.id);
    expect(region.hasAttribute("aria-label")).toBe(false);
    await waitFor(() =>
      expect(repoCards()).toEqual([
        KLEIN_REPO,
        "leejet/Z-Image-Turbo-GGUF",
        GATED_REPO,
        UNKNOWN_REPO,
        "chatonlm/sdxl-single",
      ]),
    );
    expect(simulator.callsTo("image_search")).toEqual([{ query: "" }]);
    const cards = screen.getAllByTestId("image-repo-card");
    expect(cards[2].textContent).toContain("Restricted");
    expect(cards[3].textContent).toContain("Not supported");
    expect(cards[3].textContent).toContain("No license stated");
    expect(cards[0].textContent).toContain("FLUX.2 klein 4B");
    expect(cards[0].textContent).toContain("120K");
    expect(within(cards[0]).getByTitle("120,000 downloads").textContent).toBe(
      "120K",
    );
    await userEvent.click(cards[0]);
    await waitFor(() =>
      expect(app.router.state.location.pathname).toBe(
        `/hub/images/${KLEIN_REPO}`,
      ),
    );
  });

  it("typing searches after a pause, with q in the address; Escape clears it", async () => {
    const { simulator, app } = renderHub("/hub/images");
    await screen.findAllByTestId("image-repo-card");
    const field = screen.getByRole("textbox", {
      name: "Search Hugging Face models",
    });
    await userEvent.type(field, "klein");
    await screen.findByRole("heading", { name: "Results for “klein”" });
    expect(app.router.state.location.search).toEqual({ q: "klein" });
    await waitFor(() => expect(repoCards()).toEqual([KLEIN_REPO]));
    // Une seule recherche après la pause, pas une par lettre.
    expect(simulator.callsTo("image_search")).toEqual([
      { query: "" },
      { query: "klein" },
    ]);
    await userEvent.type(field, "{Escape}");
    await screen.findByRole("heading", { name: "Popular on Hugging Face" });
    expect(app.router.state.location.search).toEqual({});
  });

  it("the address gives the search; nothing found says so", async () => {
    renderHub("/hub/images?q=nothing-at-all");
    await screen.findByRole("heading", {
      name: "Results for “nothing-at-all”",
    });
    expect(
      await screen.findByText("No image models match your search."),
    ).toBeTruthy();
    expect(
      (
        screen.getByRole("textbox", {
          name: "Search Hugging Face models",
        }) as HTMLInputElement
      ).value,
    ).toBe("nothing-at-all");
  });

  it("with “Show uncensored models”: uncensored repos are listed, with a badge", async () => {
    renderHub("/hub/images?q=uncensored", ({ backend }) => {
      backend.settings.showUncensored = true;
    });
    await waitFor(() => expect(repoCards()).toEqual([UNCENSORED_REPO]));
    const badge = within(screen.getByTestId("image-repo-card")).getByText(
      "Uncensored",
    );
    expect(badge.getAttribute("title")).toBe(
      "This model can create explicit or shocking images.",
    );
  });

  it("an installed uncensored model has its badge on its card", async () => {
    renderHub("/hub/images", ({ backend }) => {
      backend.settings.showUncensored = true;
      backend.images.install(UNCENSORED_ID);
    });
    await screen.findAllByTestId("image-model");
    expect(
      within(card("z-image-turbo-uncensored-Q4_K_M")).getByText("Uncensored"),
    ).toBeTruthy();
  });

  it("Hugging Face unreachable: full state without data, banner over cached data", async () => {
    const { backend } = renderHub("/hub/images", ({ backend }) => {
      backend.images.problem = { kind: "offline" };
    });
    expect(await screen.findByText("You're offline")).toBeTruthy();
    // Les modèles de l'app restent montrés.
    expect(screen.getAllByTestId("image-model")).toHaveLength(2);

    backend.images.stale = true;
    await userEvent.click(screen.getByRole("button", { name: "Retry" }));
    expect((await screen.findByTestId("hub-banner")).textContent).toContain(
      "You're offline. Showing models saved earlier.",
    );
    expect(screen.getAllByTestId("image-repo-card")).toHaveLength(5);
  });

  it("a failed command: its message and Retry", async () => {
    const { backend } = renderHub("/hub/images", ({ backend }) => {
      backend.images.failures.set("image_models", {
        kind: "internal",
        message: "database is locked",
      });
    });
    expect(await screen.findByText("database is locked")).toBeTruthy();
    backend.images.failures.delete("image_models");
    await userEvent.click(screen.getAllByRole("button", { name: "Retry" })[0]);
    await screen.findAllByTestId("image-model");
  });
});

describe("Images: the simulator answers like the engine", () => {
  it("the queue is emitted before image_download answers; deleting an unknown model is Ok", async () => {
    renderHub("/hub/images");
    await screen.findAllByTestId("image-model");
    const order: string[] = [];
    const stop = await listenImageDownloads(() => order.push("event"));
    await imageDownload(KLEIN_REPO, KLEIN_FILE, KLEIN_REVISION);
    order.push("answer");
    stop();
    expect(order[0]).toBe("event");
    await expect(imageDeleteModel("nobody/none/none.gguf")).resolves.toBe(null);
  });
});

describe("Image models in the Downloads tab", () => {
  it("a section of their own, linked to their repo, with the same actions", async () => {
    const { simulator } = renderHub("/hub/downloads", ({ backend }) => {
      backend.images.install(KLEIN_Q8_ID);
      backend.images.downloads.push(
        download(Z_IMAGE_ID, { completedBytes: 100 * MIB }),
        download(KLEIN_ID, { state: "failed", error: "Network error" }),
        // Fini, puis supprimé (modèles relus sans lui).
        download(UNCENSORED_ID, { state: "done", completedBytes: 400 * MIB }),
      );
    });
    const region = await screen.findByRole("region", { name: "Image models" });
    const rows = () => within(region).getAllByTestId("image-download");
    await waitFor(() =>
      expect(
        rows().map((row) => within(row).getByRole("link").textContent),
      ).toEqual([
        "Z-Image-Turbo",
        "FLUX.2 klein 4B",
        "z-image-turbo-uncensored-Q4_K_M.gguf",
      ]),
    );
    expect(within(rows()[0]).getByRole("link").getAttribute("href")).toBe(
      "/hub/images/leejet/Z-Image-Turbo-GGUF",
    );
    expect(
      within(rows()[0]).getByRole("progressbar", {
        name: "Downloading Z-Image-Turbo",
      }),
    ).toBeTruthy();
    expect(within(rows()[1]).getByRole("alert").textContent).toBe(
      "Network error",
    );
    await waitFor(() =>
      expect(rows()[2].textContent).toContain("No longer installed"),
    );
    expect(screen.getByTestId("active-downloads").textContent).toBe("1");

    const row = (name: string) =>
      rows().find(
        (element) => within(element).getByRole("link").textContent === name,
      )!;
    await userEvent.click(
      within(row("Z-Image-Turbo")).getByRole("button", { name: "Cancel" }),
    );
    expect(simulator.callsTo("image_cancel_download")).toEqual([
      { id: Z_IMAGE_ID },
    ]);
    await userEvent.click(
      within(row("FLUX.2 klein 4B")).getByRole("button", {
        name: "Try again",
      }),
    );
    await waitFor(() =>
      expect(simulator.callsTo("image_download")).toEqual([
        { repo: KLEIN_REPO, file: KLEIN_FILE, revision: KLEIN_REVISION },
      ]),
    );
    await waitFor(() =>
      expect(row("Z-Image-Turbo").textContent).toContain("Cancelled"),
    );
    await userEvent.click(
      within(row("Z-Image-Turbo")).getByRole("button", { name: "Dismiss" }),
    );
    expect(simulator.callsTo("image_dismiss_download")).toEqual([
      { id: Z_IMAGE_ID },
    ]);
  });

  it("Download again of a removed model reads the revision from its repo page", async () => {
    const { simulator } = renderHub("/hub/downloads", ({ backend }) => {
      backend.settings.showUncensored = true;
      backend.images.downloads.push(
        download(UNCENSORED_ID, { state: "done", completedBytes: 400 * MIB }),
      );
    });
    const region = await screen.findByRole("region", { name: "Image models" });
    await userEvent.click(
      await within(region).findByRole("button", { name: "Download again" }),
    );
    await waitFor(() =>
      expect(simulator.callsTo("image_download")).toEqual([
        {
          repo: UNCENSORED_REPO,
          file: "z-image-turbo-uncensored-Q4_K_M.gguf",
          revision: "a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1",
        },
      ]),
    );
    expect(simulator.callsTo("image_repo")).toEqual([
      { repo: UNCENSORED_REPO },
    ]);
  });

  it("the badge counts the image models being downloaded", async () => {
    const { backend } = renderHub("/hub/images", ({ backend }) => {
      backend.images.downloads.push(download(KLEIN_ID, { state: "verifying" }));
    });
    await waitFor(() =>
      expect(screen.getByTestId("active-downloads").textContent).toBe("1"),
    );
    await backend.images.finish(KLEIN_ID);
    await waitFor(() =>
      expect(screen.queryByTestId("active-downloads")).toBeNull(),
    );
  });
});

describe("Images in other languages", () => {
  /** Données des dépôts (noms, auteurs, licences, initiales). */
  const DATA = [
    "FLUX.2 klein 4B",
    "Z-Image-Turbo",
    "FLUX.1 dev",
    "Stable Diffusion XL",
    "flux-2-klein-4b-Q8_0",
    "FLUX.2-klein-4B-GGUF",
    "Z-Image-Turbo-GGUF",
    "FLUX.1-dev-GGUF",
    "mystery-diffusion-GGUF",
    "sdxl-single",
    "leejet",
    "someone",
    "black-forest-labs",
    "chatonlm",
    "apache-2.0",
    "openrail++",
    "other",
    "AVX2",
    "Network error",
    KLEIN_REPO,
    "F2",
    "ZI",
    "F1",
    "MD",
    "SS",
    initials("FLUX.2 klein 4B"),
    initials("flux-2-klein-4b-Q8_0"),
  ];

  it("pseudo-language: Images tab and Downloads, no text written in the code", async () => {
    usePseudoLanguage();
    const { app } = renderHub("/hub/images", ({ backend }) => {
      backend.images.machine.missingCpuFeatures = ["AVX2"];
      backend.images.machine.engineAvailable = false;
      backend.images.install(KLEIN_Q8_ID);
      backend.images.select(KLEIN_Q8_ID);
      backend.images.downloads.push(
        download(Z_IMAGE_ID, { completedBytes: 10 * MIB }),
        download(KLEIN_ID, { state: "failed", error: "Network error" }),
      );
    });
    await screen.findAllByTestId("image-model");
    await screen.findAllByTestId("image-repo-card");
    const root = () => screen.getByTestId("hub-scroll").parentElement!;
    const links = () =>
      [...root().querySelectorAll("a")].map(
        (link) => link.getAttribute("href") ?? "",
      );
    expect(untranslatedTexts(root(), [...DATA, ...links()])).toEqual([]);

    await act(() => app.router.navigate({ to: "/hub/downloads" }));
    await screen.findAllByTestId("image-download");
    expect(untranslatedTexts(root(), [...DATA, ...links()])).toEqual([]);
  });

  it("in French: sections, sizes, machine and buttons", async () => {
    renderHub("/hub/images", ({ backend }) => {
      backend.images.machine.gpu = false;
    });
    await screen.findAllByTestId("image-model");
    act(() => setLanguage("fr"));
    expect(
      screen.getByRole("heading", { name: "Modèles d'images" }),
    ).toBeTruthy();
    expect(screen.getByRole("link", { name: "Images" })).toBeTruthy();
    expect(screen.getByTestId("image-machine").textContent).toBe(
      "Aucune carte graphique trouvée : les images seront faites par le processeur, ce qui prend plusieurs minutes pour chacune.",
    );
    const klein = card("FLUX.2 klein 4B");
    expect(klein.textContent).toContain("4,8\u00a0Go à télécharger");
    expect(klein.textContent).toContain("Licence : apache-2.0");
    expect(
      within(klein).getByRole("button", { name: "Télécharger" }),
    ).toBeTruthy();
    expect(
      screen.getByRole("heading", { name: "Populaires sur Hugging Face" }),
    ).toBeTruthy();
  });
});
