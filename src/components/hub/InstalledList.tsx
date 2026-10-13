// Onglet Installed du ModelHub : tous les modèles du serveur, en
// lignes-cartes (nom lisible, étiquette, taille, paramètres, quantification,
// date, capacités), Use in chat, suppression confirmée, lien vers la fiche
// d'un modèle de Hugging Face. Chaque ligne a sa propre suppression : deux
// suppressions rapprochées gardent chacune son état et son échec. Après
// confirmation, le focus va au titre de la liste (la ligne va disparaître) ;
// Cancel le rend à la corbeille. Un modèle dont le registre a une version plus
// récente (useOutdatedModels, demandé à l'ouverture de l'onglet) porte la
// pastille « Update available » et un bouton Update, qui le met en file dans
// les téléchargements : la ligne montre alors la progression, avec Cancel
// (et Resume si la mise à jour a été mise en pause dans l'onglet Downloads).

import { CubeIcon, TrashIcon } from "@heroicons/react/24/outline";
import { Link } from "@tanstack/react-router";
import { useRef, useState } from "react";
import type { HubCapabilities } from "@/bindings/HubCapabilities";
import type { HubInstalled } from "@/bindings/HubInstalled";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { DownloadProgress } from "@/components/hub/DownloadProgress";
import { CapabilityIcons, ModelAvatar } from "@/components/hub/ModelBits";
import { CommandError, EmptyState } from "@/components/hub/Status";
import {
  CARD,
  CHIP,
  ICON_BUTTON,
  PAGE,
  PRIMARY_BUTTON,
  SECONDARY_BUTTON,
  SKELETON,
} from "@/components/hub/styles";
import {
  findDownload,
  useDownloadActions,
  useDownloads,
} from "@/hooks/useDownloads";
import { useDeleteModel, useHubInstalled, useUseInChat } from "@/hooks/useHub";
import { listsModel, useOutdatedModels } from "@/hooks/useOutdatedModels";
import { useT } from "@/i18n";
import { formatBytes, formatTimeAgo } from "@/i18n/format";
import { familyOf, isPendingDownload, repoParams } from "@/lib/hub";
import { errorMessage } from "@/lib/ipc";
import {
  displayTag,
  parseModelName,
  shortModelName as shortName,
  withTag,
} from "@/lib/modelNames";

/** Capacités d'Ollama (/api/tags) en drapeaux des icônes. */
function capabilitiesOf(model: HubInstalled): HubCapabilities {
  const has = (name: string) => model.capabilities.includes(name);
  return {
    tools: has("tools"),
    thinking: has("thinking"),
    vision: has("vision"),
    code: false,
  };
}

/** Pastille « Update available » : vert doux du thème (celui de « Fits »). */
const UPDATE_CHIP =
  "inline-flex flex-none items-center rounded-md bg-green-100 px-1.5 py-0.5 text-xs font-medium text-green-800 dark:bg-green-500/10 dark:text-green-400";

export function InstalledList() {
  const t = useT();
  const installed = useHubInstalled();
  const headingRef = useRef<HTMLHeadingElement>(null);
  // Version : la date du manifeste, qui change à chaque téléchargement.
  const outdated = useOutdatedModels(
    (installed.data ?? []).map((model) => ({
      model: model.model,
      version: model.modifiedAt ?? "",
    })),
  );

  if (installed.isPending) return <InstalledSkeleton />;
  if (installed.isError) {
    return (
      <CommandError
        message={errorMessage(installed.error)}
        onRetry={() => void installed.refetch()}
      />
    );
  }
  const models = installed.data;
  if (models.length === 0) {
    return (
      <EmptyState
        icon={<CubeIcon aria-hidden="true" className="h-6 w-6" />}
        title={t("hub.installed.empty")}
      >
        <Link to="/hub" className={PRIMARY_BUTTON}>
          {t("hub.actions.browse")}
        </Link>
      </EmptyState>
    );
  }

  const total = models.reduce((sum, model) => sum + model.size, 0);
  return (
    <div className={PAGE}>
      <h2
        ref={headingRef}
        tabIndex={-1}
        className="pt-4 font-rounded text-lg font-medium text-neutral-900 outline-none dark:text-white"
      >
        {t("hub.discover.modelCount", { count: models.length })} ·{" "}
        {formatBytes(t, total)}
      </h2>
      <ul className="mt-4 flex flex-col gap-3">
        {models.map((model) => (
          <InstalledRow
            key={model.model}
            model={model}
            outdated={listsModel(outdated, model.model)}
            onDeleting={() => headingRef.current?.focus()}
          />
        ))}
      </ul>
    </div>
  );
}

function InstalledRow({
  model,
  outdated,
  onDeleting,
}: {
  model: HubInstalled;
  /** Le registre a une version plus récente. */
  outdated: boolean;
  /** Suppression confirmée : le focus quitte la ligne, qui va disparaître. */
  onDeleting: () => void;
}) {
  const t = useT();
  const openInChat = useUseInChat();
  const remove = useDeleteModel();
  const { downloads } = useDownloads();
  const { start, cancel, resume } = useDownloadActions();
  // Nom que la file des téléchargements garde : avec son étiquette.
  const target = withTag(model.model);
  const download = findDownload(downloads, target);
  // Mise à jour en file, en cours, ou mise en pause (onglet Downloads).
  const updating = download !== null && isPendingDownload(download);
  const held = download?.state === "paused";
  const [confirming, setConfirming] = useState(false);
  const [chatError, setChatError] = useState<string | null>(null);
  const name = parseModelName(model.model);
  const repo = model.repo ?? name.repo;
  const embedding =
    model.capabilities.includes("embedding") &&
    !model.capabilities.includes("completion");
  const details = [
    formatBytes(t, model.size),
    model.parameterSize,
    model.quantizationLevel,
    model.modifiedAt
      ? t("hub.installed.modified", {
          time: formatTimeAgo(t, model.modifiedAt),
        })
      : "",
  ].filter(Boolean);
  const deleting = remove.isPending;
  // Mise à jour refusée par la file, ou en échec (tant que le modèle reste
  // périmé : un échec plus ancien ne concerne plus une version à jour).
  const updateError = start.isError
    ? errorMessage(start.error)
    : outdated && download?.state === "failed"
      ? download.error
      : null;
  const message = remove.isError
    ? errorMessage(remove.error)
    : (chatError ?? updateError);
  return (
    <li
      data-testid="installed-model"
      aria-busy={deleting || undefined}
      className={`${CARD} p-4 transition-opacity ${deleting ? "opacity-50" : ""}`}
    >
      <div className="flex flex-wrap items-center gap-x-4 gap-y-3">
        <ModelAvatar
          name={name.title}
          colorKey={
            familyOf(model.family, name.title) ?? name.publisher ?? name.title
          }
        />
        <div className="min-w-0 flex-1">
          <div className="flex min-w-0 items-center gap-2" title={model.model}>
            {repo ? (
              <Link
                to="/hub/model/$author/$repo"
                params={repoParams(repo)}
                className="min-w-0 truncate rounded-md font-medium text-neutral-900 underline-offset-2 outline-none hover:underline focus-visible:ring-2 focus-visible:ring-accent-fg dark:text-white"
              >
                {name.title}
              </Link>
            ) : (
              <span className="min-w-0 truncate font-medium text-neutral-900 dark:text-white">
                {name.title}
              </span>
            )}
            {name.tag && (
              <span className={`${CHIP} flex-none font-mono`}>
                {displayTag(name.tag)}
              </span>
            )}
            {embedding && (
              <span className={`${CHIP} flex-none`}>
                {t("hub.installed.embedding")}
              </span>
            )}
            {outdated && !updating && (
              <span className={UPDATE_CHIP}>
                {t("hub.installed.updateAvailable")}
              </span>
            )}
          </div>
          <div className="mt-1 flex flex-wrap items-center gap-x-1.5 text-xs text-neutral-500 dark:text-neutral-400">
            {name.publisher && (
              <>
                <span>{name.publisher}</span>
                <span aria-hidden="true">·</span>
              </>
            )}
            {details.map((detail, index) => (
              <span key={index} className="inline-flex items-center gap-1.5">
                {index > 0 && <span aria-hidden="true">·</span>}
                {detail}
              </span>
            ))}
          </div>
        </div>
        <CapabilityIcons capabilities={capabilitiesOf(model)} />
        <div className="flex flex-none items-center gap-1">
          {updating ? (
            <>
              {held && (
                <button
                  type="button"
                  className={PRIMARY_BUTTON}
                  disabled={resume.isPending}
                  onClick={() => resume.mutate(target)}
                >
                  {t("hub.actions.resume")}
                </button>
              )}
              <button
                type="button"
                className={SECONDARY_BUTTON}
                disabled={cancel.isPending}
                onClick={() => cancel.mutate(target)}
              >
                {t("common.actions.cancel")}
              </button>
            </>
          ) : (
            outdated && (
              <button
                type="button"
                className={PRIMARY_BUTTON}
                aria-label={t("hub.actions.updateNamed", {
                  name: shortName(model.model),
                })}
                disabled={deleting || start.isPending}
                onClick={() =>
                  start.mutate({ model: target, repo: null, label: null })
                }
              >
                {t("hub.actions.update")}
              </button>
            )
          )}
          {!embedding && (
            <button
              type="button"
              className={SECONDARY_BUTTON}
              disabled={deleting}
              onClick={() => {
                setChatError(null);
                openInChat(model.model).catch((cause: unknown) =>
                  setChatError(errorMessage(cause)),
                );
              }}
            >
              {t("hub.actions.useInChat")}
            </button>
          )}
          <button
            type="button"
            className={ICON_BUTTON}
            aria-label={t("hub.actions.deleteNamed", {
              name: shortName(model.model),
            })}
            title={t("common.actions.delete")}
            disabled={deleting || updating}
            onClick={() => setConfirming(true)}
          >
            <TrashIcon aria-hidden="true" className="h-5 w-5" />
          </button>
        </div>
      </div>
      {updating && (
        <div className="mt-3">
          <DownloadProgress download={download} name={shortName(model.model)} />
        </div>
      )}
      {message && (
        <p
          role="alert"
          className="mt-3 text-xs break-words text-red-600 dark:text-red-400"
        >
          {message}
        </p>
      )}
      {confirming && (
        <ConfirmDialog
          title={t("hub.actions.deleteConfirm", {
            name: shortName(model.model),
          })}
          message={t("hub.installed.deleteText")}
          confirmLabel={t("common.actions.delete")}
          onConfirm={() => {
            onDeleting();
            setConfirming(false);
            remove.mutate(model.model);
          }}
          onCancel={() => setConfirming(false)}
        />
      )}
    </li>
  );
}

function InstalledSkeleton() {
  const t = useT();
  return (
    <div className={PAGE} role="status" aria-label={t("hub.discover.loading")}>
      <div aria-hidden="true">
        <div className={`${SKELETON} mt-5 h-5 w-40`} />
        <div className="mt-4 flex flex-col gap-3">
          {[0, 1, 2].map((row) => (
            <div key={row} className={`${CARD} flex items-center gap-4 p-4`}>
              <div className={`${SKELETON} h-11 w-11 rounded-xl`} />
              <div className="flex-1 flex flex-col gap-2">
                <div className={`${SKELETON} h-4 w-1/3`} />
                <div className={`${SKELETON} h-3 w-1/2`} />
              </div>
            </div>
          ))}
        </div>
      </div>
    </div>
  );
}
