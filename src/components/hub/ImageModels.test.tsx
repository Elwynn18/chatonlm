// Onglet Images du ModelHub face au simulateur (faux moteur de
// src/test/images.ts) : onglet et badge, « How it works », recommandations
// (nom et variante, phrase de la famille, repères : vitesse estimée ou
// mesurée, VRAM qui tient ou dépasse, place, usage ; Details replié),
// téléchargement (révision passée, progression, fin, Use, In use),
// suppression confirmée (focus), échec (Try again avec la révision du
// modèle, Dismiss), machine (carte graphique, moteur, processeur, sans
// carte), recherche (q dans l'adresse après le délai, compatibles seulement
// avec la ligne des masqués et Show / Hide, non censurés retirés ou marqués,
// Hugging Face injoignable), section de l'onglet Downloads ; textes en
// pseudo-langue et en français.

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
import { untranslatedTexts, usePseudoLanguage } from "@/test/i18n";
import {
  GATED_REPO,
  KLEIN_9B_ID,
  KLEIN_9B_REPO,
  KLEIN_FILE,
  KLEIN_ID,
  KLEIN_Q8_ID,
  KLEIN_REPO,
  KLEIN_REVISION,
  SDXL_REPO,
  UNCENSORED_ID,
  UNCENSORED_REPO,
  UNKNOWN_REPO,
  Z_IMAGE_ID,
  Z_IMAGE_REPO,
} from "@/test/images";
import { renderHub } from "@/test/renderHub";

/** Encodeur de texte de klein 4B (pièce partagée de la famille). */
const KLEIN_ENCODER = "unsloth/Qwen3-4B-GGUF/Qwen3-4B-Q4_K_M.gguf";
/** Noms des cartes : nom affiché (recommandation, sinon famille) et variante. */
const KLEIN = "FLUX.2 klein 4B Q4_0";
const KLEIN_Q8 = "FLUX.2 klein 4B Q8_0";
const Z_IMAGE = "Z-Image-Turbo Q4_K";

configure({ asyncUtilTimeout: 5000 });
vi.setConfig({ testTimeout: 30_000 });

const MIB = 1024 ** 2;
const GIB = 1024 ** 3;

const card = (name: string) =>
  screen.getByRole("listitem", { name }) as HTMLElement;
const section = (name: string) => screen.queryByRole("list", { name });
/** Noms des cartes d'une section (titre et variante, comme leur nom ARIA). */
const titlesIn = (name: string) =>
  within(section(name)!)
    .getAllByTestId("image-model")
    .map((element) =>
      (element.getAttribute("aria-labelledby") ?? "")
        .split(" ")
        .map((id) => document.getElementById(id)!.textContent)
        .join(" "),
    );
/** Repère `name` (speed, memory, size, usage) d'une carte. */
const fact = (element: HTMLElement, name: string) =>
  element.querySelector<HTMLElement>(`[data-fact=${name}]`)!;
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
    // Comment ça marche : trois étapes, dans l'ordre.
    expect(
      within(screen.getByRole("list", { name: "How it works" }))
        .getAllByRole("listitem")
        .map((step) => step.textContent),
    ).toEqual([
      "1Download a model",
      "2Click Use",
      "3In a chat, press the Image button (or ask the model for a picture)",
    ]);
    expect(section("Your image models")).toBeNull();
    expect(titlesIn("Recommended")).toEqual([KLEIN, Z_IMAGE]);
    const klein = card(KLEIN);
    // Nom de la recommandation, puis sa variante expliquée en infobulle.
    expect(within(klein).getByRole("heading").textContent).toBe(
      "FLUX.2 klein 4B",
    );
    expect(within(klein).getByText("Q4_0").getAttribute("title")).toBe(
      "File variant. Q4: lighter, a little less faithful · Q8: more faithful, heavier",
    );
    expect(within(klein).getByTestId("image-blurb").textContent).toBe(
      "Fast and versatile, good at text in pictures. A good first choice.",
    );
    // Repères : vitesse estimée, VRAM qui tient sur la carte, place, usage.
    expect(fact(klein, "speed").textContent).toBe("≈ 5 s per image");
    expect(
      fact(klein, "speed").querySelector("[title]")!.getAttribute("title"),
    ).toBe("Estimate for one 1024×1024 picture");
    expect(fact(klein, "memory").textContent).toBe(
      "7.9 GB of VRAMFits your card (24 GB)",
    );
    expect(
      fact(klein, "memory").querySelector("[data-fit=fits]")!.className,
    ).toContain("green-100");
    // Modèle, encodeur et décodeur : rien n'est encore sur le disque.
    expect(fact(klein, "size").textContent).toBe("4.8 GB to download");
    expect(fact(klein, "usage").textContent).toBe("Free to use");
    expect(
      fact(klein, "usage").querySelector("[title]")!.getAttribute("title"),
    ).toBe("License: apache-2.0");
    expect(
      within(klein)
        .getAllByRole("button")
        .map((button) => button.textContent),
    ).toEqual(["Download", "Details"]);
    // Pas d'initiales : une icône neutre.
    expect(klein.textContent).not.toContain("F2");
    // Aucun avertissement pour une machine avec carte graphique, mais la
    // carte, en neutre.
    expect(screen.queryByTestId("image-machine")).toBeNull();
    expect(screen.getByTestId("image-gpu").textContent).toBe(
      "Your graphics card: NVIDIA GeForce RTX 3090 · 24 GB",
    );
  });

  it("Details is folded: the files, their exact license and the repo appear once opened", async () => {
    renderHub("/hub/images");
    await screen.findAllByTestId("image-model");
    const klein = card(KLEIN);
    const details = within(klein).getByRole("button", { name: "Details" });
    expect(details.getAttribute("aria-expanded")).toBe("false");
    const content = document.getElementById(
      details.getAttribute("aria-controls")!,
    )!;
    expect(content.hidden).toBe(true);
    expect(within(klein).queryByRole("link", { name: KLEIN_REPO })).toBeNull();
    await userEvent.click(details);
    expect(details.getAttribute("aria-expanded")).toBe("true");
    expect(content.hidden).toBe(false);
    expect(
      within(klein)
        .getAllByTestId("image-component")
        .map((piece) => piece.textContent),
    ).toEqual([
      `Model2.3 GB${KLEIN_REPO}/${KLEIN_FILE}License: apache-2.0`,
      "Text encoder2.3 GBunsloth/Qwen3-4B-GGUF/Qwen3-4B-Q4_K_M.ggufLicense: apache-2.0",
      "Image decoder (VAE)160 MBblack-forest-labs/FLUX.2-small-decoder/full_encoder_small_decoder.safetensorsLicense: apache-2.0",
    ]);
    expect(
      within(klein)
        .getByRole("link", { name: KLEIN_REPO })
        .getAttribute("href"),
    ).toBe(`https://huggingface.co/${KLEIN_REPO}`);
    expect(
      within(klein)
        .getByRole("link", { name: "View on Hugging Face" })
        .getAttribute("href"),
    ).toBe(`https://huggingface.co/${KLEIN_REPO}`);
    await userEvent.click(details);
    expect(content.hidden).toBe(true);
  });

  it("Download passes the pinned revision; progress, then Use and In use", async () => {
    const { simulator, backend } = renderHub("/hub/images");
    await screen.findAllByTestId("image-model");
    await userEvent.click(
      within(card(KLEIN)).getByRole("button", { name: "Download" }),
    );
    expect(simulator.callsTo("image_download")).toEqual([
      { repo: KLEIN_REPO, file: KLEIN_FILE, revision: KLEIN_REVISION },
    ]);
    // En cours : rangé dans « Your image models ».
    await waitFor(() => expect(titlesIn("Your image models")).toEqual([KLEIN]));
    await within(card(KLEIN)).findByText("Waiting…");
    expect(screen.getByTestId("active-downloads").textContent).toBe("1");

    await backend.images.progress(KLEIN_ID, 200 * MIB, 400 * MIB);
    const bar = await within(card(KLEIN)).findByRole("progressbar", {
      name: `Downloading ${KLEIN}`,
    });
    await waitFor(() => expect(bar.getAttribute("aria-valuenow")).toBe("50"));

    await backend.images.finish(KLEIN_ID);
    const use = await within(card(KLEIN)).findByRole("button", {
      name: "Use",
    });
    expect(screen.queryByTestId("active-downloads")).toBeNull();
    expect(titlesIn("Recommended")).toEqual([Z_IMAGE]);
    await userEvent.click(use);
    expect(simulator.callsTo("image_use_model")).toEqual([{ id: KLEIN_ID }]);
    await within(card(KLEIN)).findByText("In use");
    expect(
      within(card(KLEIN)).queryByRole("button", { name: "Use" }),
    ).toBeNull();
  });

  it("an installed model of the same family makes its pieces count as present", async () => {
    renderHub("/hub/images", ({ backend }) => {
      backend.images.install(KLEIN_Q8_ID);
    });
    await screen.findAllByTestId("image-model");
    // Pas une recommandation : le nom de sa famille, et sa variante.
    expect(titlesIn("Your image models")).toEqual([KLEIN_Q8]);
    // Plus que le fichier du modèle (2,3 Go) : encodeur et décodeur partagés,
    // dits déjà présents.
    expect(fact(card(KLEIN), "size").textContent).toBe(
      "2.3 GB to download (2.5 GB already on this computer)",
    );
    const installed = card(KLEIN_Q8);
    expect(within(installed).getByRole("heading").textContent).toBe(
      "FLUX.2 klein 4B",
    );
    expect(fact(installed, "size").textContent).toBe("6.5 GB on disk");
    // Installé : pas de « déjà sur cet ordinateur » sur chaque fichier.
    expect(installed.textContent).not.toContain("Already on this computer");
  });

  it("Delete asks first (in use: generation stops), then the focus goes to the card title", async () => {
    const { simulator } = renderHub("/hub/images", ({ backend }) => {
      backend.images.install(KLEIN_Q8_ID);
      backend.images.select(KLEIN_Q8_ID);
    });
    await screen.findAllByTestId("image-model");
    const installed = card(KLEIN_Q8);
    expect(within(installed).getByText("In use")).toBeTruthy();
    await userEvent.click(
      within(installed).getByRole("button", {
        name: `Delete ${KLEIN_Q8}`,
      }),
    );
    const dialog = await screen.findByRole("alertdialog", {
      name: `Delete ${KLEIN_Q8}?`,
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
        name: `Delete ${KLEIN_Q8}`,
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
      within(installed).getByRole("heading", { name: "FLUX.2 klein 4B" }),
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
      within(card(KLEIN)).getByRole("button", {
        name: `Delete ${KLEIN}`,
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
      within(card(KLEIN)).getByRole("button", { name: "Download" }),
    );
    await within(card(KLEIN)).findByText("Waiting…");
    await backend.images.fail(KLEIN_ID, "The file did not match its checksum.");
    expect((await within(card(KLEIN)).findByRole("alert")).textContent).toBe(
      "The file did not match its checksum.",
    );
    await userEvent.click(
      within(card(KLEIN)).getByRole("button", {
        name: "Try again",
      }),
    );
    await waitFor(() =>
      expect(simulator.callsTo("image_download")).toEqual([
        { repo: KLEIN_REPO, file: KLEIN_FILE, revision: KLEIN_REVISION },
        { repo: KLEIN_REPO, file: KLEIN_FILE, revision: KLEIN_REVISION },
      ]),
    );
    await within(card(KLEIN)).findByText("Waiting…");
    await backend.images.fail(KLEIN_ID, "Network error");
    await userEvent.click(
      await within(card(KLEIN)).findByRole("button", {
        name: "Dismiss",
      }),
    );
    expect(simulator.callsTo("image_dismiss_download")).toEqual([
      { id: KLEIN_ID },
    ]);
    // Recommandation sans téléchargement : de retour avec Download.
    await waitFor(() => expect(section("Your image models")).toBeNull());
    expect(
      within(card(KLEIN)).getByRole("button", { name: "Download" }),
    ).toBeTruthy();
  });

  it("a refused download shows the engine's reason on the card", async () => {
    const { backend } = renderHub("/hub/images");
    backend.images.machine.freeBytes = 1024 ** 3;
    await screen.findAllByTestId("image-model");
    await userEvent.click(
      within(card(Z_IMAGE)).getByRole("button", { name: "Download" }),
    );
    expect(
      (await within(card(Z_IMAGE)).findByRole("alert")).textContent,
    ).toMatch(/^Not enough disk space/);
  });

  it("a saved model never installed, not recommended: Dismiss forgets it, even when the processor blocks", async () => {
    const { simulator } = renderHub("/hub/images", ({ backend }) => {
      // Ligne d'avant un redémarrage : ni fichier, ni téléchargement.
      backend.images.record(KLEIN_Q8_ID);
      backend.images.machine.missingCpuFeatures = ["AVX2"];
    });
    await screen.findAllByTestId("image-model");
    const saved = card(KLEIN_Q8);
    expect(
      within(saved)
        .getAllByRole("button")
        .map((button) => button.textContent),
    ).toEqual(["Dismiss", "Details"]);
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
      within(card(KLEIN_Q8))
        .getAllByRole("button")
        .map((button) => button.textContent),
    ).toEqual(["Download", "Dismiss", "Details"]);
  });

  it("the error of an earlier action goes once another one succeeds", async () => {
    const { backend } = renderHub("/hub/images");
    await screen.findAllByTestId("image-model");
    await userEvent.click(
      within(card(KLEIN)).getByRole("button", { name: "Download" }),
    );
    await within(card(KLEIN)).findByText("Waiting…");
    backend.images.failures.set("image_cancel_download", {
      kind: "internal",
      message: "queue is busy",
    });
    await userEvent.click(
      within(card(KLEIN)).getByRole("button", { name: "Cancel" }),
    );
    await within(card(KLEIN)).findByText("queue is busy");
    backend.images.failures.delete("image_cancel_download");
    await backend.images.fail(KLEIN_ID, "Network error");
    await userEvent.click(
      await within(card(KLEIN)).findByRole("button", {
        name: "Try again",
      }),
    );
    await within(card(KLEIN)).findByText("Waiting…");
    expect(within(card(KLEIN)).queryByText("queue is busy")).toBeNull();
  });

  it("pieces finished before a failure stay on the disk: less left to download", async () => {
    const { backend } = renderHub("/hub/images");
    await screen.findAllByTestId("image-model");
    await userEvent.click(
      within(card(KLEIN)).getByRole("button", { name: "Download" }),
    );
    await within(card(KLEIN)).findByText("Waiting…");
    await backend.images.fail(KLEIN_ID, "Network error", [KLEIN_ENCODER]);
    // Modèle (2,3 Go) et décodeur (160 Mo) : l'encodeur est déjà là.
    await waitFor(() =>
      expect(card(KLEIN).textContent).toContain("2.4 GB to download"),
    );
  });

  it("a recommendation saved without its files comes before those never saved", async () => {
    renderHub("/hub/images", ({ backend }) => {
      backend.images.record(Z_IMAGE_ID);
    });
    await screen.findAllByTestId("image-model");
    expect(titlesIn("Recommended")).toEqual([Z_IMAGE, KLEIN]);
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
      within(card(KLEIN_Q8)).getByRole("button", {
        name: `Delete ${KLEIN_Q8}`,
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

describe("Images: what each card says", () => {
  it("a time measured on this computer wins over the estimate; minutes past 90 s", async () => {
    renderHub("/hub/images", ({ backend }) => {
      backend.images.install(KLEIN_ID, Z_IMAGE_ID);
      backend.images.measured.set(KLEIN_ID, 7);
      backend.images.measured.set(Z_IMAGE_ID, 300);
    });
    await screen.findAllByTestId("image-model");
    const speed = fact(card(KLEIN), "speed");
    expect(speed.textContent).toBe("≈ 7 s per image (measured)");
    expect(speed.querySelector("[title]")!.getAttribute("title")).toBe(
      "Time of the last picture made on this computer",
    );
    expect(fact(card(Z_IMAGE), "speed").textContent).toBe(
      "≈ 5 min per image (measured)",
    );
    expect(fact(card(Z_IMAGE), "memory").textContent).toBe(
      "9.7 GB of VRAMFits your card (24 GB)",
    );
  });

  it("a model bigger than the card: said in amber, and slower", async () => {
    renderHub("/hub/images", ({ backend }) => {
      backend.images.machine.vramBytes = 6 * GIB;
      backend.images.machine.gpuName = null;
    });
    await screen.findAllByTestId("image-model");
    const memory = fact(card(KLEIN), "memory");
    expect(memory.textContent).toBe(
      "7.9 GB of VRAMMore than your card: slower",
    );
    expect(memory.querySelector("[data-fit=tooBig]")!.className).toContain(
      "amber-100",
    );
    // Moteur : trois fois plus lent quand le modèle ne tient pas.
    expect(fact(card(KLEIN), "speed").textContent).toBe("≈ 15 s per image");
    // Carte sans nom connu : sa mémoire seule.
    expect(screen.getByTestId("image-gpu").textContent).toBe(
      "Your graphics card: 6.0 GB",
    );
  });

  it("a card whose memory is unknown: the figure alone", async () => {
    renderHub("/hub/images", ({ backend }) => {
      backend.images.machine.vramBytes = null;
    });
    await screen.findAllByTestId("image-model");
    expect(fact(card(KLEIN), "memory").textContent).toBe("7.9 GB of VRAM");
    expect(screen.getByTestId("image-gpu").textContent).toBe(
      "Your graphics card: NVIDIA GeForce RTX 3090",
    );
  });

  it("a family without a reference time nor a sentence: no speed (its place kept), no blurb", async () => {
    renderHub("/hub/images", ({ backend }) => {
      // Famille reconnue du moteur, sans mesure ni phrase dans l'app.
      backend.images.repos.get(KLEIN_REPO)!.summary.family = {
        id: "mystery-family",
        name: "Mystery",
      };
      backend.images.install(KLEIN_Q8_ID);
    });
    await screen.findAllByTestId("image-model");
    const mystery = card("Mystery Q8_0");
    const speed = fact(mystery, "speed");
    expect(speed.getAttribute("aria-hidden")).toBe("true");
    expect(speed.textContent).toBe("");
    expect(within(mystery).queryByTestId("image-blurb")).toBeNull();
  });

  it("usage in plain words: non-commercial in amber, a license to read as a link", async () => {
    renderHub("/hub/images", ({ backend }) => {
      backend.images.install(
        KLEIN_9B_ID,
        `${SDXL_REPO}/sd_xl_base_1.0.safetensors`,
      );
    });
    await screen.findAllByTestId("image-model");
    const klein9 = card("FLUX.2 klein 9B Q8_0");
    expect(fact(klein9, "usage").textContent).toBe("Non-commercial use only");
    expect(
      fact(klein9, "usage").querySelector("span span")!.className,
    ).toContain("amber-700");
    expect(fact(klein9, "memory").textContent).toBe(
      "18 GB of VRAMFits your card (24 GB)",
    );
    expect(fact(klein9, "speed").textContent).toBe("≈ 10 s per image");
    expect(within(klein9).getByTestId("image-blurb").textContent).toBe(
      "Bigger klein: more detail and closer to your description, but heavier and slower.",
    );
    // Fichier unique sans variante : le nom de la famille seul.
    const sdxl = card("Stable Diffusion XL");
    const link = within(fact(sdxl, "usage")).getByRole("link", {
      name: "Check the license",
    });
    expect(link.getAttribute("href")).toBe(
      `https://huggingface.co/${SDXL_REPO}`,
    );
    expect(
      fact(sdxl, "usage").querySelector("[title]")!.getAttribute("title"),
    ).toBe("License: openrail++");
  });
});

describe("Images: what the machine allows", () => {
  it("no graphics card: images by the processor, slow", async () => {
    renderHub("/hub/images", ({ backend }) => {
      // Nom et mémoire d'une carte non retenue (moteur) : ignorés.
      backend.images.machine.gpu = false;
    });
    const notice = await screen.findByTestId("image-machine");
    expect(notice.textContent).toBe(
      "No graphics card found: images will be made by the processor, which takes several minutes each.",
    );
    expect(notice.querySelector("[data-note=gpu]")!.className).toContain(
      "amber",
    );
    expect(screen.queryByTestId("image-gpu")).toBeNull();
    // Permis : Download reste là ; plusieurs minutes, VRAM sans comparaison.
    await screen.findAllByTestId("image-model");
    expect(
      within(card(KLEIN)).getByRole("button", { name: "Download" }),
    ).toBeTruthy();
    expect(fact(card(KLEIN), "speed").textContent).toBe(
      "Several minutes per image (processor)",
    );
    expect(fact(card(KLEIN), "memory").textContent).toBe("7.9 GB of VRAM");
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
      within(card(Z_IMAGE)).queryByRole("button", { name: "Download" }),
    ).toBeNull();
    expect(
      within(card(KLEIN)).queryByRole("button", { name: "Use" }),
    ).toBeNull();
    // Suppression gardée.
    expect(
      within(card(KLEIN)).getByRole("button", {
        name: `Delete ${KLEIN}`,
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
  it("without text: the most downloaded compatible repos, uncensored ones left out; a card opens its page", async () => {
    const { simulator, app } = renderHub("/hub/images");
    const heading = await screen.findByRole("heading", {
      name: "Compatible on Hugging Face",
    });
    // Région nommée par son titre (pas un aria-label en double).
    const region = screen.getByRole("region", {
      name: "Compatible on Hugging Face",
    });
    expect(region.getAttribute("aria-labelledby")).toBe(heading.id);
    expect(region.hasAttribute("aria-label")).toBe(false);
    // Le dépôt restreint et la famille inconnue sont masqués.
    await waitFor(() =>
      expect(repoCards()).toEqual([
        KLEIN_REPO,
        Z_IMAGE_REPO,
        KLEIN_9B_REPO,
        SDXL_REPO,
      ]),
    );
    expect(simulator.callsTo("image_search")).toEqual([
      { query: "", showUnsupported: false },
    ]);
    const cards = screen.getAllByTestId("image-repo-card");
    // Famille et sa phrase, usage en clair, téléchargements discrets.
    expect(cards[0].textContent).toBe(
      "FLUX.2-klein-4B-GGUFleejetFLUX.2 klein 4BFast and versatile, good at text in pictures. A good first choice.Free to use·120K",
    );
    expect(within(cards[0]).getByTitle("120,000 downloads").textContent).toBe(
      "120K",
    );
    expect(cards[2].textContent).toContain("Non-commercial use only");
    expect(cards[3].textContent).toContain("Check the license");
    expect(screen.getByTestId("image-hidden").textContent).toBe(
      "2 unsupported or restricted models hidden·Show",
    );
    await userEvent.click(cards[0]);
    await waitFor(() =>
      expect(app.router.state.location.pathname).toBe(
        `/hub/images/${KLEIN_REPO}`,
      ),
    );
  });

  it("Show lists the hidden repos at the end, set apart, with their reason; Hide", async () => {
    const { simulator, app } = renderHub("/hub/images");
    await screen.findByTestId("image-hidden");
    const show = within(screen.getByTestId("image-hidden")).getByRole(
      "button",
      { name: "Show" },
    );
    await userEvent.click(show);
    expect(app.router.state.location.search).toEqual({
      showUnsupported: true,
    });
    await waitFor(() =>
      expect(repoCards()).toEqual([
        KLEIN_REPO,
        Z_IMAGE_REPO,
        KLEIN_9B_REPO,
        SDXL_REPO,
        GATED_REPO,
        UNKNOWN_REPO,
      ]),
    );
    expect(simulator.callsTo("image_search")).toEqual([
      { query: "", showUnsupported: false },
      { query: "", showUnsupported: true },
    ]);
    const hidden = within(
      screen.getByRole("list", { name: "Unsupported or restricted models" }),
    ).getAllByTestId("image-repo-card");
    expect(hidden.map((element) => element.dataset.supported)).toEqual([
      "false",
      "false",
    ]);
    expect(hidden[0].className).toContain("border-dashed");
    expect(within(hidden[0]).getByTestId("image-repo-reason").textContent).toBe(
      "Restricted: needs a Hugging Face account",
    );
    expect(within(hidden[1]).getByTestId("image-repo-reason").textContent).toBe(
      "Not supported by ChatonLM",
    );
    // Le bouton reste le même élément (focus gardé), relibellé.
    const line = screen.getByTestId("image-hidden");
    expect(line.textContent).toBe("Unsupported or restricted models·Hide");
    const hide = within(line).getByRole("button", { name: "Hide" });
    expect(hide).toBe(show);
    await userEvent.click(hide);
    expect(app.router.state.location.search).toEqual({});
    await waitFor(() => expect(repoCards()).toHaveLength(4));
  });

  it("the address can ask for the unsupported repos; a search keeps that choice", async () => {
    const { simulator } = renderHub("/hub/images?showUnsupported=true");
    await waitFor(() => expect(repoCards()).toHaveLength(6));
    expect(simulator.callsTo("image_search")).toEqual([
      { query: "", showUnsupported: true },
    ]);
    await userEvent.type(
      screen.getByRole("textbox", { name: "Search Hugging Face models" }),
      "mystery",
    );
    await waitFor(() => expect(repoCards()).toEqual([UNKNOWN_REPO]));
    expect(simulator.callsTo("image_search").at(-1)).toEqual({
      query: "mystery",
      showUnsupported: true,
    });
  });

  it("the address may give showUnsupported as text: still understood", async () => {
    const { simulator } = renderHub("/hub/images?showUnsupported=%22true%22");
    await waitFor(() => expect(repoCards()).toHaveLength(6));
    expect(simulator.callsTo("image_search")).toEqual([
      { query: "", showUnsupported: true },
    ]);
  });

  it("a search that only finds unsupported repos: no compatible model, and the way to see them", async () => {
    renderHub("/hub/images?q=mystery");
    expect(
      await screen.findByText("No compatible image models match your search."),
    ).toBeTruthy();
    expect(screen.getByTestId("image-hidden").textContent).toBe(
      "1 unsupported or restricted model hidden·Show",
    );
    await userEvent.click(screen.getByRole("button", { name: "Show" }));
    await waitFor(() => expect(repoCards()).toEqual([UNKNOWN_REPO]));
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
    await waitFor(() =>
      expect(repoCards()).toEqual([KLEIN_REPO, KLEIN_9B_REPO]),
    );
    // Une seule recherche après la pause, pas une par lettre.
    expect(simulator.callsTo("image_search")).toEqual([
      { query: "", showUnsupported: false },
      { query: "klein", showUnsupported: false },
    ]);
    await userEvent.type(field, "{Escape}");
    await screen.findByRole("heading", { name: "Compatible on Hugging Face" });
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
    expect(screen.queryByTestId("image-hidden")).toBeNull();
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
      within(card("Z-Image-Turbo Q4_K_M")).getByText("Uncensored"),
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
    expect(screen.getAllByTestId("image-repo-card")).toHaveLength(4);
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
      ).toEqual([Z_IMAGE, KLEIN, "z-image-turbo-uncensored-Q4_K_M.gguf"]),
    );
    expect(within(rows()[0]).getByRole("link").getAttribute("href")).toBe(
      "/hub/images/leejet/Z-Image-Turbo-GGUF",
    );
    expect(
      within(rows()[0]).getByRole("progressbar", {
        name: `Downloading ${Z_IMAGE}`,
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
      within(row(Z_IMAGE)).getByRole("button", { name: "Cancel" }),
    );
    expect(simulator.callsTo("image_cancel_download")).toEqual([
      { id: Z_IMAGE_ID },
    ]);
    await userEvent.click(
      within(row(KLEIN)).getByRole("button", {
        name: "Try again",
      }),
    );
    await waitFor(() =>
      expect(simulator.callsTo("image_download")).toEqual([
        { repo: KLEIN_REPO, file: KLEIN_FILE, revision: KLEIN_REVISION },
      ]),
    );
    await waitFor(() =>
      expect(row(Z_IMAGE).textContent).toContain("Cancelled"),
    );
    await userEvent.click(
      within(row(Z_IMAGE)).getByRole("button", { name: "Dismiss" }),
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
  /** Données des dépôts (noms, auteurs, fichiers, licences, initiales). */
  const DATA = [
    "FLUX.2 klein 4B",
    "FLUX.2 klein 9B",
    "Z-Image-Turbo",
    "FLUX.1 dev",
    "Stable Diffusion XL",
    "FLUX.2-klein-4B-GGUF",
    "FLUX.2-klein-9B-GGUF",
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
    "NVIDIA GeForce RTX 3090",
    KLEIN_REPO,
    Z_IMAGE_REPO,
    "flux-2-klein-4b-Q8_0.gguf",
    "flux-2-klein-4b-Q4_0.gguf",
    "z_image_turbo-Q4_K.gguf",
    "unsloth/Qwen3-4B-GGUF",
    "Qwen3-4B-Q4_K_M.gguf",
    "black-forest-labs/FLUX.2-small-decoder",
    "full_encoder_small_decoder.safetensors",
    "unsloth/Qwen3-4B-Instruct-2507-GGUF",
    "Qwen3-4B-Instruct-2507-Q4_K_M.gguf",
    "Comfy-Org/z_image_turbo",
    "split_files/vae/ae.safetensors",
    "Q4_0",
    "Q8_0",
    "Q4_K",
    initials(KLEIN),
    initials(Z_IMAGE),
  ];

  it("pseudo-language: Images tab and Downloads, no text written in the code", async () => {
    usePseudoLanguage();
    const { app } = renderHub(
      "/hub/images?showUnsupported=true",
      ({ backend }) => {
        backend.images.machine.missingCpuFeatures = ["AVX2"];
        backend.images.machine.engineAvailable = false;
        backend.images.machine.vramBytes = 8 * GIB;
        backend.images.install(KLEIN_Q8_ID);
        backend.images.select(KLEIN_Q8_ID);
        backend.images.measured.set(KLEIN_Q8_ID, 200);
        backend.images.downloads.push(
          download(Z_IMAGE_ID, { completedBytes: 10 * MIB }),
          download(KLEIN_ID, { state: "failed", error: "Network error" }),
        );
      },
    );
    await screen.findAllByTestId("image-model");
    await waitFor(() => expect(repoCards()).toHaveLength(6));
    const root = () => screen.getByTestId("hub-scroll").parentElement!;
    const links = () =>
      [...root().querySelectorAll("a")].map(
        (link) => link.getAttribute("href") ?? "",
      );
    expect(untranslatedTexts(root(), [...DATA, ...links()])).toEqual([]);

    await act(() =>
      app.router.navigate({ to: "/hub/images", search: { q: "mystery" } }),
    );
    await screen.findByTestId("image-hidden");
    expect(untranslatedTexts(root(), [...DATA, ...links()])).toEqual([]);

    await act(() => app.router.navigate({ to: "/hub/downloads" }));
    await screen.findAllByTestId("image-download");
    expect(untranslatedTexts(root(), [...DATA, ...links()])).toEqual([]);
  });

  it("in French: sections, steps, cues, machine and buttons", async () => {
    renderHub("/hub/images", ({ backend }) => {
      backend.images.machine.gpu = false;
    });
    await screen.findAllByTestId("image-model");
    act(() => setLanguage("fr"));
    expect(
      screen.getByRole("heading", { name: "Modèles d'images" }),
    ).toBeTruthy();
    expect(screen.getByRole("link", { name: "Images" })).toBeTruthy();
    expect(
      within(screen.getByRole("list", { name: "Comment ça marche" }))
        .getAllByRole("listitem")
        .map((step) => step.textContent),
    ).toEqual([
      "1Téléchargez un modèle",
      "2Cliquez sur Utiliser",
      "3Dans une conversation, appuyez sur le bouton Image (ou demandez une image au modèle)",
    ]);
    expect(screen.getByTestId("image-machine").textContent).toBe(
      "Aucune carte graphique trouvée\u00a0: les images seront faites par le processeur, ce qui prend plusieurs minutes pour chacune.",
    );
    const klein = card(KLEIN);
    expect(fact(klein, "size").textContent).toBe("4,8\u00a0Go à télécharger");
    expect(fact(klein, "usage").textContent).toBe("Usage libre");
    expect(fact(klein, "speed").textContent).toBe(
      "Plusieurs minutes par image (processeur)",
    );
    expect(fact(klein, "memory").textContent).toBe("7,9\u00a0Go de VRAM");
    expect(within(klein).getByTestId("image-blurb").textContent).toBe(
      "Rapide et polyvalent, doué pour le texte dans les images. Un bon premier choix.",
    );
    expect(
      within(klein).getByRole("button", { name: "Télécharger" }),
    ).toBeTruthy();
    expect(
      screen.getByRole("heading", { name: "Compatibles sur Hugging Face" }),
    ).toBeTruthy();
    expect((await screen.findByTestId("image-hidden")).textContent).toBe(
      "2 modèles non pris en charge ou restreints masqués·Afficher",
    );
  });
});
