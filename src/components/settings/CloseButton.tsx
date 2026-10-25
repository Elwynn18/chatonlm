// Ligne « Close button » de Settings › General (propre à ChatonLM) : activée
// par défaut, la croix de la fenêtre la masque et l'app reste dans la zone de
// notification (raccourcis, bots Easy Link) ; coupée, la croix quitte l'app
// comme Ctrl+Q. Sans zone de notification, la croix quitte toujours :
// interrupteur grisé, et la raison. Réglage à part : « Reset to defaults » n'y
// touche pas. « Saved » de la page après succès ; sinon l'erreur sous la ligne.

import { XMarkIcon } from "@heroicons/react/20/solid";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect, useId, useRef, useState } from "react";
import { Row, Switch } from "@/components/settings/Row";
import { useT } from "@/i18n";
import { closeButtonStatus, errorMessage, setCloseToTray } from "@/lib/ipc";

/** Requête de l'état de la croix. */
const closeButtonKey = ["closeButton"] as const;

/** `onSaved` : badge « Saved » de la page. */
export function CloseButtonRow({ onSaved }: { onSaved: () => void }) {
  const t = useT();
  const queryClient = useQueryClient();
  const { data } = useQuery({
    queryKey: closeButtonKey,
    queryFn: closeButtonStatus,
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

  if (!data) return null;

  const change = async (enabled: boolean) => {
    if (saving) return;
    setSaving(true);
    setError(null);
    try {
      queryClient.setQueryData(closeButtonKey, await setCloseToTray(enabled));
      if (mounted.current) onSaved();
    } catch (cause) {
      if (mounted.current) setError(errorMessage(cause));
      await queryClient.invalidateQueries({ queryKey: closeButtonKey });
    } finally {
      if (mounted.current) setSaving(false);
    }
  };

  return (
    <div
      data-testid="close-button"
      className="flex items-start justify-between gap-4"
    >
      <Row
        icon={XMarkIcon}
        label={t("settings.general.closeButton.label")}
        description={t(
          data.trayAvailable
            ? "settings.general.closeButton.description"
            : "settings.general.closeButton.noTray",
        )}
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
        label={t("settings.general.closeButton.switch")}
        checked={data.trayAvailable && data.closeToTray}
        describedBy={descriptionId}
        disabled={saving || !data.trayAvailable}
        onChange={(enabled) => void change(enabled)}
      />
    </div>
  );
}
