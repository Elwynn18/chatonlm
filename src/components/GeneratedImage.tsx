// Image créée par le modèle d'images (propre à ChatonLM ; contrat
// .tools/research/84-image-chat-contract.md § 4) : résultat d'un appel
// `generate_image`, pièce jointe de la réponse servie par le protocole des
// pièces jointes (attachmentImageUrl, jamais d'adresse écrite en dur). Sa
// place est réservée d'après la taille du résultat (même cadre que l'aperçu
// pendant le calcul, ToolCalls.tsx) ; « Save image… » l'enregistre par le
// dialogue natif. Pas de copie : l'app n'écrit pas d'image dans le
// presse-papiers. Image illisible (pièce jointe absente) : un court texte à
// sa place, sans « Save image… ».

import { ArrowDownTrayIcon, CheckIcon } from "@heroicons/react/20/solid";
import { useEffect, useState } from "react";
import { useT } from "@/i18n";
import { attachmentImageUrl } from "@/lib/attachments";
import { imageFrame, type ImageResult } from "@/lib/generatedImages";
import { errorMessage, saveImage } from "@/lib/ipc";

/** Durée de « Saved ». */
const SAVED_MS = 2000;

export function GeneratedImage({
  result,
  prompt,
}: {
  result: ImageResult;
  /** Description donnée au modèle d'images : texte de remplacement. */
  prompt: string;
}) {
  const t = useT();
  const [state, setState] = useState<"idle" | "saving" | "saved">("idle");
  const [error, setError] = useState<string | null>(null);
  // Image qui ne se charge pas (pièce jointe supprimée ou illisible).
  const [missing, setMissing] = useState(false);

  // Retour à « Save image… » ; minuterie annulée si l'image disparaît.
  useEffect(() => {
    if (state !== "saved") return;
    const timer = setTimeout(() => setState("idle"), SAVED_MS);
    return () => clearTimeout(timer);
  }, [state]);

  const save = async () => {
    setState("saving");
    setError(null);
    try {
      const written = await saveImage(result.attachmentId);
      setState(written ? "saved" : "idle");
    } catch (failure) {
      setState("idle");
      setError(errorMessage(failure));
    }
  };

  return (
    <figure data-role="generated-image" className="m-0">
      {missing ? (
        <p
          data-testid="image-missing"
          className="m-0 text-sm text-neutral-500 dark:text-neutral-400"
        >
          {t("chat.image.missing")}
        </p>
      ) : (
        <img
          src={attachmentImageUrl(result.attachmentId)}
          alt={prompt || t("chat.image.alt", { model: result.model })}
          width={result.width}
          height={result.height}
          draggable={false}
          style={imageFrame(result.width, result.height)}
          onError={() => setMissing(true)}
          className="m-0 block h-auto rounded-2xl bg-neutral-100 object-contain select-none dark:bg-neutral-800"
        />
      )}
      <figcaption className="mt-2 flex flex-wrap items-center gap-x-3 gap-y-1 text-sm text-neutral-500 dark:text-neutral-400">
        {result.model && (
          <span>{t("chat.image.madeWith", { model: result.model })}</span>
        )}
        {!missing && (
          <button
            type="button"
            onClick={() => void save()}
            disabled={state === "saving"}
            className="inline-flex cursor-pointer items-center gap-1 rounded-md outline-none hover:text-neutral-800 focus-visible:ring-2 focus-visible:ring-accent-fg disabled:cursor-default dark:hover:text-neutral-200"
          >
            {state === "saved" ? (
              <CheckIcon aria-hidden="true" className="h-4 w-4" />
            ) : (
              <ArrowDownTrayIcon aria-hidden="true" className="h-4 w-4" />
            )}
            {t(state === "saved" ? "chat.image.saved" : "chat.image.save")}
          </button>
        )}
      </figcaption>
      {error !== null && (
        <p
          role="alert"
          className="mt-1 text-sm break-words text-red-600 dark:text-red-400"
        >
          {t("chat.image.saveFailed", { error })}
        </p>
      )}
    </figure>
  );
}
