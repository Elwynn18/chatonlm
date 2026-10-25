// Onglet « Easy Link » des réglages face au simulateur (faux moteur de
// src/test/easyLink.ts) : onglet de la page, présentation, carte d'une
// messagerie sans jeton (étapes, champ du jeton, Connect), connexion réussie
// (le jeton ne reste nulle part dans la page) ou refusée, états de la liaison
// et leurs textes, interrupteur, appairage (code, heure de fin, ouverture
// dans Telegram, annulation), compte lié par le bot (évènement), déliaison,
// retrait du bot après confirmation, invitation Discord, rappel des
// commandes ; textes en pseudo-langue, en espagnol et en français.

import {
  act,
  configure,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { EasyLinkLink } from "@/bindings/EasyLinkLink";
import type { EasyLinkPlatform } from "@/bindings/EasyLinkPlatform";
import type { EasyLinkState } from "@/bindings/EasyLinkState";
import { setLanguage } from "@/i18n";
import { formatDateTime, formatTime } from "@/i18n/format";
import { resetLinks } from "@/lib/openLink";
import { fakeBackend, type FakeBackend } from "@/test/backend";
import {
  BOT_NAMES,
  connectedBot,
  DISCORD_INVITE,
  type FakeEasyLink,
  PAIRING_MS,
  TOKEN_REFUSED,
} from "@/test/easyLink";
import { untranslatedTexts, usePseudoLanguage } from "@/test/i18n";
import { renderApp } from "@/test/renderApp";
import { installSimulator } from "@/test/simulator";

configure({ asyncUtilTimeout: 5000 });
vi.setConfig({ testTimeout: 30_000 });

afterEach(() => resetLinks());

/** Jeton à la façon de Telegram ; ses quatre derniers caractères : « 9876 ». */
const TOKEN = "123456789:AAH-secret_token_9876";

const COMMANDS = [
  "/new",
  "/stop",
  "/models",
  "/model",
  "/load",
  "/unload",
  "/status",
  "/web on|off",
  "/ephemeral on|off",
  "/keep",
  "/help",
];

/**
 * Onglet affiché après `prepare` du faux moteur, cartes lues et écouteur
 * d'« easy-link » inscrit (un évènement émis avant serait perdu).
 */
async function setup(
  prepare?: (easy: FakeEasyLink, backend: FakeBackend) => void,
) {
  const simulator = installSimulator();
  const backend = fakeBackend(simulator);
  prepare?.(backend.easyLink, backend);
  simulator.handle("open_external", () => null);
  const app = renderApp("/settings?section=easyLink");
  const page = await screen.findByTestId("easy-link-settings");
  if (!backend.easyLink.failures.has("easy_link_state")) {
    await screen.findByTestId("easy-link-telegram");
  }
  await waitFor(() => expect(simulator.listening("easy-link")).toBe(1));
  return { simulator, backend, easy: backend.easyLink, app, page };
}

const card = (platform: EasyLinkPlatform) =>
  screen.getByTestId(`easy-link-${platform}`);
const inCard = (platform: EasyLinkPlatform) => within(card(platform));
const statusOf = (platform: EasyLinkPlatform) =>
  inCard(platform).getByTestId("easy-link-status");
const tokenField = (platform: EasyLinkPlatform) =>
  inCard(platform).getByLabelText<HTMLInputElement>(
    platform === "telegram" ? "Telegram bot token" : "Discord bot token",
  );
const connectButton = (platform: EasyLinkPlatform) =>
  inCard(platform).getByRole<HTMLButtonElement>("button", { name: "Connect" });

describe("Easy Link settings", () => {
  it("is a tab of the settings, between Voice mode and Keyboard shortcuts", async () => {
    await setup();
    const tab = screen.getByTestId("settings-tab-easyLink");
    expect(tab.textContent).toBe("Easy Link");
    expect(tab.getAttribute("aria-current")).toBe("page");
    expect(tab.getAttribute("href")).toBe("/settings?section=easyLink");
    expect(
      [...screen.getByTestId("settings-tabs").querySelectorAll("a")].map(
        (link) => link.textContent,
      ),
    ).toEqual([
      "General",
      "Models",
      "Account",
      "Dictation",
      "Read aloud",
      "Voice mode",
      "Easy Link",
      "Keyboard shortcuts",
    ]);
  });

  it("says that ChatonLM must stay open and where the messages go", async () => {
    const { page } = await setup();
    expect(page.textContent).toContain(
      "The bot only replies while ChatonLM is running on this computer. By default, closing the window keeps ChatonLM running in the system tray (Settings › General).",
    );
    expect(page.textContent).toContain(
      "the messages you exchange with the bot pass through the servers of Telegram or Discord.",
    );
  });

  it("one card per messenger, Telegram first", async () => {
    await setup();
    expect(
      screen
        .getAllByRole("region")
        .map(
          (region) =>
            document.getElementById(region.getAttribute("aria-labelledby")!)
              ?.textContent,
        ),
    ).toEqual(["Easy Link", "Telegram", "Discord", "Bot commands"]);
    expect(screen.getByRole("region", { name: "Telegram" })).toBe(
      card("telegram"),
    );
    expect(screen.getByRole("region", { name: "Discord" })).toBe(
      card("discord"),
    );
  });

  it("a skeleton until the state is read", async () => {
    const simulator = installSimulator();
    const backend = fakeBackend(simulator);
    let answer!: (state: EasyLinkState) => void;
    simulator.handle(
      "easy_link_state",
      () => new Promise<EasyLinkState>((resolve) => (answer = resolve)),
    );
    renderApp("/settings?section=easyLink");
    expect(await screen.findByTestId("easy-link-loading")).toBeTruthy();
    expect(screen.queryByTestId("easy-link-telegram")).toBeNull();
    // La présentation et les commandes n'attendent pas l'état.
    expect(screen.getByText("Bot commands")).toBeTruthy();
    await act(async () => answer(structuredClone(backend.easyLink.state)));
    await screen.findByTestId("easy-link-telegram");
    expect(screen.queryByTestId("easy-link-loading")).toBeNull();
  });

  it("the reason when the state can't be read, then Try again", async () => {
    const { easy } = await setup((fake) => {
      fake.failures.set("easy_link_state", {
        kind: "internal",
        message: "database is locked",
      });
    });
    expect((await screen.findByRole("alert")).textContent).toBe(
      "Couldn't read the Easy Link settings: database is locked",
    );
    expect(screen.queryByTestId("easy-link-telegram")).toBeNull();
    easy.failures.clear();
    await userEvent.click(screen.getByRole("button", { name: "Try again" }));
    await screen.findByTestId("easy-link-telegram");
    expect(screen.queryByRole("alert")).toBeNull();
  });
});

describe("Easy Link: a bot to create", () => {
  it("Telegram: the steps, BotFather opened in the browser", async () => {
    const { simulator } = await setup();
    const steps = inCard("telegram").getByTestId("easy-link-steps");
    expect(steps.tagName).toBe("OL");
    // La première étape porte le bouton qui mène à BotFather.
    expect(
      [...steps.querySelectorAll("li")].map((step) => step.textContent),
    ).toEqual([
      "In Telegram, open @BotFather.Open BotFather",
      "Send /newbot and answer its questions.",
      "Paste the token it gives you below.",
    ]);
    await userEvent.click(
      inCard("telegram").getByRole("button", { name: "Open BotFather" }),
    );
    await waitFor(() =>
      expect(simulator.callsTo("open_external")).toEqual([
        { url: "https://t.me/BotFather" },
      ]),
    );
  });

  it("Discord: the steps, the invitation announced, the portal opened in the browser", async () => {
    const { simulator } = await setup();
    const list = inCard("discord").getByTestId("easy-link-steps");
    expect(list.querySelectorAll("li")).toHaveLength(4);
    expect(list.textContent).toContain(
      "Open the Discord Developer Portal and create an application.",
    );
    expect(list.textContent).toContain(
      "In its “Bot” tab, select “Reset Token”.",
    );
    expect(list.textContent).toContain(
      "Discord only lets you write privately to a bot you share a server with.",
    );
    await userEvent.click(
      inCard("discord").getByRole("button", {
        name: "Open the Developer Portal",
      }),
    );
    await waitFor(() =>
      expect(simulator.callsTo("open_external")).toEqual([
        { url: "https://discord.com/developers/applications" },
      ]),
    );
  });

  it("the token field hides what is typed; Connect is disabled while it is empty", async () => {
    const { simulator } = await setup();
    const field = tokenField("telegram");
    expect(field.type).toBe("password");
    expect(field.getAttribute("autocomplete")).toBe("off");
    expect(connectButton("telegram").disabled).toBe(true);
    // Des espaces ne sont pas un jeton.
    await userEvent.type(field, "   ");
    expect(connectButton("telegram").disabled).toBe(true);
    await userEvent.type(field, "{Enter}");
    await userEvent.type(field, "x");
    expect(connectButton("telegram").disabled).toBe(false);
    expect(simulator.callsTo("easy_link_connect")).toEqual([]);
  });

  it("an accepted token: sent trimmed, the card shows the bot, the token is nowhere in the page", async () => {
    const { simulator, easy } = await setup();
    await userEvent.type(tokenField("telegram"), ` ${TOKEN} `);
    await userEvent.click(connectButton("telegram"));
    await inCard("telegram").findByText(`@${BOT_NAMES.telegram}`);
    expect(simulator.callsTo("easy_link_connect")).toEqual([
      { platform: "telegram", token: TOKEN },
    ]);
    expect(inCard("telegram").getByText("Token •••• 9876")).toBeTruthy();
    // La liaison s'établit ensuite (évènement du moteur).
    await waitFor(() =>
      expect(statusOf("telegram").textContent).toBe("Connected"),
    );
    expect(easy.bot("telegram").link).toEqual({ state: "connected" });
    // Plus de champ, et le jeton n'est ni dans le texte ni dans un attribut
    // ou une valeur de champ.
    expect(
      inCard("telegram").queryByLabelText("Telegram bot token"),
    ).toBeNull();
    expect(document.documentElement.outerHTML).not.toContain(TOKEN);
    expect(
      [...document.querySelectorAll("input, textarea")].filter((input) =>
        (input as HTMLInputElement).value.includes("secret"),
      ),
    ).toEqual([]);
    // L'autre messagerie reste à relier.
    expect(tokenField("discord").value).toBe("");
    // Le bouton cliqué a disparu : le focus reste dans la carte, au titre.
    expect(document.activeElement).toBe(
      inCard("telegram").getByRole("heading", { name: "Telegram" }),
    );
  });

  it("Enter in the field connects too; while the token is checked, Connect waits", async () => {
    const { simulator, easy } = await setup();
    let answer!: (state: EasyLinkState) => void;
    simulator.handle(
      "easy_link_connect",
      () => new Promise<EasyLinkState>((resolve) => (answer = resolve)),
    );
    await userEvent.type(tokenField("discord"), `${TOKEN}{Enter}`);
    const waiting = await inCard("discord").findByRole("button", {
      name: "Checking…",
    });
    expect(waiting.getAttribute("aria-disabled")).toBe("true");
    // Un second envoi pendant la vérification est ignoré.
    await userEvent.type(tokenField("discord"), "{Enter}");
    await userEvent.click(waiting);
    expect(simulator.callsTo("easy_link_connect")).toEqual([
      { platform: "discord", token: TOKEN },
    ]);
    easy.configure("discord");
    await act(async () => answer(structuredClone(easy.state)));
    await inCard("discord").findByText(`@${BOT_NAMES.discord}`);
  });

  it("a refused token: the reason of the backend as an alert, the token kept in the field", async () => {
    const { simulator } = await setup((fake) => {
      fake.refusedTokens.add("bad-token");
    });
    await userEvent.type(tokenField("telegram"), "bad-token");
    await userEvent.click(connectButton("telegram"));
    const alert = await inCard("telegram").findByRole("alert");
    expect(alert.textContent).toBe(TOKEN_REFUSED);
    expect(tokenField("telegram").value).toBe("bad-token");
    expect(tokenField("telegram").getAttribute("aria-describedby")).toBe(
      alert.id,
    );
    expect(connectButton("telegram").disabled).toBe(false);
    expect(simulator.callsTo("easy_link_connect")).toHaveLength(1);
    // Corrigé : nouvel essai, l'alerte s'en va avec la carte du bot à créer.
    await userEvent.clear(tokenField("telegram"));
    await userEvent.type(tokenField("telegram"), TOKEN);
    await userEvent.click(connectButton("telegram"));
    await inCard("telegram").findByText(`@${BOT_NAMES.telegram}`);
    expect(inCard("telegram").queryByRole("alert")).toBeNull();
  });
});

describe("Easy Link: a connected bot", () => {
  it("shows its name, the end of its token and no token field", async () => {
    await setup((fake) => {
      fake.configure("telegram", { tokenHint: "wxyz" });
    });
    expect(
      inCard("telegram").getByTestId("easy-link-bot-name").textContent,
    ).toBe("@chaton_test_bot");
    expect(inCard("telegram").getByText("Token •••• wxyz")).toBeTruthy();
    expect(
      inCard("telegram").queryByLabelText("Telegram bot token"),
    ).toBeNull();
    expect(inCard("telegram").queryByTestId("easy-link-steps")).toBeNull();
  });

  it("a bot whose name is not known yet says so", async () => {
    await setup((fake) => {
      fake.configure("telegram", { name: "", link: { state: "connecting" } });
    });
    expect(
      inCard("telegram").getByTestId("easy-link-bot-name").textContent,
    ).toBe("Bot name not known yet");
  });

  it.each<[string, EasyLinkLink, boolean, string]>([
    ["off", { state: "off" }, false, "Turned off"],
    ["connecting", { state: "connecting" }, true, "Connecting…"],
    ["connected", { state: "connected" }, true, "Connected"],
    [
      "a refused token",
      { state: "failed", problem: "unauthorized" },
      true,
      "Telegram refused this token. Remove the bot, then connect it again with a new token.",
    ],
    [
      "a bot used elsewhere",
      { state: "failed", problem: "conflict" },
      true,
      "Another app is already using this bot, or a webhook is set on it. Close that app, or create a bot just for ChatonLM.",
    ],
    [
      "an unreachable messenger",
      { state: "failed", problem: "network" },
      true,
      "Can't reach Telegram. ChatonLM tries again on its own.",
    ],
    [
      "a refusal of the messenger",
      { state: "failed", problem: "refused" },
      true,
      "Telegram refused the connection.",
    ],
  ])("link state: %s", async (_name, link, enabled, text) => {
    await setup((fake) => {
      fake.configure("telegram", { link, enabled });
    });
    const status = statusOf("telegram");
    expect(status.textContent).toBe(text);
    expect(status.getAttribute("role")).toBe("status");
    expect(status.getAttribute("data-state")).toBe(link.state);
    expect(
      inCard("telegram")
        .getByRole("switch", { name: "Enable the Telegram bot" })
        .getAttribute("aria-checked"),
    ).toBe(String(enabled));
  });

  it("follows the link as the backend reports it, with the name of its messenger", async () => {
    const { easy } = await setup((fake) => {
      fake.configure("discord");
    });
    expect(statusOf("discord").textContent).toBe("Connected");
    await easy.setLink("discord", { state: "failed", problem: "network" });
    await waitFor(() =>
      expect(statusOf("discord").textContent).toBe(
        "Can't reach Discord. ChatonLM tries again on its own.",
      ),
    );
    await easy.setLink("discord", { state: "connected" });
    await waitFor(() =>
      expect(statusOf("discord").textContent).toBe("Connected"),
    );
  });

  it("the switch turns the bot off and on, saved at once", async () => {
    const { simulator } = await setup((fake) => {
      fake.configure("telegram");
    });
    const toggle = inCard("telegram").getByRole("switch", {
      name: "Enable the Telegram bot",
    });
    expect(toggle.getAttribute("aria-checked")).toBe("true");
    await userEvent.click(toggle);
    await waitFor(() =>
      expect(toggle.getAttribute("aria-checked")).toBe("false"),
    );
    expect(statusOf("telegram").textContent).toBe("Turned off");
    await screen.findByText("Saved");
    await userEvent.click(toggle);
    await waitFor(() =>
      expect(statusOf("telegram").textContent).toBe("Connected"),
    );
    expect(simulator.callsTo("easy_link_set_enabled")).toEqual([
      { platform: "telegram", enabled: false },
      { platform: "telegram", enabled: true },
    ]);
  });

  it("a refused command: the reason of the backend as an alert", async () => {
    const { simulator } = await setup((fake) => {
      fake.configure("telegram");
      fake.failures.set("easy_link_set_enabled", {
        kind: "internal",
        message: "database is locked",
      });
    });
    const toggle = inCard("telegram").getByRole("switch", {
      name: "Enable the Telegram bot",
    });
    await userEvent.click(toggle);
    expect((await inCard("telegram").findByRole("alert")).textContent).toBe(
      "database is locked",
    );
    expect(toggle.getAttribute("aria-checked")).toBe("true");
    expect(screen.queryByText("Saved")).toBeNull();
    expect(simulator.callsTo("easy_link_set_enabled")).toHaveLength(1);
  });

  it("Discord: the invitation link opens in the browser, once it is known", async () => {
    const { simulator, easy } = await setup((fake) => {
      fake.configure("telegram");
      fake.configure("discord", { url: null });
    });
    const invite = () =>
      screen.queryByRole("button", { name: "Invite the bot to a server" });
    expect(invite()).toBeNull();
    easy.configure("discord");
    await easy.emit();
    const button = await inCard("discord").findByRole("button", {
      name: "Invite the bot to a server",
    });
    // Dans la carte de Discord seulement : l'adresse d'un bot Telegram n'est
    // pas une invitation.
    expect(
      inCard("telegram").queryByRole("button", {
        name: "Invite the bot to a server",
      }),
    ).toBeNull();
    expect(
      document.getElementById(button.getAttribute("aria-describedby")!)
        ?.textContent,
    ).toBe(
      "Discord only lets you write privately to a bot you share a server with. Invite the bot to one of your servers, then send it a direct message. If the bot doesn't answer, allow direct messages from the members of that server in its privacy settings.",
    );
    await userEvent.click(button);
    await waitFor(() =>
      expect(simulator.callsTo("open_external")).toEqual([
        { url: DISCORD_INVITE },
      ]),
    );
  });

  it("“Remove bot” asks first, then forgets the bot and its accounts", async () => {
    const { simulator, easy } = await setup((fake) => {
      fake.configure("telegram");
      fake.configure("discord");
      fake.addAccount("telegram", "ada");
      fake.addAccount("discord", "bob");
    });
    const remove = () =>
      inCard("telegram").getByRole("button", { name: "Remove bot" });
    await userEvent.click(remove());
    const dialog = screen.getByRole("alertdialog", {
      name: "Remove the Telegram bot?",
    });
    expect(dialog.textContent).toContain(
      "ChatonLM forgets its token and unlinks its accounts. The chats stay in the history.",
    );
    await userEvent.click(
      within(dialog).getByRole("button", { name: "Cancel" }),
    );
    expect(screen.queryByRole("alertdialog")).toBeNull();
    expect(simulator.callsTo("easy_link_disconnect")).toEqual([]);
    expect(inCard("telegram").getByText("ada")).toBeTruthy();

    await userEvent.click(remove());
    await userEvent.click(
      within(screen.getByRole("alertdialog")).getByRole("button", {
        name: "Remove",
      }),
    );
    await waitFor(() => expect(tokenField("telegram").value).toBe(""));
    expect(simulator.callsTo("easy_link_disconnect")).toEqual([
      { platform: "telegram" },
    ]);
    expect(screen.queryByRole("alertdialog")).toBeNull();
    expect(inCard("telegram").queryByText("ada")).toBeNull();
    expect(easy.state.accounts.map((account) => account.name)).toEqual(["bob"]);
    // L'autre bot et son compte restent.
    expect(inCard("discord").getByText("bob")).toBeTruthy();
    expect(document.activeElement).toBe(
      inCard("telegram").getByRole("heading", { name: "Telegram" }),
    );
  });
});

describe("Easy Link: linking an account", () => {
  const pairButton = (platform: EasyLinkPlatform) =>
    inCard(platform).getByRole("button", { name: "Link an account" });

  it("no account yet: says so, with “Link an account”", async () => {
    await setup((fake) => {
      fake.configure("telegram");
    });
    expect(
      inCard("telegram").getByText("No account is linked yet."),
    ).toBeTruthy();
    expect(pairButton("telegram")).toBeTruthy();
    expect(inCard("telegram").queryByTestId("easy-link-pairing")).toBeNull();
    // Pas d'appairage sans bot.
    expect(
      inCard("discord").queryByRole("button", { name: "Link an account" }),
    ).toBeNull();
  });

  it("Telegram: the code, spelled out, its end of validity, “Open in Telegram”", async () => {
    const before = Date.now();
    const { simulator, easy } = await setup((fake) => {
      fake.configure("telegram");
      fake.configure("discord");
    });
    await userEvent.click(pairButton("telegram"));
    const box = await inCard("telegram").findByTestId("easy-link-pairing");
    expect(simulator.callsTo("easy_link_pair")).toEqual([
      { platform: "telegram" },
    ]);
    const shown = within(box).getByRole("img", {
      name: "Linking code: 4 8 2 9 1 5",
    });
    // Un seul texte, sans espace : copié tel quel ; sélectionnable.
    expect(shown.textContent).toBe("482915");
    expect(shown.className.split(" ")).toContain("select-text");
    // Lu aussitôt : il prend le focus du bouton qu'il remplace.
    expect(document.activeElement).toBe(shown);
    const expiresAt = easy.state.pairing!.expiresAt;
    expect(expiresAt).toBeGreaterThanOrEqual(before + PAIRING_MS);
    expect(
      within(box).getByText(`Valid until ${formatTime("en", expiresAt)}.`),
    ).toBeTruthy();
    expect(
      within(box).getByText(
        "Open the bot in Telegram with the button below, or send it this code.",
      ),
    ).toBeTruthy();
    expect(
      inCard("telegram").queryByRole("button", { name: "Link an account" }),
    ).toBeNull();
    // Dans la carte de sa messagerie seulement.
    expect(inCard("discord").queryByTestId("easy-link-pairing")).toBeNull();
    expect(pairButton("discord")).toBeTruthy();

    await userEvent.click(
      within(box).getByRole("button", { name: "Open in Telegram" }),
    );
    await waitFor(() =>
      expect(simulator.callsTo("open_external")).toEqual([
        { url: "https://t.me/chaton_test_bot?start=482915" },
      ]),
    );
  });

  it("Discord: the code is sent to the bot in a direct message, no link", async () => {
    await setup((fake) => {
      fake.configure("discord");
    });
    await userEvent.click(pairButton("discord"));
    const box = await inCard("discord").findByTestId("easy-link-pairing");
    expect(
      within(box).getByText("Send this code to the bot in a direct message."),
    ).toBeTruthy();
    expect(
      within(box).queryByRole("button", { name: "Open in Telegram" }),
    ).toBeNull();
    expect(within(box).getByRole("button", { name: "Cancel" })).toBeTruthy();
  });

  it("Cancel gives the code up: “Link an account” is back, with the focus", async () => {
    const { simulator, easy } = await setup((fake) => {
      fake.configure("telegram");
    });
    await userEvent.click(pairButton("telegram"));
    const box = await inCard("telegram").findByTestId("easy-link-pairing");
    await userEvent.click(within(box).getByRole("button", { name: "Cancel" }));
    await waitFor(() =>
      expect(inCard("telegram").queryByTestId("easy-link-pairing")).toBeNull(),
    );
    expect(simulator.callsTo("easy_link_cancel_pairing")).toEqual([{}]);
    expect(easy.state.pairing).toBeNull();
    expect(document.activeElement).toBe(pairButton("telegram"));
  });

  it("an account linked by the bot appears when the backend says so, and the code goes away", async () => {
    const { easy } = await setup((fake) => {
      fake.configure("telegram");
    });
    await userEvent.click(pairButton("telegram"));
    await inCard("telegram").findByTestId("easy-link-pairing");
    await easy.linkAccount("telegram", "ada");
    await waitFor(() =>
      expect(inCard("telegram").queryByTestId("easy-link-pairing")).toBeNull(),
    );
    const rows = inCard("telegram").getAllByTestId("easy-link-account");
    expect(rows).toHaveLength(1);
    expect(within(rows[0]).getByText("ada")).toBeTruthy();
    expect(
      within(rows[0]).getByText(
        `Linked on ${formatDateTime("en", easy.state.accounts[0].linkedAt)}`,
      ),
    ).toBeTruthy();
    expect(
      inCard("telegram").queryByText("No account is linked yet."),
    ).toBeNull();
    expect(pairButton("telegram")).toBeTruthy();
  });

  it("an expired code goes away on its own", async () => {
    const { easy } = await setup((fake) => {
      fake.configure("telegram");
    });
    await userEvent.click(pairButton("telegram"));
    await inCard("telegram").findByTestId("easy-link-pairing");
    await easy.expirePairing();
    await waitFor(() =>
      expect(inCard("telegram").queryByTestId("easy-link-pairing")).toBeNull(),
    );
    expect(
      inCard("telegram").getByText("No account is linked yet."),
    ).toBeTruthy();
    expect(pairButton("telegram")).toBeTruthy();
  });

  it("a code that can't be given: the reason of the backend", async () => {
    await setup((fake) => {
      fake.configure("telegram");
      fake.failures.set("easy_link_pair", {
        kind: "invalid",
        message: "Turn the bot on first",
      });
    });
    await userEvent.click(pairButton("telegram"));
    expect((await inCard("telegram").findByRole("alert")).textContent).toBe(
      "Turn the bot on first",
    );
    expect(inCard("telegram").queryByTestId("easy-link-pairing")).toBeNull();
  });

  it("linked accounts: each in the card of its messenger, unnamed ones included; Unlink removes one", async () => {
    const { simulator, easy } = await setup((fake) => {
      fake.configure("telegram");
      fake.configure("discord");
      fake.addAccount("telegram", "ada", 1_760_000_000_000);
      fake.addAccount("discord", "bob");
      fake.addAccount("telegram", "");
    });
    const names = (platform: EasyLinkPlatform) =>
      inCard(platform)
        .queryAllByTestId("easy-link-account")
        .map((row) => row.firstElementChild?.firstElementChild?.textContent);
    expect(names("telegram")).toEqual(["ada", "Unnamed account"]);
    expect(names("discord")).toEqual(["bob"]);
    expect(
      inCard("telegram").getByText(
        `Linked on ${formatDateTime("en", 1_760_000_000_000)}`,
      ),
    ).toBeTruthy();
    const list = inCard("telegram").getByRole("list", {
      name: "Linked accounts",
    });
    const unlink = within(list).getAllByRole("button", { name: "Unlink" })[0];
    // Même nom pour chaque bouton : le compte est sa description.
    expect(
      document.getElementById(unlink.getAttribute("aria-describedby")!)
        ?.textContent,
    ).toBe("ada");
    await userEvent.click(unlink);
    await waitFor(() => expect(names("telegram")).toEqual(["Unnamed account"]));
    expect(simulator.callsTo("easy_link_unlink")).toEqual([{ accountId: 1 }]);
    expect(easy.state.accounts.map((account) => account.id)).toEqual([2, 3]);
    expect(names("discord")).toEqual(["bob"]);
    expect(document.activeElement).toBe(
      inCard("telegram").getByRole("heading", { name: "Telegram" }),
    );
  });
});

describe("Easy Link: answers and events out of order", () => {
  it("a state read while an event arrives keeps the event, more recent", async () => {
    const simulator = installSimulator();
    const backend = fakeBackend(simulator);
    const easy = backend.easyLink;
    const stale = structuredClone(easy.state);
    let answer!: (state: EasyLinkState) => void;
    simulator.handle(
      "easy_link_state",
      () => new Promise<EasyLinkState>((resolve) => (answer = resolve)),
    );
    renderApp("/settings?section=easyLink");
    await screen.findByTestId("easy-link-loading");
    await waitFor(() => expect(simulator.listening("easy-link")).toBe(1));
    easy.configure("telegram");
    await easy.emit();
    await waitFor(() =>
      expect(statusOf("telegram").textContent).toBe("Connected"),
    );
    // La lecture partie avant répond après, avec l'état d'avant.
    await act(async () => answer(stale));
    await act(() => new Promise((resolve) => setTimeout(resolve, 30)));
    expect(statusOf("telegram").textContent).toBe("Connected");
  });

  it("a command answered after a newer event: the state is read again, not set back", async () => {
    const { simulator, easy } = await setup((fake) => {
      fake.configure("telegram");
    });
    let answer!: (state: EasyLinkState) => void;
    simulator.handle(
      "easy_link_set_enabled",
      () => new Promise<EasyLinkState>((resolve) => (answer = resolve)),
    );
    const reads = simulator.callsTo("easy_link_state").length;
    await userEvent.click(
      inCard("telegram").getByRole("switch", {
        name: "Enable the Telegram bot",
      }),
    );
    await waitFor(() =>
      expect(simulator.callsTo("easy_link_set_enabled")).toHaveLength(1),
    );
    // Le moteur a coupé le bot (réponse en route), puis le bot a lié un
    // compte : l'évènement, plus récent, arrive avant la réponse.
    easy.configure("telegram", { enabled: false, link: { state: "off" } });
    const answered = structuredClone(easy.state);
    easy.addAccount("telegram", "ada");
    await easy.emit();
    await inCard("telegram").findByText("ada");
    await act(async () => answer(answered));
    await waitFor(() =>
      expect(simulator.callsTo("easy_link_state")).toHaveLength(reads + 1),
    );
    await act(() => new Promise((resolve) => setTimeout(resolve, 30)));
    expect(inCard("telegram").getByText("ada")).toBeTruthy();
    expect(statusOf("telegram").textContent).toBe("Turned off");
  });
});

describe("Easy Link: the token never lingers", () => {
  it("accepted while the state is being read again: the field is emptied at once", async () => {
    const { simulator, easy } = await setup((fake) => {
      fake.configure("discord");
    });
    let connected!: (state: EasyLinkState) => void;
    simulator.handle(
      "easy_link_connect",
      () => new Promise<EasyLinkState>((resolve) => (connected = resolve)),
    );
    await userEvent.type(tokenField("telegram"), TOKEN);
    await userEvent.click(connectButton("telegram"));
    await inCard("telegram").findByRole("button", { name: "Checking…" });
    // Un évènement arrive pendant la vérification : la réponse ne sera pas
    // posée, l'état sera relu ; cette relecture tarde.
    await easy.setLink("discord", { state: "connecting" });
    simulator.handle("easy_link_state", () => new Promise(() => {}));
    easy.configure("telegram");
    await act(async () => connected(structuredClone(easy.state)));
    await waitFor(() => expect(tokenField("telegram").value).toBe(""));
    expect(document.documentElement.outerHTML).not.toContain(TOKEN);
  });
});

describe("Easy Link: the commands of the bot", () => {
  it("lists them, with “!” on Discord, photos and voice messages", async () => {
    const { page } = await setup();
    const list = within(page).getByTestId("easy-link-commands");
    expect(
      [...list.querySelectorAll("dt")].map((term) => term.textContent),
    ).toEqual([
      "/new",
      "/stop",
      "/models",
      "/model <name>",
      "/load",
      "/unload",
      "/status",
      "/web on|off",
      "/ephemeral on|off",
      "/keep",
      "/help",
    ]);
    expect(
      [...list.querySelectorAll("dd")].map((text) => text.textContent),
    ).toEqual([
      "Start a new chat",
      "Stop the reply",
      "List the installed models",
      "Choose the model",
      "Load the model into memory",
      "Unload the model from memory",
      "Show the status",
      "Turn web search on or off",
      "Ephemeral mode: chats kept out of the history, erased by /new, /ephemeral off or when ChatonLM quits",
      "Keep the current ephemeral chat in the history",
      "Show this list",
    ]);
    expect(page.textContent).toContain(
      "On Discord, commands can also start with “!”, for example !models.",
    );
    expect(page.textContent).toContain(
      "You can also send a photo, if the model can see images, or a voice message: it is transcribed by the speech model of dictation you have installed.",
    );
  });
});

describe("Easy Link in other languages", () => {
  /** Tous les états à la fois : bot à créer, bot relié, code, comptes. */
  function everything(fake: FakeEasyLink) {
    fake.configure("discord", {
      link: { state: "failed", problem: "network" },
    });
    fake.addAccount("discord", "bob");
    fake.addAccount("discord", "");
    fake.state.pairing = {
      platform: "discord",
      code: "482915",
      expiresAt: Date.now() + PAIRING_MS,
      url: null,
    };
  }

  it("is all translated", async () => {
    const { page, easy } = await setup(everything);
    usePseudoLanguage();
    const allowed = [...COMMANDS, "482915", "bob", `@${BOT_NAMES.discord}`];
    expect(untranslatedTexts(page, allowed)).toEqual([]);
    // Bot Telegram relié, avec son code et son lien ; refus d'une commande
    // (texte du moteur) ; boîte de confirmation.
    easy.state.bots = [
      connectedBot("telegram", { enabled: false, link: { state: "off" } }),
      connectedBot("discord", { link: { state: "connecting" } }),
    ];
    easy.state.pairing = {
      platform: "telegram",
      code: "482915",
      expiresAt: Date.now() + PAIRING_MS,
      url: "https://t.me/chaton_test_bot?start=482915",
    };
    await easy.emit();
    await inCard("telegram").findByTestId("easy-link-pairing");
    expect(
      untranslatedTexts(page, [...allowed, `@${BOT_NAMES.telegram}`]),
    ).toEqual([]);
    await userEvent.click(
      inCard("telegram").getByRole("button", { name: /Remove bot/ }),
    );
    expect(untranslatedTexts(screen.getByRole("alertdialog"))).toEqual([]);
  });

  it("in French", async () => {
    const { page } = await setup(everything);
    act(() => setLanguage("fr"));
    expect(screen.getByTestId("settings-tab-easyLink").textContent).toBe(
      "Easy Link",
    );
    expect(page.textContent).toContain(
      "Par défaut, fenêtre fermée, ChatonLM continue dans la zone de notification (Paramètres › Général).",
    );
    expect(
      inCard("telegram").getByLabelText("Jeton du bot Telegram"),
    ).toBeTruthy();
    expect(
      inCard("telegram").getByRole("button", { name: "Connecter" }),
    ).toBeTruthy();
    expect(statusOf("discord").textContent).toBe(
      "Discord est injoignable. ChatonLM réessaie tout seul.",
    );
    expect(
      inCard("discord").getByRole("list", { name: "Comptes liés" }),
    ).toBeTruthy();
    expect(inCard("discord").getByText("Compte sans nom")).toBeTruthy();
    expect(
      inCard("discord")
        .getByTestId("easy-link-code")
        .getAttribute("aria-label"),
    ).toBe("Code de liaison\u00a0: 4 8 2 9 1 5");
    expect(
      inCard("discord").getByRole("button", {
        name: "Inviter le bot sur un serveur",
      }),
    ).toBeTruthy();
    expect(
      [...page.querySelectorAll("dt")].map((term) => term.textContent),
    ).toContain("/model <nom>");
    await userEvent.click(
      inCard("discord").getByRole("button", { name: "Retirer le bot" }),
    );
    expect(
      within(screen.getByRole("alertdialog")).getByRole("heading").textContent,
    ).toBe("Retirer le bot Discord\u00a0?");
  });

  it("in Spanish", async () => {
    const { page } = await setup(everything);
    act(() => setLanguage("es"));
    expect(screen.getByTestId("settings-tab-easyLink").textContent).toBe(
      "Easy Link",
    );
    expect(
      inCard("telegram").getByLabelText("Token del bot de Telegram"),
    ).toBeTruthy();
    expect(
      inCard("telegram").getByRole("button", { name: "Conectar" }),
    ).toBeTruthy();
    expect(statusOf("discord").textContent).toBe(
      "No se puede contactar con Discord. ChatonLM lo vuelve a intentar automáticamente.",
    );
    expect(
      inCard("discord").getByRole("list", { name: "Cuentas vinculadas" }),
    ).toBeTruthy();
    expect(
      inCard("discord")
        .getByTestId("easy-link-code")
        .getAttribute("aria-label"),
    ).toBe("Código de vinculación: 4 8 2 9 1 5");
    expect(
      inCard("discord").getByText(
        "Envía este código al bot por mensaje directo.",
      ),
    ).toBeTruthy();
    expect(
      [...page.querySelectorAll("dt")].map((term) => term.textContent),
    ).toContain("/model <nombre>");
  });
});
