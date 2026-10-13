// Premier démarrage (propre à ChatonLM, sans équivalent dans l'app
// officielle), premier temps : choix de la langue, montré à la place de l'app.
// La visite guidée de l'app suit (Tour.tsx), dans l'app elle-même.
//
// Langue : celle du bureau est proposée d'office ; un choix s'applique tout
// de suite (l'écran change de langue sans rechargement) et devient le réglage
// Language de Settings › General. Continuer sans rien changer enregistre la
// langue proposée : le réglage ne reste pas sur « System ».

import { useEffect, useRef, useState } from "react";
import type { Settings } from "@/bindings/Settings";
import type { SettingsPatch } from "@/bindings/SettingsPatch";
import { Logo } from "@/components/Logo";
import { useSettings } from "@/hooks/useSettings";
import { LANGUAGES, type Lang, useT } from "@/i18n";
import { nativeLanguageName } from "@/i18n/format";
import { errorMessage } from "@/lib/ipc";
import { shouldBlockKey } from "@/lib/navigationGuard";
import { quit } from "@/lib/quit";
import { isQuitShortcut } from "@/lib/shortcuts";

const primaryButton =
  "h-10 min-w-28 rounded-full bg-neutral-900 px-6 text-sm font-medium text-white outline-none hover:bg-neutral-700 focus-visible:ring-2 focus-visible:ring-accent-fg focus-visible:ring-offset-2 disabled:opacity-60 dark:bg-white dark:text-neutral-900 dark:hover:bg-neutral-200 dark:focus-visible:ring-offset-neutral-900";

// Choix de langue : l'état et le repos sont deux chaînes exclusives (deux
// classes de couleur concurrentes ne se départagent pas par leur ordre).
const languageOption =
  "block cursor-pointer rounded-full px-6 py-2.5 text-sm font-medium peer-focus-visible:outline-2 peer-focus-visible:outline-offset-2 peer-focus-visible:outline-accent-fg";
// Bordure transparente : même hauteur que le repos, rien ne bouge au choix.
const languageChosen = "border border-transparent bg-accent text-neutral-900";
const languageIdle =
  "border border-neutral-300 text-neutral-900 hover:bg-neutral-100 dark:border-neutral-600 dark:text-neutral-100 dark:hover:bg-neutral-800";

/** `onDone` : la langue est choisie et enregistrée. */
export function LanguageStep({ onDone }: { onDone: () => void }) {
  const { settings } = useSettings();
  // Monté avec les réglages : le titre reçoit le focus dès l'arrivée.
  return settings ? <Page settings={settings} onDone={onDone} /> : null;
}

function Page({
  settings,
  onDone,
}: {
  settings: Settings;
  onDone: () => void;
}) {
  const t = useT();
  const { setSettings } = useSettings();
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const titleRef = useRef<HTMLHeadingElement>(null);

  // Le titre reçoit le focus : un lecteur d'écran le lit, et Tab repart de
  // lui.
  useEffect(() => {
    titleRef.current?.focus();
  }, []);

  // La route racine, qui tient d'ordinaire ces gardes, n'est pas montée :
  // Ctrl+Q quitte, la page ne se recharge pas et ne montre pas le menu de
  // WebKit (aucun champ de texte ici).
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (shouldBlockKey(event)) {
        event.preventDefault();
        return;
      }
      if (!isQuitShortcut(event)) return;
      event.preventDefault();
      quit().catch((cause: unknown) => console.error("Failed to quit:", cause));
    };
    const onContextMenu = (event: MouseEvent) => event.preventDefault();
    document.addEventListener("keydown", onKeyDown);
    document.addEventListener("contextmenu", onContextMenu, true);
    return () => {
      document.removeEventListener("keydown", onKeyDown);
      document.removeEventListener("contextmenu", onContextMenu, true);
    };
  }, []);

  /** Enregistre ; faux, avec le message affiché, si le moteur refuse. */
  const save = async (patch: SettingsPatch) => {
    setSaving(true);
    setError(null);
    try {
      await setSettings(patch);
      return true;
    } catch (cause) {
      setError(errorMessage(cause));
      return false;
    } finally {
      setSaving(false);
    }
  };

  const language: Lang =
    settings.language === "system"
      ? settings.effectiveLanguage
      : settings.language;

  const confirm = async () => {
    if (settings.language === "system" && !(await save({ language }))) return;
    onDone();
  };

  return (
    <main
      data-testid="onboarding-language"
      className="flex h-screen overflow-y-auto bg-white px-6 dark:bg-neutral-900"
    >
      <div className="m-auto w-full max-w-md py-10 text-center">
        <Logo />
        <h1
          ref={titleRef}
          tabIndex={-1}
          className="text-xl font-medium text-neutral-900 outline-none dark:text-neutral-100"
        >
          {t("onboarding.language.title")}
        </h1>
        <p className="mt-2 text-sm text-neutral-500 dark:text-neutral-400">
          {t("onboarding.language.description")}
        </p>
        <div
          role="radiogroup"
          aria-label={t("onboarding.language.group")}
          className="mx-auto mt-6 flex max-w-60 flex-col gap-2"
        >
          {LANGUAGES.map((code) => (
            <label key={code} className="relative">
              <input
                type="radio"
                name="onboarding-language"
                value={code}
                checked={language === code}
                onChange={() => void save({ language: code })}
                className="peer sr-only"
              />
              <span
                lang={code}
                className={`${languageOption} ${
                  language === code ? languageChosen : languageIdle
                }`}
              >
                {nativeLanguageName(code)}
              </span>
            </label>
          ))}
        </div>
        <div className="mt-8">
          <button
            type="button"
            data-testid="onboarding-continue"
            className={primaryButton}
            disabled={saving}
            onClick={() => void confirm()}
          >
            {t("onboarding.actions.continue")}
          </button>
        </div>
        {error && (
          <p
            role="alert"
            className="mt-4 text-xs break-words text-red-600 dark:text-red-400"
          >
            {t("onboarding.error", { error })}
          </p>
        )}
      </div>
    </main>
  );
}
