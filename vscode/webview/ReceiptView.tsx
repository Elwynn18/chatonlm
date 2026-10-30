// Reçu de ce qui part au modèle avec un envoi, sous le message envoyé :
// replié en une ligne (éléments, jetons estimés, destination), déplié en
// détail (chaque élément, part du contexte, éléments retirés et pourquoi).
import { ChevronDownIcon } from "@heroicons/react/20/solid";
import {
  CloudIcon,
  ComputerDesktopIcon,
  GlobeAltIcon,
  QuestionMarkCircleIcon,
} from "@heroicons/react/24/outline";
import { useId, useState } from "react";
import { useT } from "@/i18n";
import type { Receipt } from "../shared/protocol";
import {
  contextPercent,
  destinationDetail,
  itemKind,
  itemLines,
  receiptSummary,
} from "./labels";

const ICONS = {
  local: ComputerDesktopIcon,
  cloud: CloudIcon,
  remote: GlobeAltIcon,
  unknown: QuestionMarkCircleIcon,
} as const;

export function ReceiptView({ receipt }: { receipt: Receipt }) {
  const t = useT();
  const [open, setOpen] = useState(false);
  const bodyId = useId();
  // Hors de cette machine (ou sans le savoir) : marqué en roux.
  const away = receipt.destination !== "local";
  const percent = contextPercent(receipt);
  const Icon = ICONS[receipt.destination];
  const history = receipt.history ?? null;

  return (
    <div
      className="mt-1 max-w-full self-end text-xs text-neutral-500 dark:text-neutral-400"
      data-testid="receipt"
      data-destination={receipt.destination}
    >
      <button
        type="button"
        className="ml-auto flex cursor-pointer items-center gap-1 rounded-md px-1 py-0.5 text-right hover:text-neutral-800 dark:hover:text-neutral-200"
        aria-expanded={open}
        aria-controls={bodyId}
        onClick={() => setOpen((value) => !value)}
      >
        <Icon
          className={`h-3.5 w-3.5 shrink-0 ${away ? "text-accent-fg" : ""}`}
          aria-hidden="true"
        />
        <span>{receiptSummary(t, receipt)}</span>
        <ChevronDownIcon
          className={`h-3.5 w-3.5 shrink-0 transition-transform ${open ? "" : "-rotate-90"}`}
          aria-hidden="true"
        />
      </button>
      <div
        id={bodyId}
        hidden={!open}
        className="mt-1 space-y-1 rounded-xl bg-neutral-100 px-3 py-2 text-neutral-600 dark:bg-neutral-800 dark:text-neutral-300"
      >
        <p>{destinationDetail(t, receipt)}</p>
        {history && (
          <p data-testid="receipt-history">
            {t("vscode.receipt.historyDetail", {
              count: history.messages,
              chars: t("vscode.receipt.chars", { count: history.chars }),
            })}
          </p>
        )}
        {percent !== null && receipt.contextLength !== null && (
          <p>
            {t("vscode.receipt.context", {
              percent,
              size: receipt.contextLength,
            })}
          </p>
        )}
        {receipt.items.length > 0 && (
          <ul className="space-y-0.5">
            {receipt.items.map((item, index) => {
              const lines = itemLines(t, item);
              return (
                <li key={index} className="break-words">
                  <span className="font-medium text-neutral-800 dark:text-neutral-200">
                    {itemKind(t, item)}
                  </span>{" "}
                  {item.label}
                  {item.path && item.path !== item.label && (
                    <>
                      {" · "}
                      <span className="font-mono">{item.path}</span>
                    </>
                  )}
                  {lines && <> · {lines}</>}
                  {" · "}
                  {t("vscode.receipt.chars", { count: item.chars })}
                </li>
              );
            })}
          </ul>
        )}
        {receipt.blocked.length > 0 && (
          <div data-testid="receipt-blocked">
            <p className="font-medium text-neutral-800 dark:text-neutral-200">
              {t("vscode.receipt.blocked")}
            </p>
            <ul className="space-y-0.5">
              {receipt.blocked.map((entry, index) => (
                <li key={index} className="break-words">
                  <span className="font-mono">{entry.label}</span>
                  {" · "}
                  {t(
                    entry.reason === "secretFile"
                      ? "vscode.receipt.secretFile"
                      : "vscode.receipt.secretValue",
                  )}
                </li>
              ))}
            </ul>
          </div>
        )}
        {away && <p>{t("vscode.receipt.bestEffort")}</p>}
      </div>
    </div>
  );
}
