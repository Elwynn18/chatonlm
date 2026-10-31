// Textes du panneau construits à partir d'une donnée (état de la liaison,
// reçu) : une fonction pure qui reçoit `t`, comme dans src/lib.
import type { TFunction } from "@/i18n";
import { formatBytes } from "@/i18n/format";
import type {
  AgentCall,
  AgentTool,
  CompletionView,
  Connection,
  GpuPlan,
  GpuSlot,
  ImageView,
  Receipt,
  ReceiptItem,
  StudyView,
  TourView,
  VoiceView,
} from "../shared/protocol";

/** Nom de l'état de la liaison avec l'app. */
export function connectionLabel(t: TFunction, connection: Connection): string {
  switch (connection) {
    case "connecting":
      return t("vscode.status.connecting");
    case "connected":
      return t("vscode.status.connected");
    case "notPaired":
      return t("vscode.status.notPaired");
    case "appClosed":
      return t("vscode.status.appClosed");
    case "bridgeOff":
      return t("vscode.status.bridgeOff");
    case "incompatible":
      return t("vscode.status.incompatible");
    case "error":
      return t("vscode.status.error");
  }
}

/** Où part l'envoi, en quelques mots (« stays on this computer »…). */
export function destinationLabel(t: TFunction, receipt: Receipt): string {
  switch (receipt.destination) {
    case "local":
      return t("vscode.receipt.local");
    case "cloud":
      return t("vscode.receipt.cloud");
    case "remote":
      return t("vscode.receipt.remote");
    case "unknown":
      return t("vscode.receipt.unknown");
  }
}

/** Où part l'envoi, en une phrase (reçu déplié). */
export function destinationDetail(t: TFunction, receipt: Receipt): string {
  switch (receipt.destination) {
    case "local":
      return t("vscode.receipt.localDetail");
    case "cloud":
      return t("vscode.receipt.cloudDetail");
    case "remote":
      return t("vscode.receipt.remoteDetail");
    case "unknown":
      return t("vscode.receipt.unknownDetail");
  }
}

/**
 * Ligne du reçu repliée : « 3 items · ~1200 tokens · stays on this
 * computer », « your message only · … » pour un message tapé, et les
 * messages précédents qui partent avec (« + 4 earlier messages »).
 */
export function receiptSummary(t: TFunction, receipt: Receipt): string {
  const history = receipt.history ?? null;
  const items =
    receipt.items.length > 0
      ? t("vscode.receipt.items", { count: receipt.items.length })
      : t(history ? "vscode.receipt.message" : "vscode.receipt.messageOnly");
  const tokens = t("vscode.receipt.tokens", { count: receipt.tokens });
  const destination = destinationLabel(t, receipt);
  return history
    ? t("vscode.receipt.summaryHistory", {
        items,
        history: t("vscode.receipt.history", { count: history.messages }),
        tokens,
        destination,
      })
    : t("vscode.receipt.summary", { items, tokens, destination });
}

/** Nature d'un élément du reçu. */
export function itemKind(t: TFunction, item: ReceiptItem): string {
  switch (item.kind) {
    case "selection":
      return t("vscode.receipt.kind.selection");
    case "file":
      return t("vscode.receipt.kind.file");
    case "diagnostic":
      return t("vscode.receipt.kind.diagnostic");
    case "diff":
      return t("vscode.receipt.kind.diff");
    case "terminal":
      return t("vscode.receipt.kind.terminal");
  }
}

/** Lignes d'un élément (« lines 3–12 », « line 7 »), ou null. */
export function itemLines(t: TFunction, item: ReceiptItem): string | null {
  if (!item.lines) return null;
  const [start, end] = item.lines;
  // Numéros de ligne écrits tels quels (pas de séparateur de milliers).
  return start === end
    ? t("vscode.receipt.line", { line: String(start) })
    : t("vscode.receipt.lines", { start: String(start), end: String(end) });
}

/** Part du contexte du modèle prise par l'envoi, en pour cent (au moins 1). */
export function contextPercent(receipt: Receipt): number | null {
  const size = receipt.contextLength;
  if (size === null || size <= 0) return null;
  const percent = Math.round((receipt.tokens / size) * 100);
  return receipt.tokens > 0 ? Math.max(1, percent) : 0;
}

/** « lines 3–12 » / « line 7 » pour une paire de lignes. */
export function linesLabel(t: TFunction, [start, end]: [number, number]) {
  return start === end
    ? t("vscode.receipt.line", { line: String(start) })
    : t("vscode.receipt.lines", { start: String(start), end: String(end) });
}

/** Titre de la carte d'une visite. */
export function tourTitle(t: TFunction, tour: TourView): string {
  switch (tour.kind) {
    case "file":
      return t("vscode.tour.titleFile", { path: tour.path });
    case "selection":
      return t("vscode.tour.titleSelection", { path: tour.path });
    case "diff":
      return t("vscode.tour.titleDiff", { path: tour.path });
  }
}

/** Où en est la visite (« Part 3 of 8 », « Paused »…). */
export function tourProgress(t: TFunction, tour: TourView): string {
  const step = t("vscode.tour.step", {
    current: Math.max(1, tour.current + 1),
    total: tour.segments.length,
  });
  switch (tour.status) {
    case "planning":
      return t("vscode.tour.planning");
    case "playing":
      return step;
    case "paused":
      return t("vscode.tour.pausedStep", { step });
    case "ended":
      return t("vscode.tour.ended");
  }
}

/** État de la séance vocale, en quelques mots. */
export function voicePhaseLabel(t: TFunction, voice: VoiceView): string {
  if (voice.muted) return t("vscode.voice.phase.muted");
  switch (voice.phase) {
    case "off":
    case "starting":
      return t("vscode.voice.phase.starting");
    case "listening":
    case "hearing":
      return t("vscode.voice.phase.listening");
    case "transcribing":
      return t("vscode.voice.phase.transcribing");
    case "responding":
      return t("vscode.voice.phase.thinking");
    case "speaking":
      return t("vscode.voice.phase.speaking");
    case "paused":
      return t("vscode.voice.phase.paused");
  }
}

/** Où part ce qui est dit pendant la séance (modèle de cette machine, d'une
 * autre, du cloud). */
export function voiceDestination(t: TFunction, voice: VoiceView): string {
  switch (voice.destination) {
    case "local":
      return t("vscode.voice.where.local");
    case "cloud":
      return t("vscode.voice.where.cloud");
    case "remote":
      return t("vscode.voice.where.remote");
    case "unknown":
      return t("vscode.voice.where.unknown");
  }
}

/** Ce que la séance partage avec le modèle (« src/app.ts, lines 3–40 »…). */
export function sharedItems(t: TFunction, voice: VoiceView): string[] {
  return voice.shared.map((item) =>
    item.kind === "terminal"
      ? t("vscode.voice.terminal", { command: item.label })
      : item.lines
        ? t("vscode.voice.passage", {
            path: item.path ?? item.label,
            lines: linesLabel(t, item.lines),
          })
        : item.label,
  );
}

/** État de l'autocomplétion en une phrase (carte « Graphics card »). */
export function completionStatus(t: TFunction, view: CompletionView): string {
  switch (view.status) {
    case "off":
      return t("vscode.gpu.status.off");
    case "disconnected":
      return t("vscode.gpu.status.disconnected");
    case "notHere":
      return t("vscode.gpu.status.notHere");
    case "remoteWindow":
      return t("vscode.gpu.status.remoteWindow");
    case "otherProvider":
      return t("vscode.gpu.status.otherProvider", {
        name: view.otherProvider ?? "",
      });
    case "notLocal":
      return t("vscode.gpu.status.notLocal");
    case "missing":
      return t("vscode.gpu.status.missing", { model: view.model ?? "" });
    case "checking":
      return t("vscode.gpu.status.checking");
    case "unusable":
      // Texte de l'app, déjà traduit (modèle sans FIM…).
      return view.error ?? t("vscode.gpu.status.disconnected");
    case "ready":
      return t("vscode.gpu.status.ready", { model: view.model ?? "" });
    case "slow":
      return t("vscode.gpu.status.slow");
    case "paused":
      switch (view.reason ?? "notLoaded") {
        case "exclusive":
          return t("vscode.gpu.paused.exclusive");
        case "chatBusy":
          return t("vscode.gpu.paused.chatBusy");
        case "doesNotFit":
          return t("vscode.gpu.paused.doesNotFit");
        case "notInstalled":
          return t("vscode.gpu.paused.notInstalled");
        case "notLocal":
          return t("vscode.gpu.paused.notLocal");
        case "notLoaded":
          return t("vscode.gpu.paused.notLoaded");
      }
  }
}

/** Mode du plan : ensemble, l'un après l'autre, ne tient pas, inconnu. */
export function gpuModeLabel(t: TFunction, mode: GpuPlan["mode"]): string {
  switch (mode) {
    case "together":
      return t("vscode.gpu.mode.together");
    case "exclusive":
      return t("vscode.gpu.mode.exclusive");
    case "doesNotFit":
      return t("vscode.gpu.mode.doesNotFit");
    case "unknown":
      return t("vscode.gpu.mode.unknown");
  }
}

/** Modèle tenu par le chef : nom, taille, en mémoire ou non. */
export function slotLabel(t: TFunction, slot: GpuSlot): string {
  const size = slot.loaded ? slot.sizeBytes : (slot.estimatedBytes ?? 0);
  return [
    slot.model,
    size > 0 ? formatBytes(t, size) : null,
    t(slot.loaded ? "vscode.gpu.loaded" : "vscode.gpu.notLoaded"),
    slot.partialGpu ? t("vscode.gpu.partial") : null,
  ]
    .filter((part): part is string => part !== null)
    .join(" · ");
}

/** Où en est le cours demandé depuis VS Code. */
export function studyProgress(t: TFunction, study: StudyView): string {
  switch (study.status) {
    case "creating":
      return t("vscode.study.creating");
    case "planning":
      return t("vscode.study.planning");
    case "ready":
      return study.levelsTotal > 0
        ? t("vscode.study.levels", {
            count: study.levelsTotal,
            ready: study.levelsReady,
          })
        : t("vscode.study.ready");
    case "failed":
      return study.error ?? t("vscode.study.failed");
  }
}

/** Ligne de ce qui a été retiré avant l'envoi (fichier sensible, secret). */
export function blockedLabel(
  t: TFunction,
  blocked: StudyView["blocked"][number],
): string {
  return blocked.reason === "secretFile"
    ? t("vscode.study.blockedFile", { label: blocked.label })
    : t("vscode.study.blockedValue", { label: blocked.label });
}

/** Où en est l'image demandée. */
/** Ce que le lecteur d'écran annonce de l'image : l'étape, jamais chaque
 * pas d'échantillonnage (20 à 30 annonces sinon). */
export function imageAnnouncement(t: TFunction, image: ImageView): string {
  if (image.status === "generating" && image.stage === "generating") {
    return t("vscode.image.making");
  }
  return imageProgress(t, image);
}

export function imageProgress(t: TFunction, image: ImageView): string {
  switch (image.status) {
    case "generating":
      if (image.stage === "generating" && image.step !== null && image.total) {
        return t("vscode.image.step", { step: image.step, total: image.total });
      }
      if (image.stage === "queued") return t("vscode.image.queued");
      if (image.stage === "loading") return t("vscode.image.loading");
      return t("vscode.image.starting");
    case "ready":
      return image.exists
        ? t("vscode.image.readyExists", { path: image.target })
        : t("vscode.image.ready", { path: image.target });
    case "saving":
      return t("vscode.image.saving");
    case "saved":
      return t("vscode.image.saved", { path: image.target });
    case "cancelled":
      return t("vscode.image.cancelled");
    case "failed":
      return image.error ?? t("vscode.image.failed");
  }
}

/** Nom d'un outil du serveur MCP, pour l'utilisateur. */
export function agentToolLabel(t: TFunction, tool: AgentTool): string {
  switch (tool) {
    case "chatonlm_ask":
      return t("vscode.agents.tools.ask");
    case "chatonlm_generate_image":
      return t("vscode.agents.tools.image");
    case "chatonlm_speak":
      return t("vscode.agents.tools.speak");
    case "chatonlm_proofread":
      return t("vscode.agents.tools.proofread");
    case "chatonlm_models":
      return t("vscode.agents.tools.models");
  }
}

/** « Copilot used ChatonLM: ask a local model ». */
export function agentCallTitle(t: TFunction, call: AgentCall): string {
  const tool = agentToolLabel(t, call.tool);
  return call.agent
    ? t("vscode.agents.used", { agent: call.agent, tool })
    : t("vscode.agents.usedUnknown", { tool });
}

/** « Copilot is using ChatonLM: ask a local model » (annonce). */
export function agentAnnounce(t: TFunction, call: AgentCall): string {
  const tool = agentToolLabel(t, call.tool);
  return call.agent
    ? t("vscode.agents.announceAgent", { agent: call.agent, tool })
    : t("vscode.agents.announceUnknown", { tool });
}

/** Où en est un appel. */
export function agentCallState(t: TFunction, call: AgentCall): string {
  switch (call.state) {
    case "running":
      return t("vscode.agents.running");
    case "done":
      return t("vscode.agents.done");
    case "failed":
      return t("vscode.agents.failed");
    case "stopped":
      return t("vscode.agents.stopped");
  }
}
