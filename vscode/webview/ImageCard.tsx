// Carte de l'image demandée pour un chemin absent (lot 4a) : avancement
// (étape, échantillonnage, calcul par le processeur), aperçu, puis « Save »
// (rien n'est écrit avant) ou « Discard ». L'écriture et la confirmation d'un
// écrasement sont faites par l'hôte.
import { XMarkIcon } from "@heroicons/react/20/solid";
import { useT } from "@/i18n";
import type { ImageView } from "../shared/protocol";
import { imageAnnouncement, imageProgress } from "./labels";

const PILL =
  "cursor-pointer rounded-full border border-neutral-300 px-3 py-1 text-xs text-neutral-700 hover:bg-neutral-100 dark:border-neutral-600 dark:text-neutral-200 dark:hover:bg-neutral-800";
const PRIMARY =
  "cursor-pointer rounded-full bg-accent px-3 py-1 text-xs font-medium text-neutral-900 hover:brightness-95";

export function ImageCard({
  image,
  onCommand,
}: {
  image: ImageView;
  onCommand: (command: "save" | "discard" | "cancel") => void;
}) {
  const t = useT();
  const generating = image.status === "generating";
  const percent =
    image.step !== null && image.total
      ? Math.round((image.step / image.total) * 100)
      : null;
  return (
    <section
      aria-label={t("vscode.image.label")}
      data-testid="image"
      data-status={image.status}
      className="mx-3 mt-2 shrink-0 rounded-2xl bg-neutral-100 px-3 py-2 text-sm dark:bg-neutral-800"
    >
      <div className="flex items-center gap-2">
        <div className="min-w-0 flex-1">
          <p className="truncate font-medium text-neutral-900 dark:text-white">
            {image.target}
          </p>
          <p className="truncate text-xs text-neutral-600 dark:text-neutral-300">
            {image.prompt}
          </p>
        </div>
        {!generating && image.status !== "saving" && (
          <button
            type="button"
            aria-label={t("vscode.image.close")}
            title={t("vscode.image.close")}
            onClick={() => onCommand("discard")}
            className="flex h-7 w-7 shrink-0 cursor-pointer items-center justify-center rounded-full text-neutral-700 hover:bg-neutral-200 dark:text-neutral-200 dark:hover:bg-neutral-700/75"
          >
            <XMarkIcon className="h-4 w-4" />
          </button>
        )}
      </div>
      {/* Texte visible, sans annonce à chaque pas d'échantillonnage (la
          barre le dit) ; seuls les changements d'étape sont annoncés. */}
      <p
        data-testid="image-progress"
        className={
          image.status === "failed"
            ? "mt-1 text-xs text-red-600 dark:text-red-400"
            : "mt-1 text-xs text-neutral-600 dark:text-neutral-300"
        }
      >
        {imageProgress(t, image)}
      </p>
      <p role="status" data-testid="image-status" className="sr-only">
        {imageAnnouncement(t, image)}
      </p>
      {generating && (
        <div
          role="progressbar"
          aria-label={t("vscode.image.progress")}
          aria-valuemin={0}
          aria-valuemax={100}
          aria-valuenow={percent ?? undefined}
          className="mt-1 h-1.5 w-full overflow-hidden rounded-full bg-neutral-200 dark:bg-neutral-700"
        >
          <div
            className="h-full rounded-full bg-accent"
            style={{ width: `${percent ?? 5}%` }}
          />
        </div>
      )}
      {generating && image.cpu && (
        <p className="mt-1 text-xs text-neutral-600 dark:text-neutral-300">
          {t("vscode.image.cpu")}
        </p>
      )}
      {image.preview && (
        <img
          src={image.preview}
          alt={image.prompt}
          className="mt-2 max-h-64 w-full rounded-xl object-contain"
        />
      )}
      <div className="mt-2 flex flex-wrap gap-1.5">
        {generating && (
          <button
            type="button"
            className={PILL}
            onClick={() => onCommand("cancel")}
          >
            {t("vscode.image.stop")}
          </button>
        )}
        {image.status === "ready" && (
          <>
            <button
              type="button"
              className={PRIMARY}
              onClick={() => onCommand("save")}
            >
              {image.exists
                ? t("vscode.image.replace")
                : t("vscode.image.save")}
            </button>
            <button
              type="button"
              className={PILL}
              onClick={() => onCommand("discard")}
            >
              {t("vscode.image.discard")}
            </button>
          </>
        )}
      </div>
    </section>
  );
}
