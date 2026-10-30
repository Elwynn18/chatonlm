// Onglet « VS Code » des réglages (propre à ChatonLM : l'app officielle n'a
// rien de tel) : le pont local par lequel l'extension ChatonLM pour VS Code
// converse avec les modèles. Le moteur tient le pont ; l'onglet règle et
// montre son état (useBridge.ts : lu en arrivant, puis tenu à jour par
// l'évènement « bridge »).
// - Carte du pont : interrupteur « Allow VS Code to connect » (coupé par
//   défaut ; enregistré aussitôt, « Saved » de la page) et la phrase qui dit
//   que rien ne passe par le réseau ; état du pont annoncé (il change sans
//   action de l'utilisateur) : système sans dossier privé (`unavailable`) ou
//   ouverture refusée (`failed`) en clair.
// - Carte des éditeurs appairés : nom, date d'appairage, « Connected » ou
//   dernière connexion, « Remove » (sans confirmation, comme « Unlink »
//   d'Easy Link : l'éditeur se réappaire en deux gestes) ; « Pair an editor »
//   (pont ouvert seulement) montre le code à six chiffres, épelé aux lecteurs
//   d'écran, son heure de fin et la commande à lancer dans VS Code ; pas de
//   minuterie : le moteur retire le code consommé ou expiré.
// Un bouton focalisé qui disparaît laisse le focus dans sa carte (code
// montré, bouton « Pair an editor » revenu, sinon le titre). Réglages à part
// (tables du pont) : « Reset to defaults » de General n'y touche pas.

import { ArrowPathIcon, CodeBracketIcon } from "@heroicons/react/20/solid";
import { type ReactNode, useId, useRef, useState } from "react";
import type { BridgeClient } from "@/bindings/BridgeClient";
import type { BridgePairing } from "@/bindings/BridgePairing";
import type { BridgeState } from "@/bindings/BridgeState";
import { Row, Switch } from "@/components/settings/Row";
import { useBridge, useBridgeCommand } from "@/hooks/useBridge";
import { useFocusRescue } from "@/hooks/useFocusRescue";
import { useT } from "@/i18n";
import { formatDateTime, formatTime } from "@/i18n/format";
import { Trans } from "@/i18n/Trans";
import {
  bridgeCancelPairing,
  bridgePair,
  bridgeSetEnabled,
  bridgeUnlink,
  errorMessage,
} from "@/lib/ipc";

const CARD = "overflow-hidden rounded-xl bg-white dark:bg-neutral-800";
const TITLE =
  "rounded-md text-sm/6 font-medium text-zinc-950 outline-none dark:text-white";
const TEXT = "text-sm/6 text-zinc-950 dark:text-white";
const MUTED = "text-sm/6 text-zinc-500 dark:text-zinc-400";
const ALERT = "text-xs break-words text-red-600 dark:text-red-400";
const DIVIDED = "border-t border-zinc-950/5 pt-4 dark:border-white/10";
// aria-disabled comme disabled : le bouton d'une action en cours garde le
// focus (un bouton désactivé le perd).
const SMALL_BUTTON =
  "inline-flex flex-none items-center gap-1.5 rounded-lg border border-zinc-950/10 bg-white px-3 py-1 text-sm/6 font-medium text-zinc-950 shadow-sm outline-none hover:bg-zinc-50 focus-visible:ring-2 focus-visible:ring-accent-fg disabled:cursor-default disabled:opacity-50 aria-disabled:cursor-default aria-disabled:opacity-50 dark:border-white/15 dark:bg-zinc-900 dark:text-white dark:hover:bg-zinc-800";
// select-text : la page des réglages ne se sélectionne pas, une commande à
// recopier, si.
const CODE =
  "rounded-md bg-neutral-100 px-1.5 py-0.5 font-mono text-[13px] text-neutral-900 select-text dark:bg-neutral-900 dark:text-neutral-100";

const code = (chunks: string) => <code className={CODE}>{chunks}</code>;

/**
 * Lance une commande du pont, sauf si elle court déjà ; son refus (texte du
 * moteur, déjà traduit) est gardé pour être montré.
 */
function useRun() {
  const [failure, setFailure] = useState<string | null>(null);
  function run<A>(
    command: {
      isPending: boolean;
      mutateAsync: (argument: A) => Promise<unknown>;
    },
    argument: A,
    after?: () => void,
  ) {
    if (command.isPending) return;
    setFailure(null);
    command.mutateAsync(argument).then(
      () => after?.(),
      (cause: unknown) => setFailure(errorMessage(cause)),
    );
  }
  return { failure, run };
}

export function VsCode({
  onSaved,
}: {
  /** Un changement est enregistré (« Saved » de la page). */
  onSaved: () => void;
}) {
  const t = useT();
  const { data, error, refetch } = useBridge();
  const titleId = useId();

  let content: ReactNode;
  if (data) {
    content = <BridgeSwitch state={data} onSaved={onSaved} />;
  } else if (error) {
    content = (
      <div className="flex items-start justify-between gap-4">
        <p
          role="alert"
          className="min-w-0 text-sm/6 break-words text-red-600 dark:text-red-400"
        >
          {t("settings.vscode.loadFailed", { message: errorMessage(error) })}
        </p>
        <button
          type="button"
          className={SMALL_BUTTON}
          onClick={() => void refetch()}
        >
          <ArrowPathIcon aria-hidden="true" className="h-4 w-4" />
          {t("settings.tryAgain")}
        </button>
      </div>
    );
  } else {
    content = (
      <div
        data-testid="vscode-loading"
        aria-hidden="true"
        className="h-16 animate-pulse rounded-lg bg-neutral-200 dark:bg-neutral-700"
      />
    );
  }

  return (
    <div data-testid="vscode-settings" className="space-y-4">
      <section aria-labelledby={titleId} className={CARD}>
        <div className="space-y-4 p-4">
          <h2 id={titleId} className={TITLE}>
            {t("settings.tabs.vscode")}
          </h2>
          {content}
        </div>
      </section>
      {data && <EditorsCard state={data} />}
    </div>
  );
}

/** Interrupteur du pont et son état. */
function BridgeSwitch({
  state,
  onSaved,
}: {
  state: BridgeState;
  onSaved: () => void;
}) {
  const t = useT();
  const descriptionId = useId();
  const setEnabled = useBridgeCommand(bridgeSetEnabled);
  const { failure, run } = useRun();
  const problem =
    state.status === "unavailable" || state.status === "failed"
      ? state.status
      : null;

  return (
    <>
      <div className="flex items-start justify-between gap-4">
        <Row
          icon={CodeBracketIcon}
          label={t("settings.vscode.enable.label")}
          description={t("settings.vscode.enable.description")}
          descriptionId={descriptionId}
        />
        <Switch
          label={t("settings.vscode.enable.label")}
          describedBy={descriptionId}
          checked={state.enabled}
          onChange={(enabled) => run(setEnabled, enabled, onSaved)}
        />
      </div>
      {/* Région toujours présente : l'état du pont change sans action de
          l'utilisateur (ouverture refusée), son nouvel état est annoncé. */}
      <div role="status" data-testid="vscode-status" data-state={state.status}>
        {problem && (
          <p className="text-sm/6 break-words text-red-600 dark:text-red-400">
            {t(`settings.vscode.status.${problem}`)}
          </p>
        )}
      </div>
      {failure !== null && (
        <p role="alert" className={ALERT}>
          {failure}
        </p>
      )}
    </>
  );
}

/** Éditeurs appairés, code d'appairage. */
function EditorsCard({ state }: { state: BridgeState }) {
  const t = useT();
  const titleId = useId();
  const hintId = useId();
  const cardRef = useRef<HTMLElement>(null);
  const titleRef = useRef<HTMLHeadingElement>(null);
  const pair = useBridgeCommand(bridgePair);
  const cancel = useBridgeCommand(bridgeCancelPairing);
  const unlink = useBridgeCommand(bridgeUnlink);
  const { failure, run } = useRun();
  // Bouton focalisé retiré : « Pair an editor » remplacé par le code (le
  // focus va au code, lu aussitôt), code retiré (annulé, consommé, expiré :
  // retour à « Pair an editor »), sinon (Remove) le titre.
  useFocusRescue(cardRef, (lost) => {
    const card = cardRef.current;
    const next = lost.matches("[data-vscode-pair]")
      ? card?.querySelector<HTMLElement>("[data-vscode-code]")
      : lost.closest("[data-vscode-pairing]")
        ? card?.querySelector<HTMLElement>("[data-vscode-pair]:not(:disabled)")
        : null;
    return next ?? titleRef.current;
  });
  const listening = state.enabled && state.status === "listening";

  return (
    <section
      ref={cardRef}
      data-testid="vscode-editors"
      aria-labelledby={titleId}
      className={CARD}
    >
      <div className="space-y-3 p-4">
        <h2 ref={titleRef} id={titleId} tabIndex={-1} className={TITLE}>
          {t("settings.vscode.editors.title")}
        </h2>
        {state.clients.length === 0 ? (
          <p className={MUTED}>{t("settings.vscode.editors.empty")}</p>
        ) : (
          <ul aria-labelledby={titleId}>
            {state.clients.map((client) => (
              <EditorRow
                key={client.id}
                client={client}
                removing={unlink.isPending && unlink.variables === client.id}
                onRemove={() => run(unlink, client.id)}
              />
            ))}
          </ul>
        )}
        <div className={DIVIDED}>
          {state.pairing ? (
            <PairingCode
              pairing={state.pairing}
              cancelling={cancel.isPending}
              onCancel={() => run(cancel, undefined)}
            />
          ) : (
            <>
              <button
                type="button"
                data-vscode-pair=""
                className={SMALL_BUTTON}
                // Pont fermé ou en échec : pas de code possible.
                disabled={!listening}
                aria-describedby={state.enabled ? undefined : hintId}
                aria-disabled={pair.isPending}
                onClick={() => run(pair, undefined)}
              >
                {t("settings.vscode.pairing.start")}
              </button>
              {!state.enabled && (
                <p id={hintId} className={`mt-2 ${MUTED}`}>
                  {t("settings.vscode.pairing.needsBridge")}
                </p>
              )}
            </>
          )}
          {failure !== null && (
            <p role="alert" className={`mt-3 ${ALERT}`}>
              {failure}
            </p>
          )}
        </div>
      </div>
    </section>
  );
}

/** Un éditeur appairé : nom, date d'appairage, connexion, Remove. */
function EditorRow({
  client,
  removing,
  onRemove,
}: {
  client: BridgeClient;
  removing: boolean;
  onRemove: () => void;
}) {
  const t = useT();
  const nameId = useId();
  return (
    <li
      data-testid="vscode-editor"
      className="flex items-center justify-between gap-4 border-t border-zinc-950/5 py-3 first:border-t-0 first:pt-0 last:pb-0 dark:border-white/10"
    >
      <div className="min-w-0">
        <div id={nameId} className={`${TEXT} break-words`}>
          {client.name || t("settings.vscode.editors.unnamed")}
        </div>
        <p className={MUTED}>
          {t("settings.vscode.editors.pairedOn", {
            date: formatDateTime(t.lang, client.pairedAt),
          })}
        </p>
        <div data-testid="vscode-editor-connection" className="mt-1">
          {client.connected ? (
            <span className="inline-flex items-center rounded-md bg-green-100 px-1.5 py-0.5 text-xs/5 font-medium text-green-800 dark:bg-green-800 dark:text-green-100">
              {t("settings.vscode.editors.connected")}
            </span>
          ) : (
            <p className={MUTED}>
              {client.lastSeenAt === null
                ? t("settings.vscode.editors.neverSeen")
                : t("settings.vscode.editors.lastSeen", {
                    date: formatDateTime(t.lang, client.lastSeenAt),
                  })}
            </p>
          )}
        </div>
      </div>
      <button
        type="button"
        className={SMALL_BUTTON}
        aria-describedby={nameId}
        aria-disabled={removing}
        onClick={onRemove}
      >
        {t("common.actions.remove")}
      </button>
    </li>
  );
}

/**
 * Code d'appairage en cours : les six chiffres en grand, leur heure de fin de
 * validité et la commande à lancer dans VS Code. Les chiffres restent un seul
 * texte (copié sans espace), espacés par la feuille de style ; un lecteur
 * d'écran les entend un par un (voir EasyLink.tsx).
 */
function PairingCode({
  pairing,
  cancelling,
  onCancel,
}: {
  pairing: BridgePairing;
  cancelling: boolean;
  onCancel: () => void;
}) {
  const t = useT();
  const spelled = t("settings.vscode.pairing.codeLabel", {
    code: [...pairing.code].join(" "),
  });
  return (
    <div
      data-vscode-pairing=""
      data-testid="vscode-pairing"
      className="space-y-2 rounded-lg border border-neutral-200 bg-neutral-50 p-4 dark:border-neutral-700 dark:bg-neutral-900"
    >
      {/* role img : un seul objet pour le lecteur d'écran, qui lit le nom
          (chiffres épelés) et non le nombre. Il prend le focus à son
          apparition (EditorsCard) : le code est lu aussitôt. */}
      <div
        role="img"
        aria-label={spelled}
        tabIndex={-1}
        data-vscode-code=""
        data-testid="vscode-code"
        className="rounded-md font-mono text-3xl/10 font-medium tracking-[0.3em] text-zinc-950 outline-none select-text dark:text-white"
      >
        {pairing.code}
      </div>
      <p className={MUTED}>
        {t("settings.vscode.pairing.expires", {
          time: formatTime(t.lang, pairing.expiresAt),
        })}
      </p>
      <p className={TEXT}>
        <Trans k="settings.vscode.pairing.instructions" tags={{ code }} />
      </p>
      <div className="pt-1">
        <button
          type="button"
          className={SMALL_BUTTON}
          aria-disabled={cancelling}
          onClick={onCancel}
        >
          {t("common.actions.cancel")}
        </button>
      </div>
    </div>
  );
}
