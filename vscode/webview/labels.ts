// Textes du panneau construits à partir d'une donnée (état de la liaison,
// reçu) : une fonction pure qui reçoit `t`, comme dans src/lib.
import type { TFunction } from "@/i18n";
import type { Connection, Receipt, ReceiptItem } from "../shared/protocol";

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
