/*
 * SPDX-License-Identifier: MIT
 * SPDX-FileCopyrightText: Ollama
 * SPDX-FileCopyrightText: 2026 Elwynn18 and ChatonLM contributors
 *
 * Adapted from Ollama (https://github.com/ollama/ollama),
 * file app/ui/app/src/components/Settings.tsx, commit 42e911bc3d05.
 * Modified for ChatonLM: tabs instead of one page (settings/SettingsTabs.tsx,
 * the "section" search parameter): General (default), Models, Account,
 * Dictation, Read aloud, Voice mode, Easy Link and Keyboard shortcuts; changing tab replaces the history entry and leaves the focus on
 * the tab. General: one card with the interface language (ChatonLM's own row,
 * first: System, English, Español, Français, each language written in
 * itself; the whole app switches at once, without reloading), server address,
 * model location, context length and theme (no cloud switch: ChatonLM does not
 * manage the server; no network or apps), ChatonLM's own "Updates" card
 * (settings/Updates.tsx: installed versions, "Check for updates", state of
 * the update, the official "Auto-download updates" switch; untouched by
 * "Reset to defaults"; scrolled into view and focused each time the page is
 * opened for it from the system tray, card=updates), then "Reset to
 * defaults", then ChatonLM's own "Support ChatonLM" card, last
 * (settings/Donate.tsx: what donations pay for, and a button that opens the
 * donation page of the website);
 * Account: the Ollama
 * account card (settings/OllamaAccount.tsx), focused when the page is opened
 * for it (section=account); the model location is the
 * folder of the Ollama server the app talks to, changed in its systemd
 * service (see settings/ModelLocation.tsx), with an alert under it when that
 * service can't see its own .ollama folder (settings/OllamaHome.tsx, "Fix");
 * the context length
 * is sent with each request, with a "Default" stop (the server's choice), and
 * is never disabled; "Saved" only once a change is saved, an error when it
 * isn't; "Reset to defaults" resets the language, the address, the context
 * length and the theme in one write, without clearing the selected model nor touching the
 * model location or web search; the context length knob stays on the chosen
 * step while it is saved; "Saved" announced again on each save; controls
 * rewritten (no copy of the original ui/ components); Dictation: ChatonLM's
 * own cards (settings/Dictation.tsx), shown and focused each time the page
 * is opened for them (section=dictation), untouched by "Reset to defaults";
 * Models: ChatonLM's own cards (settings/Models.tsx: how long models stay
 * loaded, preloading of the chosen model, the models in memory), untouched
 * by "Reset to defaults"; texts in English, Spanish and French (src/i18n),
 * the English ones unchanged; General also has ChatonLM's own "Ollama" row,
 * before the server address (settings/OllamaSource.tsx: which Ollama serves,
 * the one of this computer or the one built into ChatonLM, with its version,
 * and the button that switches to the other); while the built-in Ollama is
 * chosen, the server address shows its address, read-only (chosen for lack
 * of another Ollama, the address field shows http://localhost:11434, never
 * its private address), and the model location is its folder; models left in
 * its folder are pointed out under
 * the model location, and so is a missing folder chosen for it (the default
 * one used instead); Keyboard shortcuts: ChatonLM's own cards
 * (settings/Shortcuts.tsx: system-wide shortcuts of dictation and of the
 * correction of the selected text, then the app shortcuts),
 * untouched by "Reset to defaults". General also has ChatonLM's own
 * "Close button" row (settings/CloseButton.tsx: hide to the system tray or
 * quit) and "Applications menu" row, last, only when running from an
 * AppImage (settings/MenuEntry.tsx), both untouched by "Reset to defaults".
 * Read aloud:
 * ChatonLM's own cards (settings/ReadAloud.tsx: reading new responses aloud
 * automatically, the state of the voices), the switch turned off by "Reset
 * to defaults" in the same write. Voice mode: ChatonLM's own cards
 * (settings/VoiceMode.tsx: taking turns, pause, interrupting by speaking,
 * thinking, voice instructions, links to dictation and voices), untouched
 * by "Reset to defaults". Easy Link: ChatonLM's own cards
 * (settings/EasyLink.tsx: the Telegram and Discord bots that let one talk to
 * the local models from a messenger, their linked accounts, the commands of
 * the bots), untouched by "Reset to defaults".
 */

import {
  ArrowPathIcon,
  CogIcon,
  FolderIcon,
  LanguageIcon,
  ServerIcon,
  SwatchIcon,
} from "@heroicons/react/20/solid";
import { useQueryClient } from "@tanstack/react-query";
import { useEffect, useId, useRef, useState } from "react";
import type { LanguageChoice } from "@/bindings/LanguageChoice";
import type { SettingsPatch } from "@/bindings/SettingsPatch";
import type { Theme } from "@/bindings/Theme";
import { AddressField } from "@/components/AddressField";
import { Dictation } from "@/components/settings/Dictation";
import { CloseButtonRow } from "@/components/settings/CloseButton";
import { Donate } from "@/components/settings/Donate";
import { EasyLink } from "@/components/settings/EasyLink";
import { MenuEntryRow } from "@/components/settings/MenuEntry";
import { ModelLocation } from "@/components/settings/ModelLocation";
import { Models } from "@/components/settings/Models";
import { OllamaHomeAlert } from "@/components/settings/OllamaHome";
import {
  BuiltinLeftoverNotice,
  BuiltinModelsDirNotice,
  OllamaRow,
} from "@/components/settings/OllamaSource";
import { OllamaAccount } from "@/components/settings/OllamaAccount";
import { Row, RowError } from "@/components/settings/Row";
import { ReadAloud } from "@/components/settings/ReadAloud";
import { VoiceMode } from "@/components/settings/VoiceMode";
import { Shortcuts } from "@/components/settings/Shortcuts";
import { Updates } from "@/components/settings/Updates";
import {
  type SettingsTab,
  SettingsTabs,
} from "@/components/settings/SettingsTabs";
import { Slider } from "@/components/settings/Slider";
import { ollamaRuntimeKey, useOllamaRuntime } from "@/hooks/useOllamaRuntime";
import { useSettings } from "@/hooks/useSettings";
import { LANGUAGES, type TKey, useT } from "@/i18n";
import { nativeLanguageName } from "@/i18n/format";
import { errorMessage } from "@/lib/ipc";
import { DEFAULT_ADDRESS } from "@/lib/serverAdvice";

const savedConfirmationDuration = 3000;

/** Crans de Context length après « Default » (0, traduit au rendu). */
const CONTEXT_LENGTH_STEPS = [
  { value: 4096, label: "4k" },
  { value: 8192, label: "8k" },
  { value: 16384, label: "16k" },
  { value: 32768, label: "32k" },
  { value: 65536, label: "64k" },
  { value: 131072, label: "128k" },
  { value: 262144, label: "256k" },
];

const THEMES: { value: Theme; label: TKey }[] = [
  { value: "system", label: "settings.general.theme.system" },
  { value: "light", label: "settings.general.theme.light" },
  { value: "dark", label: "settings.general.theme.dark" },
];

const DEFAULT_SETTINGS = {
  language: "system",
  serverUrl: "",
  contextLength: 0,
  theme: "system",
  // Lecture automatique (onglet Read aloud) : coupée.
  autoRead: false,
} as const satisfies SettingsPatch;

type RowField = "language" | "contextLength" | "theme";

/** Onglet des réglages (paramètre `section` de l'adresse). */
export type SettingsSection = SettingsTab;

/** Carte de General qui prend le focus (paramètre `card` de l'adresse). */
export type SettingsCard = "updates";

/** Onglet d'une valeur brute du paramètre (General par défaut). */
function tabOf(section: unknown): SettingsTab {
  return section === "models" ||
    section === "account" ||
    section === "dictation" ||
    section === "readAloud" ||
    section === "voiceMode" ||
    section === "easyLink" ||
    section === "shortcuts"
    ? section
    : "general";
}

/**
 * Page des réglages, en onglets ; `section` « account » : la carte du compte
 * prend le focus (lien de l'erreur de recherche web) ; « dictation » : la
 * carte de la dictée (menu de la zone de notification) ; sinon General,
 * avec `card` « updates » : la carte des mises à jour (menu de la zone de
 * notification).
 */
export function Settings({
  section,
  card,
  request = null,
}: {
  section?: SettingsSection;
  /** Carte de General à montrer (paramètre `card` de l'adresse). */
  card?: SettingsCard;
  /**
   * Navigation qui a ouvert la page ou l'onglet (clé d'historique), null
   * pour un simple changement d'onglet : la carte demandée prend le focus à
   * chaque nouvelle demande.
   */
  request?: string | null;
} = {}) {
  const tab = tabOf(section);
  const t = useT();
  const { settings, error, setSettings } = useSettings({ refresh: true });
  const queryClient = useQueryClient();
  const { data: runtime } = useOllamaRuntime({ enabled: tab === "general" });
  // L'intégré choisi : l'adresse réglée ne sert plus, la sienne est montrée.
  const builtinChosen =
    runtime?.source === "builtin" && runtime.kind === "builtin";
  const [showSaved, setShowSaved] = useState(false);
  // Nouveau badge à chaque succès : remplacé, il est annoncé de nouveau.
  const [savedCount, setSavedCount] = useState(0);
  const [resetting, setResetting] = useState(false);
  // Échec du dernier Reset (texte écrit au rendu, dans la langue affichée).
  const [resetFailed, setResetFailed] = useState(false);
  const [rowErrors, setRowErrors] = useState<Partial<Record<RowField, string>>>(
    {},
  );
  // Change à chaque Reset réussi : le brouillon d'adresse est abandonné.
  const [resets, setResets] = useState(0);
  // Cran choisi en attente de la réponse : la pastille y reste (pas de
  // retour à l'ancien cran) et un second appui part de lui.
  const [pendingContext, setPendingContext] = useState<number | null>(null);
  const pendingContextSaves = useRef(0);
  const savedTimer = useRef<number | null>(null);
  // Une réponse peut arriver après la sortie de la page : rien n'est alors
  // affiché ni programmé.
  const mounted = useRef(false);
  const addressId = useId();
  const modelsId = useId();
  const ids = {
    language: useId(),
    languageDescription: useId(),
    ollamaDescription: useId(),
    address: useId(),
    addressDescription: useId(),
    modelsDescription: useId(),
    context: useId(),
    contextDescription: useId(),
    theme: useId(),
    themeDescription: useId(),
  };

  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
      if (savedTimer.current !== null) window.clearTimeout(savedTimer.current);
    };
  }, []);

  const hideSaved = () => {
    if (savedTimer.current !== null) window.clearTimeout(savedTimer.current);
    savedTimer.current = null;
    setShowSaved(false);
  };

  // Seulement après la réponse du backend (l'officiel l'affiche dès le clic
  // pour la longueur de contexte, même si l'enregistrement échoue).
  const showSavedConfirmation = () => {
    if (!mounted.current) return;
    if (savedTimer.current !== null) window.clearTimeout(savedTimer.current);
    setShowSaved(true);
    setSavedCount((count) => count + 1);
    savedTimer.current = window.setTimeout(() => {
      setShowSaved(false);
      savedTimer.current = null;
    }, savedConfirmationDuration);
  };

  const saveField = async (field: RowField, patch: SettingsPatch) => {
    setRowErrors((errors) => ({ ...errors, [field]: undefined }));
    try {
      await setSettings(patch);
    } catch (cause) {
      setRowErrors((errors) => ({ ...errors, [field]: errorMessage(cause) }));
      return;
    }
    showSavedConfirmation();
  };

  const saveContextLength = async (contextLength: number) => {
    setPendingContext(contextLength);
    pendingContextSaves.current += 1;
    await saveField("contextLength", { contextLength });
    pendingContextSaves.current -= 1;
    if (pendingContextSaves.current === 0 && mounted.current) {
      setPendingContext(null);
    }
  };

  // Renvoie l'adresse enregistrée (normalisée par le moteur) au champ.
  const saveAddress = async (url: string) => {
    let saved;
    try {
      saved = await setSettings({ serverUrl: url });
    } catch (cause) {
      throw new Error(errorMessage(cause), { cause });
    }
    showSavedConfirmation();
    // L'Ollama qui sert peut changer avec l'adresse.
    void queryClient.invalidateQueries({ queryKey: ollamaRuntimeKey });
    return saved.serverUrl;
  };

  const reset = async () => {
    if (resetting) return;
    setResetting(true);
    hideSaved();
    setResetFailed(false);
    setRowErrors({});
    try {
      await setSettings(DEFAULT_SETTINGS);
      if (!mounted.current) return;
      setResets((count) => count + 1);
      showSavedConfirmation();
    } catch (cause) {
      console.error("Failed to reset settings:", cause);
      setResetFailed(true);
    } finally {
      setResetting(false);
    }
  };

  if (error) {
    return (
      <div className="flex flex-1 items-center justify-center">
        <div className="text-red-600 dark:text-red-400">
          {t("settings.loadFailed")}
        </div>
      </div>
    );
  }
  if (!settings) return null;

  return (
    <main
      data-testid="settings"
      className="flex min-h-0 w-full flex-1 flex-col select-none dark:bg-neutral-900"
    >
      <SettingsTabs current={tab} />
      <div className="w-full flex-1 overflow-y-auto overscroll-contain p-6 pt-4">
        {tab === "models" && (
          <div className="mx-auto max-w-4xl min-w-0">
            {/* Réglages du chargement : Reset (General) n'y touche pas. */}
            <Models onSaved={showSavedConfirmation} />
          </div>
        )}

        {tab === "account" && (
          <div className="mx-auto max-w-4xl min-w-0">
            <OllamaAccount focusOnShow={request !== null} />
          </div>
        )}

        {tab === "dictation" && (
          // min-w-0 : la commande à copier (raccourci) défile dans son bloc.
          <div className="mx-auto max-w-4xl min-w-0">
            {/* Réglages à part (table de la dictée) : Reset n'y touche pas. */}
            <Dictation focusRequest={request} onSaved={showSavedConfirmation} />
          </div>
        )}

        {tab === "readAloud" && (
          <div className="mx-auto max-w-4xl min-w-0">
            {/* Lecture automatique : Reset (General) la coupe. */}
            <ReadAloud onSaved={showSavedConfirmation} />
          </div>
        )}

        {tab === "voiceMode" && (
          <div className="mx-auto max-w-4xl min-w-0">
            {/* Réglages à part (table du mode vocal) : Reset n'y touche pas. */}
            <VoiceMode onSaved={showSavedConfirmation} />
          </div>
        )}

        {tab === "easyLink" && (
          <div className="mx-auto max-w-4xl min-w-0">
            {/* Bots et comptes liés à part : Reset (General) n'y touche pas. */}
            <EasyLink onSaved={showSavedConfirmation} />
          </div>
        )}

        {tab === "shortcuts" && (
          // min-w-0 : la commande à copier (repli sans portail) défile.
          <div className="mx-auto max-w-4xl min-w-0">
            {/* Raccourcis à part : Reset n'y touche pas. */}
            <Shortcuts onSaved={showSavedConfirmation} />
          </div>
        )}

        {tab === "general" && (
          <fieldset
            disabled={resetting}
            aria-busy={resetting}
            // min-w-0 : un fieldset prend au moins la largeur de son contenu
            // (min-inline-size: min-content) ; une longue commande à copier
            // (Model location) élargirait sinon toute la carte au-delà de la
            // fenêtre, au lieu de défiler dans son bloc.
            className="mx-auto max-w-4xl min-w-0 space-y-4 border-0 p-0"
          >
            <div className="relative overflow-hidden rounded-xl bg-white dark:bg-neutral-800">
              <div className="space-y-4 p-4">
                {/* Propre à ChatonLM, en tête : trouvée sans lire la langue
                    affichée (chaque langue est écrite dans elle-même). */}
                <div className="flex items-start justify-between gap-4">
                  <Row
                    icon={LanguageIcon}
                    label={
                      <span id={ids.language}>
                        {t("settings.general.language.label")}
                      </span>
                    }
                    description={t("settings.general.language.description")}
                    descriptionId={ids.languageDescription}
                  >
                    <RowError message={rowErrors.language} />
                  </Row>
                  <div className="flex-shrink-0">
                    <Choice
                      options={[
                        {
                          value: "system",
                          label: t("settings.general.language.system"),
                        },
                        ...LANGUAGES.map((code) => ({
                          value: code,
                          label: nativeLanguageName(code),
                          lang: code,
                        })),
                      ]}
                      value={settings.language}
                      onChange={(language: LanguageChoice) =>
                        void saveField("language", { language })
                      }
                      labelledBy={ids.language}
                      describedBy={ids.languageDescription}
                    />
                  </div>
                </div>

                <OllamaRow
                  runtime={runtime}
                  onSaved={showSavedConfirmation}
                  descriptionId={ids.ollamaDescription}
                />

                <Row
                  icon={ServerIcon}
                  label={
                    <label id={ids.address} htmlFor={addressId}>
                      {t("settings.general.address.label")}
                    </label>
                  }
                  description={
                    builtinChosen
                      ? t("settings.ollama.address")
                      : t("chat.server.addressHint")
                  }
                  descriptionId={ids.addressDescription}
                >
                  <div className="mt-3">
                    {builtinChosen ? (
                      <input
                        id={addressId}
                        value={runtime.url}
                        readOnly
                        aria-describedby={ids.addressDescription}
                        className="h-9 w-full min-w-0 rounded-lg border border-neutral-300 bg-neutral-50 px-3 text-sm text-neutral-700 outline-none focus-visible:border-accent-fg focus-visible:ring-2 focus-visible:ring-accent-fg/40 dark:border-neutral-700 dark:bg-neutral-900 dark:text-neutral-300"
                      />
                    ) : (
                      <AddressField
                        key={resets}
                        id={addressId}
                        saved={settings.serverUrl}
                        // L'intégré sert faute d'autre Ollama : son adresse
                        // est privée ; un champ vide donne celle-ci.
                        effective={
                          runtime?.kind === "builtin"
                            ? DEFAULT_ADDRESS
                            : settings.effectiveServerUrl
                        }
                        onSave={saveAddress}
                        describedBy={ids.addressDescription}
                      />
                    )}
                  </div>
                </Row>

                {/* Dossier du serveur d'Ollama, pas un réglage de l'app : Reset
                  n'y touche pas. */}
                <Row
                  icon={FolderIcon}
                  label={
                    <label htmlFor={modelsId}>
                      {t("settings.general.modelLocation.label")}
                    </label>
                  }
                  description={t("settings.general.modelLocation.description")}
                  descriptionId={ids.modelsDescription}
                >
                  <ModelLocation
                    inputId={modelsId}
                    describedBy={ids.modelsDescription}
                    serverUrl={settings.effectiveServerUrl}
                    onSaved={showSavedConfirmation}
                  />
                  {/* Clé d'Ollama cachée à son service (ProtectHome=). */}
                  <OllamaHomeAlert onSaved={showSavedConfirmation} />
                  {/* Modèles restés chez l'intégré, celui de la machine servant. */}
                  <BuiltinLeftoverNotice
                    runtime={runtime}
                    onSaved={showSavedConfirmation}
                  />
                  {/* Dossier choisi pour l'intégré introuvable : celui par défaut. */}
                  <BuiltinModelsDirNotice runtime={runtime} />
                </Row>

                <Row
                  icon={CogIcon}
                  label={
                    <span id={ids.context}>
                      {t("settings.general.contextLength.label")}
                    </span>
                  }
                  description={t("settings.general.contextLength.description")}
                  descriptionId={ids.contextDescription}
                >
                  <div className="mt-3">
                    <Slider
                      options={[
                        {
                          value: 0,
                          label: t("settings.general.contextLength.default"),
                        },
                        ...CONTEXT_LENGTH_STEPS,
                      ]}
                      value={pendingContext ?? settings.contextLength}
                      onChange={(contextLength) =>
                        void saveContextLength(contextLength)
                      }
                      disabled={resetting}
                      labelledBy={ids.context}
                      describedBy={ids.contextDescription}
                    />
                  </div>
                  <RowError message={rowErrors.contextLength} />
                </Row>

                <div className="flex items-start justify-between gap-4">
                  <Row
                    icon={SwatchIcon}
                    label={
                      <span id={ids.theme}>
                        {t("settings.general.theme.label")}
                      </span>
                    }
                    description={t("settings.general.theme.description")}
                    descriptionId={ids.themeDescription}
                  >
                    <RowError message={rowErrors.theme} />
                  </Row>
                  <div className="flex-shrink-0">
                    <Choice
                      options={THEMES.map(({ value, label }) => ({
                        value,
                        label: t(label),
                      }))}
                      value={settings.theme}
                      onChange={(theme: Theme) =>
                        void saveField("theme", { theme })
                      }
                      labelledBy={ids.theme}
                      describedBy={ids.themeDescription}
                    />
                  </div>
                </div>

                {/* Réglage à part : Reset n'y touche pas. */}
                <CloseButtonRow onSaved={showSavedConfirmation} />

                {/* AppImage seulement ; réglage à part : Reset n'y touche
                    pas. */}
                <MenuEntryRow onSaved={showSavedConfirmation} />
              </div>
            </div>

            <Updates
              onSaved={showSavedConfirmation}
              focusRequest={card === "updates" ? request : null}
            />

            <div className="flex items-center justify-between gap-4 px-4">
              {resetFailed ? (
                <p
                  role="alert"
                  className="text-xs text-red-600 dark:text-red-400"
                >
                  {t("settings.general.reset.error")}
                </p>
              ) : (
                <span />
              )}
              <button
                type="button"
                onClick={() => void reset()}
                disabled={resetting}
                className="flex flex-shrink-0 items-center gap-2 rounded-lg border border-zinc-950/10 bg-white px-3 py-1.5 text-sm/6 font-medium text-zinc-950 shadow-sm outline-none hover:bg-zinc-50 focus-visible:ring-2 focus-visible:ring-accent-fg disabled:opacity-50 dark:border-white/15 dark:bg-zinc-900 dark:text-white dark:hover:bg-zinc-800"
              >
                {resetting && (
                  <ArrowPathIcon
                    aria-hidden="true"
                    className="h-4 w-4 animate-spin text-zinc-500 dark:text-zinc-400"
                  />
                )}
                {resetting
                  ? t("settings.general.reset.resetting")
                  : t("settings.general.reset.button")}
              </button>
            </div>

            {/* Propre à ChatonLM, tout en bas : aucun réglage. */}
            <Donate />
          </fieldset>
        )}

        {/* Région toujours présente : seul son contenu change, pour que
            l'annonce soit fiable. */}
        <div
          role="status"
          className="fixed bottom-4 left-1/2 z-50 -translate-x-1/2"
        >
          {showSaved && (
            <span
              key={savedCount}
              className="inline-flex items-center rounded-md bg-green-100 px-1.5 py-0.5 text-sm/5 font-medium text-green-800 sm:text-xs/5 dark:bg-green-800 dark:text-green-100"
            >
              {t("settings.saved")}
            </span>
          )}
        </div>
      </div>
    </main>
  );
}

/**
 * Choix d'une valeur (thème, langue) : boutons radio natifs (flèches du
 * clavier comprises). `lang` : langue du libellé, s'il n'est pas dans celle
 * de l'interface (lecteurs d'écran).
 */
function Choice<T extends string>({
  options,
  value,
  onChange,
  labelledBy,
  describedBy,
}: {
  options: readonly { value: T; label: string; lang?: string }[];
  value: T;
  onChange: (value: T) => void;
  labelledBy: string;
  describedBy: string;
}) {
  const name = useId();
  return (
    <div
      role="radiogroup"
      aria-labelledby={labelledBy}
      aria-describedby={describedBy}
      className="inline-flex rounded-lg bg-neutral-100 p-0.5 dark:bg-neutral-900"
    >
      {options.map((option) => (
        <label key={option.value} className="relative">
          <input
            type="radio"
            name={name}
            value={option.value}
            checked={value === option.value}
            onChange={() => onChange(option.value)}
            className="peer sr-only"
          />
          <span
            lang={option.lang}
            className="block cursor-pointer rounded-md px-3 py-1 text-sm/6 text-neutral-600 peer-checked:bg-white peer-checked:text-neutral-900 peer-checked:shadow-sm peer-focus-visible:ring-2 peer-focus-visible:ring-accent-fg peer-disabled:cursor-not-allowed peer-disabled:opacity-50 dark:text-neutral-300 dark:peer-checked:bg-neutral-700 dark:peer-checked:text-white"
          >
            {option.label}
          </span>
        </label>
      ))}
    </div>
  );
}
