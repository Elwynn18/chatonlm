/*
 * SPDX-License-Identifier: MIT
 * SPDX-FileCopyrightText: Ollama
 * SPDX-FileCopyrightText: 2026 Elwynn18 and ChatonLM contributors
 *
 * Adapted from Ollama (https://github.com/ollama/ollama),
 * file app/ui/app/src/components/ChatForm.tsx, commit 42e911bc3d05.
 * Modified for ChatonLM: no cloud models; web search button for models that
 * can use tools (cloud status read only for such a model; for a model known
 * not to use tools the button stays, greyed out, and says why), without the
 * "Web search requires an Ollama account" banner nor any account read: a
 * search without an account ends with that error in the chat, whose
 * "Generate without web search" (or "Retry" once signed in) asks the chat
 * form to send the last message again without (or with) web search (retry),
 * the focus going back to the message field; files are staged by the
 * backend (native picker opened by the backend, chips in ComposerFiles), and
 * sending waits for the files still being read; Enter respects file errors
 * like the send button; images are kept for a model whose capabilities are
 * unknown (the backend decides once they are known); the picker failure is
 * shown with the official text, aligned with the composer like the other
 * errors above it; one picker at a time for all chats (it is not modal on
 * Linux), its files going to the composer shown when it closes; files
 * dropped anywhere on the window (native drops, official overlay over the
 * whole window) or pasted (DropOverlay, src/lib/pastedFiles.ts); editing a
 * message is started by the chat view (startEdit), keeps its attachments by
 * id, and Escape cancels it only when nothing else has used the key; Think
 * button for any model that can think, from the values Ollama allows and
 * remembered per model (src/lib/think.ts), its value sent with the message
 * (false included); Heroicons instead of SF Symbols; native Tab order (the
 * official Tab cycle never leaves the message area, a keyboard trap);
 * labelled send button; Escape stops a reply only when nothing else (model
 * picker, Think menu, menu, rename field, dialog) has already used it; the
 * "A newer version of ... is available" banner (StaleModelBanner.tsx) is
 * shown here, above the form, where the selected model is known (the
 * official app shows it from Chat.tsx), unless the chat view has its own
 * banner to show (Ollama unreachable, error of a new chat); the
 * message field never takes the focus from another field, a menu or a
 * dialog, an open popover dialog included (nor, when a reply ends, from
 * any other control), also takes it on mount during a reply, and leaves
 * Space to the focused button; Stop stays clickable while Ollama is
 * unreachable; texts translated into English, Spanish and French
 * (src/i18n); Enter, Escape and Ctrl+V read from the shortcut catalog
 * (src/lib/shortcuts.ts), also shown in the settings;
 * ChatonLM's own voice mode: a "wave" button next to Send (and Ctrl+Shift+
 * Space, which also ends it) starts it for this chat, and its bar
 * (VoiceModeBar.tsx) takes the place of the message field while it lasts
 * (the draft, its files and the edit hint come back at the end; files, Send
 * and Stop are hidden — the bar interrupts the reply —, Think too unless
 * the "Let models think" setting is on; the model picker and web search
 * stay; the bar stays usable above the veil while Ollama is unreachable);
 * its transcribed sentences are sent through onSubmit like a message, with
 * thinking turned off when the model allows it (src/lib/think.ts) and
 * `voice`; the failure of its launch is shown above the composer, like the
 * picker failure; starting to edit a message ends it, and it can't start
 * during an edit; files dropped or pasted during it are ignored; the draft
 * of a new chat (text, staged files) goes to the composer of the chat
 * created by its first sentence, which gives it back at the end; the
 * composer carries a `data-tour` mark, shown by the first-launch tour;
 * ChatonLM's own context gauge (given by the chat view, ContextGauge.tsx) sits
 * next to the model picker, under the veil while Ollama is unreachable.
 */

import { ArrowUpIcon, StopIcon } from "@heroicons/react/20/solid";
import { PlusIcon } from "@heroicons/react/24/outline";
import {
  type KeyboardEvent,
  type ReactNode,
  type Ref,
  useCallback,
  useEffect,
  useEffectEvent,
  useImperativeHandle,
  useLayoutEffect,
  useRef,
  useState,
} from "react";
import type { AttachmentInfo } from "@/bindings/AttachmentInfo";
import type { Message } from "@/bindings/Message";
import type { StagedFile } from "@/bindings/StagedFile";
import type { Think } from "@/bindings/Think";
import type { VoiceLanguage } from "@/bindings/VoiceLanguage";
import type { VoiceModeUtterance } from "@/bindings/VoiceModeUtterance";
import { ComposerFiles } from "@/components/ComposerFiles";
import { DropOverlay } from "@/components/DropOverlay";
import { ErrorMessage } from "@/components/ErrorMessage";
import { Logo } from "@/components/Logo";
import { ModelPicker, type ModelPickerHandle } from "@/components/ModelPicker";
import { StaleModelBanner } from "@/components/StaleModelBanner";
import { ThinkButton } from "@/components/ThinkButton";
import { VoiceModeBar } from "@/components/VoiceModeBar";
import { VoiceModeButton } from "@/components/VoiceModeButton";
import { WebSearchButton } from "@/components/WebSearchButton";
import { useCloudStatus } from "@/hooks/useCloudStatus";
import {
  addPathsToActiveComposer,
  release,
  useComposerFiles,
} from "@/hooks/useComposerFiles";
import { useFileDrop } from "@/hooks/useFileDrop";
import { useHasVision, useToolsSupport } from "@/hooks/useModelCapabilities";
import { useSelectedModel } from "@/hooks/useModels";
import { useSettings } from "@/hooks/useSettings";
import { useThink } from "@/hooks/useThink";
import {
  claimVoiceModeDraft,
  clearVoiceModeError,
  peekVoiceModeDraft,
  registerVoiceModeDraft,
  startVoiceMode,
  stopVoiceMode,
  useVoiceModeFor,
  useVoiceModeSettings,
  useVoiceModeUtterances,
} from "@/hooks/useVoiceMode";
import { useT } from "@/i18n";
import { activatedBySpace, isEditable } from "@/lib/editable";
import { errorMessage, pickFiles, voiceModeReplyFailed } from "@/lib/ipc";
import { classifyPaste } from "@/lib/pastedFiles";
import { thinkToSend, voiceThinkToSend } from "@/lib/think";
import { overlayOpen } from "@/lib/overlay";
import {
  isEscapeKey,
  isPasteShortcut,
  isSendShortcut,
  isVoiceModeShortcut,
} from "@/lib/shortcuts";

/**
 * Sélecteur natif ouvert : sous Linux il n'est pas modal (rfd ignore la
 * fenêtre parente), un second clic sur « + », ici ou dans le composer d'une
 * autre conversation, en ouvrirait un autre.
 */
let pickerOpen = false;

/** Hauteur d'une ligne du champ de saisie ; il grandit jusqu'à 8 lignes. */
const LINE_HEIGHT = 24;
const MAX_LINES = 8;

/** Phrase du mode vocal envoyée par onSubmit : tour du moteur, langue détectée. */
export interface VoiceSubmit {
  turn: number;
  language: VoiceLanguage | null;
}

interface ChatFormProps {
  chatId: string;
  hasMessages: boolean;
  /** Une réponse est en cours : Entrée n'envoie pas, le bouton arrête. */
  isStreaming: boolean;
  /** Ollama ne répond plus : zone de saisie inactive. */
  isDisabled: boolean;
  /** Bandeau affiché au-dessus de la zone de saisie. */
  banner?: ReactNode;
  /** Jauge du contexte (conversation existante), près du sélecteur de modèle. */
  contextGauge?: ReactNode;
  /**
   * `think` : valeur du bouton Think, undefined s'il n'y en a pas ;
   * `attachments` : fichiers préparés à joindre, dans l'ordre ; `kept` :
   * pièces jointes gardées du message modifié ; `webSearch` : recherche web
   * demandée (modèle `tools`, bouton actif, nuage non coupé) ; `editOf` :
   * message réécrit sans passer par la zone de saisie (relance sans
   * recherche web), sinon celui que la vue modifie ; `voice` : phrase du
   * mode vocal (jamais une modification, sans fichier).
   */
  onSubmit: (
    content: string,
    model: string,
    think: Think | undefined,
    attachments: StagedFile[],
    kept: AttachmentInfo[],
    webSearch: boolean,
    editOf?: number,
    voice?: VoiceSubmit,
  ) => void;
  onCancel: () => void;
  /** Message en cours de modification (commencée par startEdit), ou null. */
  editingMessage?: Message | null;
  /** Modification annulée (Échap) : la vue n'estompe plus les messages. */
  onCancelEdit?: () => void;
  ref?: Ref<ChatFormHandle>;
}

export interface ChatFormHandle {
  /** Reprend le texte et les pièces jointes d'un message à modifier. */
  startEdit: (message: Message) => void;
  /**
   * Renvoie `message` (enregistré) à l'identique, en modification, avec le
   * modèle et le Think de la zone de saisie, sans toucher à ce qu'elle
   * contient ; le réglage de la recherche web prend la valeur `webSearch`
   * (demandée comme pour un envoi : modèle `tools`, nuage non coupé ;
   * absente, le réglage actuel est gardé). Le
   * focus revient à la zone de saisie. Sans effet pendant une réponse,
   * Ollama perdu ou sans modèle.
   */
  retry: (message: Message, options?: { webSearch?: boolean }) => void;
}

/**
 * La zone de saisie peut prendre le focus sans l'enlever à un champ (renommage
 * d'une conversation, recherche d'un modèle), à un menu ou à une boîte de
 * dialogue : la frappe en cours y partirait et Entrée l'enverrait au modèle
 * (l'officiel le prend toujours). À la fin d'une réponse, seulement si le
 * focus n'est nulle part ou déjà dans la zone : l'utilisateur fait autre
 * chose. Au montage, la navigation vient de lui : le lien de la barre latérale
 * qu'il vient d'activer peut céder le focus.
 */
function mayTakeFocus(composer: HTMLElement | null, replyEnded: boolean) {
  if (overlayOpen()) return false;
  if (replyEnded) {
    // Texte sélectionné hors de la zone (pour le copier) : prendre le focus
    // remplacerait cette sélection.
    const selection = window.getSelection();
    if (
      selection &&
      !selection.isCollapsed &&
      !(composer?.contains(selection.anchorNode) ?? false)
    )
      return false;
  }
  const active = document.activeElement;
  if (active === null || active === document.body) return true;
  // Panneau ouvert (celui de la jauge du contexte) : il garde le focus, et
  // reste ouvert (son échec s'y lit).
  if (active.closest("[role='dialog']")) return false;
  // Un champ garde le focus (la zone de saisie l'a peut-être déjà).
  if (isEditable(active)) return false;
  return !replyEnded || (composer?.contains(active) ?? false);
}

export function ChatForm({
  chatId,
  hasMessages,
  isStreaming,
  isDisabled,
  banner,
  contextGauge,
  onSubmit,
  onCancel,
  editingMessage = null,
  onCancelEdit,
  ref,
}: ChatFormProps) {
  const t = useT();
  // Brouillon cédé par la zone de saisie de la nouvelle conversation dont la
  // séance du mode vocal vient d'être adoptée par celle-ci (lu une fois).
  const [handed] = useState(() => peekVoiceModeDraft(chatId));
  const [content, setContent] = useState(handed?.content ?? "");
  // Pièces jointes du message modifié encore gardées.
  const [kept, setKept] = useState<AttachmentInfo[]>([]);
  const composingRef = useRef(false);
  const textareaRef = useRef<HTMLTextAreaElement>(null);
  const composerRef = useRef<HTMLDivElement>(null);
  const modelPickerRef = useRef<ModelPickerHandle>(null);
  const wasStreamingRef = useRef(isStreaming);
  const { selected } = useSelectedModel(chatId);
  const { control: thinkControl, setThink } = useThink(selected?.model ?? null);
  // Erreur du sélecteur : message du moteur, ou "" (texte officiel, traduit
  // à l'affichage) ; null : aucune.
  const [pickerError, setPickerError] = useState<string | null>(null);
  // Comme l'officiel, tout fichier ajouté (sélecteur, dépôt, collage) efface
  // l'erreur du sélecteur.
  const composerFiles = useComposerFiles({
    onStaged: () => setPickerError(null),
    initialFiles: handed?.files,
  });
  const { addPasted, addClipboardImage, discard } = composerFiles;
  // Mode vocal de cette conversation : barre à la place du champ.
  const voice = useVoiceModeFor(chatId);
  // Pas de dépôt pendant la séance : les fichiers sont masqués et ne
  // partiraient avec aucune phrase (ni voile, ni fichier préparé).
  const dropped = useFileDrop((paths) => {
    if (!voice.shown) composerFiles.addPaths(paths);
  });
  const dragging = dropped && !voice.shown;
  // Capacités connues sans « vision » : images signalées, retirées à l'envoi.
  const visionMissing = useHasVision(selected?.model ?? null) === false;
  // Recherche web : modèle qui sait appeler des outils, nuage du serveur non
  // coupé ; réglage persistant.
  const { settings, setSettings } = useSettings();
  const webSearchEnabled = settings?.webSearchEnabled ?? false;
  const toolsSupport = useToolsSupport(selected?.model ?? null);
  const supportsWebSearch = toolsSupport === true;
  const { cloudDisabled } = useCloudStatus({ enabled: supportsWebSearch });
  const webSearchUsable = supportsWebSearch && !cloudDisabled;
  const { settings: voiceSettings } = useVoiceModeSettings({
    enabled: voice.shown,
  });
  const keepThink = voiceSettings?.keepThink ?? false;
  const canStartVoice =
    !isDisabled && selected !== null && editingMessage === null;

  // Phrase transcrite : envoyée comme un message, avec le modèle et la
  // recherche web de la zone de saisie, la réflexion coupée (sauf réglage).
  useVoiceModeUtterances(
    chatId,
    voice.shown,
    (utterance: VoiceModeUtterance) => {
      if (!selected) {
        voiceModeReplyFailed(
          utterance.turn,
          t("voiceMode.errors.noModel"),
        ).catch((error: unknown) =>
          console.error("Failed to report the voice send:", error),
        );
        return;
      }
      onSubmit(
        utterance.text,
        selected.model,
        voiceThinkToSend(thinkControl, keepThink),
        [],
        [],
        webSearchUsable && webSearchEnabled,
        undefined,
        { turn: utterance.turn, language: utterance.language },
      );
    },
  );

  // Séance adoptée par la conversation créée par l'envoi vocal : le
  // brouillon (texte, fichiers préparés, retirés d'ici sans être libérés)
  // passe à sa zone de saisie, qui le rend à la fin de la séance.
  const takeDraft = useEffectEvent(() => ({
    content,
    files: composerFiles.take(),
  }));
  useEffect(() => {
    if (!voice.shown) return;
    return registerVoiceModeDraft(chatId, () => takeDraft());
  }, [chatId, voice.shown]);
  // Brouillon repris : retiré du magasin une fois la zone montée.
  useEffect(() => {
    if (handed) claimVoiceModeDraft(chatId);
  }, [handed, chatId]);

  const setWebSearch = useCallback(
    (enabled: boolean) => {
      setSettings({ webSearchEnabled: enabled }).catch((error: unknown) =>
        console.error("Failed to save the web search setting:", error),
      );
    },
    [setSettings],
  );

  // Nuage coupé sur le serveur : la recherche web est désactivée.
  useEffect(() => {
    if (cloudDisabled && webSearchEnabled) setWebSearch(false);
  }, [cloudDisabled, webSearchEnabled, setWebSearch]);

  const resize = () => {
    const textarea = textareaRef.current;
    if (!textarea) return;
    // Reset height to auto to get the correct scrollHeight, then cap at 8 lines
    textarea.style.height = "auto";
    textarea.style.height = `${Math.min(textarea.scrollHeight, LINE_HEIGHT * MAX_LINES)}px`;
  };

  // Une erreur de fichier bloque aussi Entrée (l'officiel ne bloque que le
  // bouton) ; un fichier en cours de lecture partirait sans être joint.
  const canSend =
    !isStreaming &&
    !isDisabled &&
    content.trim() !== "" &&
    selected !== null &&
    composerFiles.errors.length === 0 &&
    !composerFiles.busy;
  // Ollama perdu pendant une réponse : on doit pouvoir l'arrêter à la souris.
  const stopAboveVeil = isStreaming && isDisabled;

  const submit = () => {
    if (!canSend || !selected) return;
    // Prepare attachments for submission, excluding unsupported images
    const taken = composerFiles.take();
    const attachments = visionMissing
      ? taken.filter((file) => file.kind !== "image")
      : taken;
    release(
      taken.filter((file) => !attachments.includes(file)).map((f) => f.id),
    );
    const keptToSend = visionMissing
      ? kept.filter((attachment) => attachment.kind !== "image")
      : kept;
    onSubmit(
      content,
      selected.model,
      thinkToSend(thinkControl),
      attachments,
      keptToSend,
      webSearchUsable && webSearchEnabled,
    );
    clearVoiceModeError(chatId);
    setContent("");
    setKept([]);
    const textarea = textareaRef.current;
    if (textarea) {
      textarea.style.height = "auto";
      textarea.focus();
    }
  };

  // Comme l'officiel : le composer reprend le message (les fichiers
  // préparés avant sont libérés, la liste est remplacée).
  useImperativeHandle(
    ref,
    () => ({
      startEdit: (message: Message) => {
        // On veut taper : le mode vocal se termine, le champ revient.
        if (voice.shown) stopVoiceMode();
        setContent(message.content);
        setKept(message.attachments);
        discard();
      },
      retry: (
        message: Message,
        { webSearch = webSearchEnabled }: { webSearch?: boolean } = {},
      ) => {
        if (isStreaming || isDisabled || !selected) return;
        if (webSearch !== webSearchEnabled) setWebSearch(webSearch);
        // Comme un envoi : images retirées pour un modèle sans vision.
        const keptToSend = visionMissing
          ? message.attachments.filter((a) => a.kind !== "image")
          : message.attachments;
        onSubmit(
          message.content,
          selected.model,
          thinkToSend(thinkControl),
          [],
          keptToSend,
          webSearch && webSearchUsable,
          message.id,
        );
        // Le bouton cliqué part avec l'erreur : le focus irait à la page.
        textareaRef.current?.focus();
      },
    }),
    [
      discard,
      voice.shown,
      isStreaming,
      isDisabled,
      selected,
      webSearchEnabled,
      webSearchUsable,
      setWebSearch,
      visionMissing,
      onSubmit,
      thinkControl,
    ],
  );

  const cancelEdit = useCallback(() => {
    setContent("");
    setKept([]);
    discard();
    // Comme resetChatForm de l'officiel : la zone reprend sa hauteur.
    if (textareaRef.current) textareaRef.current.style.height = "auto";
    onCancelEdit?.();
    setTimeout(() => textareaRef.current?.focus(), 0);
  }, [discard, onCancelEdit]);

  // Focus and setup textarea when editing
  useLayoutEffect(() => {
    const textarea = textareaRef.current;
    if (editingMessage === null || !textarea) return;
    textarea.focus();
    textarea.style.transition = "height 0.2s ease-out, opacity 0.3s ease-in";
    textarea.style.height = "auto";
    textarea.style.height = `${Math.min(textarea.scrollHeight, LINE_HEIGHT * MAX_LINES)}px`;
  }, [editingMessage]);

  // Focus au montage et au changement de conversation, même pendant une
  // réponse (première réponse d'une nouvelle conversation, retour sur une
  // conversation qui répond)
  useEffect(() => {
    const timer = setTimeout(() => {
      if (mayTakeFocus(composerRef.current, false)) {
        textareaRef.current?.focus();
      }
    }, 0);
    return () => clearTimeout(timer);
  }, [chatId]);

  // Focus à la fin d'une réponse
  useEffect(() => {
    const replyEnded = wasStreamingRef.current && !isStreaming;
    wasStreamingRef.current = isStreaming;
    if (!replyEnded) return;
    const timer = setTimeout(() => {
      if (mayTakeFocus(composerRef.current, true)) {
        textareaRef.current?.focus();
      }
    }, 0);
    return () => clearTimeout(timer);
  }, [isStreaming]);

  // Fin du mode vocal : le champ revient avec le brouillon (sa hauteur) et
  // reprend le focus s'il est libre.
  const wasVoiceRef = useRef(voice.shown);
  useEffect(() => {
    const ended = wasVoiceRef.current && !voice.shown;
    wasVoiceRef.current = voice.shown;
    if (!ended) return;
    const textarea = textareaRef.current;
    if (textarea) {
      textarea.style.height = "auto";
      textarea.style.height = `${Math.min(textarea.scrollHeight, LINE_HEIGHT * MAX_LINES)}px`;
    }
    const timer = setTimeout(() => {
      if (mayTakeFocus(composerRef.current, false)) {
        textareaRef.current?.focus();
      }
    }, 0);
    return () => clearTimeout(timer);
  }, [voice.shown]);

  // Global keyboard and paste event handlers
  const voiceShown = voice.shown;
  useEffect(() => {
    const focusTextareaIfAppropriate = (target: EventTarget | null) => {
      const textarea = textareaRef.current;
      if (!textarea || textarea === document.activeElement) return;
      if (isEditable(target) || overlayOpen()) return;
      textarea.focus();
    };

    const handleKeyDown = (e: globalThis.KeyboardEvent) => {
      // Ctrl+Maj+Espace : démarre ou termine le mode vocal, depuis toute la
      // page (champ compris), sauf boîte ou menu ouvert.
      if (isVoiceModeShortcut(e)) {
        if (e.defaultPrevented || overlayOpen()) return;
        e.preventDefault();
        if (voiceShown) stopVoiceMode();
        else if (canStartVoice) void startVoiceMode(chatId);
        return;
      }

      if (isEscapeKey(e)) {
        // Already used by the model picker, a menu, a dialog or a field.
        if (e.defaultPrevented) return;
        // Mode vocal : la barre le prend (fin de la séance).
        if (voiceShown) return;
        // Handle escape key for canceling
        if (editingMessage) {
          e.preventDefault();
          cancelEdit();
        } else if (isStreaming) {
          e.preventDefault();
          onCancel();
        }
        return;
      }

      // Handle paste shortcuts
      if (isPasteShortcut(e)) {
        focusTextareaIfAppropriate(e.target);
        return;
      }

      // Handle auto-focus when typing printable characters
      if (e.key.length === 1 && !e.ctrlKey && !e.metaKey && !e.altKey) {
        // Espace active le bouton, le lien ou l'option qui a le focus
        if (e.key === " " && activatedBySpace(e.target)) return;
        focusTextareaIfAppropriate(e.target);
      }
    };

    const handlePaste = (e: ClipboardEvent) => {
      // Mode vocal : pas de champ ni de fichiers affichés ; un champ qui a
      // le focus (recherche d'un modèle) colle lui-même.
      if (voiceShown) return;
      focusTextareaIfAppropriate(e.target);
      if (!e.clipboardData) return;
      // Files only when the clipboard has no text, like the official app
      const paste = classifyPaste(e.clipboardData);
      if (paste.kind === "files") {
        e.preventDefault();
        addPasted(paste.files);
      } else if (paste.kind === "unreadable") {
        addClipboardImage();
      }
    };

    window.addEventListener("keydown", handleKeyDown);
    document.addEventListener("paste", handlePaste);
    return () => {
      window.removeEventListener("keydown", handleKeyDown);
      document.removeEventListener("paste", handlePaste);
    };
  }, [
    isStreaming,
    onCancel,
    addPasted,
    addClipboardImage,
    editingMessage,
    cancelEdit,
    voiceShown,
    canStartVoice,
    chatId,
  ]);

  const handleFilesUpload = async () => {
    if (pickerOpen) return;
    pickerOpen = true;
    setPickerError(null);
    try {
      // Au composer affiché quand le sélecteur se ferme (peut-être celui
      // d'une autre conversation).
      addPathsToActiveComposer(await pickFiles());
    } catch (error) {
      console.error("Error selecting multiple files:", error);
      setPickerError(errorMessage(error));
    } finally {
      pickerOpen = false;
    }
  };

  const handleKeyDown = (e: KeyboardEvent<HTMLTextAreaElement>) => {
    const composing = composingRef.current || e.nativeEvent.isComposing;
    // Enter sends, Shift+Enter adds a line; never during an IME composition
    if (isSendShortcut(e.nativeEvent) && !composing) {
      e.preventDefault();
      if (!isStreaming) submit();
      return;
    }
  };

  return (
    <div className={`px-3 pb-3 ${hasMessages ? "mt-auto" : "my-auto"}`}>
      {chatId === "new" && <Logo />}
      {/* Drop zone overlay */}
      {dragging && <DropOverlay />}
      {banner ? (
        <div className="pb-2">{banner}</div>
      ) : (
        <StaleModelBanner
          model={selected?.info ?? null}
          isStreaming={isStreaming}
        />
      )}
      {/* File upload error message */}
      {pickerError !== null && (
        <div className="mx-auto w-full max-w-[768px] px-5">
          <ErrorMessage
            message={pickerError || t("chat.composer.selectFilesFailed")}
          />
        </div>
      )}
      {/* Lancement du mode vocal en échec (prérequis, micro, téléchargement) */}
      {voice.error !== null && !voice.shown && (
        <div className="mx-auto w-full max-w-[768px] px-5">
          <ErrorMessage message={voice.error} />
        </div>
      )}
      <div
        ref={composerRef}
        data-tour="composer"
        className={`relative mx-auto flex min-h-[88px] w-full max-w-[768px] flex-col items-center rounded-3xl bg-white pt-4 pb-2 transition-opacity duration-200 after:pointer-events-none after:absolute after:inset-0 after:rounded-3xl after:ring-[1.5px] after:ring-neutral-300 after:ring-inset dark:bg-neutral-800 dark:after:ring-white/10 ${isDisabled ? "opacity-50" : "opacity-100"}`}
      >
        {/* Voile qui bloque la zone ; seul Stop reste au-dessus (stopAboveVeil) */}
        {isDisabled && (
          <div
            data-testid="composer-veil"
            className="absolute inset-0 z-50 rounded-3xl"
          />
        )}

        {editingMessage && !voice.shown && (
          <div className="w-full px-5 pb-2">
            <p className="text-xs text-neutral-500 dark:text-neutral-400">
              {t("chat.composer.editHint")}
            </p>
          </div>
        )}

        {!voice.shown && (
          <ComposerFiles
            kept={kept}
            onRemoveKept={(id) =>
              setKept((current) => current.filter((a) => a.id !== id))
            }
            files={composerFiles.files}
            reading={composerFiles.reading}
            errors={composerFiles.errors}
            visionMissing={visionMissing}
            onRemove={composerFiles.remove}
            onRemoveError={composerFiles.removeError}
          />
        )}

        {/* Barre du mode vocal au-dessus du voile (Ollama perdu) : la séance
            reste maîtrisable (micro, fin). */}
        <div
          className={`relative w-full px-5 ${voice.shown && isDisabled ? "z-60" : ""}`}
        >
          {voice.shown ? (
            <VoiceModeBar chatId={chatId} view={voice} />
          ) : (
            <textarea
              ref={textareaRef}
              value={content}
              onChange={(e) => {
                setContent(e.target.value);
                resize();
              }}
              placeholder={t("chat.composer.placeholder")}
              aria-label={t("chat.composer.label")}
              disabled={isDisabled}
              rows={1}
              onKeyDown={handleKeyDown}
              onCompositionStart={() => {
                composingRef.current = true;
              }}
              onCompositionEnd={() => {
                // Let the Enter that ends the composition pass first
                setTimeout(() => {
                  composingRef.current = false;
                }, 10);
              }}
              className="allow-context-menu min-h-[24px] w-full resize-none overflow-y-auto border-none bg-transparent leading-6 text-neutral-700 outline-none placeholder:text-neutral-400 dark:text-white dark:placeholder:text-neutral-500"
            />
          )}
        </div>

        {/* Controls */}
        <div className="flex w-full items-center justify-end gap-2 px-3 pt-2">
          {/* Tool buttons, hidden while Ollama is unreachable */}
          {!isDisabled && (
            <div className="flex flex-1 items-center justify-end gap-2">
              <div className="flex gap-2">
                {/* File Upload Buttons (pas de fichier en mode vocal) */}
                {!voice.shown && (
                  <button
                    type="button"
                    onClick={() => void handleFilesUpload()}
                    className="flex h-9 w-9 cursor-pointer items-center justify-center rounded-full border border-transparent bg-neutral-200 focus:outline-2 focus:outline-offset-2 focus:outline-accent-fg dark:bg-neutral-700"
                    title={t("chat.composer.attach")}
                    aria-label={t("chat.composer.attach")}
                  >
                    <PlusIcon className="h-4.5 w-4.5 stroke-2 text-neutral-500 dark:text-neutral-400" />
                  </button>
                )}
                {thinkControl.kind !== "none" &&
                  (!voice.shown || keepThink) && (
                    <ThinkButton
                      control={thinkControl}
                      onChange={setThink}
                      // Un seul menu ouvert à la fois. L'inverse est
                      // automatique : le sélecteur prend le focus pour sa
                      // recherche, et le menu Think se ferme quand il le perd.
                      onDropdownToggle={(isOpen) => {
                        if (isOpen) modelPickerRef.current?.close();
                      }}
                    />
                  )}
                {/* Modèle qui ne sait pas appeler d'outils : bouton gardé,
                    grisé, avec la raison (il ne disparaît plus sans rien
                    dire en changeant de modèle). */}
                <WebSearchButton
                  isVisible={
                    (supportsWebSearch && cloudDisabled === false) ||
                    toolsSupport === false
                  }
                  isUnavailable={toolsSupport === false}
                  isActive={webSearchEnabled}
                  onToggle={() => setWebSearch(!webSearchEnabled)}
                />
              </div>
            </div>
          )}

          <div className="relative z-20 flex items-center gap-2">
            <ModelPicker
              ref={modelPickerRef}
              chatId={chatId}
              isDisabled={isDisabled}
              onModelSelect={() => textareaRef.current?.focus()}
              onEscape={() => textareaRef.current?.focus()}
            />
          </div>
          {contextGauge && (
            <div className="relative z-20 flex items-center">
              {contextGauge}
            </div>
          )}
          {!isDisabled && !voice.shown && (
            <VoiceModeButton chatId={chatId} disabled={!canStartVoice} />
          )}
          {/* Hors du groupe z-20 : sinon il ne peut pas passer au-dessus du
              voile, qui partage le contexte d'empilement de la zone. Pas en
              mode vocal : sa barre interrompt la réponse. */}
          {!voice.shown && (
            <button
              type="button"
              onClick={isStreaming ? onCancel : submit}
              disabled={!isStreaming && !canSend}
              aria-label={t(
                isStreaming ? "chat.composer.stop" : "chat.composer.send",
              )}
              className={`flex h-9 w-9 cursor-pointer items-center justify-center rounded-full bg-black text-white focus:outline-2 focus:outline-offset-2 focus:outline-accent-fg disabled:cursor-default disabled:opacity-10 dark:bg-white dark:text-black ${stopAboveVeil ? "relative z-60" : ""}`}
            >
              {isStreaming ? (
                <StopIcon className="h-3.5 w-3.5" />
              ) : (
                <ArrowUpIcon className="h-4 w-4" />
              )}
            </button>
          )}
        </div>
      </div>
    </div>
  );
}
