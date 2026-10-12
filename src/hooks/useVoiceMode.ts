// Mode conversation vocale (propre à ChatonLM) : magasin global hors de React
// (une seule séance dans l'app), lu par useSyncExternalStore. Il tient l'état
// du moteur (événement « voice-mode »), la conversation dont la page a lancé
// la séance (`owner`), le lancement (prérequis, boîte d'accord,
// téléchargements, démarrage) et les phrases transcrites
// (« voice-mode-utterance »), remises à la zone de saisie de cette
// conversation, qui les envoie. Les niveaux (« voice-mode-level », ~15 fois
// par seconde) ont leurs propres abonnés : l'indicateur les écrit dans une
// variable CSS, sans rendre la page de nouveau.
//
// Un seul écouteur, monté avec l'app (useVoiceModeEvents, route racine). Il
// termine la séance quand la page de sa conversation n'est plus affichée
// (autre conversation, réglages, ModelHub...), sauf pour la conversation
// créée par le premier envoi vocal d'une nouvelle conversation, qui l'adopte
// (adoptVoiceModeChat, avant d'y aller) avec le brouillon de la zone de
// saisie (texte, fichiers préparés), rendu à la fin de la séance sur sa
// page ; une séance que plus aucune page ne tient (page rechargée) est
// arrêtée.

import {
  type QueryClient,
  useMutation,
  useQuery,
  useQueryClient,
} from "@tanstack/react-query";
import { useRouterState } from "@tanstack/react-router";
import {
  type RefObject,
  useEffect,
  useEffectEvent,
  useSyncExternalStore,
} from "react";
import type { StagedFile } from "@/bindings/StagedFile";
import type { VoiceModeLevel } from "@/bindings/VoiceModeLevel";
import type { VoiceModeRequirements } from "@/bindings/VoiceModeRequirements";
import type { VoiceModeSettings } from "@/bindings/VoiceModeSettings";
import type { VoiceModeSettingsPatch } from "@/bindings/VoiceModeSettingsPatch";
import type { VoiceModeState } from "@/bindings/VoiceModeState";
import type { VoiceModeUtterance } from "@/bindings/VoiceModeUtterance";
import { release } from "@/hooks/useComposerFiles";
import {
  errorMessage,
  getVoiceModeSettings,
  listenVoiceMode,
  listenVoiceModeLevel,
  listenVoiceModeUtterance,
  updateVoiceModeSettings,
  voiceModeInfo,
  voiceModeRequirements,
  voiceModeStart,
  voiceModeState,
  voiceModeStop,
} from "@/lib/ipc";
import { chatOfPath } from "@/lib/readAloud";
import { VOICE_MODE_OFF } from "@/lib/voiceMode";

/**
 * Lancement d'une séance par une page : lecture des prérequis, accord
 * demandé (fichiers manquants), téléchargements en cours (identifiants des
 * fichiers attendus), puis démarrage du moteur.
 */
export type VoiceModeLaunch =
  | { step: "checking" }
  | { step: "asking"; requirements: VoiceModeRequirements }
  | {
      step: "downloading";
      requirements: VoiceModeRequirements;
      /** Modèle de transcription attendu, null s'il est déjà installé. */
      speech: string | null;
      /** Voix attendue, null si elle est déjà installée. */
      voice: string | null;
    }
  | { step: "starting" };

export interface VoiceModeSnapshot {
  /** État du moteur. */
  state: VoiceModeState;
  /** Conversation de la séance ou de son lancement (« new » comprise). */
  owner: string | null;
  launch: VoiceModeLaunch | null;
  /** Échec du dernier lancement, montré sur la page qui l'a demandé. */
  error: { chatId: string; message: string } | null;
  /**
   * Séance adoptée par la conversation créée (`to`) dont la zone de saisie
   * n'est pas encore montée : celle de « new » (`from`) garde sa barre
   * jusque-là (l'adresse change avant la page affichée).
   */
  adopted: { from: string; to: string } | null;
}

const INITIAL: VoiceModeSnapshot = {
  state: VOICE_MODE_OFF,
  owner: null,
  launch: null,
  error: null,
  adopted: null,
};

/** Brouillon de la zone de saisie : texte et fichiers préparés. */
export interface VoiceModeDraft {
  content: string;
  files: StagedFile[];
}

/** Zones de saisie qui cèdent leur brouillon à l'adoption, par conversation. */
const draftTakers = new Map<string, () => VoiceModeDraft>();
/** Brouillons cédés, par conversation qui les reprend. */
const handedDrafts = new Map<string, VoiceModeDraft>();

let snapshot: VoiceModeSnapshot = INITIAL;
const listeners = new Set<() => void>();

function set(change: Partial<VoiceModeSnapshot>) {
  snapshot = { ...snapshot, ...change };
  for (const listener of listeners) listener();
}

export function subscribeVoiceMode(listener: () => void) {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

export function voiceModeSnapshot(): VoiceModeSnapshot {
  return snapshot;
}

const SILENCE: VoiceModeLevel = { input: 0, output: 0 };
let level: VoiceModeLevel = SILENCE;
const levelListeners = new Set<(level: VoiceModeLevel) => void>();

/** Niveaux du micro et de la voix, à chaque événement (hors de React). */
export function subscribeVoiceModeLevel(
  listener: (level: VoiceModeLevel) => void,
) {
  levelListeners.add(listener);
  return () => {
    levelListeners.delete(listener);
  };
}

function setLevel(next: VoiceModeLevel) {
  level = next;
  for (const listener of levelListeners) listener(next);
}

/** Zones de saisie qui envoient les phrases, par conversation. */
const handlers = new Map<string, (utterance: VoiceModeUtterance) => void>();
/** Phrases arrivées avant que la page de la séance ne soit prête. */
let queued: VoiceModeUtterance[] = [];
/** Dernier tour remis : une phrase n'est jamais envoyée deux fois. */
let lastTurn = 0;
/**
 * Numéro du lancement : change à chaque lancement et à chaque arrêt ; la
 * réponse d'une commande d'un lancement abandonné est ignorée.
 */
let generation = 0;
/** Événements « voice-mode » reçus (une lecture plus ancienne est ignorée). */
let received = 0;

function stopEngine() {
  voiceModeStop().catch((error: unknown) =>
    console.error("Failed to stop voice mode:", error),
  );
}

/** Lancement en échec : la page le dit au-dessus de la zone de saisie. */
function fail(chatId: string, message: string) {
  set({ owner: null, launch: null, error: { chatId, message } });
}

/**
 * Lance le mode vocal pour la conversation `chatId` : prérequis lus ;
 * machine incapable → erreur ; fichiers manquants → accord demandé
 * (confirmVoiceModeDownloads) ; sinon démarrage. Sans effet si la séance
 * est déjà celle de cette page.
 */
export async function startVoiceMode(chatId: string): Promise<void> {
  if (snapshot.owner === chatId) return;
  if (snapshot.owner !== null) stopVoiceMode();
  generation += 1;
  const run = generation;
  lastTurn = 0;
  queued = [];
  set({ owner: chatId, launch: { step: "checking" }, error: null });
  let requirements: VoiceModeRequirements;
  try {
    requirements = await voiceModeRequirements();
  } catch (error) {
    if (run === generation) fail(chatId, errorMessage(error));
    return;
  }
  if (run !== generation) return;
  if (requirements.unavailable !== null) {
    fail(chatId, requirements.unavailable);
    return;
  }
  if (!requirements.speechModelInstalled || !requirements.voiceInstalled) {
    set({ launch: { step: "asking", requirements } });
    return;
  }
  await begin(run, chatId);
}

/** Démarrage du moteur ; le lancement s'efface au premier état actif. */
async function begin(run: number, chatId: string) {
  set({ launch: { step: "starting" } });
  try {
    await voiceModeStart();
  } catch (error) {
    if (run === generation) fail(chatId, errorMessage(error));
    return;
  }
  if (run !== generation) {
    // Arrêté pendant le démarrage, et personne d'autre ne l'a relancé : le
    // moteur a pu démarrer après l'arrêt.
    if (snapshot.owner === null) stopEngine();
    return;
  }
  if (snapshot.state.phase !== "off") set({ launch: null });
}

/**
 * Accord donné : la boîte se ferme, la page relit les prérequis (un fichier a
 * pu s'installer entre-temps). Renvoie le numéro du lancement, null s'il n'y
 * avait pas de boîte.
 */
export function voiceModeConfirmed(): number | null {
  if (snapshot.launch?.step !== "asking") return null;
  set({ launch: { step: "checking" } });
  return generation;
}

/**
 * Prérequis relus après l'accord (lancement `run`) : ce qui manque encore,
 * que la page télécharge ; plus rien : la séance démarre (null). Null aussi
 * si le lancement a été abandonné entre-temps.
 */
export function voiceModeDownloading(
  run: number,
  requirements: VoiceModeRequirements,
): { speech: string | null; voice: string | null } | null {
  const owner = snapshot.owner;
  if (
    run !== generation ||
    owner === null ||
    snapshot.launch?.step !== "checking"
  ) {
    return null;
  }
  if (requirements.unavailable !== null) {
    fail(owner, requirements.unavailable);
    return null;
  }
  const speech = requirements.speechModelInstalled
    ? null
    : requirements.speechModel;
  const voice = requirements.voiceInstalled ? null : requirements.voiceModel;
  if (speech === null && voice === null) {
    void begin(run, owner);
    return null;
  }
  set({ launch: { step: "downloading", requirements, speech, voice } });
  return { speech, voice };
}

/** Fichiers installés : la séance démarre. */
export function voiceModeDownloadsReady() {
  const owner = snapshot.owner;
  if (snapshot.launch?.step !== "downloading" || owner === null) return;
  void begin(generation, owner);
}

/** Lancement en échec (téléchargement, mise en file) ; texte déjà traduit. */
export function failVoiceModeLaunch(message: string) {
  const owner = snapshot.owner;
  if (snapshot.launch === null || owner === null) return;
  generation += 1;
  fail(owner, message);
}

/**
 * Termine la séance ou abandonne son lancement (les téléchargements déjà
 * lancés continuent, dans le ModelHub). La barre disparaît aussitôt.
 */
export function stopVoiceMode() {
  const { owner, launch, state } = snapshot;
  generation += 1;
  queued = [];
  if (owner === null && launch === null && state.phase === "off") return;
  set({ owner: null, launch: null, adopted: null });
  if (state.phase !== "off" || launch?.step === "starting") stopEngine();
}

/**
 * La nouvelle conversation (`from`, « new ») vient d'être créée par l'envoi
 * vocal : la séance la suit sur sa page (`to`).
 */
export function adoptVoiceModeChat(from: string, to: string) {
  if (snapshot.owner !== from) return;
  const draft = draftTakers.get(from)?.();
  if (draft && (draft.content !== "" || draft.files.length > 0)) {
    handedDrafts.set(to, draft);
  }
  set({ owner: to, adopted: { from, to } });
}

/**
 * La zone de saisie de `chatId` cède son brouillon (fichiers retirés d'elle,
 * pas libérés) si sa séance est adoptée ; renvoie la désinscription.
 */
export function registerVoiceModeDraft(
  chatId: string,
  take: () => VoiceModeDraft,
): () => void {
  draftTakers.set(chatId, take);
  return () => {
    if (draftTakers.get(chatId) === take) draftTakers.delete(chatId);
  };
}

/** Brouillon cédé à la conversation `chatId`, sans le retirer (rendu). */
export function peekVoiceModeDraft(chatId: string): VoiceModeDraft | null {
  return handedDrafts.get(chatId) ?? null;
}

/** Brouillon repris par la zone de saisie de `chatId` (après le montage). */
export function claimVoiceModeDraft(chatId: string) {
  handedDrafts.delete(chatId);
}

/** Efface l'échec du dernier lancement montré sur la page `chatId`. */
export function clearVoiceModeError(chatId: string) {
  if (snapshot.error?.chatId === chatId) set({ error: null });
}

/**
 * Page affichée (conversation, « new », ou null hors d'une conversation) :
 * la séance d'une autre page se termine, l'échec d'une autre page s'efface.
 */
export function voiceModePageShown(chatId: string | null) {
  if (snapshot.error !== null && snapshot.error.chatId !== chatId) {
    set({ error: null });
  }
  // Brouillon cédé à une page qui ne s'est pas affichée : ses fichiers ne
  // partiront plus.
  for (const [to, draft] of handedDrafts) {
    if (to === chatId) continue;
    handedDrafts.delete(to);
    release(draft.files.map((file) => file.id));
  }
  if (snapshot.owner !== null && snapshot.owner !== chatId) stopVoiceMode();
}

/** Applique un état reçu du moteur. */
export function applyVoiceModeState(state: VoiceModeState) {
  received += 1;
  const change: Partial<VoiceModeSnapshot> = { state };
  if (state.phase !== "off") {
    if (snapshot.launch?.step === "starting") change.launch = null;
    else if (snapshot.owner === null && snapshot.launch === null) {
      // Plus aucune page ne la tient (page rechargée, arrêt pendant le
      // démarrage) : le micro ne reste pas ouvert sans barre.
      stopEngine();
    }
  } else {
    setLevel(SILENCE);
    // Séance finie (fin, masquage de la fenêtre, conversation supprimée) ;
    // pendant un lancement, l'état d'avant ne compte pas.
    if (snapshot.launch === null) {
      change.owner = null;
      change.adopted = null;
      queued = [];
    }
  }
  set(change);
}

export function applyVoiceModeLevel(next: VoiceModeLevel) {
  setLevel(next);
}

/**
 * Phrase transcrite : remise à la zone de saisie de la conversation de la
 * séance, ou gardée jusqu'à ce qu'elle soit prête (page qui change pour la
 * conversation créée).
 */
export function applyVoiceModeUtterance(utterance: VoiceModeUtterance) {
  if (utterance.turn <= lastTurn) return;
  lastTurn = utterance.turn;
  const owner = snapshot.owner;
  if (owner === null) return;
  const handler = handlers.get(owner);
  if (handler) handler(utterance);
  else queued.push(utterance);
}

/** Inscrit la zone de saisie de `chatId` ; renvoie la désinscription. */
export function registerVoiceModeUtterances(
  chatId: string,
  handler: (utterance: VoiceModeUtterance) => void,
): () => void {
  handlers.set(chatId, handler);
  // Zone de saisie de la conversation qui a adopté la séance montée : celle
  // de « new » est partie.
  if (snapshot.adopted?.to === chatId) set({ adopted: null });
  if (snapshot.owner === chatId && queued.length > 0) {
    const pending = queued;
    queued = [];
    for (const utterance of pending) handler(utterance);
  }
  return () => {
    if (handlers.get(chatId) === handler) handlers.delete(chatId);
  };
}

/** Pour les essais : remet le magasin à zéro. */
export function resetVoiceModeForTests() {
  snapshot = INITIAL;
  level = SILENCE;
  handlers.clear();
  draftTakers.clear();
  handedDrafts.clear();
  queued = [];
  lastTurn = 0;
  generation = 0;
  received = 0;
  for (const listener of listeners) listener();
}

/**
 * Écoute « voice-mode », « voice-mode-level » et « voice-mode-utterance »
 * tant que le composant est monté : une seule fois, au niveau de l'app
 * (route racine). Lit l'état au montage (séance orpheline arrêtée) et
 * termine la séance quand sa page n'est plus affichée.
 */
export function useVoiceModeEvents() {
  useEffect(() => {
    let active = true;
    const stops: (() => void)[] = [];
    const keep = (stop: () => void) => {
      if (active) stops.push(stop);
      else stop();
    };
    listenVoiceMode(applyVoiceModeState).then(keep, (error: unknown) =>
      console.error("Failed to listen for voice mode:", error),
    );
    listenVoiceModeLevel(applyVoiceModeLevel).then(keep, (error: unknown) =>
      console.error("Failed to listen for voice mode levels:", error),
    );
    listenVoiceModeUtterance(applyVoiceModeUtterance).then(
      keep,
      (error: unknown) =>
        console.error("Failed to listen for voice mode utterances:", error),
    );
    const before = received;
    voiceModeState().then(
      (state) => {
        // Un événement arrivé pendant la lecture est plus récent qu'elle.
        if (active && received === before) applyVoiceModeState(state);
      },
      (error: unknown) =>
        console.error("Failed to read the voice mode state:", error),
    );
    return () => {
      active = false;
      for (const stop of stops) stop();
    };
  }, []);

  const chat = useRouterState({
    select: (state) => chatOfPath(state.location.pathname),
  });
  useEffect(() => {
    voiceModePageShown(chat);
  }, [chat]);
}

/** Tout le magasin (séance, propriétaire, lancement, échec). */
export function useVoiceModeSnapshot(): VoiceModeSnapshot {
  return useSyncExternalStore(subscribeVoiceMode, voiceModeSnapshot);
}

/** Mode vocal vu de la page de la conversation `chatId`. */
export interface VoiceModeView {
  /** Barre affichée : séance ou lancement de cette page. */
  shown: boolean;
  /** État du moteur (repos si la séance n'est pas celle de cette page). */
  state: VoiceModeState;
  launch: VoiceModeLaunch | null;
  /** Échec du dernier lancement de cette page. */
  error: string | null;
}

export function useVoiceModeFor(chatId: string): VoiceModeView {
  const current = useVoiceModeSnapshot();
  // Page de « new » encore affichée après l'adoption : sa barre reste
  // jusqu'au changement de page (pas de champ ni de bouton entre-temps).
  const mine =
    current.owner === chatId ||
    (current.adopted?.from === chatId && current.owner === current.adopted.to);
  return {
    shown: mine && (current.launch !== null || current.state.phase !== "off"),
    state: mine ? current.state : VOICE_MODE_OFF,
    launch: mine ? current.launch : null,
    error: current.error?.chatId === chatId ? current.error.message : null,
  };
}

/**
 * Remet les phrases de la séance à `onUtterance` tant que `enabled` (page de
 * la conversation `chatId`, séance à elle) ; la dernière fonction donnée est
 * appelée.
 */
export function useVoiceModeUtterances(
  chatId: string,
  enabled: boolean,
  onUtterance: (utterance: VoiceModeUtterance) => void,
) {
  const handle = useEffectEvent(onUtterance);
  useEffect(() => {
    if (!enabled) return;
    return registerVoiceModeUtterances(chatId, (utterance) =>
      handle(utterance),
    );
  }, [chatId, enabled]);
}

/**
 * Écrit le niveau `source` (micro ou voix) dans la variable CSS
 * `--voice-level` (0 à 1) de l'élément, à chaque événement, sans rendre le
 * composant ; 0 sans source ou si le système demande moins de mouvement.
 */
export function useVoiceModeLevel(
  ref: RefObject<HTMLElement | null>,
  source: "input" | "output" | null,
  reduced: boolean,
) {
  useEffect(() => {
    const element = ref.current;
    if (!element) return;
    const write = (current: VoiceModeLevel) => {
      const value = source && !reduced ? current[source] : 0;
      element.style.setProperty(
        "--voice-level",
        String(Math.min(1, Math.max(0, value))),
      );
    };
    write(level);
    return subscribeVoiceModeLevel(write);
  }, [ref, source, reduced]);
}

export const voiceModeSettingsKey = ["voiceMode", "settings"] as const;
export const voiceModeInfoKey = ["voiceMode", "info"] as const;

/** Mises à jour des réglages : en série, dans l'ordre où elles sont lancées. */
const settingsMutationKey = ["voiceMode", "update"] as const;

function applySaved(queryClient: QueryClient, settings: VoiceModeSettings) {
  // Une autre mise à jour attend encore : la réponse ne la contient pas ; la
  // dernière écrit.
  if (queryClient.isMutating({ mutationKey: settingsMutationKey }) <= 1) {
    queryClient.setQueryData(voiceModeSettingsKey, settings);
  }
}

/**
 * Réglages du mode vocal (table à part : « Reset to defaults » n'y touche
 * pas). `update` montre aussitôt le changement, puis la réponse du moteur ;
 * un échec relit les réglages et rejette. `enabled` : lus seulement quand on
 * en a besoin ; `refresh` : relus au montage (page Settings).
 */
export function useVoiceModeSettings({
  enabled = true,
  refresh = false,
}: { enabled?: boolean; refresh?: boolean } = {}) {
  const queryClient = useQueryClient();
  const query = useQuery({
    queryKey: voiceModeSettingsKey,
    queryFn: getVoiceModeSettings,
    staleTime: Infinity,
    enabled,
    refetchOnMount: refresh ? "always" : true,
  });
  const mutation = useMutation({
    mutationKey: settingsMutationKey,
    mutationFn: (patch: VoiceModeSettingsPatch) =>
      updateVoiceModeSettings(patch),
    scope: { id: "voice-mode-update" },
    onMutate: async (patch) => {
      await queryClient.cancelQueries({ queryKey: voiceModeSettingsKey });
      queryClient.setQueryData<VoiceModeSettings>(
        voiceModeSettingsKey,
        (old) => (old ? { ...old, ...patch } : old),
      );
    },
    onSuccess: (settings) => applySaved(queryClient, settings),
    onError: () =>
      queryClient.invalidateQueries({ queryKey: voiceModeSettingsKey }),
  });
  return {
    settings: query.data,
    error: query.error,
    reload: query.refetch,
    update: mutation.mutateAsync,
  };
}

/** Consigne vocale par défaut : fixe, lue une fois. */
export function useVoiceModeInfo({
  enabled = true,
}: { enabled?: boolean } = {}) {
  return useQuery({
    queryKey: voiceModeInfoKey,
    queryFn: voiceModeInfo,
    staleTime: Infinity,
    enabled,
  });
}
