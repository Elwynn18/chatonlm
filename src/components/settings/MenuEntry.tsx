// Ligne « Applications menu » de Settings › General (étape 18a, propre à
// ChatonLM), seulement quand ChatonLM tourne depuis une AppImage (les paquets
// installent eux-mêmes leur entrée) : activée par défaut, l'app s'inscrit au
// menu des applications à chaque démarrage (chemin de l'AppImage tenu à
// jour) ; coupée, l'entrée et l'icône sont retirées aussitôt et ne reviennent
// plus. Réglage à part : « Reset to defaults » n'y touche pas. « Saved » de
// la page après succès ; sinon la raison du moteur sous la ligne (le choix
// est relu : il est enregistré même quand le fichier n'a pas pu changer).

import { Squares2X2Icon } from "@heroicons/react/20/solid";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect, useId, useRef, useState } from "react";
import { Row, Switch } from "@/components/settings/Row";
import { useT } from "@/i18n";
import { errorMessage, menuEntryStatus, setMenuEntry } from "@/lib/ipc";

/** Requête de l'état de l'entrée. */
const menuEntryKey = ["menuEntry"] as const;

/** Rien hors AppImage. `onSaved` : badge « Saved » de la page. */
export function MenuEntryRow({ onSaved }: { onSaved: () => void }) {
  const t = useT();
  const queryClient = useQueryClient();
  const { data } = useQuery({
    queryKey: menuEntryKey,
    queryFn: menuEntryStatus,
  });
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const descriptionId = useId();
  // Une réponse peut arriver après la sortie de la page.
  const mounted = useRef(false);

  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);

  if (!data?.available) return null;

  const change = async (enabled: boolean) => {
    if (saving) return;
    setSaving(true);
    setError(null);
    try {
      queryClient.setQueryData(menuEntryKey, await setMenuEntry(enabled));
      if (mounted.current) onSaved();
    } catch (cause) {
      if (mounted.current) setError(errorMessage(cause));
      await queryClient.invalidateQueries({ queryKey: menuEntryKey });
    } finally {
      if (mounted.current) setSaving(false);
    }
  };

  return (
    <div
      data-testid="menu-entry"
      className="flex items-start justify-between gap-4"
    >
      <Row
        icon={Squares2X2Icon}
        label={t("settings.general.menuEntry.label")}
        description={t("settings.general.menuEntry.description")}
        descriptionId={descriptionId}
      >
        {error && (
          <p
            role="alert"
            className="mt-2 text-xs break-words text-red-600 dark:text-red-400"
          >
            {error}
          </p>
        )}
      </Row>
      <Switch
        label={t("settings.general.menuEntry.switch")}
        checked={data.enabled}
        describedBy={descriptionId}
        disabled={saving}
        onChange={(enabled) => void change(enabled)}
      />
    </div>
  );
}
