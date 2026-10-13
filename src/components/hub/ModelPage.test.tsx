// Fiche d'un dépôt face au simulateur : faits, versions (conseillée cochée,
// indisponibles grisées), téléchargement, progression, Installed, Use in
// chat, annulation, échec, dépôt restreint ou introuvable, README.

import {
  act,
  configure,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import type { Settings } from "@/bindings/Settings";
import { hubInstalledKey } from "@/hooks/useHub";
import { modelsKey } from "@/hooks/useModels";
import { settingsKey } from "@/hooks/useSettings";
import { SERVER_URL } from "@/test/backend";
import { GIB } from "@/test/hub";
import { renderHub } from "@/test/renderHub";

configure({ asyncUtilTimeout: 5000 });
vi.setConfig({ testTimeout: 30_000 });

const QWEN = "/hub/model/unsloth/Qwen3.5-9B-GGUF";
const Q8 = "hf.co/unsloth/Qwen3.5-9B-GGUF:Q8_0";
const Q4 = "hf.co/unsloth/Qwen3.5-9B-GGUF:Q4_K_M";

async function page() {
  await screen.findByRole("heading", { name: "Choose a version" });
}

const radios = () => screen.getAllByRole("radio") as HTMLInputElement[];
const checked = () => radios().find((radio) => radio.checked)?.value;
const ollamaName = () => screen.getByTestId("ollama-name").textContent;
const downloadButton = () =>
  screen.getByRole("button", { name: /^Download · / });

describe("Model page", () => {
  it("shows the header, the facts and the announced capabilities", async () => {
    renderHub(QWEN);
    await page();
    expect(screen.getByRole("heading", { level: 2 }).textContent).toBe(
      "Qwen3.5-9B",
    );
    expect(screen.getByText("Verified")).toBeTruthy();
    const links = screen.getAllByRole("link", { name: /View on Hugging Face/ });
    expect(links[0].getAttribute("href")).toBe(
      "https://huggingface.co/unsloth/Qwen3.5-9B-GGUF",
    );
    const facts = Object.fromEntries(
      [...document.querySelectorAll("dl > div")].map((item) => [
        item.querySelector("dt")?.textContent,
        item.querySelector("dd")?.textContent,
      ]),
    );
    expect(facts).toEqual({
      Parameters: "9.7B",
      Context: "256K tokens",
      Architecture: "qwen35",
      License: "apache-2.0",
      Downloads: "1.9M",
      Updated: expect.any(String),
      Languages: "English, Chinese, French +3",
    });
    const capabilities = screen.getByRole("list", { name: "Capabilities" });
    expect(
      within(capabilities)
        .getAllByRole("listitem")
        .map((item) => item.textContent),
    ).toEqual(["Tools", "Reasoning", "Vision"]);
    expect(
      screen.getByText("Announced by the model. Checked after download."),
    ).toBeTruthy();
  });

  it("checks the recommended version; unavailable ones are greyed with their reason", async () => {
    renderHub(QWEN);
    await page();
    // Pastilles affichées une fois la machine lue.
    await screen.findAllByText("Fits your GPU");
    expect(radios()).toHaveLength(6);
    expect(checked()).toBe("Q8_0");
    const bf16 = radios().find((radio) => radio.value === "BF16")!;
    expect(bf16.disabled).toBe(true);
    expect(bf16.closest("label")?.textContent).toContain(
      "Stored in a subfolder — not supported by Ollama yet",
    );
    const q8 = radios().find((radio) => radio.value === "Q8_0")!;
    expect(q8.closest("label")?.textContent).toContain("Recommended");
    expect(q8.closest("label")?.textContent).toContain("High quality");
    expect(q8.closest("label")?.textContent).toContain("Fits your GPU");
    // Les deux poids de chaque version, dits à part ; la mémoire précise le
    // contexte compté.
    const weights = (radio: HTMLInputElement) =>
      within(radio.closest("label")!).getByTestId("footprint");
    expect(weights(q8).textContent).toBe("Download 10 GB·VRAM 16 GB");
    expect(
      weights(q8).querySelectorAll("[title]")[1].getAttribute("title"),
    ).toMatch(/^Needs about 16 GB of memory with a \d+K context\.$/);
    expect(
      weights(q8).querySelectorAll("[title]")[0].getAttribute("title"),
    ).toBe(
      "Size of the files to download, and the space they take on the disk.",
    );
    // Version indisponible : sa taille seule.
    expect(weights(bf16).textContent).toMatch(/^Download \d+ GB$/);
    expect(downloadButton().textContent).toBe("Download · 10 GB");
    expect(ollamaName()).toBe(Q8);

    await userEvent.click(
      screen.getByRole("button", { name: "Show all 9 versions" }),
    );
    expect(radios().map((radio) => radio.value)).toEqual([
      "BF16",
      "Q8_0",
      "Q6_K",
      "Q5_K_M",
      "UD-Q4_K_XL",
      "Q4_K_M",
      "IQ4_XS",
      "Q3_K_M",
      "Q2_K",
    ]);
    await userEvent.click(radios().find((radio) => radio.value === "Q4_K_M")!);
    expect(checked()).toBe("Q4_K_M");
    expect(downloadButton().textContent).toBe("Download · 6.3 GB");
    expect(ollamaName()).toBe(Q4);
    // Une version choisie hors de la fenêtre reste visible une fois repliée.
    await userEvent.click(radios().find((radio) => radio.value === "Q2_K")!);
    await userEvent.click(
      screen.getByRole("button", { name: "Show fewer versions" }),
    );
    expect(radios()).toHaveLength(6);
    expect(checked()).toBe("Q2_K");
    expect(
      screen.getByRole("button", { name: "Copy Ollama name" }),
    ).toBeTruthy();
  });

  it("moves between versions with the arrow keys, skipping unavailable ones", async () => {
    renderHub("/hub/model/bartowski/Llama-3.3-70B-Instruct-GGUF");
    await page();
    // 70B : BF16 et Q8_0 découpés ; rien ne tient en entier, Q4_K_M conseillé.
    expect(checked()).toBe("Q4_K_M");
    await userEvent.click(
      screen.getByRole("button", { name: /^Show all \d+ versions$/ }),
    );
    for (const value of ["BF16", "Q8_0"]) {
      const radio = radios().find((r) => r.value === value)!;
      expect(radio.disabled).toBe(true);
      expect(radio.closest("label")?.textContent).toContain(
        "Split into several files — not supported by Ollama yet",
      );
    }
    await screen.findAllByText("Too large");
    const q4 = radios().find((radio) => radio.value === "Q4_K_M")!;
    expect(q4.closest("label")?.textContent).toContain("Too large");
    q4.focus();
    await userEvent.keyboard("{ArrowDown}");
    expect(checked()).toBe("IQ4_XS");
    await userEvent.keyboard("{ArrowUp}{ArrowUp}{ArrowUp}");
    // Q6_K, puis plus haut seulement des versions indisponibles.
    expect(checked()).not.toBe("Q8_0");
    expect(checked()).not.toBe("BF16");
  });

  it("downloads, shows the progress, then Installed and Use in chat", async () => {
    const { simulator, hub, app } = renderHub(QWEN);
    await page();
    await userEvent.click(downloadButton());
    expect(simulator.callsTo("hub_download")).toEqual([
      {
        request: { model: Q8, repo: "unsloth/Qwen3.5-9B-GGUF", label: "Q8_0" },
      },
    ]);
    await screen.findByText("Starting…");
    expect(screen.getByTestId("active-downloads").textContent).toBe("1");

    await hub.progress(Q8, 2.1 * GIB, {
      total: 5 * GIB,
      bytesPerSecond: 48 * 1024 ** 2,
    });
    // React Query prévient ses abonnés après un setTimeout(0) : findBy.
    expect(
      await screen.findByText("2.1 GB of 5.0 GB · 48 MB/s · 1 min left"),
    ).toBeTruthy();
    const bar = screen.getByRole("progressbar", { name: "Downloading Q8_0" });
    expect(bar.getAttribute("aria-valuenow")).toBe("42");
    expect(bar.getAttribute("aria-valuetext")).toBe(
      "2.1 GB of 5.0 GB · 48 MB/s · 1 min left",
    );

    await hub.setState(Q8, "finishing");
    expect(await screen.findByText("Finishing…")).toBeTruthy();

    app.queryClient.setQueryData(modelsKey, []);
    await hub.finish(Q8);
    await screen.findByText("Installed", { selector: "p" });
    // Le sélecteur relira ses modèles.
    expect(app.queryClient.getQueryState(modelsKey)?.isInvalidated).toBe(true);
    expect(screen.queryByTestId("active-downloads")).toBeNull();
    await userEvent.click(screen.getByRole("button", { name: "Use in chat" }));
    await waitFor(() => expect(app.path()).toBe("/c/new"));
    expect(simulator.callsTo("update_settings").at(-1)).toEqual({
      patch: { selectedModel: Q8 },
    });
  });

  it("an installed version shows Installed at once (name compared without case)", async () => {
    renderHub(QWEN, ({ backend }) => {
      backend.models.unshift(Q4.toLowerCase());
    });
    await page();
    const q4 = radios().find((radio) => radio.value === "Q4_K_M")!;
    await waitFor(() =>
      expect(
        within(q4.closest("label")!).queryByRole("img", { name: "Installed" }),
      ).toBeTruthy(),
    );
    await userEvent.click(q4);
    expect(screen.getByText("Installed", { selector: "p" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "Use in chat" })).toBeTruthy();
    expect(screen.queryByRole("button", { name: /^Download/ })).toBeNull();
  });

  it("a version downloaded then deleted offers Download, even while the installed models are read", async () => {
    const { simulator, app } = renderHub(QWEN, ({ hub }) => {
      hub.downloads.push({
        model: Q8,
        repo: "unsloth/Qwen3.5-9B-GGUF",
        label: "Q8_0",
        state: "done",
        completed: GIB,
        total: GIB,
        bytesPerSecond: null,
        error: null,
        startedAt: 1,
        finishedAt: 100,
      });
    });
    await page();
    await waitFor(() => expect(downloadButton()).toBeTruthy());
    // Relecture des installés (retour de focus...) qui ne finit pas.
    simulator.handle("hub_installed", () => new Promise(() => {}));
    void app.queryClient.invalidateQueries({ queryKey: hubInstalledKey });
    await act(() => new Promise((resolve) => setTimeout(resolve, 20)));
    expect(
      app.queryClient.getQueryState([...hubInstalledKey, SERVER_URL])
        ?.fetchStatus,
    ).toBe("fetching");
    expect(downloadButton()).toBeTruthy();
    expect(screen.queryByText("Installed", { selector: "p" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Use in chat" })).toBeNull();
  });

  it("a version just downloaded is Installed before the installed models are read again", async () => {
    const { simulator, hub } = renderHub(QWEN);
    await page();
    await userEvent.click(downloadButton());
    await screen.findByText("Starting…");
    simulator.handle("hub_installed", () => new Promise(() => {}));
    await hub.finish(Q8);
    await screen.findByText("Installed", { selector: "p" });
    expect(screen.getByRole("button", { name: "Use in chat" })).toBeTruthy();
  });

  it("Cancel stops the download", async () => {
    const { simulator, hub } = renderHub(QWEN);
    await page();
    await userEvent.click(downloadButton());
    await hub.progress(Q8, GIB, { total: 10 * GIB });
    await userEvent.click(
      await screen.findByRole("button", { name: "Cancel" }),
    );
    expect(simulator.callsTo("hub_cancel_download")).toEqual([{ model: Q8 }]);
    expect(
      await screen.findByRole("button", { name: /^Download · / }),
    ).toBeTruthy();
    expect(screen.queryByRole("progressbar")).toBeNull();
  });

  it("a download paused elsewhere shows where it stopped, Resume and Cancel", async () => {
    const { simulator, hub } = renderHub(QWEN);
    await page();
    await userEvent.click(downloadButton());
    await hub.progress(Q8, 4 * GIB, { total: 10 * GIB });
    // Mis en pause depuis l'onglet Downloads.
    await hub.setState(Q8, "paused");
    const bar = await screen.findByRole("progressbar");
    await waitFor(() =>
      expect(bar.getAttribute("aria-valuetext")).toBe(
        "Paused · 4.0 GB of 10 GB",
      ),
    );
    expect(bar.getAttribute("aria-valuenow")).toBe("40");
    expect(screen.queryByRole("button", { name: /^Download · / })).toBeNull();
    expect(screen.getByRole("button", { name: "Cancel" })).toBeTruthy();
    await userEvent.click(screen.getByRole("button", { name: "Resume" }));
    expect(simulator.callsTo("hub_resume_download")).toEqual([{ model: Q8 }]);
    await waitFor(() =>
      expect(screen.queryByRole("button", { name: "Resume" })).toBeNull(),
    );
    expect(screen.getByRole("progressbar")).toBeTruthy();
  });

  it("a failure shows the message, Try again downloads again", async () => {
    const { simulator, hub } = renderHub(QWEN);
    await page();
    await userEvent.click(downloadButton());
    await screen.findByText("Starting…");
    const message =
      "This version isn't available on Hugging Face. Choose another version.";
    await hub.fail(Q8, message);
    expect((await screen.findByRole("alert")).textContent).toBe(message);
    await userEvent.click(screen.getByRole("button", { name: "Try again" }));
    await screen.findByText("Starting…");
    expect(simulator.callsTo("hub_download")).toHaveLength(2);
  });

  it("a refused download shows the backend's error", async () => {
    renderHub(QWEN, ({ hub }) => {
      hub.failures.set("hub_download", {
        kind: "invalid",
        message: "invalid model name",
      });
    });
    await page();
    await userEvent.click(downloadButton());
    expect((await screen.findByRole("alert")).textContent).toBe(
      "invalid model name",
    );
  });

  it("a restricted repository has no download button", async () => {
    renderHub("/hub/model/google/gemma-3-27b-it-qat-q4_0-gguf");
    await page();
    expect(
      screen.getByText(
        "This model needs a Hugging Face account. ChatonLM can't download it.",
      ),
    ).toBeTruthy();
    expect(screen.queryByRole("radio")).toBeNull();
    expect(screen.queryByRole("button", { name: /^Download/ })).toBeNull();
  });

  it("an unknown repository: not found, no Retry, a link to Hugging Face", async () => {
    renderHub("/hub/model/nobody/nothing-GGUF");
    expect(
      await screen.findByText(
        "This model doesn't exist on Hugging Face, or it's private.",
      ),
    ).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Retry" })).toBeNull();
    expect(
      screen
        .getByRole("link", { name: /View on Hugging Face/ })
        .getAttribute("href"),
    ).toBe("https://huggingface.co/nobody/nothing-GGUF");
  });

  it("Hugging Face says the repository is restricted", async () => {
    renderHub(QWEN, ({ hub }) => {
      hub.problems.hub_repo = { kind: "restricted" };
      hub.cached = false;
    });
    expect(
      await screen.findByText(
        "This model needs a Hugging Face account. ChatonLM can't download it.",
      ),
    ).toBeTruthy();
  });

  it("offline: saved page with a banner, or You're offline", async () => {
    renderHub(QWEN, ({ hub }) => {
      hub.problems.hub_repo = { kind: "offline" };
    });
    await page();
    expect(screen.getByTestId("hub-banner").textContent).toMatch(
      /^You're offline\. Showing models saved/,
    );
  });

  it("offline with nothing saved: Retry", async () => {
    const { simulator, hub } = renderHub(QWEN, ({ hub }) => {
      hub.problems.hub_repo = { kind: "offline" };
      hub.cached = false;
    });
    expect(
      await screen.findByRole("heading", { name: "You're offline" }),
    ).toBeTruthy();
    hub.problems = {};
    await userEvent.click(screen.getByRole("button", { name: "Retry" }));
    await page();
    expect(simulator.callsTo("hub_repo")).toHaveLength(2);
  });

  it("Back returns to the filtered list it came from", async () => {
    const { app } = renderHub("/hub");
    await screen.findByRole("heading", { name: "Trending" });
    await userEvent.click(screen.getByRole("button", { name: "Code" }));
    await waitFor(() =>
      expect(app.router.state.location.search).toEqual({ uses: "code" }),
    );
    await userEvent.click(screen.getAllByTestId("model-card")[0]);
    await page();
    await userEvent.click(screen.getByRole("button", { name: "Back" }));
    await waitFor(() => expect(app.path()).toBe("/hub"));
    expect(app.router.state.location.search).toEqual({ uses: "code" });
    await screen.findByRole("heading", { name: "Trending" });
    expect(
      screen.getByRole("button", { name: "Code" }).getAttribute("aria-pressed"),
    ).toBe("true");
  });

  it("opened first, it leads back to Discover", async () => {
    const { app } = renderHub(QWEN);
    await page();
    expect(screen.queryByRole("button", { name: "Back" })).toBeNull();
    const discover = within(screen.getByTestId("hub-scroll")).getByRole(
      "link",
      { name: "Discover" },
    );
    expect(discover.getAttribute("href")).toBe("/hub");
    await userEvent.click(discover);
    await waitFor(() => expect(app.path()).toBe("/hub"));
  });

  it("reads the page again when the context length changes (memory estimates)", async () => {
    const { simulator, app } = renderHub(QWEN);
    await page();
    expect(simulator.callsTo("hub_repo")).toHaveLength(1);
    const settings = app.queryClient.getQueryData<Settings>(settingsKey)!;
    act(() =>
      app.queryClient.setQueryData(settingsKey, {
        ...settings,
        contextLength: 4096,
      }),
    );
    await waitFor(() => expect(simulator.callsTo("hub_repo")).toHaveLength(2));
  });

  it("other sources open their own page", async () => {
    const { app } = renderHub(QWEN);
    await page();
    await userEvent.click(
      screen.getByRole("link", { name: "Qwen/Qwen3-8B-GGUF" }),
    );
    await waitFor(() =>
      expect(app.path()).toBe("/hub/model/Qwen/Qwen3-8B-GGUF"),
    );
    await waitFor(() =>
      expect(screen.getByRole("heading", { level: 2 }).textContent).toBe(
        "Qwen3-8B",
      ),
    );
    expect(ollamaName()).toBe("hf.co/Qwen/Qwen3-8B-GGUF:Q8_0");
  });
});

describe("Model page: README", () => {
  const README = [
    "# Qwen3.5",
    "",
    "Read the [paper](https://arxiv.org/abs/1) or the [license](LICENSE).",
    "",
    "![Benchmarks](https://github.com/qwen/raw/main/bench.png)",
    "",
    '<script>alert("x")</script>',
    "",
    "The model thinks in <think>tags</think>.",
    "",
    ...Array.from({ length: 80 }, (_, i) => `Paragraph ${i} with some words.`),
  ].join("\n\n");

  it("renders the model card safely; links open in the browser", async () => {
    const { simulator } = renderHub(QWEN, ({ hub }) => {
      hub.readmes.set("unsloth/Qwen3.5-9B-GGUF", README);
    });
    simulator.handle("open_external", () => null);
    const readme = await screen.findByTestId("readme");
    expect(simulator.callsTo("hub_readme")).toEqual([
      {
        id: "unsloth/Qwen3.5-9B-GGUF",
        revision: "sha-unsloth/Qwen3.5-9B-GGUF",
      },
    ]);
    // Sous le titre de la page (h1), du modèle (h2) et de About (h3).
    expect(
      within(readme).getByRole("heading", { name: "Qwen3.5", level: 4 }),
    ).toBeTruthy();
    expect(readme.querySelector("script, think, img")).toBeNull();
    expect(readme.textContent).toContain('<script>alert("x")</script>');
    expect(readme.textContent).toContain("<think>tags</think>");
    const chip = within(readme).getByRole("link", {
      name: /Image: Benchmarks/,
    });
    expect(chip.textContent).toBe("Image: Benchmarksgithub.com");
    const license = within(readme).getByRole("link", { name: "license" });
    expect(license.getAttribute("href")).toBe(
      "https://huggingface.co/unsloth/Qwen3.5-9B-GGUF/blob/main/LICENSE",
    );
    await userEvent.click(within(readme).getByRole("link", { name: "paper" }));
    await waitFor(() =>
      expect(simulator.callsTo("open_external")).toEqual([
        { url: "https://arxiv.org/abs/1" },
      ]),
    );
    // Long : replié derrière Show more.
    expect(readme.className).toContain("max-h-[36rem]");
    const more = screen.getByRole("button", { name: "Show more" });
    await userEvent.click(more);
    expect(more.getAttribute("aria-expanded")).toBe("true");
    expect(readme.className).not.toContain("max-h-[36rem]");
    expect(more.textContent).toBe("Show less");
  });

  it("a model card that couldn't be loaded has Retry", async () => {
    const { simulator, hub } = renderHub(QWEN, ({ hub }) => {
      hub.problems.hub_readme = { kind: "offline" };
      hub.cached = false;
    });
    expect(
      await screen.findByText(/The model card couldn't be loaded\./),
    ).toBeTruthy();
    hub.problems = {};
    await userEvent.click(screen.getByRole("button", { name: "Retry" }));
    expect(await screen.findByTestId("readme")).toBeTruthy();
    expect(simulator.callsTo("hub_readme")).toHaveLength(2);
  });

  it("a model card that couldn't be loaded is read again when the page opens again", async () => {
    const { simulator, hub, app } = renderHub(QWEN, ({ hub }) => {
      hub.problems.hub_readme = { kind: "rateLimited", retryAfter: 30 };
      hub.cached = false;
    });
    await screen.findByText(/The model card couldn't be loaded\./);
    await app.router.navigate({ to: "/hub" });
    await screen.findByRole("heading", { name: "Trending" });
    hub.problems = {};
    await app.router.navigate({
      to: "/hub/model/$author/$repo",
      params: { author: "unsloth", repo: "Qwen3.5-9B-GGUF" },
    });
    expect(await screen.findByTestId("readme")).toBeTruthy();
    expect(simulator.callsTo("hub_readme")).toHaveLength(2);
  });

  it("a model card read is kept when the page opens again", async () => {
    const { simulator, app } = renderHub(QWEN);
    await screen.findByTestId("readme");
    await app.router.navigate({ to: "/hub" });
    await screen.findByRole("heading", { name: "Trending" });
    await app.router.navigate({
      to: "/hub/model/$author/$repo",
      params: { author: "unsloth", repo: "Qwen3.5-9B-GGUF" },
    });
    await screen.findByTestId("readme");
    expect(simulator.callsTo("hub_readme")).toHaveLength(1);
  });

  it("the banner's Retry reads a failed model card again", async () => {
    let failing = true;
    const { simulator, hub } = renderHub(QWEN, ({ hub }) => {
      hub.problems.hub_repo = { kind: "offline" };
    });
    simulator.handle("hub_readme", () =>
      failing
        ? { text: null, problem: { kind: "offline" } }
        : { text: "# Card\n\nBack online.", problem: null },
    );
    const banner = await screen.findByTestId("hub-banner");
    await screen.findByText(/The model card couldn't be loaded\./);
    failing = false;
    hub.problems = {};
    await userEvent.click(
      within(banner).getByRole("button", { name: "Retry" }),
    );
    expect((await screen.findByTestId("readme")).textContent).toContain(
      "Back online.",
    );
    expect(simulator.callsTo("hub_readme")).toHaveLength(2);
  });

  it("an image inside a link opens the link, in a single link", async () => {
    const { simulator } = renderHub(QWEN, ({ hub }) => {
      hub.readmes.set(
        "unsloth/Qwen3.5-9B-GGUF",
        'Join us: <a href="https://discord.gg/unsloth"><img src="https://github.com/unslothai/unsloth/raw/main/images/Discord%20button.png"></a>',
      );
    });
    simulator.handle("open_external", () => null);
    const readme = await screen.findByTestId("readme");
    expect(readme.querySelectorAll("a")).toHaveLength(1);
    expect(readme.querySelector("a a")).toBeNull();
    const link = within(readme).getByRole("link", {
      name: /Image: Discord button\.png/,
    });
    expect(link.getAttribute("href")).toBe("https://discord.gg/unsloth");
    await userEvent.click(within(link).getByText(/Image: Discord/));
    await waitFor(() =>
      expect(simulator.callsTo("open_external")).toEqual([
        { url: "https://discord.gg/unsloth" },
      ]),
    );
  });

  it("a folded model card unfolds when the keyboard enters it", async () => {
    renderHub(QWEN, ({ hub }) => {
      hub.readmes.set("unsloth/Qwen3.5-9B-GGUF", README);
    });
    const readme = await screen.findByTestId("readme");
    const more = screen.getByRole("button", { name: "Show more" });
    expect(more.getAttribute("aria-expanded")).toBe("false");
    // Le dernier élément atteignable au clavier avant le README, puis Tab.
    const first = within(readme).getAllByRole("link")[0];
    const focusable = [
      ...document.querySelectorAll<HTMLElement>(
        "a[href], button:not([disabled]), input:checked",
      ),
    ];
    act(() => focusable[focusable.indexOf(first) - 1].focus());
    await userEvent.tab();
    expect(document.activeElement).toBe(first);
    expect(more.getAttribute("aria-expanded")).toBe("true");
    expect(readme.className).not.toContain("max-h-[36rem]");
  });

  it("without a README: a link to Hugging Face", async () => {
    renderHub(QWEN, ({ hub }) => {
      hub.readmes.set("unsloth/Qwen3.5-9B-GGUF", null);
    });
    expect(
      await screen.findByText(/This model has no model card\./),
    ).toBeTruthy();
  });
});
