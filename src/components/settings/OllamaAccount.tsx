/*
 * SPDX-License-Identifier: MIT
 * SPDX-FileCopyrightText: Ollama
 * SPDX-FileCopyrightText: 2026 Elwynn18 and ChatonLM contributors
 *
 * Adapted from Ollama (https://github.com/ollama/ollama),
 * file app/ui/app/src/components/Settings.tsx, commit 42e911bc3d05.
 * Modified for ChatonLM: the account card on its own; the sign-in page is
 * opened by the backend (window.open is not available), the links by the
 * browser through the backend (failure notice of LinkErrorNotice); the
 * skeleton only while the account was never read (not on each re-read);
 * Sign out does not reload the window: the account is read again (shown
 * signed out at once, restored if signing out fails); the avatar is a data:
 * URL read by the backend, hidden if it cannot be shown; the error box is an
 * alert; Sign In disabled with its dots while the page is being opened (a
 * double click opened two pages); cloud turned off on the server (known):
 * the account is not read (the server would still ask ollama.com), the card
 * says why, with the official OLLAMA_NO_CLOUD text or "Cloud is turned off
 * on the Ollama server." (ours), without buttons; opened from the web search
 * error (section=account), the account is read again and the card is
 * scrolled into view, Sign In or the card itself (region) taking the focus,
 * unless the user has put it elsewhere meanwhile; controls rewritten (no copy
 * of the original ui/ components), the layers of the Upgrade button being
 * spans (no block inside a button); an account that cannot be read (Ollama
 * in error or unreachable) is never shown as "Not connected" with Sign In
 * (the official app does, after its retries): from the first failed try,
 * "Couldn't check your Ollama account: <reason>" with "Retrying…" while the
 * retries go on, then Try again (the card keeps the focus); opened for the
 * account, the focus goes to Sign In, else to the card (failure included);
 * when the reason is Ollama's own folder hidden from its systemd service
 * ("account unavailable", "id_ed25519: permission denied"), a link to the
 * fix in Settings > General (settings/OllamaHome.tsx); texts in English,
 * Spanish and French (src/i18n), the English ones unchanged.
 */

import { useEffect, useRef, useState } from "react";
import { OllamaHomeLink } from "@/components/settings/OllamaHome";
import { useAccount, useSignIn } from "@/hooks/useAccount";
import { useCloudStatus } from "@/hooks/useCloudStatus";
import { type TFunction, useT } from "@/i18n";
import { errorMessage, isAppError } from "@/lib/ipc";
import { openLink } from "@/lib/openLink";

const UPGRADE_URL = "https://ollama.com/upgrade";
const MANAGE_URL = "https://ollama.com/settings";

const BUTTON =
  "cursor-pointer rounded-lg border px-3 py-2 text-sm font-medium shadow-sm outline-none focus-visible:ring-2 focus-visible:ring-accent-fg disabled:cursor-default disabled:opacity-50";
const WHITE = `${BUTTON} border-zinc-950/10 bg-white text-zinc-950 hover:bg-zinc-50 dark:border-white/15 dark:bg-zinc-900 dark:text-white dark:hover:bg-zinc-800`;
const ZINC = `${BUTTON} border-zinc-700/90 bg-zinc-600 text-white hover:bg-zinc-500 dark:border-white/5 dark:hover:bg-zinc-500`;

function AnimatedDots() {
  return (
    <span className="inline-flex">
      <span className="animate-pulse">.</span>
      <span className="animate-pulse" style={{ animationDelay: "0.2s" }}>
        .
      </span>
      <span className="animate-pulse" style={{ animationDelay: "0.4s" }}>
        .
      </span>
    </span>
  );
}

/** Pourquoi le compte n'est pas lu : nuage coupé, par l'environnement ou non. */
function cloudOffText(t: TFunction, source: string) {
  return source === "env" || source === "both"
    ? t("settings.account.cloudOffEnv")
    : t("settings.account.cloudOff");
}

/**
 * Échec de Sign In (`reason` : message du moteur, vide pour le texte
 * officiel par défaut) ou de Sign out ; écrit au rendu.
 */
type ConnectionError =
  { action: "signIn"; reason: string } | { action: "signOut" };

/** Message d'un échec de la connexion, ou le texte officiel par défaut. */
function signInError(error: unknown): ConnectionError {
  const reason =
    isAppError(error) || error instanceof Error ? errorMessage(error) : "";
  return { action: "signIn", reason };
}

function connectionErrorText(t: TFunction, error: ConnectionError) {
  if (error.action === "signOut") return t("settings.account.disconnectFailed");
  return error.reason || t("settings.account.connectFailed");
}

/**
 * Carte « Ollama account » des réglages : compte relié (nom, e-mail,
 * Upgrade pour l'offre gratuite, Manage, Sign out, avatar) ou non (Sign In).
 * Après Sign In, le compte est relu toutes les 5 s, jusqu'au retour sur la
 * fenêtre (une dernière relecture) ou à la connexion. Nuage coupé sur le
 * serveur : le compte n'est pas lu (le serveur interrogerait ollama.com
 * quand même), il attend donc l'état du nuage. `focusOnShow` : une fois son
 * contenu connu, la carte défile en vue et Sign In prend le focus (la carte
 * elle-même s'il n'y a rien à relier) ; rien ne s'ouvre dans le navigateur.
 */
export function OllamaAccount({
  focusOnShow = false,
}: { focusOnShow?: boolean } = {}) {
  const t = useT();
  const cloud = useCloudStatus();
  const cloudOff = cloud.cloudStatus?.disabled === true;
  const {
    account,
    isLoading: accountLoading,
    isAuthenticated,
    refresh,
    isRefreshing,
    signOut,
    isFetching,
    failure,
    retrying,
    retry,
  } = useAccount({ enabled: cloud.isSettled && !cloudOff, fresh: focusOnShow });
  const isLoading = !cloud.isSettled || accountLoading;
  const signIn = useSignIn({ isAuthenticated, refresh });
  const [connectionError, setConnectionError] =
    useState<ConnectionError | null>(null);
  // Avatar qui n'a pas pu être affiché (caché, comme l'officiel).
  const [failedAvatar, setFailedAvatar] = useState<string | null>(null);

  const handleConnectOllamaAccount = async () => {
    setConnectionError(null);
    // If user is already authenticated, no need to connect
    if (isAuthenticated) return;
    try {
      await signIn.start();
    } catch (error) {
      console.error("Error connecting to Ollama account:", error);
      setConnectionError(signInError(error));
    }
  };

  const handleDisconnectOllamaAccount = async () => {
    setConnectionError(null);
    signIn.stop();
    try {
      await signOut();
    } catch {
      setConnectionError({ action: "signOut" });
    }
  };

  const user = account?.state === "signedIn" ? account : null;
  // Page en cours d'ouverture, compte attendu ou relu.
  const busy = isRefreshing || signIn.starting || signIn.awaiting;
  const avatar = user?.avatar ?? null;

  const cardRef = useRef<HTMLDivElement>(null);
  const signInRef = useRef<HTMLButtonElement>(null);
  const focusedRef = useRef(false);
  // Contenu à jour : le compte relu en arrivant (il a pu être relié au
  // terminal depuis) avant de choisir où mettre le focus ; ou son échec.
  const shown = cloudOff || failure !== null || (!isLoading && !isFetching);
  useEffect(() => {
    if (!focusOnShow || !shown || focusedRef.current) return;
    focusedRef.current = true;
    const card = cardRef.current;
    if (!card) return;
    // L'utilisateur est déjà ailleurs (lecture longue, champ d'adresse...) :
    // ni défilement ni focus volé (Espace ouvrirait le navigateur).
    const active = document.activeElement;
    if (active !== null && active !== document.body && !card.contains(active)) {
      return;
    }
    // jsdom n'a pas scrollIntoView.
    card.scrollIntoView?.({ block: "nearest" });
    // Lecture en échec : la carte (Try again n'y est pas encore, ou sera
    // remplacé par « Retrying… » dès la relecture faite en arrivant).
    (signInRef.current ?? card).focus({ preventScroll: true });
  }, [focusOnShow, shown]);

  const handleRetry = () => {
    // Try again laisse place à « Retrying… » : le focus reste dans la carte.
    cardRef.current?.focus({ preventScroll: true });
    retry();
  };

  return (
    <div
      ref={cardRef}
      data-testid="ollama-account"
      role="region"
      aria-label={t("settings.account.title")}
      tabIndex={-1}
      className="overflow-hidden rounded-xl bg-white outline-none dark:bg-neutral-800"
    >
      <div className="p-4">
        {cloudOff && cloud.cloudStatus ? (
          <div>
            <div className="text-sm/6 text-zinc-950 dark:text-white">
              {t("settings.account.title")}
            </div>
            <p className="text-sm/6 text-zinc-500 dark:text-zinc-400">
              {cloudOffText(t, cloud.cloudStatus.source)}
            </p>
          </div>
        ) : failure !== null ? (
          // Lecture en échec : la raison, jamais le squelette ni « Not
          // connected ».
          <div
            data-testid="account-failure"
            className="flex items-start justify-between gap-4"
          >
            <div className="min-w-0">
              <div className="text-sm/6 text-zinc-950 dark:text-white">
                {t("settings.account.title")}
              </div>
              <p
                role="alert"
                className="text-sm/6 break-words text-red-600 dark:text-red-400"
              >
                {t("settings.account.checkError", { reason: failure })}
              </p>
              <OllamaHomeLink error={failure} />
              {retrying && (
                <p className="text-sm/6 text-zinc-500 dark:text-zinc-400">
                  {t("settings.account.retrying")}
                </p>
              )}
            </div>
            {!retrying && (
              <button
                type="button"
                className={`${WHITE} flex-none`}
                onClick={handleRetry}
              >
                {t("settings.tryAgain")}
              </button>
            )}
          </div>
        ) : isLoading ? (
          // Loading skeleton, this will only happen if the app started recently
          <div
            data-testid="account-loading"
            className="flex items-center justify-between"
          >
            <div className="space-y-2">
              <div className="h-4 w-24 animate-pulse rounded bg-neutral-200 dark:bg-neutral-700"></div>
              <div className="h-3 w-32 animate-pulse rounded bg-neutral-200 dark:bg-neutral-700"></div>
            </div>
            <div className="h-10 w-10 animate-pulse rounded-full bg-neutral-200 dark:bg-neutral-700"></div>
          </div>
        ) : user ? (
          <div className="flex items-center justify-between gap-4">
            <div className="min-w-0">
              <div className="flex items-center space-x-2">
                <div className="truncate text-sm font-medium text-neutral-900 dark:text-white">
                  {user.name}
                </div>
              </div>
              <p className="truncate text-sm text-neutral-500 dark:text-neutral-400">
                {user.email}
              </p>
              <div className="mt-2 flex items-center space-x-2">
                {user.plan === "free" && (
                  <button
                    type="button"
                    className="cursor-pointer rounded-lg bg-accent px-3 py-2 text-sm font-medium text-neutral-900 outline-none hover:bg-accent/85 focus-visible:ring-2 focus-visible:ring-accent-fg focus-visible:ring-offset-2 dark:focus-visible:ring-offset-neutral-800"
                    onClick={() => void openLink(UPGRADE_URL)}
                  >
                    {t("settings.account.upgrade")}
                  </button>
                )}
                <button
                  type="button"
                  className={WHITE}
                  onClick={() => void openLink(MANAGE_URL)}
                >
                  {t("settings.account.manage")}
                </button>
                <button
                  type="button"
                  className={ZINC}
                  onClick={() => void handleDisconnectOllamaAccount()}
                >
                  {t("settings.account.signOut")}
                </button>
              </div>
            </div>
            {avatar && avatar !== failedAvatar && (
              <img
                src={avatar}
                alt={user.name}
                className="h-10 w-10 flex-shrink-0 rounded-full bg-neutral-200 dark:bg-neutral-700"
                onError={() => setFailedAvatar(avatar)}
              />
            )}
          </div>
        ) : (
          <div className="flex items-center justify-between gap-4">
            <div>
              <div className="text-sm/6 text-zinc-950 dark:text-white">
                {t("settings.account.title")}
              </div>
              <p className="text-sm/6 text-zinc-500 dark:text-zinc-400">
                {t("settings.account.notConnected")}
              </p>
            </div>
            <button
              ref={signInRef}
              type="button"
              className={WHITE}
              onClick={() => void handleConnectOllamaAccount()}
              disabled={busy}
              aria-label={t("settings.account.signIn")}
            >
              {busy ? <AnimatedDots /> : t("settings.account.signIn")}
            </button>
          </div>
        )}
        {connectionError && (
          <div
            role="alert"
            className="mt-3 rounded-lg border border-red-200 bg-red-50 p-3 dark:border-red-800 dark:bg-red-900/20"
          >
            <p className="text-sm text-red-600 dark:text-red-400">
              {connectionErrorText(t, connectionError)}
            </p>
          </div>
        )}
      </div>
    </div>
  );
}
