// Raccourci global dont l'app fait choisir les touches (propre à ChatonLM,
// Windows : `chooser` vaut « app » dans le statut des raccourcis ; sous Linux,
// le bureau les choisit et rien d'ici n'est affiché). Ligne partagée par
// l'onglet « Keyboard shortcuts » et la carte « Dictation » des réglages :
// touches liées et « Change… », ou la raison d'un échec (combinaison tenue
// par une autre application) avec « Change… » et « Try again » ; jamais de
// commande à lier soi-même (Windows n'a pas de réglage système des
// raccourcis).
//
// « Change… » ouvre la saisie : les raccourcis globaux sont suspendus par le
// moteur (shortcuts_recording), la zone prend le focus et la combinaison
// pressée est proposée au moteur, qui la contrôle (touche permise,
// modificateur, autre raccourci de ChatonLM) : acceptée, le statut rendu
// remplace celui de l'écran et la saisie se ferme ; refusée, sa raison
// (texte du moteur) s'affiche sous la zone, qui reste ouverte. Échap seule,
// « Cancel » ou la perte du focus ferment sans rien changer ; « Reset to
// default » rend la combinaison par défaut. Tab reste au navigateur. Fermée,
// la saisie rend le focus à « Change… ».

import { useQueryClient } from "@tanstack/react-query";
import {
  type FocusEvent,
  type KeyboardEvent,
  useEffect,
  useId,
  useRef,
  useState,
} from "react";
import type { ShortcutsStatus } from "@/bindings/ShortcutsStatus";
import type { ShortcutStatus } from "@/bindings/ShortcutStatus";
import { KBD } from "@/components/ShortcutKeys";
import { dictationStatusKey } from "@/hooks/useDictation";
import { applyShortcutsStatus } from "@/hooks/useShortcuts";
import { useT } from "@/i18n";
import {
  errorMessage,
  type GlobalShortcutId,
  shortcutsRecording,
  shortcutsResetTrigger,
  shortcutsSetTrigger,
} from "@/lib/ipc";
import { recordedKey, setRecordingShortcut } from "@/lib/shortcuts";

const MUTED = "text-sm/6 text-zinc-500 dark:text-zinc-400";
const SMALL_BUTTON =
  "inline-flex flex-none items-center gap-1.5 rounded-lg border border-zinc-950/10 bg-white px-3 py-1 text-sm/6 font-medium text-zinc-950 shadow-sm outline-none hover:bg-zinc-50 focus-visible:ring-2 focus-visible:ring-accent-fg disabled:cursor-default disabled:opacity-50 dark:border-white/15 dark:bg-zinc-900 dark:text-white dark:hover:bg-zinc-800";
const ZONE =
  "inline-flex min-w-56 cursor-default items-center justify-center rounded-lg border border-dashed border-zinc-400 bg-zinc-50 px-3 py-1 text-sm/6 text-zinc-700 outline-none focus:border-solid focus:border-accent-fg focus:ring-2 focus:ring-accent-fg/40 dark:border-zinc-500 dark:bg-zinc-900 dark:text-zinc-200";
const ALERT = "text-xs break-words text-red-600 dark:text-red-400";

const report = (action: string) => (cause: unknown) =>
  console.error(`Failed to ${action}:`, cause);

/**
 * Ligne d'un raccourci global choisi dans l'app. `name` : nom du raccourci
 * (« Dictation »), pour les lecteurs d'écran ; `onRetry` : nouvel essai
 * d'enregistrement (« Try again », après un échec) ; `busy` : une action de
 * l'écran est en cours.
 */
export function AppShortcut({
  id,
  name,
  status,
  busy,
  onRetry,
}: {
  id: GlobalShortcutId;
  name: string;
  status: Extract<ShortcutStatus, { state: "bound" | "unavailable" }>;
  busy: boolean;
  onRetry: () => void;
}) {
  const t = useT();
  const [recording, setRecording] = useState(false);
  const change = useRef<HTMLButtonElement>(null);
  // Saisie fermée par elle-même (combinaison acceptée, Échap, « Cancel ») :
  // le focus revient à « Change… ». Pas quand il est parti ailleurs.
  const giveFocusBack = useRef(false);
  useEffect(() => {
    if (recording || !giveFocusBack.current) return;
    giveFocusBack.current = false;
    change.current?.focus({ preventScroll: true });
  }, [recording]);

  if (recording) {
    return (
      <Recorder
        id={id}
        name={name}
        onDone={(focusLeft) => {
          giveFocusBack.current = !focusLeft;
          setRecording(false);
        }}
      />
    );
  }

  const changeButton = (
    <button
      ref={change}
      type="button"
      className={SMALL_BUTTON}
      disabled={busy}
      onClick={() => setRecording(true)}
    >
      {t("settings.shortcuts.global.change")}
    </button>
  );

  if (status.state === "bound") {
    return (
      <span className="flex flex-wrap items-center gap-3">
        {status.trigger && <kbd className={KBD}>{status.trigger}</kbd>}
        {changeButton}
      </span>
    );
  }
  return (
    <span className="flex flex-wrap items-center gap-3">
      <span className="text-sm/6 text-amber-700 dark:text-amber-400">
        {status.reason}
      </span>
      {changeButton}
      <button
        type="button"
        className={SMALL_BUTTON}
        disabled={busy}
        onClick={onRetry}
      >
        {t("settings.shortcuts.global.tryAgain")}
      </button>
    </span>
  );
}

/**
 * Saisie de la combinaison ; `onDone` la ferme (`focusLeft` : parce que le
 * focus en est sorti).
 */
function Recorder({
  id,
  name,
  onDone,
}: {
  id: GlobalShortcutId;
  name: string;
  onDone: (focusLeft?: boolean) => void;
}) {
  const t = useT();
  const queryClient = useQueryClient();
  const zone = useRef<HTMLButtonElement>(null);
  const hintId = useId();
  const [error, setError] = useState<string | null>(null);
  // Demande au moteur en cours : les touches suivantes attendent sa réponse.
  const pending = useRef(false);
  const mounted = useRef(false);

  // Ouverte : raccourcis globaux suspendus (la combinaison actuelle arrive
  // ici au lieu de lancer la dictée) et raccourcis de l'app au repos ;
  // fermée, quelle qu'en soit la raison, tout est rétabli.
  useEffect(() => {
    mounted.current = true;
    setRecordingShortcut(true);
    shortcutsRecording(true).catch(report("suspend the global shortcuts"));
    zone.current?.focus({ preventScroll: true });
    return () => {
      mounted.current = false;
      setRecordingShortcut(false);
      shortcutsRecording(false).catch(report("resume the global shortcuts"));
    };
  }, []);

  /** Statut rendu par le moteur : il remplace celui de l'écran. */
  const accept = (status: ShortcutsStatus) => {
    applyShortcutsStatus(queryClient, status);
    // La carte Dictation lit aussi son raccourci dans le statut de la dictée.
    void queryClient.invalidateQueries({ queryKey: dictationStatusKey });
    if (mounted.current) onDone();
  };
  /** Refus du moteur : sa raison, la zone reste ouverte et garde le focus. */
  const refuse = (cause: unknown) => {
    pending.current = false;
    if (!mounted.current) return;
    setError(errorMessage(cause));
    zone.current?.focus({ preventScroll: true });
  };
  const ask = (request: Promise<ShortcutsStatus>) => {
    pending.current = true;
    setError(null);
    request.then(accept, refuse);
  };

  const onKeyDown = (event: KeyboardEvent<HTMLButtonElement>) => {
    const recorded = recordedKey(event.nativeEvent);
    if (recorded.kind === "pass") return;
    // Ni clic du bouton (Entrée, Espace), ni raccourci de l'app.
    event.preventDefault();
    event.stopPropagation();
    if (recorded.kind === "ignore" || pending.current) return;
    if (recorded.kind === "cancel") onDone();
    else ask(shortcutsSetTrigger(id, recorded.keys));
  };

  // Focus sorti de la saisie (autre champ, autre fenêtre) : fermée.
  const onBlur = (event: FocusEvent<HTMLDivElement>) => {
    if (!event.currentTarget.contains(event.relatedTarget)) onDone(true);
  };

  return (
    <div
      role="group"
      aria-label={t("settings.shortcuts.recorder.label", { name })}
      data-testid={`shortcut-recorder-${id}`}
      className="space-y-2"
      onBlur={onBlur}
    >
      <div className="flex flex-wrap items-center gap-3">
        <button
          ref={zone}
          type="button"
          data-testid="shortcut-recorder-zone"
          aria-describedby={hintId}
          className={ZONE}
          onKeyDown={onKeyDown}
          // Entrée et Espace relâchées ne « cliquent » pas la zone.
          onKeyUp={(event) => event.preventDefault()}
        >
          {t("settings.shortcuts.recorder.prompt")}
        </button>
        <button
          type="button"
          className={SMALL_BUTTON}
          onClick={() => {
            if (!pending.current) ask(shortcutsResetTrigger(id));
          }}
        >
          {t("settings.shortcuts.recorder.reset")}
        </button>
        <button type="button" className={SMALL_BUTTON} onClick={() => onDone()}>
          {t("common.actions.cancel")}
        </button>
      </div>
      <p id={hintId} className={MUTED}>
        {t("settings.shortcuts.recorder.hint")}
      </p>
      {error && (
        <p role="alert" className={ALERT}>
          {error}
        </p>
      )}
    </div>
  );
}
