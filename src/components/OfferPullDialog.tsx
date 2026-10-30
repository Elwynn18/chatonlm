// Proposition de téléchargement venue de VS Code (propre à ChatonLM) :
// l'extension ne télécharge jamais rien elle-même ; elle demande à l'app de
// montrer sa fenêtre (`models.offerPull`, seulement pour un modèle
// d'autocomplétion proposé), et c'est l'utilisateur qui accepte ici. Le
// téléchargement suit alors le chemin du ModelHub (barre latérale, page des
// téléchargements). Monté par la route racine, sur toute page. Un modèle
// dont la licence n'autorise pas l'usage commercial (Qwen2.5-Coder 3B) le
// dit ici, puisque c'est ici que l'utilisateur accepte.

import { useEffect, useState } from "react";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { useT } from "@/i18n";
import type { OfferPull } from "@/bindings/OfferPull";
import { errorMessage, hubDownload, listenOfferPull } from "@/lib/ipc";

export function OfferPullDialog() {
  const t = useT();
  const [offer, setOffer] = useState<OfferPull | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let cancelled = false;
    void listenOfferPull((offer) => {
      setError(null);
      setOffer(offer);
    }).then((stop) => {
      if (cancelled) stop();
      else unlisten = stop;
    });
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, []);

  if (error !== null) {
    return (
      <ConfirmDialog
        message={error}
        onConfirm={() => setError(null)}
        onCancel={() => setError(null)}
      />
    );
  }
  if (offer === null) return null;
  const { model } = offer;

  const download = async () => {
    setOffer(null);
    try {
      await hubDownload({ model, repo: null, label: null });
    } catch (cause) {
      setError(errorMessage(cause));
    }
  };

  return (
    <ConfirmDialog
      title={t("settings.vscode.offerPull.title", { model })}
      message={
        offer.commercialUse ? (
          t("settings.vscode.offerPull.message", { model })
        ) : (
          <>
            <p>{t("settings.vscode.offerPull.message", { model })}</p>
            <p>{t("settings.vscode.offerPull.nonCommercial")}</p>
          </>
        )
      }
      confirmLabel={t("settings.vscode.offerPull.download")}
      onConfirm={() => void download()}
      onCancel={() => setOffer(null)}
    />
  );
}
