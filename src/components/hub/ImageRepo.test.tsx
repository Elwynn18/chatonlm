// Fiche d'un dépôt de modèles d'images face au simulateur (faux moteur de
// src/test/images.ts) : faits et licence, choix du fichier (conseillé coché,
// autre fichier choisi), pièces annexes (rôle, taille, déjà présentes),
// total et place libre, Download avec la révision de la fiche, progression et
// Cancel, fichier installé ; Download désactivé avec sa raison (famille
// inconnue, dépôt restreint, non censuré sans le réglage, processeur,
// place) ; dépôt introuvable ; pseudo-langue et français.

import {
  act,
  configure,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { setLanguage } from "@/i18n";
import {
  GATED_REPO,
  KLEIN_FILE,
  KLEIN_ID,
  KLEIN_Q8_FILE,
  KLEIN_Q8_ID,
  KLEIN_REPO,
  KLEIN_REVISION,
  SDXL_REPO,
  UNCENSORED_REPO,
  UNKNOWN_REPO,
} from "@/test/images";
import { untranslatedTexts, usePseudoLanguage } from "@/test/i18n";
import { renderHub } from "@/test/renderHub";

configure({ asyncUtilTimeout: 5000 });
vi.setConfig({ testTimeout: 30_000 });

const MIB = 1024 ** 2;
/** Commit du dépôt plus récent que la recommandation épinglée. */
const NEWER = "9999999999999999999999999999999999999999";
const GIB = 1024 ** 3;

const page = (repo: string) => `/hub/images/${repo}`;
const radios = () =>
  screen
    .getAllByTestId("image-file")
    .map((label) => label.querySelector("input")!);
const downloadButton = () =>
  screen.getByRole("button", { name: "Download" }) as HTMLButtonElement;
const reason = () => screen.getByTestId("image-download-reason").textContent;

describe("Image repo page", () => {
  it("facts, files with the recommended one checked, pieces, total and free space", async () => {
    renderHub(page(KLEIN_REPO));
    expect((await screen.findByRole("heading", { level: 2 })).textContent).toBe(
      "FLUX.2-klein-4B-GGUF",
    );
    const facts = screen.getByRole("region", { name: "Details" });
    expect(facts.textContent).toContain("FamilyFLUX.2 klein 4B");
    expect(facts.textContent).toContain("Downloads120K");
    expect(
      within(facts)
        .getByRole("link", { name: "apache-2.0" })
        .getAttribute("href"),
    ).toBe(`https://huggingface.co/${KLEIN_REPO}`);
    expect(
      screen
        .getByRole("link", { name: "View on Hugging Face" })
        .getAttribute("href"),
    ).toBe(`https://huggingface.co/${KLEIN_REPO}`);

    const group = screen.getByRole("group", { name: "Choose a file" });
    expect(radios().map((radio) => [radio.value, radio.checked])).toEqual([
      [KLEIN_FILE, true],
      [KLEIN_Q8_FILE, false],
      ["flux-2-klein-4b-BF16.gguf", false],
    ]);
    const first = within(group).getAllByTestId("image-file")[0];
    expect(first.textContent).toContain("Q4_0");
    expect(first.textContent).toContain("2.3 GB");
    expect(first.textContent).toContain("Recommended");

    const pieces = screen.getAllByTestId("image-component");
    expect(pieces.map((piece) => piece.textContent)).toEqual([
      "Text encoder2.3 GBunsloth/Qwen3-4B-GGUF/Qwen3-4B-Q4_K_M.ggufLicense: apache-2.0",
      "Image decoder (VAE)160 MBblack-forest-labs/FLUX.2-small-decoder/full_encoder_small_decoder.safetensorsLicense: apache-2.0",
    ]);
    expect(screen.getByTestId("image-total").textContent).toBe(
      "To download: 4.8 GB",
    );
    expect((await screen.findByTestId("image-free")).textContent).toBe(
      "Free space: 200 GB",
    );
    expect(downloadButton().disabled).toBe(false);
  });

  it("Download sends the chosen file and the page's revision; progress and Cancel", async () => {
    const { simulator, backend } = renderHub(page(KLEIN_REPO));
    await screen.findAllByTestId("image-file");
    await userEvent.click(radios()[1]);
    expect(radios()[1].checked).toBe(true);
    expect(screen.getByTestId("image-total").textContent).toBe(
      "To download: 6.5 GB",
    );
    await userEvent.click(downloadButton());
    expect(simulator.callsTo("image_download")).toEqual([
      { repo: KLEIN_REPO, file: KLEIN_Q8_FILE, revision: KLEIN_REVISION },
    ]);
    await screen.findByRole("progressbar", {
      name: `Downloading ${KLEIN_Q8_FILE}`,
    });
    await backend.images.progress(KLEIN_Q8_ID, 100 * MIB, 400 * MIB);
    await userEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(simulator.callsTo("image_cancel_download")).toEqual([
      { id: KLEIN_Q8_ID },
    ]);
    await screen.findByRole("button", { name: "Download" });
  });

  it("an installed file: said, with a link to the tab; pieces already there are not counted", async () => {
    renderHub(page(KLEIN_REPO), ({ backend }) => {
      backend.images.install(KLEIN_ID);
    });
    await screen.findAllByTestId("image-file");
    expect(screen.getByText("This file is installed.")).toBeTruthy();
    expect(
      screen
        .getByRole("link", { name: "See your image models" })
        .getAttribute("href"),
    ).toBe("/hub/images");
    expect(screen.queryByRole("button", { name: "Download" })).toBeNull();
    for (const piece of screen.getAllByTestId("image-component")) {
      expect(piece.textContent).toContain("Already on this computer");
    }
    // Un autre fichier : seul lui reste à télécharger.
    await userEvent.click(radios()[1]);
    expect(screen.getByTestId("image-total").textContent).toBe(
      "To download: 4.0 GB",
    );
    expect(downloadButton().disabled).toBe(false);
  });

  it("a file being downloaded: no Installed badge, still counted in the total", async () => {
    renderHub(page(KLEIN_REPO), ({ backend }) => {
      // Le moteur dit `installed` d'un fichier en cours (pas de second
      // Download) : la fiche ne doit pas le montrer installé.
      const row = backend.images.record(KLEIN_ID);
      backend.images.downloads.push({
        id: KLEIN_ID,
        name: row.name,
        state: "downloading",
        completedBytes: 100 * MIB,
        totalBytes: 4 * GIB,
        error: null,
      });
    });
    await screen.findByRole("progressbar");
    expect(radios()[0].checked).toBe(true);
    expect(screen.getAllByTestId("image-file")[0].textContent).not.toContain(
      "Installed",
    );
    expect(screen.getByTestId("image-total").textContent).toBe(
      "To download: 4.8 GB",
    );
    expect(screen.queryByText("This file is installed.")).toBeNull();
  });

  it("a finished download turns the page to installed", async () => {
    const { backend } = renderHub(page(KLEIN_REPO));
    await screen.findAllByTestId("image-file");
    await userEvent.click(downloadButton());
    await screen.findByRole("progressbar");
    await backend.images.finish(KLEIN_ID);
    await screen.findByText("This file is installed.");
  });

  it("a single-file SDXL repo: the file name as its label, no quantization", async () => {
    renderHub(page(SDXL_REPO));
    await screen.findAllByTestId("image-file");
    expect(screen.getAllByTestId("image-file")[0].textContent).toBe(
      "sd_xl_base_1.0.safetensors6.5 GBRecommended",
    );
    expect(
      screen.getByText("Not an open license: read it before using the model."),
    ).toBeTruthy();
  });

  it("a failed download: the page stays downloadable, with the failure message", async () => {
    const { backend } = renderHub(page(KLEIN_REPO));
    await screen.findAllByTestId("image-file");
    await userEvent.click(downloadButton());
    await screen.findByRole("progressbar");
    await backend.images.fail(KLEIN_ID, "Network error");
    expect((await screen.findByRole("alert")).textContent).toBe(
      "Network error",
    );
    // Ligne gardée en base, fichiers absents : pas « installé ».
    await waitFor(() => expect(downloadButton().disabled).toBe(false));
    expect(screen.queryByText("This file is installed.")).toBeNull();
    expect(screen.getAllByTestId("image-file")[0].textContent).not.toContain(
      "Installed",
    );
  });

  it("a recommendation whose repo has a newer commit: Download from its page works", async () => {
    const { simulator } = renderHub(page(KLEIN_REPO), ({ backend }) => {
      backend.images.repos.get(KLEIN_REPO)!.revision = NEWER;
    });
    await screen.findAllByTestId("image-file");
    await userEvent.click(downloadButton());
    expect(simulator.callsTo("image_download")).toEqual([
      { repo: KLEIN_REPO, file: KLEIN_FILE, revision: NEWER },
    ]);
    await screen.findByRole("progressbar");
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("a Download error goes when another file is chosen", async () => {
    const { backend } = renderHub(page(KLEIN_REPO));
    backend.images.failures.set("image_download", {
      kind: "invalid",
      message: "Not enough disk space",
    });
    await screen.findAllByTestId("image-file");
    await userEvent.click(downloadButton());
    expect((await screen.findByRole("alert")).textContent).toBe(
      "Not enough disk space",
    );
    await userEvent.click(radios()[1]);
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("Download then Cancel: the focus goes to the page title, not to the page body", async () => {
    renderHub(page(KLEIN_REPO));
    await screen.findAllByTestId("image-file");
    const title = screen.getByRole("heading", { level: 2 });
    await userEvent.click(downloadButton());
    await screen.findByRole("progressbar");
    await waitFor(() => expect(document.activeElement).toBe(title));
    await userEvent.click(screen.getByRole("button", { name: "Cancel" }));
    await screen.findByRole("button", { name: "Download" });
    await waitFor(() => expect(document.activeElement).toBe(title));
  });
});

describe("Image repo page: why Download is off", () => {
  it("an unknown family", async () => {
    renderHub(page(UNKNOWN_REPO));
    await screen.findAllByTestId("image-file");
    expect(
      screen.getByRole("region", { name: "Details" }).textContent,
    ).toContain("FamilyNot supported");
    expect(downloadButton().disabled).toBe(true);
    expect(reason()).toBe(
      "ChatonLM doesn't recognize this model's family, so it doesn't know which other files it needs.",
    );
    expect(downloadButton().getAttribute("aria-describedby")).toBe(
      screen.getByTestId("image-download-reason").id,
    );
  });

  it("a restricted repo: no file to choose", async () => {
    renderHub(page(GATED_REPO));
    await screen.findByRole("heading", { level: 2 });
    await waitFor(() =>
      expect(reason()).toBe(
        "This model needs a Hugging Face account. ChatonLM can't download it.",
      ),
    );
    expect(screen.queryByRole("group")).toBeNull();
    expect(downloadButton().disabled).toBe(true);
  });

  it("an uncensored repo without the setting: badge, and the way to turn it on", async () => {
    renderHub(page(UNCENSORED_REPO));
    await screen.findAllByTestId("image-file");
    expect(screen.getByText("Uncensored")).toBeTruthy();
    expect(downloadButton().disabled).toBe(true);
    expect(reason()).toBe(
      "Turn on “Show uncensored models” in Settings › Models to download this model.",
    );
    expect(
      screen
        .getByRole("link", { name: "Settings › Models" })
        .getAttribute("href"),
    ).toBe("/settings?section=models");
  });

  it("an uncensored repo with the setting: downloadable", async () => {
    renderHub(page(UNCENSORED_REPO), ({ backend }) => {
      backend.settings.showUncensored = true;
    });
    await screen.findAllByTestId("image-file");
    expect(downloadButton().disabled).toBe(false);
    expect(screen.queryByTestId("image-download-reason")).toBeNull();
  });

  it("a processor the engine can't use", async () => {
    renderHub(page(KLEIN_REPO), ({ backend }) => {
      backend.images.machine.missingCpuFeatures = ["AVX2"];
    });
    await screen.findAllByTestId("image-file");
    await waitFor(() =>
      expect(reason()).toBe(
        "This computer's processor lacks instructions the image engine needs (AVX2).",
      ),
    );
    expect(downloadButton().disabled).toBe(true);
  });

  it("not enough free space (100 MB kept free)", async () => {
    renderHub(page(KLEIN_REPO), ({ backend }) => {
      // 4,84 Gio : plus que les 4,77 Gio à télécharger, moins la marge comprise.
      backend.images.machine.freeBytes = 5_200_000_000;
    });
    await screen.findAllByTestId("image-file");
    await waitFor(() =>
      expect(reason()).toBe(
        "Not enough free space: 4.8 GB needed, 4.8 GB free.",
      ),
    );
    expect(downloadButton().disabled).toBe(true);
  });

  it("enough room once the margin is counted", async () => {
    renderHub(page(KLEIN_REPO), ({ backend }) => {
      backend.images.machine.freeBytes = 5.5 * GIB;
    });
    await screen.findByTestId("image-free");
    expect(screen.queryByTestId("image-download-reason")).toBeNull();
    expect(downloadButton().disabled).toBe(false);
  });
});

describe("Image repo page: Hugging Face", () => {
  it("an unknown repo: not found, link to Hugging Face, no Retry", async () => {
    renderHub(page("nobody/nothing"));
    expect(await screen.findByText("Model not found")).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Retry" })).toBeNull();
    expect(
      screen
        .getByRole("link", { name: "View on Hugging Face" })
        .getAttribute("href"),
    ).toBe("https://huggingface.co/nobody/nothing");
  });

  it("offline with the page in the cache: banner over it", async () => {
    renderHub(page(KLEIN_REPO), ({ backend }) => {
      backend.images.problem = { kind: "offline" };
      backend.images.stale = true;
    });
    expect((await screen.findByTestId("hub-banner")).textContent).toContain(
      "You're offline.",
    );
    expect(screen.getAllByTestId("image-file")).toHaveLength(3);
  });
});

describe("Image repo page in other languages", () => {
  const DATA = [
    "FLUX.2-klein-4B-GGUF",
    "FLUX.2 klein 4B",
    "leejet",
    "apache-2.0",
    "Q4_0",
    "Q8_0",
    "BF16",
    "F2",
    "AVX2",
    "flux-2-klein-4b-Q4_0.gguf",
    "flux-2-klein-4b-Q8_0.gguf",
    "flux-2-klein-4b-BF16.gguf",
    "unsloth/Qwen3-4B-GGUF",
    "Qwen3-4B-Q4_K_M.gguf",
    "black-forest-labs/FLUX.2-small-decoder",
    "full_encoder_small_decoder.safetensors",
    "Z-Image-Turbo-Uncensored-GGUF",
    "someone",
    "z-image-turbo-uncensored-Q4_K_M.gguf",
    "Z-Image-Turbo",
    "Q4_K_M",
    "ZI",
    "unsloth/Qwen3-4B-Instruct-2507-GGUF",
    "Qwen3-4B-Instruct-2507-Q4_K_M.gguf",
    "Comfy-Org/z_image_turbo",
    "split_files/vae/ae.safetensors",
  ];

  it("pseudo-language: page, pieces, reasons, no text written in the code", async () => {
    usePseudoLanguage();
    const { app } = renderHub(page(KLEIN_REPO), ({ backend }) => {
      backend.images.machine.gpu = false;
      backend.images.machine.missingCpuFeatures = ["AVX2"];
      backend.images.install(KLEIN_Q8_ID);
    });
    await screen.findAllByTestId("image-file");
    await screen.findByTestId("image-download-reason");
    const root = () => screen.getByTestId("hub-scroll").parentElement!;
    const links = () =>
      [...root().querySelectorAll("a")].map(
        (link) => link.getAttribute("href") ?? "",
      );
    expect(untranslatedTexts(root(), [...DATA, ...links()])).toEqual([]);

    await act(() =>
      app.router.navigate({
        to: "/hub/images/$author/$repo",
        params: { author: "someone", repo: "Z-Image-Turbo-Uncensored-GGUF" },
      }),
    );
    await screen.findByTestId("image-download-reason");
    expect(untranslatedTexts(root(), [...DATA, ...links()])).toEqual([]);
  });

  it("in French: files, pieces, total and reason", async () => {
    renderHub(page(UNCENSORED_REPO));
    await screen.findAllByTestId("image-file");
    act(() => setLanguage("fr"));
    expect(
      screen.getByRole("group", { name: "Choisissez un fichier" }),
    ).toBeTruthy();
    expect(screen.getByText("Non censuré")).toBeTruthy();
    expect(reason()).toBe(
      "Activez « Afficher les modèles non censurés » dans Paramètres › Modèles pour télécharger ce modèle.",
    );
    expect(
      screen
        .getAllByTestId("image-component")
        .map(
          (piece) =>
            piece.textContent?.startsWith("Encodeur de texte") ||
            piece.textContent?.startsWith("Décodeur d'images (VAE)"),
        ),
    ).toEqual([true, true]);
    expect(screen.getByTestId("image-total").textContent).toMatch(
      /^À télécharger\u00a0: 6,3\u00a0Go$/,
    );
  });
});
