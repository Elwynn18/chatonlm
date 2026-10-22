// Onglet « Keyboard shortcuts » des réglages (propre à ChatonLM). D'abord les
// raccourcis du système, qui marchent dans toute application et dont le
// bureau choisit les touches (portail GlobalShortcuts) : la dictée (son
// interrupteur est dans l'onglet Dictation, lien quand elle est coupée) et
// « Correct selected text » (interrupteur `proofreadShortcut`, coupé par
// défaut ; sa première liaison ouvre la boîte du bureau ; sous lui,
// « Replace the selection » : interrupteur `proofreadPaste`, activé par
// défaut, le texte corrigé est collé à la place de la sélection au lieu
// d'être seulement copié). Chacun montre son
// état : touches liées et « Change… » (boîte du bureau), attente du bureau,
// refus (« Set shortcut »), échec (« Try again »), ou la commande à lier
// soi-même quand le portail manque. Sous Windows (`chooser` « app » dans le
// statut), ChatonLM enregistre lui-même les touches et les fait choisir ici :
// « Change… » ouvre la saisie de la combinaison (ShortcutRecorder.tsx), un
// échec (combinaison tenue par une autre application) offre « Change… » et
// « Try again », sans commande à lier. Ensuite les raccourcis de l'app, en
// lecture, par groupe, depuis le catalogue de src/lib/shortcuts.ts (celui
// des gestionnaires).
//
// « Try again », « Set shortcut » et « Change… » agissent sur les deux
// raccourcis à la fois (shortcuts_bind les relie ensemble) : mêmes actions
// pour les deux lignes, occupées ensemble, et leur échec montré une fois,
// sous les deux lignes, tant que le statut n'a pas changé depuis.

import {
  ArrowPathIcon,
  ClipboardDocumentCheckIcon,
  DocumentCheckIcon,
  MicrophoneIcon,
} from "@heroicons/react/20/solid";
import { useQueryClient } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { type ReactNode, useId, useState } from "react";
import type { ShortcutChooser } from "@/bindings/ShortcutChooser";
import type { ShortcutStatus } from "@/bindings/ShortcutStatus";
import { CommandBlock } from "@/components/CommandBlock";
import { Row, RowError, Switch } from "@/components/settings/Row";
import { AppShortcut } from "@/components/settings/ShortcutRecorder";
import { KBD, ShortcutKeys } from "@/components/ShortcutKeys";
import { useSettings } from "@/hooks/useSettings";
import {
  shortcutsStatusKey,
  useShortcutsActions,
  useShortcutsStatus,
} from "@/hooks/useShortcuts";
import { type TKey, useT } from "@/i18n";
import { errorMessage, type GlobalShortcutId } from "@/lib/ipc";
import { isWindows } from "@/lib/platform";
import {
  SHORTCUT_GROUPS,
  SHORTCUTS,
  type ShortcutGroup,
  type ShortcutId,
} from "@/lib/shortcuts";

const MUTED = "text-sm/6 text-zinc-500 dark:text-zinc-400";
const LINK =
  "rounded-md text-sm/6 text-zinc-700 underline underline-offset-2 outline-none hover:text-zinc-950 focus-visible:ring-2 focus-visible:ring-accent-fg dark:text-zinc-300 dark:hover:text-white";
const SMALL_BUTTON =
  "inline-flex flex-none items-center gap-1.5 rounded-lg border border-zinc-950/10 bg-white px-3 py-1 text-sm/6 font-medium text-zinc-950 shadow-sm outline-none hover:bg-zinc-50 focus-visible:ring-2 focus-visible:ring-accent-fg disabled:cursor-default disabled:opacity-50 dark:border-white/15 dark:bg-zinc-900 dark:text-white dark:hover:bg-zinc-800";
const ALERT = "mt-2 text-xs break-words text-red-600 dark:text-red-400";
const CARD = "overflow-hidden rounded-xl bg-white dark:bg-neutral-800";

const GROUP_LABELS: Record<ShortcutGroup, TKey> = {
  general: "settings.shortcuts.app.groups.general",
  chat: "settings.shortcuts.app.groups.chat",
  voiceMode: "settings.shortcuts.app.groups.voiceMode",
  proofread: "settings.shortcuts.app.groups.proofread",
  hub: "settings.shortcuts.app.groups.hub",
};

const ACTION_LABELS: Record<ShortcutId, TKey> = {
  newChat: "settings.shortcuts.app.actions.newChat",
  newEphemeralChat: "settings.shortcuts.app.actions.newEphemeralChat",
  settings: "settings.shortcuts.app.actions.settings",
  quit: "settings.shortcuts.app.actions.quit",
  zoomIn: "settings.shortcuts.app.actions.zoomIn",
  zoomOut: "settings.shortcuts.app.actions.zoomOut",
  actualSize: "settings.shortcuts.app.actions.actualSize",
  undoTyping: "settings.shortcuts.app.actions.undoTyping",
  redoTyping: "settings.shortcuts.app.actions.redoTyping",
  send: "settings.shortcuts.app.actions.send",
  newLine: "settings.shortcuts.app.actions.newLine",
  stopAnswer: "settings.shortcuts.app.actions.stopAnswer",
  cancelEdit: "settings.shortcuts.app.actions.cancelEdit",
  paste: "settings.shortcuts.app.actions.paste",
  voiceModeToggle: "settings.shortcuts.app.actions.voiceModeToggle",
  voiceModeSpace: "settings.shortcuts.app.actions.voiceModeSpace",
  voiceModeEnd: "settings.shortcuts.app.actions.voiceModeEnd",
  correct: "settings.shortcuts.app.actions.correct",
  rephrase: "settings.shortcuts.app.actions.rephrase",
  stopProofread: "settings.shortcuts.app.actions.stopProofread",
  undoCorrection: "settings.shortcuts.app.actions.undoCorrection",
  redoCorrection: "settings.shortcuts.app.actions.redoCorrection",
  pasteInReview: "settings.shortcuts.app.actions.pasteInReview",
  searchModels: "settings.shortcuts.app.actions.searchModels",
};

/** Échec d'une action, avec le statut (en JSON) qu'il concerne. */
interface Failure {
  message: string;
  status: string;
}

export function Shortcuts({ onSaved }: { onSaved: () => void }) {
  const t = useT();
  const queryClient = useQueryClient();
  const status = useShortcutsStatus();
  const { settings, setSettings } = useSettings();
  const { bind, configure, refresh } = useShortcutsActions();
  const [saveError, setSaveError] = useState<string | null>(null);
  const [failure, setFailure] = useState<Failure | null>(null);
  // Statut changé depuis l'échec (raccourci lié, coupé...) : l'erreur ne
  // le concerne plus. Ajusté pendant le rendu, sans effet.
  const statusKey = JSON.stringify(status.data ?? null);
  if (failure && failure.status !== statusKey) setFailure(null);
  const busy = bind.isPending || configure.isPending;
  /**
   * Lance une action ; son échec est noté avec le statut relu après elle
   * (les rappels de `mutate` viennent après la relecture de onSettled).
   */
  const act = (mutation: typeof bind | typeof configure) => {
    setFailure(null);
    mutation.mutate(undefined, {
      onError: (cause) =>
        setFailure({
          message: errorMessage(cause),
          status: JSON.stringify(
            queryClient.getQueryData(shortcutsStatusKey) ?? null,
          ),
        }),
    });
  };
  const actions = {
    busy,
    onBind: () => act(bind),
    onConfigure: () => act(configure),
  };
  const ids = {
    global: useId(),
    globalDescription: useId(),
    dictation: useId(),
    proofread: useId(),
    proofreadDescription: useId(),
    paste: useId(),
    pasteDescription: useId(),
    app: useId(),
  };
  // Échec de l'enregistrement de « Replace the selection ».
  const [pasteError, setPasteError] = useState<string | null>(null);
  const togglePaste = async (proofreadPaste: boolean) => {
    setPasteError(null);
    try {
      await setSettings({ proofreadPaste });
    } catch (cause) {
      setPasteError(errorMessage(cause));
      return;
    }
    onSaved();
  };

  const toggle = async (proofreadShortcut: boolean) => {
    setSaveError(null);
    try {
      await setSettings({ proofreadShortcut });
    } catch (cause) {
      setSaveError(errorMessage(cause));
      return;
    }
    // Liaison commencée ou raccourci délié : statut relu (l'évènement
    // l'apporte aussi).
    void refresh();
    onSaved();
  };

  const data = status.data;
  return (
    <div data-testid="shortcuts" className="space-y-4">
      <section aria-labelledby={ids.global} className={CARD}>
        <div className="space-y-4 p-4">
          <div>
            <h2
              id={ids.global}
              className="text-sm/6 font-medium text-zinc-950 dark:text-white"
            >
              {t("settings.shortcuts.global.title")}
            </h2>
            <p id={ids.globalDescription} className={MUTED}>
              {t(
                data?.chooser === "app"
                  ? "settings.shortcuts.global.descriptionApp"
                  : "settings.shortcuts.global.description",
              )}
            </p>
          </div>

          <Row
            icon={MicrophoneIcon}
            label={t("settings.shortcuts.global.dictation.label")}
            description={t("settings.shortcuts.global.dictation.description")}
            descriptionId={ids.dictation}
          >
            {data && (
              <ShortcutLine
                testId="shortcut-dictation"
                id="dictation"
                name={t("settings.shortcuts.global.dictation.label")}
                chooser={data.chooser}
                actions={actions}
                status={data.dictation}
                canConfigure={data.canConfigure}
                command={data.dictateCommand}
                off={
                  <p className={MUTED}>
                    {t("settings.shortcuts.global.dictation.off")}{" "}
                    <Link
                      to="/settings"
                      search={{ section: "dictation" }}
                      className={LINK}
                    >
                      {t("settings.shortcuts.global.dictation.settings")}
                    </Link>
                  </p>
                }
              />
            )}
          </Row>

          <div className="flex items-start justify-between gap-4">
            <Row
              icon={DocumentCheckIcon}
              label={
                <span id={ids.proofread}>
                  {t("settings.shortcuts.global.proofread.label")}
                </span>
              }
              description={t("settings.shortcuts.global.proofread.description")}
              descriptionId={ids.proofreadDescription}
            >
              {data && (
                <ShortcutLine
                  testId="shortcut-proofread"
                  id="proofread"
                  name={t("settings.shortcuts.global.proofread.label")}
                  chooser={data.chooser}
                  actions={actions}
                  status={data.proofread}
                  canConfigure={data.canConfigure}
                  command={data.proofreadCommand}
                  off={
                    <p className={MUTED}>
                      {t("settings.shortcuts.global.proofread.off")}
                    </p>
                  }
                />
              )}
              <RowError message={saveError} />
            </Row>
            <Switch
              label={t("settings.shortcuts.global.proofread.label")}
              checked={settings?.proofreadShortcut ?? false}
              describedBy={ids.proofreadDescription}
              disabled={!settings}
              onChange={(checked) => void toggle(checked)}
            />
          </div>

          <div className="flex items-start justify-between gap-4">
            <Row
              icon={ClipboardDocumentCheckIcon}
              label={
                <span id={ids.paste}>
                  {t("settings.shortcuts.global.proofread.paste.label")}
                </span>
              }
              description={
                <>
                  {t("settings.shortcuts.global.proofread.paste.description")}
                  {/* Linux : le bureau demande une fois l'autorisation
                      d'envoyer la touche (portail RemoteDesktop). */}
                  {!isWindows() && (
                    <>
                      {" "}
                      {t("settings.shortcuts.global.proofread.paste.askHint")}
                    </>
                  )}
                </>
              }
              descriptionId={ids.pasteDescription}
            >
              <RowError message={pasteError} />
            </Row>
            <Switch
              label={t("settings.shortcuts.global.proofread.paste.label")}
              checked={settings?.proofreadPaste ?? true}
              describedBy={ids.pasteDescription}
              disabled={!settings}
              onChange={(checked) => void togglePaste(checked)}
            />
          </div>

          {failure && (
            <p role="alert" data-testid="shortcut-failure" className={ALERT}>
              {failure.message}
            </p>
          )}

          {status.isError && !data && (
            <p role="alert" className={ALERT}>
              {t("settings.shortcuts.global.loadFailed", {
                message: errorMessage(status.error),
              })}
            </p>
          )}
        </div>
      </section>

      <section aria-labelledby={ids.app} className={CARD}>
        <div className="space-y-4 p-4">
          <h2
            id={ids.app}
            className="text-sm/6 font-medium text-zinc-950 dark:text-white"
          >
            {t("settings.shortcuts.app.title")}
          </h2>
          <div className="grid gap-x-10 gap-y-5 md:grid-cols-2">
            {SHORTCUT_GROUPS.map((group) => (
              <AppGroup key={group} group={group} />
            ))}
          </div>
        </div>
      </section>
    </div>
  );
}

/** Un groupe de raccourcis de l'app : action, puis touches. */
function AppGroup({ group }: { group: ShortcutGroup }) {
  const t = useT();
  const titleId = useId();
  return (
    <div>
      <h3
        id={titleId}
        className="mb-1 text-xs font-medium tracking-wide text-zinc-500 uppercase dark:text-zinc-400"
      >
        {t(GROUP_LABELS[group])}
      </h3>
      <dl aria-labelledby={titleId} data-testid={`shortcut-group-${group}`}>
        {SHORTCUTS.filter((entry) => entry.group === group).map((entry) => (
          <div
            key={entry.id}
            className="flex items-center justify-between gap-4 border-b border-neutral-100 py-1.5 last:border-b-0 dark:border-neutral-700/60"
          >
            <dt className="text-sm/6 text-zinc-800 dark:text-zinc-200">
              {t(ACTION_LABELS[entry.id])}
            </dt>
            <dd className="flex-none">
              <ShortcutKeys keys={entry.keys} />
            </dd>
          </div>
        ))}
      </dl>
    </div>
  );
}

/**
 * État d'un raccourci du système : coupé (`off`), attente du bureau, touches
 * liées et « Change… », refus ou échec, commande à lier soi-même.
 */
function ShortcutLine({
  testId,
  id,
  name,
  chooser,
  actions: { busy, onBind, onConfigure },
  status,
  canConfigure,
  command,
  off,
}: {
  testId: string;
  id: GlobalShortcutId;
  /** Nom du raccourci, pour la saisie de ses touches. */
  name: string;
  /** Qui choisit les touches : le bureau (Linux) ou l'app (Windows). */
  chooser: ShortcutChooser;
  /** Actions partagées par les deux lignes. */
  actions: { busy: boolean; onBind: () => void; onConfigure: () => void };
  status: ShortcutStatus;
  canConfigure: boolean;
  command: string;
  off: ReactNode;
}) {
  const t = useT();
  const notSet = (viaDialog: boolean) => (
    <span className="flex flex-wrap items-center gap-3">
      <span className="text-sm/6 text-zinc-950 dark:text-white">
        {t("settings.shortcuts.global.notSet")}
      </span>
      <button
        type="button"
        className={SMALL_BUTTON}
        disabled={busy}
        onClick={viaDialog ? onConfigure : onBind}
      >
        {t("settings.shortcuts.global.set")}
      </button>
    </span>
  );

  // Touches choisies dans l'app (Windows) : saisie, jamais de commande.
  if (
    chooser === "app" &&
    (status.state === "bound" || status.state === "unavailable")
  ) {
    return (
      <div className="mt-2" data-testid={testId}>
        <AppShortcut
          id={id}
          name={name}
          status={status}
          busy={busy}
          onRetry={onBind}
        />
      </div>
    );
  }

  let line: ReactNode;
  switch (status.state) {
    case "disabled":
      line = off;
      break;
    case "binding":
      line = (
        <p className={`${MUTED} flex items-center gap-2`}>
          <ArrowPathIcon
            aria-hidden="true"
            className="h-4 w-4 animate-spin text-zinc-500"
          />
          {t("settings.shortcuts.global.waiting")}
        </p>
      );
      break;
    case "bound":
      line = status.trigger ? (
        <span className="flex flex-wrap items-center gap-3">
          <kbd className={KBD}>{status.trigger}</kbd>
          {canConfigure && (
            <button
              type="button"
              className={SMALL_BUTTON}
              disabled={busy}
              onClick={onConfigure}
            >
              {t("settings.shortcuts.global.change")}
            </button>
          )}
        </span>
      ) : (
        // Lié sans touche : la boîte du bureau la fait choisir, si elle existe.
        notSet(canConfigure)
      );
      break;
    case "unavailable":
      line = (
        <div className="space-y-2">
          {status.problem === "refused" ? (
            notSet(false)
          ) : (
            <span className="flex flex-wrap items-center gap-3">
              <span className="text-sm/6 text-amber-700 dark:text-amber-400">
                {status.reason}
              </span>
              {/* Sans portail, un nouvel essai ne peut pas réussir. */}
              {status.problem !== "noPortal" && (
                <button
                  type="button"
                  className={SMALL_BUTTON}
                  disabled={busy}
                  onClick={onBind}
                >
                  {t("settings.shortcuts.global.tryAgain")}
                </button>
              )}
            </span>
          )}
          <p className={MUTED}>{t("settings.shortcuts.global.bindCommand")}</p>
          <CommandBlock command={command} />
        </div>
      );
      break;
  }
  return (
    <div className="mt-2" data-testid={testId}>
      {line}
    </div>
  );
}
