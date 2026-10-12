// Onglet Installed du ModelHub : tous les modèles du serveur, en
// lignes-cartes (nom lisible, étiquette, taille, paramètres, quantification,
// date, capacités), Use in chat, suppression confirmée, lien vers la fiche
// d'un modèle de Hugging Face. Chaque ligne a sa propre suppression : deux
// suppressions rapprochées gardent chacune son état et son échec. Après
// confirmation, le focus va au titre de la liste (la ligne va disparaître) ;
// Cancel le rend à la corbeille.

import { CubeIcon, TrashIcon } from "@heroicons/react/24/outline";
import { Link } from "@tanstack/react-router";
import { useRef, useState } from "react";
import type { HubCapabilities } from "@/bindings/HubCapabilities";
import type { HubInstalled } from "@/bindings/HubInstalled";
import { ConfirmDialog } from "@/components/ConfirmDialog";
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
import { useDeleteModel, useHubInstalled, useUseInChat } from "@/hooks/useHub";
import { useT } from "@/i18n";
import { formatBytes, formatTimeAgo } from "@/i18n/format";
import { familyOf, repoParams } from "@/lib/hub";
import { errorMessage } from "@/lib/ipc";
import { displayTag, parseModelName } from "@/lib/modelNames";

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

/** Nom court d'un modèle installé (« SmolLM2-135M-Instruct:Q4_K_M »). */
function shortName(model: string): string {
  const name = parseModelName(model);
  if (!name.repo) return model.trim();
  return name.tag ? `${name.title}:${displayTag(name.tag)}` : name.title;
}

export function InstalledList() {
  const t = useT();
  const installed = useHubInstalled();
  const headingRef = useRef<HTMLHeadingElement>(null);

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
            onDeleting={() => headingRef.current?.focus()}
          />
        ))}
      </ul>
    </div>
  );
}

function InstalledRow({
  model,
  onDeleting,
}: {
  model: HubInstalled;
  /** Suppression confirmée : le focus quitte la ligne, qui va disparaître. */
  onDeleting: () => void;
}) {
  const t = useT();
  const openInChat = useUseInChat();
  const remove = useDeleteModel();
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
  const message = remove.isError ? errorMessage(remove.error) : chatError;
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
            disabled={deleting}
            onClick={() => setConfirming(true)}
          >
            <TrashIcon aria-hidden="true" className="h-5 w-5" />
          </button>
        </div>
      </div>
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
