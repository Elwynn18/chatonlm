// Onglets des réglages (propres à ChatonLM : l'officiel est une page unique) :
// General, Models, Account, Dictation, Read aloud, Voice mode, Keyboard
// shortcuts, portés par le
// paramètre `section` de l'adresse (General sans paramètre). Même style que
// les onglets du ModelHub : vrais liens (Tab puis Entrée), onglet courant
// annoncé par aria-current ; fenêtre étroite ou zoom fort : les onglets
// passent à la ligne. Changer d'onglet remplace l'entrée d'historique (pas
// d'empilement) et laisse le focus sur l'onglet : l'état `settingsTab` dit à
// la route que ce n'est pas une demande de montrer une carte.

import { Link } from "@tanstack/react-router";
import { TAB, TAB_BAR, TAB_CURRENT, TAB_IDLE } from "@/components/hub/styles";
import { type TKey, useT } from "@/i18n";

declare module "@tanstack/react-router" {
  interface HistoryState {
    /** Changement d'onglet des réglages : aucune carte à montrer. */
    settingsTab?: boolean;
  }
}

export type SettingsTab =
  | "general"
  | "models"
  | "account"
  | "dictation"
  | "readAloud"
  | "voiceMode"
  | "shortcuts";

const TABS: { name: SettingsTab; label: TKey }[] = [
  { name: "general", label: "settings.tabs.general" },
  { name: "models", label: "settings.tabs.models" },
  { name: "account", label: "settings.tabs.account" },
  { name: "dictation", label: "settings.tabs.dictation" },
  { name: "readAloud", label: "settings.tabs.readAloud" },
  { name: "voiceMode", label: "settings.tabs.voiceMode" },
  { name: "shortcuts", label: "settings.tabs.shortcuts" },
];

export function SettingsTabs({ current }: { current: SettingsTab }) {
  const t = useT();
  return (
    <nav
      aria-label={t("settings.title")}
      className="flex-none px-6 pt-4 select-none"
    >
      <div className="mx-auto max-w-4xl">
        {/* Sept onglets, plus longs en espagnol et en français : à la ligne
            plutôt que de déborder. */}
        <div
          data-testid="settings-tabs"
          className={`${TAB_BAR} max-w-full flex-wrap`}
        >
          {TABS.map(({ name, label }) => (
            <Link
              key={name}
              to="/settings"
              search={name === "general" ? {} : { section: name }}
              replace
              state={{ settingsTab: true }}
              // Le routeur marque lui-même le lien actif (aria-current) :
              // seulement à l'adresse exacte, comme l'onglet courant.
              activeOptions={{ exact: true, includeSearch: true }}
              draggable={false}
              data-testid={`settings-tab-${name}`}
              className={`${TAB} ${current === name ? TAB_CURRENT : TAB_IDLE}`}
              {...(current === name ? { "aria-current": "page" as const } : {})}
            >
              {t(label)}
            </Link>
          ))}
        </div>
      </div>
    </nav>
  );
}
