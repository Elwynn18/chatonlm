// Carte « Report a bug » de Settings › General (propre à ChatonLM), juste
// au-dessus de celle des dons : l'utilisateur décrit le problème, laisse s'il
// le veut une adresse pour qu'on lui réponde, et joint le diagnostic (coché
// par défaut, visible avant l'envoi). Le moteur envoie le tout par le service
// de chatonlm.com, qui le relaie par courriel, comme le formulaire de contact
// du site. Rien ne part sans un clic sur « Send ». « Open logs folder » ouvre
// le dossier des journaux. « Reset to defaults » n'y touche pas (aucun
// réglage).

import { ArrowPathIcon, BugAntIcon } from "@heroicons/react/20/solid";
import { type FormEvent, useId, useState } from "react";
import type { ReportOutcome } from "@/bindings/ReportOutcome";
import { Row } from "@/components/settings/Row";
import { type TKey, useT } from "@/i18n";
import {
  bugReportDiagnostic,
  errorMessage,
  openAppLogs,
  sendBugReport,
} from "@/lib/ipc";

/** Bornes du service (`MESSAGE_MIN` et `MESSAGE_MAX` du moteur). */
export const MESSAGE_MIN = 10;
export const MESSAGE_MAX = 5000;

/** Adresse donnée quand le service ne répond pas (celle du site). */
export const CONTACT_EMAIL = "contact.chatonlm@gmail.com";

const FIELD =
  "w-full min-w-0 rounded-lg border border-neutral-300 bg-white px-3 text-sm text-neutral-900 outline-none focus-visible:border-accent-fg focus-visible:ring-2 focus-visible:ring-accent-fg/40 dark:border-neutral-700 dark:bg-neutral-800 dark:text-neutral-100";
const LABEL = "text-sm font-medium text-neutral-900 dark:text-neutral-100";
const HINT = "text-xs text-neutral-500 dark:text-neutral-400";
const SMALL_BUTTON =
  "inline-flex flex-none cursor-pointer items-center gap-1.5 rounded-lg border border-zinc-950/10 bg-white px-3 py-1 text-sm/6 font-medium text-zinc-950 shadow-sm outline-none hover:bg-zinc-50 focus-visible:ring-2 focus-visible:ring-accent-fg disabled:cursor-default disabled:opacity-50 dark:border-white/15 dark:bg-zinc-900 dark:text-white dark:hover:bg-zinc-800";
// aria-disabled pendant l'envoi : le bouton garde le focus.
const SEND_BUTTON =
  "inline-flex flex-none cursor-pointer items-center gap-1.5 rounded-lg border border-zinc-700/90 bg-zinc-800 px-3 py-1 text-sm/6 font-medium text-white shadow-sm outline-none hover:bg-zinc-700 focus-visible:ring-2 focus-visible:ring-accent-fg disabled:cursor-default disabled:opacity-50 aria-disabled:cursor-default aria-disabled:opacity-50 dark:border-white/10 dark:bg-white dark:text-zinc-900 dark:hover:bg-zinc-200";
const ALERT = "text-xs break-words text-red-600 dark:text-red-400";

const FAILURES: Record<Exclude<ReportOutcome, "sent">, TKey> = {
  tooMany: "settings.bugReport.error.tooMany",
  invalidEmail: "settings.bugReport.error.invalidEmail",
  invalidMessage: "settings.bugReport.error.invalidMessage",
  unavailable: "settings.bugReport.error.unavailable",
};

type Diagnostic =
  | { state: "hidden" }
  | { state: "loading" }
  | { state: "shown"; text: string }
  | { state: "failed"; message: string };

type Sending =
  | { state: "idle" }
  | { state: "sending" }
  | { state: "sent" }
  | { state: "failed"; outcome: Exclude<ReportOutcome, "sent"> }
  | { state: "error"; message: string };

export function BugReport() {
  const t = useT();
  const titleId = useId();
  const descriptionId = useId();
  const messageId = useId();
  const messageHintId = useId();
  const emailId = useId();
  const emailHintId = useId();
  const diagnosticId = useId();
  const diagnosticHintId = useId();
  const statusId = useId();

  const [message, setMessage] = useState("");
  const [email, setEmail] = useState("");
  const [includeDiagnostic, setIncludeDiagnostic] = useState(true);
  const [diagnostic, setDiagnostic] = useState<Diagnostic>({
    state: "hidden",
  });
  const [sending, setSending] = useState<Sending>({ state: "idle" });
  const [logsError, setLogsError] = useState<string | null>(null);

  const length = message.trim().length;
  const tooShort = length < MESSAGE_MIN;
  const busy = sending.state === "sending";

  const toggleDiagnostic = async () => {
    if (diagnostic.state === "shown" || diagnostic.state === "failed") {
      setDiagnostic({ state: "hidden" });
      return;
    }
    if (diagnostic.state === "loading") return;
    setDiagnostic({ state: "loading" });
    try {
      setDiagnostic({ state: "shown", text: await bugReportDiagnostic() });
    } catch (error) {
      setDiagnostic({ state: "failed", message: errorMessage(error) });
    }
  };

  const submit = async (event: FormEvent) => {
    event.preventDefault();
    if (busy || tooShort) return;
    setSending({ state: "sending" });
    try {
      const outcome = await sendBugReport({
        message,
        email: email.trim(),
        includeDiagnostic,
      });
      if (outcome === "sent") {
        setSending({ state: "sent" });
        setMessage("");
      } else {
        setSending({ state: "failed", outcome });
      }
    } catch (error) {
      setSending({ state: "error", message: errorMessage(error) });
    }
  };

  const openLogs = async () => {
    setLogsError(null);
    try {
      await openAppLogs();
    } catch (error) {
      setLogsError(errorMessage(error));
    }
  };

  const emailInvalid =
    sending.state === "failed" && sending.outcome === "invalidEmail";

  return (
    <section
      aria-labelledby={titleId}
      data-testid="bug-report"
      className="overflow-hidden rounded-xl bg-white dark:bg-neutral-800"
    >
      <form onSubmit={(event) => void submit(event)} className="space-y-4 p-4">
        <Row
          icon={BugAntIcon}
          label={
            <h2 id={titleId} className="font-medium">
              {t("settings.bugReport.title")}
            </h2>
          }
          description={t("settings.bugReport.description")}
          descriptionId={descriptionId}
        />

        <div className="space-y-1.5">
          <label htmlFor={messageId} className={LABEL}>
            {t("settings.bugReport.message.label")}
          </label>
          <textarea
            id={messageId}
            rows={4}
            maxLength={MESSAGE_MAX}
            value={message}
            onChange={(event) => {
              setMessage(event.target.value);
              if (sending.state !== "sending") setSending({ state: "idle" });
            }}
            placeholder={t("settings.bugReport.message.placeholder")}
            aria-describedby={messageHintId}
            className={`${FIELD} resize-y py-2`}
          />
          <p id={messageHintId} className={HINT}>
            {t("settings.bugReport.message.hint", { min: MESSAGE_MIN })}
          </p>
        </div>

        <div className="space-y-1.5">
          <label htmlFor={emailId} className={LABEL}>
            {t("settings.bugReport.email.label")}
          </label>
          <input
            id={emailId}
            type="email"
            autoComplete="email"
            spellCheck={false}
            maxLength={254}
            value={email}
            onChange={(event) => {
              setEmail(event.target.value);
              if (emailInvalid) setSending({ state: "idle" });
            }}
            aria-invalid={emailInvalid || undefined}
            aria-describedby={emailHintId}
            className={`${FIELD} h-9`}
          />
          <p id={emailHintId} className={HINT}>
            {t("settings.bugReport.email.hint")}
          </p>
        </div>

        <div className="space-y-2">
          <div className="flex flex-wrap items-center justify-between gap-2">
            <label className="flex cursor-pointer items-center gap-2 text-sm text-neutral-900 dark:text-neutral-100">
              <input
                type="checkbox"
                checked={includeDiagnostic}
                onChange={(event) => setIncludeDiagnostic(event.target.checked)}
                aria-describedby={diagnosticHintId}
                className="h-4 w-4 cursor-pointer accent-accent-fg"
              />
              {t("settings.bugReport.diagnostic.label")}
            </label>
            <button
              type="button"
              aria-expanded={diagnostic.state === "shown"}
              aria-controls={diagnosticId}
              onClick={() => void toggleDiagnostic()}
              className="cursor-pointer rounded text-sm font-medium text-accent-fg underline-offset-2 hover:underline"
            >
              {diagnostic.state === "shown" || diagnostic.state === "failed"
                ? t("settings.bugReport.diagnostic.hide")
                : t("settings.bugReport.diagnostic.show")}
            </button>
          </div>
          <p id={diagnosticHintId} className={HINT}>
            {t("settings.bugReport.diagnostic.hint")}
          </p>
          <div id={diagnosticId}>
            {diagnostic.state === "loading" && (
              <p className={HINT}>
                {t("settings.bugReport.diagnostic.loading")}
              </p>
            )}
            {diagnostic.state === "failed" && (
              <p role="alert" className={ALERT}>
                {t("settings.bugReport.diagnostic.error", {
                  error: diagnostic.message,
                })}
              </p>
            )}
            {diagnostic.state === "shown" && (
              <pre
                tabIndex={0}
                aria-label={t("settings.bugReport.diagnostic.label")}
                className="max-h-64 overflow-auto rounded-lg bg-neutral-100 p-3 font-mono text-xs/5 break-words whitespace-pre-wrap text-neutral-900 select-text dark:bg-neutral-900 dark:text-neutral-100"
              >
                {diagnostic.text}
              </pre>
            )}
          </div>
        </div>

        <div className="flex flex-wrap items-center justify-between gap-3">
          <button
            type="button"
            onClick={() => void openLogs()}
            className={SMALL_BUTTON}
          >
            {t("settings.bugReport.openLogs")}
          </button>
          <button
            type="submit"
            disabled={tooShort}
            aria-disabled={busy || undefined}
            aria-describedby={statusId}
            className={SEND_BUTTON}
          >
            {busy && (
              <ArrowPathIcon
                aria-hidden="true"
                className="h-4 w-4 animate-spin"
              />
            )}
            {busy
              ? t("settings.bugReport.sending")
              : t("settings.bugReport.send")}
          </button>
        </div>

        {/* Région toujours présente : l'annonce de la suite est fiable. */}
        <div id={statusId} aria-live="polite" className="empty:hidden">
          {sending.state === "sent" && (
            <p className="text-sm text-green-800 dark:text-green-100">
              {t("settings.bugReport.sent")}
            </p>
          )}
          {sending.state === "failed" && (
            <p className={ALERT}>
              {t(FAILURES[sending.outcome], {
                min: MESSAGE_MIN,
                max: MESSAGE_MAX,
                email: CONTACT_EMAIL,
              })}
            </p>
          )}
          {sending.state === "error" && (
            <p className={ALERT}>
              {t("settings.bugReport.error.unknown", {
                error: sending.message,
                email: CONTACT_EMAIL,
              })}
            </p>
          )}
          {logsError !== null && (
            <p className={ALERT}>
              {t("settings.bugReport.logsError", { error: logsError })}
            </p>
          )}
        </div>
      </form>
    </section>
  );
}
