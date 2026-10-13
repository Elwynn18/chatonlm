// Faux moteur d'Easy Link pour le simulateur (tests vitest seulement) : les
// deux bots (Telegram puis Discord, sans jeton par défaut), les comptes liés
// et le code d'appairage, en mémoire. Comme le moteur, chaque commande
// renvoie l'état d'après, sans jamais rendre un jeton (ses quatre derniers
// caractères seulement). easy_link_connect vérifie le jeton (refusé s'il est
// dans `refusedTokens`, erreur du moteur), l'enregistre, active le bot et
// répond « connecting » ; la liaison passe à « connected » par l'évènement
// « easy-link » qui suit. easy_link_set_enabled coupe la liaison (« off ») ou
// la relance de même. easy_link_disconnect oublie le bot, ses comptes et son
// code. easy_link_pair donne un nouveau code (un seul à la fois, dix
// minutes), easy_link_cancel_pairing l'abandonne, easy_link_unlink retire un
// compte. Le test joue ce que fait le moteur de lui-même : linkAccount() (un
// compte envoie le code au bot : compte lié, code consommé), expirePairing(),
// setLink() (liaison perdue, refusée...), chacun suivi de « easy-link ».
// Une panne d'une commande se force par `failures`.

import type { AppError } from "@/bindings/AppError";
import type { EasyLinkAccount } from "@/bindings/EasyLinkAccount";
import type { EasyLinkBot } from "@/bindings/EasyLinkBot";
import type { EasyLinkLink } from "@/bindings/EasyLinkLink";
import type { EasyLinkPlatform } from "@/bindings/EasyLinkPlatform";
import type { EasyLinkState } from "@/bindings/EasyLinkState";
import type { Args, Simulator } from "@/test/simulator";

/** Messageries, dans l'ordre de l'état. */
export const EASY_LINK_PLATFORMS: readonly EasyLinkPlatform[] = [
  "telegram",
  "discord",
];

/** Nom d'utilisateur que la messagerie donne au bot vérifié. */
export const BOT_NAMES: Record<EasyLinkPlatform, string> = {
  telegram: "chaton_test_bot",
  discord: "ChatonTest",
};

/** Lien d'invitation du faux bot Discord sur un serveur. */
export const DISCORD_INVITE =
  "https://discord.com/oauth2/authorize?client_id=123456789012345678&scope=bot&permissions=274877975552";

/** Refus d'un jeton par la messagerie (texte du moteur, déjà traduit). */
export const TOKEN_REFUSED = "This token was refused. Check it and try again.";

/** Durée de validité d'un code (PAIRING_TTL du moteur) : dix minutes. */
export const PAIRING_MS = 10 * 60 * 1000;

/** Adresse du bot : la sienne (Telegram), son invitation (Discord). */
function botUrl(platform: EasyLinkPlatform, name: string): string {
  return platform === "telegram" ? `https://t.me/${name}` : DISCORD_INVITE;
}

function emptyBot(platform: EasyLinkPlatform): EasyLinkBot {
  return {
    platform,
    configured: false,
    tokenHint: "",
    enabled: false,
    name: "",
    link: { state: "off" },
    url: null,
  };
}

/** Bot relié et connecté, comme après un easy_link_connect réussi. */
export function connectedBot(
  platform: EasyLinkPlatform,
  fields: Partial<EasyLinkBot> = {},
): EasyLinkBot {
  return {
    platform,
    configured: true,
    tokenHint: "1234",
    enabled: true,
    name: BOT_NAMES[platform],
    link: { state: "connected" },
    url: botUrl(platform, BOT_NAMES[platform]),
    ...fields,
  };
}

export interface FakeEasyLink {
  /** État rendu par easy_link_state (bots : Telegram puis Discord). */
  state: EasyLinkState;
  /** Jetons que la messagerie refuse (easy_link_connect rejette TOKEN_REFUSED). */
  refusedTokens: Set<string>;
  /** Erreur (panne) renvoyée par une commande, par nom. */
  failures: Map<string, AppError>;
  /** Bot d'une messagerie. */
  bot(platform: EasyLinkPlatform): EasyLinkBot;
  /**
   * Pose un bot déjà relié (connecté, sauf `fields`), sans évènement : à
   * appeler avant le rendu.
   */
  configure(platform: EasyLinkPlatform, fields?: Partial<EasyLinkBot>): void;
  /** Ajoute un compte lié, sans évènement : à appeler avant le rendu. */
  addAccount(
    platform: EasyLinkPlatform,
    name: string,
    linkedAt?: number,
  ): EasyLinkAccount;
  /** Émet « easy-link » avec l'état du moment. */
  emit(): Promise<void>;
  /** La liaison d'un bot change (perdue, refusée, rétablie) : évènement. */
  setLink(platform: EasyLinkPlatform, link: EasyLinkLink): Promise<void>;
  /**
   * Un compte envoie le code au bot : il est lié, le code est consommé,
   * puis « easy-link ». Fait échouer l'essai sans code pour cette messagerie.
   */
  linkAccount(platform: EasyLinkPlatform, name: string): Promise<void>;
  /** Le code expire : retiré de l'état, puis « easy-link ». */
  expirePairing(): Promise<void>;
}

function invalid(message: string): AppError {
  return { kind: "invalid", message };
}

export function fakeEasyLink(simulator: Simulator): FakeEasyLink {
  let lastAccountId = 0;
  let lastCode = 482_914;

  /** Copie : chaque réponse est un nouvel objet, comme celles du moteur. */
  const snapshot = (): EasyLinkState => structuredClone(easy.state);

  function replaceBot(bot: EasyLinkBot) {
    easy.state.bots = easy.state.bots.map((current) =>
      current.platform === bot.platform ? bot : current,
    );
  }

  const easy: FakeEasyLink = {
    state: {
      bots: EASY_LINK_PLATFORMS.map(emptyBot),
      accounts: [],
      pairing: null,
    },
    refusedTokens: new Set(),
    failures: new Map(),
    bot(platform) {
      const bot = easy.state.bots.find((b) => b.platform === platform);
      if (!bot) throw simulator.fail(`no bot for "${platform}"`);
      return bot;
    },
    configure(platform, fields = {}) {
      replaceBot(connectedBot(platform, fields));
    },
    addAccount(platform, name, linkedAt = Date.now()) {
      lastAccountId += 1;
      const account = { id: lastAccountId, platform, name, linkedAt };
      easy.state.accounts = [...easy.state.accounts, account];
      return account;
    },
    emit: () => simulator.emitEvent("easy-link", snapshot()),
    async setLink(platform, link) {
      replaceBot({ ...easy.bot(platform), link });
      await easy.emit();
    },
    async linkAccount(platform, name) {
      if (easy.state.pairing?.platform !== platform) {
        throw simulator.fail(`linkAccount: no pairing code for "${platform}"`);
      }
      easy.addAccount(platform, name);
      easy.state.pairing = null;
      await easy.emit();
    },
    async expirePairing() {
      easy.state.pairing = null;
      await easy.emit();
    },
  };

  function failure(command: string) {
    const error = easy.failures.get(command);
    if (error) throw error;
  }

  /** Messagerie d'une commande ; un argument inconnu fait échouer l'essai. */
  function platformOf(command: string, args: Args): EasyLinkPlatform {
    const platform = args.platform as EasyLinkPlatform;
    if (!EASY_LINK_PLATFORMS.includes(platform)) {
      throw simulator.fail(`${command}: bad arguments ${JSON.stringify(args)}`);
    }
    return platform;
  }

  /** Bot relié d'une commande, ou le refus du moteur. */
  function configuredBot(platform: EasyLinkPlatform): EasyLinkBot {
    const bot = easy.bot(platform);
    if (!bot.configured) throw invalid("Connect a bot first");
    return bot;
  }

  /** Liaison établie par le moteur après la réponse de la commande. */
  function connectLater(platform: EasyLinkPlatform) {
    setTimeout(() => {
      if (simulator.disposed) return;
      const bot = easy.bot(platform);
      // Coupé ou retiré entre-temps : plus rien à établir.
      if (bot.link.state !== "connecting") return;
      void easy.setLink(platform, { state: "connected" });
    }, 0);
  }

  simulator.handle("easy_link_state", () => {
    failure("easy_link_state");
    return snapshot();
  });
  simulator.handle("easy_link_connect", (args: Args) => {
    const platform = platformOf("easy_link_connect", args);
    const { token } = args;
    if (typeof token !== "string" || token.trim() === "") {
      throw simulator.fail(
        `easy_link_connect: bad arguments ${JSON.stringify({ platform })}`,
      );
    }
    failure("easy_link_connect");
    if (easy.refusedTokens.has(token)) throw invalid(TOKEN_REFUSED);
    const name = BOT_NAMES[platform];
    replaceBot({
      platform,
      configured: true,
      tokenHint: token.slice(-4),
      enabled: true,
      name,
      link: { state: "connecting" },
      url: botUrl(platform, name),
    });
    connectLater(platform);
    return snapshot();
  });
  simulator.handle("easy_link_disconnect", (args: Args) => {
    const platform = platformOf("easy_link_disconnect", args);
    failure("easy_link_disconnect");
    replaceBot(emptyBot(platform));
    easy.state.accounts = easy.state.accounts.filter(
      (account) => account.platform !== platform,
    );
    if (easy.state.pairing?.platform === platform) easy.state.pairing = null;
    return snapshot();
  });
  simulator.handle("easy_link_set_enabled", (args: Args) => {
    const platform = platformOf("easy_link_set_enabled", args);
    if (typeof args.enabled !== "boolean") {
      throw simulator.fail(
        `easy_link_set_enabled: bad arguments ${JSON.stringify(args)}`,
      );
    }
    failure("easy_link_set_enabled");
    const bot = configuredBot(platform);
    replaceBot({
      ...bot,
      enabled: args.enabled,
      link: { state: args.enabled ? "connecting" : "off" },
    });
    if (args.enabled) connectLater(platform);
    return snapshot();
  });
  simulator.handle("easy_link_pair", (args: Args) => {
    const platform = platformOf("easy_link_pair", args);
    failure("easy_link_pair");
    const bot = configuredBot(platform);
    lastCode += 1;
    const code = String(lastCode);
    easy.state.pairing = {
      platform,
      code,
      expiresAt: Date.now() + PAIRING_MS,
      url:
        platform === "telegram"
          ? `https://t.me/${bot.name}?start=${code}`
          : null,
    };
    return snapshot();
  });
  simulator.handle("easy_link_cancel_pairing", () => {
    failure("easy_link_cancel_pairing");
    easy.state.pairing = null;
    return snapshot();
  });
  simulator.handle("easy_link_unlink", (args: Args) => {
    if (typeof args.accountId !== "number") {
      throw simulator.fail(
        `easy_link_unlink: bad arguments ${JSON.stringify(args)}`,
      );
    }
    failure("easy_link_unlink");
    easy.state.accounts = easy.state.accounts.filter(
      (account) => account.id !== args.accountId,
    );
    return snapshot();
  });

  return easy;
}
