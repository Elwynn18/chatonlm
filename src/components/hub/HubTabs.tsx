// Onglets du ModelHub, en deux groupes sur une ligne : à gauche ce que l'on
// parcourt (Discover, fiches comprises ; Transcription et Polish, modèles de
// la dictée ; Voices, voix de la lecture à voix haute), à droite ce qui est
// sur la machine (Installed, Downloads avec le nombre de téléchargements
// actifs, modèles de transcription et voix compris).
// Fenêtre étroite : le second groupe passe à la ligne. Vrais liens, page
// courante annoncée par aria-current.

import { Link, useRouterState } from "@tanstack/react-router";
import { TAB, TAB_BAR, TAB_CURRENT, TAB_IDLE } from "@/components/hub/styles";
import { useSpeechDownloads } from "@/hooks/useDictation";
import { useDownloads } from "@/hooks/useDownloads";
import { useVoiceDownloads } from "@/hooks/useReadAloud";
import { useT } from "@/i18n";
import { isActiveSpeechDownload } from "@/lib/dictation";
import { isActiveDownload } from "@/lib/hub";

/** Onglet de chaque préfixe d'adresse (Discover sinon). */
const TABS = [
  ["/hub/installed", "installed"],
  ["/hub/downloads", "downloads"],
  ["/hub/transcription", "transcription"],
  ["/hub/polish", "polish"],
  ["/hub/voices", "voices"],
] as const;

export function HubTabs() {
  const t = useT();
  const path = useRouterState({ select: (state) => state.location.pathname });
  const { downloads } = useDownloads();
  const speech = useSpeechDownloads();
  const voices = useVoiceDownloads();
  const active =
    downloads.filter(isActiveDownload).length +
    speech.downloads.filter(isActiveSpeechDownload).length +
    voices.downloads.filter(isActiveSpeechDownload).length;
  const tab =
    TABS.find(([prefix]) => path.startsWith(prefix))?.[1] ?? "discover";
  const current = (name: string) =>
    tab === name ? { "aria-current": "page" as const } : {};
  const classOf = (name: string) =>
    `${TAB} ${tab === name ? TAB_CURRENT : TAB_IDLE}`;

  return (
    <nav
      aria-label={t("hub.title")}
      className="flex-none px-6 pb-3 select-none"
    >
      <div className="mx-auto flex max-w-6xl flex-wrap items-center justify-between gap-2">
        <div data-testid="hub-tabs-browse" className={TAB_BAR}>
          <Link
            to="/hub"
            // Le routeur marquerait « /hub » courant sur tous les onglets.
            activeOptions={{ exact: true, includeSearch: false }}
            draggable={false}
            className={classOf("discover")}
            {...current("discover")}
          >
            {t("hub.tabs.discover")}
          </Link>
          <Link
            to="/hub/transcription"
            draggable={false}
            className={classOf("transcription")}
            {...current("transcription")}
          >
            {t("hub.tabs.transcription")}
          </Link>
          <Link
            to="/hub/polish"
            draggable={false}
            className={classOf("polish")}
            {...current("polish")}
          >
            {t("hub.tabs.polish")}
          </Link>
          <Link
            to="/hub/voices"
            draggable={false}
            className={classOf("voices")}
            {...current("voices")}
          >
            {t("hub.tabs.voices")}
          </Link>
        </div>
        <div data-testid="hub-tabs-local" className={TAB_BAR}>
          <Link
            to="/hub/installed"
            draggable={false}
            className={classOf("installed")}
            {...current("installed")}
          >
            {t("hub.tabs.installed")}
          </Link>
          <Link
            to="/hub/downloads"
            draggable={false}
            className={classOf("downloads")}
            {...current("downloads")}
          >
            {t("hub.tabs.downloads")}
            {active > 0 && (
              <span
                data-testid="active-downloads"
                className="min-w-5 rounded-full bg-neutral-900 px-1.5 text-center text-[11px] leading-5 font-medium text-white dark:bg-white dark:text-neutral-900"
              >
                {active}
              </span>
            )}
          </Link>
        </div>
      </div>
    </nav>
  );
}
