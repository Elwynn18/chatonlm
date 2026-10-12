// Onglet Polish du ModelHub (étape 17, propre à ChatonLM) : modèles Ollama
// conseillés pour nettoyer la transcription de la dictée (mesures du contrat
// 42 § 0), téléchargés par le pull du ModelHub (onglet Downloads), lus
// installés par list_models ; « Use for polish » règle le modèle de la
// dictée, « In use » pour celui qui l'est. Tout autre modèle installé se
// choisit dans les réglages. Liste d'Ollama pas encore lue, ou Ollama
// injoignable : ni Download ni Use, puisqu'on ne sait pas ce qui est
// installé.

import { Link } from "@tanstack/react-router";
import { useRef, useState } from "react";
import { DownloadProgress } from "@/components/hub/DownloadProgress";
import { ModelAvatar } from "@/components/hub/ModelBits";
import { InUse, RecommendedBadge } from "@/components/hub/SpeechModels";
import {
  CARD,
  CHIP,
  PAGE,
  PRIMARY_BUTTON,
  QUIET_LINK,
  SECONDARY_BUTTON,
} from "@/components/hub/styles";
import { useDictationSettings } from "@/hooks/useDictation";
import {
  findDownload,
  useDownloadActions,
  useDownloads,
} from "@/hooks/useDownloads";
import { useFocusRescue } from "@/hooks/useFocusRescue";
import { useModels } from "@/hooks/useModels";
import { useT } from "@/i18n";
import {
  POLISH_MODELS,
  type PolishModel,
  polishModelNote,
  polishModelSize,
} from "@/lib/dictation";
import { familyOf, isActiveDownload } from "@/lib/hub";
import { errorMessage } from "@/lib/ipc";
import { parseModelName, sameModel } from "@/lib/modelNames";

export function PolishModels() {
  const t = useT();
  const models = useModels();
  const { settings } = useDictationSettings();
  const names = (models.data ?? []).map((model) => model.model);
  // Liste lue (même ancienne) ; sinon on ne sait pas ce qui est installé.
  const read = models.data !== undefined;
  // Échec, même pendant les nouveaux essais de useModels.
  const unreachable = !read && (models.isError || models.failureCount > 0);
  return (
    <div className={PAGE}>
      <p className="pt-4 text-sm text-neutral-600 dark:text-neutral-300">
        {t("hub.polish.intro")}
      </p>
      {unreachable && (
        <p className="mt-3 text-sm text-amber-700 dark:text-amber-400">
          {t("hub.polish.unreachable")}
        </p>
      )}
      <ul
        aria-label={t("hub.polish.label")}
        className="mt-4 flex flex-col gap-3"
      >
        {POLISH_MODELS.map((entry) => (
          <PolishModelCard
            key={entry.model}
            entry={entry}
            // Liste pas lue : ni Download ni Use (on ne sait pas).
            installed={
              read ? names.some((name) => sameModel(name, entry.model)) : null
            }
            inUse={
              settings !== undefined &&
              sameModel(settings.polishModel, entry.model)
            }
          />
        ))}
      </ul>
      <p className="mt-4 text-sm text-neutral-600 dark:text-neutral-300">
        {t("hub.polish.anyModel")}{" "}
        <Link
          to="/settings"
          search={{ section: "dictation" }}
          className={QUIET_LINK}
        >
          {t("hub.speech.settingsLink")}
        </Link>
      </p>
    </div>
  );
}

function PolishModelCard({
  entry,
  installed,
  inUse,
}: {
  entry: PolishModel;
  /** Présent dans Ollama ; null tant que la liste n'est pas lue. */
  installed: boolean | null;
  inUse: boolean;
}) {
  const t = useT();
  const { downloads } = useDownloads();
  const { start, cancel } = useDownloadActions();
  const { update } = useDictationSettings();
  const [useError, setUseError] = useState<string | null>(null);
  const titleRef = useRef<HTMLHeadingElement>(null);
  const cardRef = useRef<HTMLLIElement>(null);
  // Use for polish, Download ou Cancel remplacés : focus au titre.
  useFocusRescue(cardRef, titleRef);
  const name = parseModelName(entry.model);
  const download = findDownload(downloads, entry.model);
  const active = download !== null && isActiveDownload(download);
  const failed =
    !installed && download?.state === "failed"
      ? (download.error ?? t("hub.downloadState.failed"))
      : null;
  const error =
    useError ??
    (start.isError ? errorMessage(start.error) : null) ??
    (cancel.isError ? errorMessage(cancel.error) : null);
  return (
    <li
      ref={cardRef}
      data-testid="polish-model"
      aria-labelledby={`polish-${entry.model}`}
      className={`${CARD} p-4`}
    >
      <div className="flex flex-wrap items-start gap-x-4 gap-y-3">
        <ModelAvatar
          name={name.title}
          colorKey={familyOf(name.title) ?? name.title}
        />
        <div className="min-w-0 flex-1">
          <div className="flex flex-wrap items-center gap-2">
            <h3
              ref={titleRef}
              id={`polish-${entry.model}`}
              tabIndex={-1}
              className="rounded-md font-mono text-[15px] font-medium text-neutral-900 outline-none dark:text-white"
            >
              {entry.model}
            </h3>
            {entry.recommended && <RecommendedBadge />}
            <span className={CHIP}>{polishModelSize(t, entry)}</span>
          </div>
          {entry.noteKind && (
            <p className="mt-1 text-sm text-neutral-600 dark:text-neutral-300">
              {polishModelNote(t, entry)}
            </p>
          )}
          {active && (
            <div className="mt-3">
              <DownloadProgress download={download} name={entry.model} />
            </div>
          )}
          {failed && (
            <p
              role="alert"
              className="mt-2 text-xs break-words text-red-600 dark:text-red-400"
            >
              {failed}
            </p>
          )}
          {error && (
            <p
              role="alert"
              className="mt-2 text-xs break-words text-red-600 dark:text-red-400"
            >
              {error}
            </p>
          )}
        </div>
        {/* Une clé par action : jamais le même bouton relabellisé sous le
            doigt (comme SpeechModels). */}
        <div className="flex flex-none items-center gap-1.5">
          {active ? (
            <button
              key="cancel"
              type="button"
              className={SECONDARY_BUTTON}
              disabled={cancel.isPending}
              onClick={() => cancel.mutate(download.model)}
            >
              {t("common.actions.cancel")}
            </button>
          ) : installed === true ? (
            inUse ? (
              <InUse />
            ) : (
              <button
                key="use"
                type="button"
                className={PRIMARY_BUTTON}
                onClick={() => {
                  setUseError(null);
                  update({ polishModel: entry.model }).catch((cause: unknown) =>
                    setUseError(errorMessage(cause)),
                  );
                }}
              >
                {t("hub.polish.use")}
              </button>
            )
          ) : installed === false ? (
            <button
              key="download"
              type="button"
              className={PRIMARY_BUTTON}
              disabled={start.isPending}
              onClick={() =>
                start.mutate({ model: entry.model, repo: null, label: null })
              }
            >
              {t("hub.actions.download")}
            </button>
          ) : null}
        </div>
      </div>
    </li>
  );
}
