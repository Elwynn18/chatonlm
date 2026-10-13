// Onglet « Easy Link » des réglages (propre à ChatonLM : l'app officielle n'a
// rien de tel) : relier un bot Telegram ou Discord pour parler à ses modèles
// locaux depuis une messagerie. Le moteur tient les bots ; l'onglet règle et
// montre leur état (useEasyLink.ts : lu en arrivant, puis tenu à jour par
// l'évènement « easy-link »).
// - Présentation : ChatonLM doit rester ouvert (zone de notification), et les
//   messages passent par les serveurs de la messagerie.
// - Une carte par messagerie. Sans jeton : les étapes pour créer le bot et le
//   champ du jeton (jamais gardé par la page : il part au moteur, qui n'en
//   rend que les quatre derniers caractères ; refus du moteur en alerte, le
//   jeton reste dans le champ). Avec jeton : nom du bot, indice du jeton,
//   état de la liaison (annoncé : il change sans action de l'utilisateur),
//   interrupteur, invitation sur un serveur (Discord : sans serveur commun,
//   pas de message privé), comptes liés (« Unlink »), « Link an account » (le
//   code à six chiffres, épelé aux lecteurs d'écran, et son heure de fin :
//   pas de minuterie, le moteur retire le code consommé ou expiré), « Remove
//   bot » après confirmation (les comptes sont déliés avec lui).
// - Rappel des commandes du bot.
// Un bouton focalisé qui disparaît laisse le focus dans sa carte (code
// montré, bouton « Link an account » revenu, sinon le titre). Réglages à
// part (tables d'Easy Link) : « Reset to defaults » de General n'y touche
// pas.

import {
  ArrowPathIcon,
  ComputerDesktopIcon,
  LockClosedIcon,
} from "@heroicons/react/20/solid";
import {
  type FormEvent,
  Fragment,
  type ReactNode,
  useId,
  useRef,
  useState,
} from "react";
import type { EasyLinkAccount } from "@/bindings/EasyLinkAccount";
import type { EasyLinkBot } from "@/bindings/EasyLinkBot";
import type { EasyLinkPairing } from "@/bindings/EasyLinkPairing";
import type { EasyLinkPlatform } from "@/bindings/EasyLinkPlatform";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { Row, Switch } from "@/components/settings/Row";
import { useEasyLink, useEasyLinkCommand } from "@/hooks/useEasyLink";
import { useFocusRescue } from "@/hooks/useFocusRescue";
import { type TKey, useT } from "@/i18n";
import { formatDateTime, formatTime } from "@/i18n/format";
import { Trans } from "@/i18n/Trans";
import {
  easyLinkCancelPairing,
  easyLinkConnect,
  easyLinkDisconnect,
  easyLinkPair,
  easyLinkSetEnabled,
  easyLinkUnlink,
  errorMessage,
} from "@/lib/ipc";
import { openLink } from "@/lib/openLink";

/** Où l'on crée le bot : @BotFather, portail des développeurs de Discord. */
const BOT_FACTORY_URLS: Record<EasyLinkPlatform, string> = {
  telegram: "https://t.me/BotFather",
  discord: "https://discord.com/developers/applications",
};

/** Étapes de création du bot ; la première porte le bouton qui y mène. */
const SETUP_STEPS: Record<EasyLinkPlatform, readonly TKey[]> = {
  telegram: [
    "settings.easyLink.setup.telegram.step1",
    "settings.easyLink.setup.telegram.step2",
    "settings.easyLink.setup.telegram.step3",
  ],
  discord: [
    "settings.easyLink.setup.discord.step1",
    "settings.easyLink.setup.discord.step2",
    "settings.easyLink.setup.discord.step3",
    "settings.easyLink.setup.discord.step4",
  ],
};

/**
 * Commandes du bot (jamais traduites, comme lui), avec le nom de leur
 * paramètre s'il y en a un.
 */
const COMMANDS: readonly {
  command: string;
  argument?: TKey;
  description: TKey;
}[] = [
  { command: "/new", description: "settings.easyLink.commands.new" },
  { command: "/stop", description: "settings.easyLink.commands.stop" },
  { command: "/models", description: "settings.easyLink.commands.models" },
  {
    command: "/model",
    argument: "settings.easyLink.commands.modelName",
    description: "settings.easyLink.commands.model",
  },
  { command: "/load", description: "settings.easyLink.commands.load" },
  { command: "/unload", description: "settings.easyLink.commands.unload" },
  { command: "/status", description: "settings.easyLink.commands.status" },
  { command: "/web on|off", description: "settings.easyLink.commands.web" },
  { command: "/help", description: "settings.easyLink.commands.help" },
];

const CARD = "overflow-hidden rounded-xl bg-white dark:bg-neutral-800";
const TITLE =
  "rounded-md text-sm/6 font-medium text-zinc-950 outline-none dark:text-white";
const TEXT = "text-sm/6 text-zinc-950 dark:text-white";
const MUTED = "text-sm/6 text-zinc-500 dark:text-zinc-400";
const ALERT = "text-xs break-words text-red-600 dark:text-red-400";
const DIVIDED = "border-t border-zinc-950/5 pt-4 dark:border-white/10";
// aria-disabled comme disabled : le bouton d'une action en cours garde le
// focus (un bouton désactivé le perd).
const SMALL_BUTTON =
  "inline-flex flex-none items-center gap-1.5 rounded-lg border border-zinc-950/10 bg-white px-3 py-1 text-sm/6 font-medium text-zinc-950 shadow-sm outline-none hover:bg-zinc-50 focus-visible:ring-2 focus-visible:ring-accent-fg disabled:cursor-default disabled:opacity-50 aria-disabled:cursor-default aria-disabled:opacity-50 dark:border-white/15 dark:bg-zinc-900 dark:text-white dark:hover:bg-zinc-800";
const PRIMARY_BUTTON =
  "inline-flex flex-none items-center gap-1.5 rounded-lg border border-zinc-700/90 bg-zinc-800 px-3 py-1 text-sm/6 font-medium text-white shadow-sm outline-none hover:bg-zinc-700 focus-visible:ring-2 focus-visible:ring-accent-fg disabled:cursor-default disabled:opacity-50 aria-disabled:cursor-default aria-disabled:opacity-50 dark:border-white/10 dark:bg-white dark:text-zinc-900 dark:hover:bg-zinc-200";
// select-text : la page des réglages ne se sélectionne pas, une commande ou
// un code à recopier, si.
const CODE =
  "rounded-md bg-neutral-100 px-1.5 py-0.5 font-mono text-[13px] text-neutral-900 select-text dark:bg-neutral-900 dark:text-neutral-100";

const code = (chunks: string) => <code className={CODE}>{chunks}</code>;

export function EasyLink({
  onSaved,
}: {
  /** Un changement est enregistré (« Saved » de la page). */
  onSaved: () => void;
}) {
  const t = useT();
  const { data, error, refetch } = useEasyLink();
  const ids = {
    title: useId(),
    keepOpen: useId(),
    privacy: useId(),
    commands: useId(),
  };

  let cards: ReactNode;
  if (data) {
    cards = data.bots.map((bot) => (
      <BotCard
        key={bot.platform}
        bot={bot}
        accounts={data.accounts.filter(
          (account) => account.platform === bot.platform,
        )}
        pairing={data.pairing?.platform === bot.platform ? data.pairing : null}
        onSaved={onSaved}
      />
    ));
  } else if (error) {
    cards = (
      <div className={CARD}>
        <div className="flex items-start justify-between gap-4 p-4">
          <p
            role="alert"
            className="min-w-0 text-sm/6 break-words text-red-600 dark:text-red-400"
          >
            {t("settings.easyLink.loadFailed", {
              message: errorMessage(error),
            })}
          </p>
          <button
            type="button"
            className={SMALL_BUTTON}
            onClick={() => void refetch()}
          >
            <ArrowPathIcon aria-hidden="true" className="h-4 w-4" />
            {t("settings.tryAgain")}
          </button>
        </div>
      </div>
    );
  } else {
    cards = (
      <div className={CARD}>
        <div className="p-4">
          <div
            data-testid="easy-link-loading"
            aria-hidden="true"
            className="h-24 animate-pulse rounded-lg bg-neutral-200 dark:bg-neutral-700"
          />
        </div>
      </div>
    );
  }

  return (
    <div data-testid="easy-link-settings" className="space-y-4">
      <section aria-labelledby={ids.title} className={CARD}>
        <div className="space-y-4 p-4">
          <div>
            <h2 id={ids.title} className={TITLE}>
              {t("settings.tabs.easyLink")}
            </h2>
            <p className={MUTED}>{t("settings.easyLink.intro.description")}</p>
          </div>
          <Row
            icon={ComputerDesktopIcon}
            label={t("settings.easyLink.intro.keepOpen.label")}
            description={t("settings.easyLink.intro.keepOpen.description")}
            descriptionId={ids.keepOpen}
          />
          <Row
            icon={LockClosedIcon}
            label={t("settings.easyLink.intro.privacy.label")}
            description={t("settings.easyLink.intro.privacy.description")}
            descriptionId={ids.privacy}
          />
        </div>
      </section>

      {cards}

      <section aria-labelledby={ids.commands} className={CARD}>
        <div className="space-y-3 p-4">
          <div>
            <h2 id={ids.commands} className={TITLE}>
              {t("settings.easyLink.commands.title")}
            </h2>
            <p className={MUTED}>{t("settings.easyLink.commands.intro")}</p>
          </div>
          <dl
            data-testid="easy-link-commands"
            className="grid grid-cols-[auto_1fr] items-baseline gap-x-4 gap-y-1.5"
          >
            {COMMANDS.map(({ command, argument, description }) => (
              <Fragment key={command}>
                <dt>
                  <code className={CODE}>
                    {command}
                    {argument && ` <${t(argument)}>`}
                  </code>
                </dt>
                <dd className={TEXT}>{t(description)}</dd>
              </Fragment>
            ))}
          </dl>
          <p className={MUTED}>
            <Trans k="settings.easyLink.commands.discord" tags={{ code }} />
          </p>
          <p className={MUTED}>{t("settings.easyLink.commands.media")}</p>
        </div>
      </section>
    </div>
  );
}

/** Carte d'une messagerie : bot à créer, ou bot relié. */
function BotCard({
  bot,
  accounts,
  pairing,
  onSaved,
}: {
  bot: EasyLinkBot;
  /** Comptes liés de cette messagerie. */
  accounts: EasyLinkAccount[];
  /** Code d'appairage en cours, s'il est pour cette messagerie. */
  pairing: EasyLinkPairing | null;
  onSaved: () => void;
}) {
  const t = useT();
  const titleId = useId();
  const cardRef = useRef<HTMLElement>(null);
  const titleRef = useRef<HTMLHeadingElement>(null);
  // Bouton focalisé retiré : « Link an account » remplacé par le code (le
  // focus va au code, lu aussitôt), code retiré (annulé, consommé, expiré :
  // retour à « Link an account »), sinon (Connect, Unlink, Remove) le titre.
  useFocusRescue(cardRef, (lost) => {
    const card = cardRef.current;
    const next = lost.matches("[data-easy-link-pair]")
      ? card?.querySelector<HTMLElement>("[data-easy-link-code]")
      : lost.closest("[data-easy-link-pairing]")
        ? card?.querySelector<HTMLElement>("[data-easy-link-pair]")
        : null;
    return next ?? titleRef.current;
  });
  const platformName = t(`settings.easyLink.platforms.${bot.platform}`);

  return (
    <section
      ref={cardRef}
      data-testid={`easy-link-${bot.platform}`}
      aria-labelledby={titleId}
      className={CARD}
    >
      <div className="space-y-4 p-4">
        <h2 ref={titleRef} id={titleId} tabIndex={-1} className={TITLE}>
          {platformName}
        </h2>
        {bot.configured ? (
          <LinkedBot
            bot={bot}
            accounts={accounts}
            pairing={pairing}
            platformName={platformName}
            onSaved={onSaved}
          />
        ) : (
          <BotSetup platform={bot.platform} platformName={platformName} />
        )}
      </div>
    </section>
  );
}

/**
 * Bot à créer : les étapes, puis le jeton à coller. Le jeton n'existe que
 * dans ce champ : envoyé au moteur, qui le vérifie auprès de la messagerie ;
 * accepté, la carte passe au bot relié et le champ disparaît avec lui.
 */
function BotSetup({
  platform,
  platformName,
}: {
  platform: EasyLinkPlatform;
  platformName: string;
}) {
  const t = useT();
  const fieldId = useId();
  const errorId = useId();
  const [token, setToken] = useState("");
  const connect = useEasyLinkCommand((value: string) =>
    easyLinkConnect(platform, value),
  );
  // Garde lue hors du rendu : Entrée deux fois avant le rendu qui désactive
  // le bouton ne vérifie pas le jeton deux fois.
  const sending = useRef(false);
  // Un jeton collé vient souvent avec une espace ou un retour à la ligne.
  const cleaned = token.trim();

  const submit = (event: FormEvent) => {
    event.preventDefault();
    if (cleaned === "" || sending.current) return;
    sending.current = true;
    connect.mutate(cleaned, {
      onSuccess: () => setToken(""),
      onSettled: () => {
        sending.current = false;
      },
    });
  };

  return (
    <>
      <div>
        <p className={MUTED}>{t("settings.easyLink.setup.intro")}</p>
        <ol
          data-testid="easy-link-steps"
          // Valeur arbitraire, pas la classe toute faite des listes
          // numérotées : générée (même citée dans un commentaire), elle
          // toucherait les listes des réponses (scripts/check-bundle.mjs).
          className={`mt-1 list-[decimal] space-y-1.5 pl-5 ${TEXT}`}
        >
          {SETUP_STEPS[platform].map((step, index) => (
            <li key={step}>
              <Trans k={step} tags={{ code }} />
              {index === 0 && (
                <div className="mt-1.5">
                  <button
                    type="button"
                    className={SMALL_BUTTON}
                    onClick={() => void openLink(BOT_FACTORY_URLS[platform])}
                  >
                    {t(`settings.easyLink.setup.${platform}.open`)}
                  </button>
                </div>
              )}
            </li>
          ))}
        </ol>
      </div>
      <form onSubmit={submit} className={DIVIDED}>
        <label htmlFor={fieldId} className={TEXT}>
          {t("settings.easyLink.token.label", { platform: platformName })}
        </label>
        <div className="mt-2 flex gap-2">
          <input
            id={fieldId}
            type="password"
            autoComplete="off"
            autoCapitalize="off"
            spellCheck={false}
            value={token}
            onChange={(event) => setToken(event.target.value)}
            aria-invalid={connect.isError || undefined}
            aria-describedby={connect.isError ? errorId : undefined}
            className="h-9 min-w-0 flex-1 rounded-lg border border-neutral-300 bg-white px-3 text-sm text-neutral-900 outline-none focus-visible:border-accent-fg focus-visible:ring-2 focus-visible:ring-accent-fg/40 dark:border-neutral-700 dark:bg-neutral-800 dark:text-neutral-100"
          />
          <button
            type="submit"
            disabled={cleaned === ""}
            // Vérification en cours : aria-disabled, le bouton garde le focus.
            aria-disabled={connect.isPending}
            className="inline-flex h-9 flex-none items-center gap-1.5 rounded-lg border border-neutral-300 px-3 text-sm font-medium text-neutral-800 outline-none hover:bg-neutral-100 focus-visible:ring-2 focus-visible:ring-accent-fg disabled:opacity-60 aria-disabled:cursor-default aria-disabled:opacity-60 dark:border-neutral-700 dark:text-neutral-200 dark:hover:bg-neutral-800"
          >
            {connect.isPending && (
              <ArrowPathIcon
                aria-hidden="true"
                className="h-4 w-4 animate-spin"
              />
            )}
            {connect.isPending
              ? t("settings.easyLink.connect.checking")
              : t("settings.easyLink.connect.button")}
          </button>
        </div>
        {connect.isError && (
          // Texte du moteur, déjà dans la langue affichée : montré tel quel.
          <p id={errorId} role="alert" className={`mt-2 ${ALERT}`}>
            {errorMessage(connect.error)}
          </p>
        )}
      </form>
    </>
  );
}

/** Bot relié : identité, liaison, comptes liés, appairage, retrait. */
function LinkedBot({
  bot,
  accounts,
  pairing,
  platformName,
  onSaved,
}: {
  bot: EasyLinkBot;
  accounts: EasyLinkAccount[];
  pairing: EasyLinkPairing | null;
  platformName: string;
  onSaved: () => void;
}) {
  const t = useT();
  const { platform } = bot;
  const accountsId = useId();
  const inviteId = useId();
  const [confirming, setConfirming] = useState(false);
  // Dernier refus d'une commande de la carte (texte du moteur).
  const [failure, setFailure] = useState<string | null>(null);
  const setEnabled = useEasyLinkCommand((enabled: boolean) =>
    easyLinkSetEnabled(platform, enabled),
  );
  const pair = useEasyLinkCommand(() => easyLinkPair(platform));
  const cancel = useEasyLinkCommand(easyLinkCancelPairing);
  const unlink = useEasyLinkCommand(easyLinkUnlink);
  const disconnect = useEasyLinkCommand(() => easyLinkDisconnect(platform));

  /** Lance une commande, sauf si elle court déjà ; son refus est montré. */
  function run<A>(
    command: {
      isPending: boolean;
      mutateAsync: (argument: A) => Promise<unknown>;
    },
    argument: A,
    after?: () => void,
  ) {
    if (command.isPending) return;
    setFailure(null);
    command.mutateAsync(argument).then(
      () => after?.(),
      (cause: unknown) => setFailure(errorMessage(cause)),
    );
  }

  const inviteUrl = platform === "discord" ? bot.url : null;

  return (
    <>
      <div className="flex items-start justify-between gap-4">
        <div className="min-w-0">
          <p
            data-testid="easy-link-bot-name"
            className={
              bot.name ? `${TEXT} font-medium break-words select-text` : MUTED
            }
          >
            {bot.name ? `@${bot.name}` : t("settings.easyLink.bot.unknownName")}
          </p>
          <p className={MUTED}>
            {t("settings.easyLink.bot.token", { hint: bot.tokenHint })}
          </p>
          {/* Région toujours présente : la liaison change sans action de
              l'utilisateur (perdue, rétablie), son nouvel état est annoncé. */}
          <div
            role="status"
            data-testid="easy-link-status"
            data-state={bot.link.state}
            className="mt-1"
          >
            <LinkStatus bot={bot} platformName={platformName} />
          </div>
        </div>
        <Switch
          label={t("settings.easyLink.bot.enable", { platform: platformName })}
          checked={bot.enabled}
          onChange={(enabled) => run(setEnabled, enabled, onSaved)}
        />
      </div>

      {inviteUrl && (
        <div>
          <p id={inviteId} className={MUTED}>
            {t("settings.easyLink.invite.description")}
          </p>
          <button
            type="button"
            className={`mt-2 ${SMALL_BUTTON}`}
            aria-describedby={inviteId}
            onClick={() => void openLink(inviteUrl)}
          >
            {t("settings.easyLink.invite.button")}
          </button>
        </div>
      )}

      <div className={`space-y-3 ${DIVIDED}`}>
        <h3 id={accountsId} className={TEXT}>
          {t("settings.easyLink.accounts.title")}
        </h3>
        {accounts.length === 0 ? (
          <p className={MUTED}>{t("settings.easyLink.accounts.empty")}</p>
        ) : (
          <ul aria-labelledby={accountsId}>
            {accounts.map((account) => (
              <AccountRow
                key={account.id}
                account={account}
                unlinking={unlink.isPending && unlink.variables === account.id}
                onUnlink={() => run(unlink, account.id)}
              />
            ))}
          </ul>
        )}
        {pairing ? (
          <PairingCode
            pairing={pairing}
            cancelling={cancel.isPending}
            onCancel={() => run(cancel, undefined)}
          />
        ) : (
          <button
            type="button"
            data-easy-link-pair=""
            className={SMALL_BUTTON}
            aria-disabled={pair.isPending}
            onClick={() => run(pair, undefined)}
          >
            {t("settings.easyLink.pairing.start")}
          </button>
        )}
      </div>

      <div className={DIVIDED}>
        {failure !== null && (
          <p role="alert" className={`mb-3 ${ALERT}`}>
            {failure}
          </p>
        )}
        <button
          type="button"
          className={SMALL_BUTTON}
          aria-disabled={disconnect.isPending}
          onClick={() => {
            if (!disconnect.isPending) setConfirming(true);
          }}
        >
          {t("settings.easyLink.bot.remove")}
        </button>
      </div>

      {confirming && (
        <ConfirmDialog
          title={t("settings.easyLink.bot.removeTitle", {
            platform: platformName,
          })}
          message={t("settings.easyLink.bot.removeText")}
          confirmLabel={t("common.actions.remove")}
          onConfirm={() => {
            // Le bouton va disparaître avec le bot : la carte garde le focus
            // (useFocusRescue de BotCard, au titre).
            setConfirming(false);
            run(disconnect, undefined);
          }}
          onCancel={() => setConfirming(false)}
        />
      )}
    </>
  );
}

/** État de la liaison du bot avec sa messagerie, en clair. */
function LinkStatus({
  bot,
  platformName,
}: {
  bot: EasyLinkBot;
  platformName: string;
}) {
  const t = useT();
  const { link } = bot;
  if (link.state === "failed") {
    return (
      <p className="text-sm/6 break-words text-red-600 dark:text-red-400">
        {t(`settings.easyLink.problem.${link.problem}`, {
          platform: platformName,
        })}
      </p>
    );
  }
  if (link.state === "connected") {
    return (
      <span className="inline-flex items-center rounded-md bg-green-100 px-1.5 py-0.5 text-xs/5 font-medium text-green-800 dark:bg-green-800 dark:text-green-100">
        {t("settings.easyLink.link.connected")}
      </span>
    );
  }
  return <p className={MUTED}>{t(`settings.easyLink.link.${link.state}`)}</p>;
}

/** Un compte lié : nom, date de liaison, Unlink. */
function AccountRow({
  account,
  unlinking,
  onUnlink,
}: {
  account: EasyLinkAccount;
  unlinking: boolean;
  onUnlink: () => void;
}) {
  const t = useT();
  const nameId = useId();
  return (
    <li
      data-testid="easy-link-account"
      className="flex items-center justify-between gap-4 border-t border-zinc-950/5 py-3 first:border-t-0 first:pt-0 last:pb-0 dark:border-white/10"
    >
      <div className="min-w-0">
        <div id={nameId} className={`${TEXT} break-words`}>
          {account.name || t("settings.easyLink.accounts.unnamed")}
        </div>
        <p className={MUTED}>
          {t("settings.easyLink.accounts.linkedOn", {
            date: formatDateTime(t.lang, account.linkedAt),
          })}
        </p>
      </div>
      <button
        type="button"
        className={SMALL_BUTTON}
        aria-describedby={nameId}
        aria-disabled={unlinking}
        onClick={onUnlink}
      >
        {t("settings.easyLink.accounts.unlink")}
      </button>
    </li>
  );
}

/**
 * Code d'appairage en cours : les six chiffres en grand, leur heure de fin de
 * validité et la marche à suivre. Les chiffres restent un seul texte (copié
 * sans espace), espacés par la feuille de style ; un lecteur d'écran les
 * entend un par un (« 4 8 2 9 1 5 », pas « quatre cent quatre-vingt-deux
 * mille... »).
 */
function PairingCode({
  pairing,
  cancelling,
  onCancel,
}: {
  pairing: EasyLinkPairing;
  cancelling: boolean;
  onCancel: () => void;
}) {
  const t = useT();
  const { url } = pairing;
  const spelled = t("settings.easyLink.pairing.codeLabel", {
    code: [...pairing.code].join(" "),
  });
  return (
    <div
      data-easy-link-pairing=""
      data-testid="easy-link-pairing"
      className="space-y-2 rounded-lg border border-neutral-200 bg-neutral-50 p-4 dark:border-neutral-700 dark:bg-neutral-900"
    >
      {/* role img : un seul objet pour le lecteur d'écran, qui lit le nom
          (chiffres épelés) et non le nombre. Il prend le focus à son
          apparition (BotCard) : le code est lu aussitôt. */}
      <div
        role="img"
        aria-label={spelled}
        tabIndex={-1}
        data-easy-link-code=""
        data-testid="easy-link-code"
        className="rounded-md font-mono text-3xl/10 font-medium tracking-[0.3em] text-zinc-950 outline-none select-text dark:text-white"
      >
        {pairing.code}
      </div>
      <p className={MUTED}>
        {t("settings.easyLink.pairing.expires", {
          time: formatTime(t.lang, pairing.expiresAt),
        })}
      </p>
      <p className={TEXT}>
        {t(`settings.easyLink.pairing.${pairing.platform}`)}
      </p>
      <div className="flex flex-wrap gap-2 pt-1">
        {url && (
          <button
            type="button"
            className={PRIMARY_BUTTON}
            onClick={() => void openLink(url)}
          >
            {t("settings.easyLink.pairing.openTelegram")}
          </button>
        )}
        <button
          type="button"
          className={SMALL_BUTTON}
          aria-disabled={cancelling}
          onClick={onCancel}
        >
          {t("common.actions.cancel")}
        </button>
      </div>
    </div>
  );
}
