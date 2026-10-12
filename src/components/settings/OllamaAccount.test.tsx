// Carte « Ollama account » des réglages face au simulateur : non relié,
// relié, Sign In et relecture du compte, Sign out, erreurs, liens.

import {
  act,
  configure,
  fireEvent,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { AccountStatus } from "@/bindings/AccountStatus";
import {
  accountKey,
  accountQueryOptions,
  SIGN_IN_POLL_MS,
} from "@/hooks/useAccount";
import { setLanguage } from "@/i18n";
import { resetLinks } from "@/lib/openLink";
import { type FakeBackend, fakeBackend } from "@/test/backend";
import { untranslatedTexts, usePseudoLanguage } from "@/test/i18n";
import { renderApp } from "@/test/renderApp";
import { installSimulator } from "@/test/simulator";

configure({ asyncUtilTimeout: 5000 });

afterEach(() => {
  resetLinks();
  vi.useRealTimers();
});

const ADA: AccountStatus = {
  state: "signedIn",
  name: "ada",
  email: "ada@example.com",
  plan: "free",
  avatar: "data:image/png;base64,iVBORw0KGgo=",
};

/** Réglages affichés, après `prepare` du faux backend. */
function installAndRender(
  prepare: (backend: FakeBackend) => void,
  path = "/settings?section=account",
) {
  const simulator = installSimulator();
  const backend = fakeBackend(simulator);
  prepare(backend);
  simulator.handle("open_external", () => null);
  const app = renderApp(path);
  return { simulator, backend, app };
}

function setup(account: AccountStatus = { state: "signedOut" }) {
  return installAndRender((backend) => {
    backend.account = account;
  });
}

const card = () => screen.findByTestId("ollama-account");
const reads = (simulator: ReturnType<typeof installSimulator>) =>
  simulator.callsTo("account_status").length;

/** Laisse passer les réponses de l'IPC simulé. */
const settle = () =>
  act(() => new Promise((resolve) => setTimeout(resolve, 30)));

describe("Settings: Ollama account", () => {
  it("not connected: the Account tab, with Sign In", async () => {
    setup();
    const account = await card();
    await within(account).findByText("Not connected");
    expect(within(account).getByText("Ollama account")).toBeTruthy();
    expect(
      within(account).getByRole("button", { name: "Sign In" }).textContent,
    ).toBe("Sign In");
    // Seule sur son onglet : la carte du serveur est dans General.
    expect(
      screen.getByTestId("settings-tab-account").getAttribute("aria-current"),
    ).toBe("page");
    expect(screen.queryByText("Server address")).toBeNull();
  });

  it("shows a skeleton until the account is read", async () => {
    const { simulator } = setup();
    let answer!: (account: AccountStatus) => void;
    simulator.handle(
      "account_status",
      () => new Promise<AccountStatus>((resolve) => (answer = resolve)),
    );
    const account = await card();
    expect(within(account).getByTestId("account-loading")).toBeTruthy();
    expect(within(account).queryByText("Not connected")).toBeNull();
    // Lu une fois l'état du nuage connu.
    await waitFor(() => expect(reads(simulator)).toBe(1));
    expect(within(account).getByTestId("account-loading")).toBeTruthy();
    await act(async () => answer(ADA));
    await within(account).findByText("ada");
    expect(within(account).queryByTestId("account-loading")).toBeNull();
  });

  it("signed in: name, e-mail, Upgrade for the free plan, Manage, Sign out, avatar", async () => {
    const { simulator } = setup(ADA);
    const account = await card();
    await within(account).findByText("ada");
    expect(within(account).getByText("ada@example.com")).toBeTruthy();
    const avatar = within(account).getByRole("img", { name: "ada" });
    expect(avatar.getAttribute("src")).toBe(ADA.avatar);
    expect(avatar.className).toContain("h-10 w-10");
    expect(avatar.className).toContain("rounded-full");

    await userEvent.click(
      within(account).getByRole("button", { name: "Upgrade" }),
    );
    await userEvent.click(
      within(account).getByRole("button", { name: "Manage" }),
    );
    await waitFor(() =>
      expect(simulator.callsTo("open_external")).toEqual([
        { url: "https://ollama.com/upgrade" },
        { url: "https://ollama.com/settings" },
      ]),
    );
    expect(within(account).getByRole("button", { name: "Sign out" }));

    // Image illisible : cachée.
    fireEvent.error(avatar);
    expect(within(account).queryByRole("img")).toBeNull();
  });

  it("no Upgrade for a paid plan, no avatar without one", async () => {
    setup({ ...ADA, plan: "pro", avatar: null });
    const account = await card();
    await within(account).findByText("ada");
    expect(within(account).queryByRole("button", { name: "Upgrade" })).toBe(
      null,
    );
    expect(within(account).getByRole("button", { name: "Manage" }));
    expect(within(account).queryByRole("img")).toBeNull();
  });

  it("a link that can't be opened shows the link notice", async () => {
    const { simulator } = setup(ADA);
    simulator.handle("open_external", () => {
      throw { kind: "internal", message: "xdg-open not found" };
    });
    const account = await card();
    await userEvent.click(
      await within(account).findByRole("button", { name: "Manage" }),
    );
    expect((await screen.findByRole("alert")).textContent).toContain(
      "https://ollama.com/settings",
    );
  });

  it("Sign In opens the page, then reads the account every 5 s until it is signed in", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const { simulator, backend } = setup();
    const account = await card();
    const signIn = await within(account).findByRole("button", {
      name: "Sign In",
    });
    await within(account).findByText("Not connected");
    await userEvent.click(signIn);
    await waitFor(() =>
      expect(simulator.callsTo("account_sign_in")).toHaveLength(1),
    );
    // Points animés, bouton inactif pendant l'attente ; une relecture tout
    // de suite.
    await waitFor(() => expect(signIn.textContent).toBe("..."));
    expect((signIn as HTMLButtonElement).disabled).toBe(true);
    await waitFor(() => expect(reads(simulator)).toBe(2));
    const before = reads(simulator);

    await act(() => vi.advanceTimersByTimeAsync(SIGN_IN_POLL_MS));
    await waitFor(() => expect(reads(simulator)).toBe(before + 1));
    await act(() => vi.advanceTimersByTimeAsync(SIGN_IN_POLL_MS));
    await waitFor(() => expect(reads(simulator)).toBe(before + 2));

    // Relié dans le navigateur : la relecture suivante le montre, puis plus
    // aucune.
    backend.account = ADA;
    await act(() => vi.advanceTimersByTimeAsync(SIGN_IN_POLL_MS));
    await within(account).findByText("ada");
    const after = reads(simulator);
    await act(() => vi.advanceTimersByTimeAsync(3 * SIGN_IN_POLL_MS));
    expect(reads(simulator)).toBe(after);
  });

  it("back on the window, the waiting stops after one last read", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const { simulator } = setup();
    const account = await card();
    const signIn = await within(account).findByRole("button", {
      name: "Sign In",
    });
    await within(account).findByText("Not connected");
    await userEvent.click(signIn);
    await waitFor(() => expect(signIn.textContent).toBe("..."));
    // Relecture immédiate faite (toujours pas relié) : l'attente continue.
    await waitFor(() => expect(reads(simulator)).toBe(2));
    const before = reads(simulator);

    act(() => {
      window.dispatchEvent(new Event("focus"));
    });
    await waitFor(() => expect(reads(simulator)).toBe(before + 1));
    // Toujours pas relié : de nouveau Sign In, plus de relecture.
    await waitFor(() => expect(signIn.textContent).toBe("Sign In"));
    expect((signIn as HTMLButtonElement).disabled).toBe(false);
    await act(() => vi.advanceTimersByTimeAsync(3 * SIGN_IN_POLL_MS));
    expect(reads(simulator)).toBe(before + 1);
  });

  it("shows why signing in failed, or the official text", async () => {
    const { simulator, backend } = setup();
    backend.accountErrors.signIn = {
      kind: "internal",
      message: "Failed to fetch connect URL",
    };
    const account = await card();
    const signIn = await within(account).findByRole("button", {
      name: "Sign In",
    });
    await userEvent.click(signIn);
    expect((await within(account).findByRole("alert")).textContent).toBe(
      "Failed to fetch connect URL",
    );
    // Pas d'attente : pas de relecture.
    expect(signIn.textContent).toBe("Sign In");

    simulator.handle("account_sign_in", () => {
      throw "boom";
    });
    await userEvent.click(signIn);
    await waitFor(() =>
      expect(within(account).getByRole("alert").textContent).toBe(
        "Failed to connect to Ollama account",
      ),
    );
  });

  it("Sign out shows the account signed out at once, then reads it again", async () => {
    const { simulator, backend } = setup(ADA);
    let answer!: () => void;
    simulator.handle(
      "account_sign_out",
      () =>
        new Promise<null>((resolve) => {
          answer = () => {
            backend.account = { state: "signedOut" };
            resolve(null);
          };
        }),
    );
    const account = await card();
    await userEvent.click(
      await within(account).findByRole("button", { name: "Sign out" }),
    );
    await within(account).findByText("Not connected");
    const before = reads(simulator);
    await act(async () => answer());
    await waitFor(() => expect(reads(simulator)).toBe(before + 1));
    expect(within(account).getByText("Not connected")).toBeTruthy();
    expect(within(account).queryByRole("alert")).toBeNull();
  });

  it("a failed sign-out keeps the account and says so", async () => {
    const { backend } = setup(ADA);
    backend.accountErrors.signOut = { kind: "ollama", message: "status 500" };
    const account = await card();
    await userEvent.click(
      await within(account).findByRole("button", { name: "Sign out" }),
    );
    expect((await within(account).findByRole("alert")).textContent).toBe(
      "Failed to disconnect Ollama account",
    );
    await settle();
    expect(within(account).getByText("ada")).toBeTruthy();
  });

  it("Sign In on a server already signed in (terminal) shows the account at once", async () => {
    const { simulator, backend } = setup();
    const account = await card();
    await within(account).findByText("Not connected");
    // Relié au terminal : le cache « non relié » est périmé, aucune page ne
    // s'ouvre.
    backend.account = ADA;
    await userEvent.click(
      within(account).getByRole("button", { name: "Sign In" }),
    );
    await within(account).findByText("ada");
    expect(simulator.callsTo("account_sign_in")).toHaveLength(1);
  });

  it("Sign In is disabled while the page is being opened: one page only", async () => {
    const { simulator } = setup();
    let open!: () => void;
    simulator.handle(
      "account_sign_in",
      () => new Promise((resolve) => (open = () => resolve(null))),
    );
    const account = await card();
    await within(account).findByText("Not connected");
    const signIn = within(account).getByRole<HTMLButtonElement>("button", {
      name: "Sign In",
    });
    fireEvent.click(signIn);
    fireEvent.click(signIn);
    await waitFor(() => expect(signIn.disabled).toBe(true));
    expect(signIn.textContent).toBe("...");
    await userEvent.click(signIn);
    await waitFor(() =>
      expect(simulator.callsTo("account_sign_in")).toHaveLength(1),
    );
    await act(async () => open());
    await waitFor(() => expect(reads(simulator)).toBe(2));
    expect(simulator.callsTo("account_sign_in")).toHaveLength(1);
  });

  it.each([
    [
      "env",
      "The OLLAMA_NO_CLOUD environment variable is currently forcing cloud off.",
    ],
    [
      "both",
      "The OLLAMA_NO_CLOUD environment variable is currently forcing cloud off.",
    ],
    ["config", "Cloud is turned off on the Ollama server."],
  ])(
    "cloud turned off (%s): the account is not read, the card says why",
    async (source, text) => {
      const { simulator } = installAndRender((backend) => {
        backend.account = ADA;
        backend.cloud = { disabled: true, source };
      });
      const account = await card();
      await within(account).findByText(text);
      expect(within(account).getByText("Ollama account")).toBeTruthy();
      expect(within(account).queryByRole("button")).toBeNull();
      await settle();
      expect(simulator.callsTo("cloud_status")).toHaveLength(1);
      expect(reads(simulator)).toBe(0);
    },
  );

  it("while the cloud status is read: skeleton, account not read yet", async () => {
    let release!: () => void;
    const { simulator } = installAndRender((backend) => {
      backend.account = ADA;
    });
    simulator.handle(
      "cloud_status",
      () =>
        new Promise(
          (resolve) =>
            (release = () => resolve({ disabled: false, source: "none" })),
        ),
    );
    const account = await card();
    expect(within(account).getByTestId("account-loading")).toBeTruthy();
    await waitFor(() =>
      expect(simulator.callsTo("cloud_status")).toHaveLength(1),
    );
    await settle();
    expect(reads(simulator)).toBe(0);
    await act(async () => release());
    await within(account).findByText("ada");
  });

  it("cloud status unknown: the account is read as usual", async () => {
    const { simulator } = installAndRender((backend) => {
      backend.account = ADA;
      backend.cloud = null;
    });
    const account = await card();
    await within(account).findByText("ada");
    expect(reads(simulator)).toBe(1);
  });

  describe("opened for the account (section=account)", () => {
    it("Sign In takes the focus once the account is read", async () => {
      installAndRender(() => {}, "/settings?section=account");
      const account = await card();
      const signIn = await within(account).findByRole("button", {
        name: "Sign In",
      });
      await waitFor(() => expect(document.activeElement).toBe(signIn));
    });

    it("cloud turned off: the card takes the focus", async () => {
      const { simulator } = installAndRender((backend) => {
        backend.cloud = { disabled: true, source: "config" };
      }, "/settings?section=account");
      const account = await card();
      await within(account).findByText(
        "Cloud is turned off on the Ollama server.",
      );
      await waitFor(() => expect(document.activeElement).toBe(account));
      expect(simulator.callsTo("account_sign_in")).toEqual([]);
    });

    it("only once: the card read again keeps the focus where it is", async () => {
      const { simulator, app } = installAndRender(
        () => {},
        "/settings?section=account",
      );
      const account = await card();
      const signIn = await within(account).findByRole("button", {
        name: "Sign In",
      });
      await waitFor(() => expect(document.activeElement).toBe(signIn));
      // Ailleurs dans la page : un onglet.
      const elsewhere = screen.getByTestId("settings-tab-general");
      act(() => elsewhere.focus());
      // Compte oublié puis relu (autre serveur...) : squelette, puis la carte.
      let answer!: () => void;
      simulator.handle(
        "account_status",
        () =>
          new Promise<AccountStatus>(
            (resolve) => (answer = () => resolve({ state: "signedOut" })),
          ),
      );
      act(() => {
        void app.queryClient.resetQueries({ queryKey: ["account"] });
      });
      await within(account).findByTestId("account-loading");
      await act(async () => answer());
      await within(account).findByText("Not connected");
      await settle();
      expect(document.activeElement).toBe(elsewhere);
    });

    it("the card itself focused meanwhile (click): Sign In still takes it", async () => {
      const { simulator } = installAndRender(
        () => {},
        "/settings?section=account",
      );
      let answer!: () => void;
      simulator.handle(
        "account_status",
        () =>
          new Promise<AccountStatus>(
            (resolve) => (answer = () => resolve({ state: "signedOut" })),
          ),
      );
      const account = await card();
      await waitFor(() => expect(reads(simulator)).toBe(1));
      act(() => account.focus());
      await act(async () => answer());
      const signIn = await within(account).findByRole("button", {
        name: "Sign In",
      });
      await waitFor(() => expect(document.activeElement).toBe(signIn));
    });

    it("the user already elsewhere (on a tab): the focus is left alone", async () => {
      const { simulator } = installAndRender(
        () => {},
        "/settings?section=account",
      );
      let answer!: () => void;
      simulator.handle(
        "account_status",
        () =>
          new Promise<AccountStatus>(
            (resolve) => (answer = () => resolve({ state: "signedOut" })),
          ),
      );
      const tab = await screen.findByTestId("settings-tab-dictation");
      act(() => tab.focus());
      await waitFor(() => expect(reads(simulator)).toBe(1));
      await act(async () => answer());
      const account = await card();
      await within(account).findByText("Not connected");
      await settle();
      expect(document.activeElement).toBe(tab);
    });

    it("opened from its tab: the focus stays on the tab", async () => {
      installAndRender(() => {}, "/settings");
      const tab = await screen.findByTestId("settings-tab-account");
      await userEvent.click(tab);
      const account = await card();
      await within(account).findByText("Not connected");
      await settle();
      expect(document.activeElement).toBe(tab);
    });

    it("another section: the General tab, without the account", async () => {
      installAndRender(() => {}, "/settings?section=other");
      await screen.findByText("Server address");
      expect(
        screen.getByTestId("settings-tab-general").getAttribute("aria-current"),
      ).toBe("page");
      expect(screen.queryByTestId("ollama-account")).toBeNull();
    });
  });
});

describe("Settings: Ollama account that cannot be read", () => {
  const UNAVAILABLE = {
    kind: "ollama",
    message: "account unavailable",
  } as const;
  const REASON = "Couldn't check your Ollama account: account unavailable";

  /** Toutes les relectures automatiques de l'officiel (~15 s). */
  const exhaustRetries = () => act(() => vi.advanceTimersByTimeAsync(20_000));

  it("from the first failure: the reason and Retrying…, never Not connected nor Sign In", async () => {
    const { simulator } = installAndRender((backend) => {
      backend.accountErrors.status = UNAVAILABLE;
    });
    const account = await card();
    expect((await within(account).findByRole("alert")).textContent).toBe(
      REASON,
    );
    // Les nouveaux essais de l'officiel continuent.
    expect(reads(simulator)).toBeLessThan(11);
    expect(within(account).getByText("Retrying…")).toBeTruthy();
    expect(within(account).queryByTestId("account-loading")).toBeNull();
    expect(within(account).queryByText("Not connected")).toBeNull();
    expect(within(account).queryByRole("button")).toBeNull();
  });

  it("a later try that succeeds shows the account", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const { backend } = installAndRender((backend) => {
      backend.accountErrors.status = UNAVAILABLE;
    });
    const account = await card();
    await within(account).findByText(REASON);
    backend.accountErrors.status = null;
    backend.account = ADA;
    await act(() => vi.advanceTimersByTimeAsync(2_000));
    await within(account).findByText("ada");
    expect(within(account).queryByText(REASON)).toBeNull();
    expect(within(account).queryByText("Retrying…")).toBeNull();
  });

  it("retries over: Try again reads it again, the reason staying until it answers", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const { simulator, backend } = installAndRender((backend) => {
      backend.accountErrors.status = UNAVAILABLE;
    });
    const account = await card();
    await within(account).findByText(REASON);
    await exhaustRetries();
    const retry = await within(account).findByRole("button", {
      name: "Try again",
    });
    expect(within(account).queryByText("Retrying…")).toBeNull();
    const before = reads(simulator);

    // Encore en échec : la raison reste, avec Retrying… ; focus dans la carte.
    let answer!: () => void;
    simulator.handle(
      "account_status",
      () =>
        new Promise((_, reject) => {
          answer = () => reject(UNAVAILABLE);
        }),
    );
    act(() => retry.focus());
    await userEvent.click(retry);
    expect(reads(simulator)).toBe(before + 1);
    expect(within(account).getByText(REASON)).toBeTruthy();
    expect(within(account).getByText("Retrying…")).toBeTruthy();
    expect(within(account).queryByTestId("account-loading")).toBeNull();
    expect(document.activeElement).toBe(account);
    await act(async () => answer());

    // Puis le serveur répond : non relié, cette fois pour de bon.
    simulator.handle("account_status", () => ({ state: "signedOut" }));
    backend.accountErrors.status = null;
    await act(() => vi.advanceTimersByTimeAsync(2_000));
    await within(account).findByText("Not connected");
    expect(within(account).getByRole("button", { name: "Sign In" }));
    expect(within(account).queryByText(REASON)).toBeNull();
  });

  it("opened for the account (section=account): the card takes the focus while it retries", async () => {
    installAndRender((backend) => {
      backend.accountErrors.status = UNAVAILABLE;
    }, "/settings?section=account");
    const account = await card();
    await within(account).findByText(REASON);
    await waitFor(() => expect(document.activeElement).toBe(account));
  });

  it("an earlier failure read again on arrival: the reason stays, never the skeleton", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const { app, simulator } = installAndRender((backend) => {
      backend.accountErrors.status = UNAVAILABLE;
    }, "/settings");
    // Compte lu ailleurs (erreur de la recherche web), essais épuisés.
    void app.queryClient.prefetchQuery(accountQueryOptions);
    await exhaustRetries();
    expect(app.queryClient.getQueryState(accountKey)?.status).toBe("error");
    let answer!: () => void;
    simulator.handle(
      "account_status",
      () =>
        new Promise((_, reject) => {
          answer = () => reject(UNAVAILABLE);
        }),
    );
    // Lien de l'erreur : onglet Account, compte relu en arrivant.
    await act(() =>
      app.router.navigate({ to: "/settings", search: { section: "account" } }),
    );
    const account = await card();
    await waitFor(() =>
      expect(app.queryClient.getQueryState(accountKey)?.fetchStatus).toBe(
        "fetching",
      ),
    );
    expect(within(account).getByText(REASON)).toBeTruthy();
    expect(within(account).getByText("Retrying…")).toBeTruthy();
    expect(within(account).queryByTestId("account-loading")).toBeNull();
    await waitFor(() => expect(document.activeElement).toBe(account));
    await act(async () => answer());
  });
});

describe("Settings: Ollama account in other languages", () => {
  /** Données du compte et raisons venues du serveur : montrées telles quelles. */
  const DATA = ["ada", "ada@example.com", "account unavailable"];
  const UNAVAILABLE = {
    kind: "ollama",
    message: "account unavailable",
  } as const;

  /**
   * Langue réglée et affichée (chaque lecture des réglages rend celle du
   * moteur).
   */
  function speak(backend: FakeBackend, lang: "fr" | "es") {
    Object.assign(backend.settings, {
      language: lang,
      effectiveLanguage: lang,
    });
    act(() => setLanguage(lang));
  }

  it("connected: every text from the catalogs, a failed sign-out too", async () => {
    const { backend } = setup(ADA);
    const account = await card();
    await within(account).findByText("ada");
    usePseudoLanguage();
    await within(account).findByText("⟦Upgrade⟧");
    expect(account.getAttribute("aria-label")).toBe("⟦Ollama account⟧");
    expect(untranslatedTexts(account, DATA)).toEqual([]);

    backend.accountErrors.signOut = { kind: "ollama", message: "status 500" };
    await userEvent.click(
      within(account).getByRole("button", { name: /Sign out/ }),
    );
    await within(account).findByText("⟦Failed to disconnect Ollama account⟧");
    expect(untranslatedTexts(account, DATA)).toEqual([]);
  });

  it("not connected: Sign In and the official failure text from the catalogs", async () => {
    const { simulator } = setup();
    const account = await card();
    await within(account).findByText("Not connected");
    usePseudoLanguage();
    await within(account).findByText("⟦Not connected⟧");
    expect(untranslatedTexts(account, DATA)).toEqual([]);

    simulator.handle("account_sign_in", () => {
      throw "boom";
    });
    await userEvent.click(
      within(account).getByRole("button", { name: /Sign In/ }),
    );
    await within(account).findByText("⟦Failed to connect to Ollama account⟧");
    expect(untranslatedTexts(account, DATA)).toEqual([]);
  });

  it("cannot be read: the reason, Retrying… and Try again from the catalogs", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    installAndRender((backend) => {
      backend.accountErrors.status = UNAVAILABLE;
    });
    usePseudoLanguage();
    const account = await card();
    await within(account).findByText("⟦Retrying…⟧");
    expect(untranslatedTexts(account, DATA)).toEqual([]);
    await act(() => vi.advanceTimersByTimeAsync(20_000));
    await within(account).findByRole("button", { name: "⟦Try again⟧" });
    expect(untranslatedTexts(account, DATA)).toEqual([]);
  });

  it.each(["env", "config"] as const)(
    "cloud turned off (%s): the reason from the catalogs",
    async (source) => {
      installAndRender((backend) => {
        backend.cloud = { disabled: true, source };
      });
      usePseudoLanguage();
      const account = await card();
      await within(account).findByText(/^⟦.*cloud/i);
      expect(untranslatedTexts(account, DATA)).toEqual([]);
    },
  );

  it("speaks French and Spanish: signed out, signed in, unreadable", async () => {
    const { backend } = setup();
    const account = await card();
    await within(account).findByText("Not connected");
    speak(backend, "fr");
    expect(account.getAttribute("aria-label")).toBe("Compte Ollama");
    expect(within(account).getByText("Non connecté")).toBeTruthy();
    expect(
      within(account).getByRole("button", { name: "Se connecter" }),
    ).toBeTruthy();

    speak(backend, "es");
    backend.account = ADA;
    await userEvent.click(
      within(account).getByRole("button", { name: "Iniciar sesión" }),
    );
    await within(account).findByText("ada");
    expect(
      within(account)
        .getAllByRole("button")
        .map((button) => button.textContent),
    ).toEqual(["Mejorar plan", "Administrar", "Cerrar sesión"]);
  });

  it("says in French that the account can't be checked", async () => {
    const { backend } = installAndRender((backend) => {
      backend.accountErrors.status = UNAVAILABLE;
    });
    speak(backend, "fr");
    const account = await card();
    expect((await within(account).findByRole("alert")).textContent).toBe(
      "Impossible de vérifier votre compte Ollama : account unavailable",
    );
    expect(within(account).getByText("Nouvel essai…")).toBeTruthy();
  });
});
