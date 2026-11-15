// Outil Transcription (propre à ChatonLM) : nouvelle transcription. Adresse
// d'une vidéo (tout site lu par yt-dlp, sans en mettre aucun en avant) OU
// fichier audio ou vidéo de l'ordinateur (sélecteur natif, ou dépôt sur la
// fenêtre : seuls ces chemins sont transcriptibles, le moteur les tient) ;
// modèle installé et langue, avec les conseils du moteur (Whisper lent sur
// un long son, Parakeet qui ne garde qu'une langue par passage) ; aucun
// bouton pour enregistrer le son ou la vidéo (rapport 105, § 6).
//
// Adresse sans yt-dlp ni Deno : boîte d'accord, installation suivie par
// « tools-status », puis la transcription part d'elle-même une fois les
// outils prêts, avec le modèle et la langue choisis à ce moment ; changer
// d'adresse ou choisir un fichier pendant l'attente l'abandonne. Quitter la
// page pendant l'attente l'abandonne aussi : au retour, l'adresse et les
// choix reviennent dans le formulaire, avec un mot pour relancer. Au départ,
// la page de la transcription s'ouvre.

import {
  ArrowRightIcon,
  DocumentArrowUpIcon,
  ExclamationTriangleIcon,
  FilmIcon,
  XMarkIcon,
} from "@heroicons/react/20/solid";
import { type QueryClient, useQueryClient } from "@tanstack/react-query";
import { Link, useNavigate } from "@tanstack/react-router";
import {
  type FormEvent,
  useEffect,
  useId,
  useMemo,
  useRef,
  useState,
} from "react";
import { createPortal } from "react-dom";
import type { TranscriptionError } from "@/bindings/TranscriptionError";
import type { TranscriptionRequest } from "@/bindings/TranscriptionRequest";
import { PRIMARY_BUTTON, QUIET_BUTTON } from "@/components/study/ui";
import { ErrorNotice } from "@/components/transcription/JobProgress";
import {
  clock,
  type EngineAdvice,
  engineAdvice,
  isRunning,
  isWebAddress,
  toolsBusy,
} from "@/components/transcription/model";
import {
  toolsStatusKey,
  transcriptionListKey,
  transcriptionStatusKey,
  useToolsStatus,
  useTranscriptionOptions,
  useTranscriptionStatus,
} from "@/components/transcription/queries";
import {
  ToolsConsent,
  ToolsPanel,
} from "@/components/transcription/ToolsPanel";
import { useDictationInfo } from "@/hooks/useDictation";
import { useFileDrop } from "@/hooks/useFileDrop";
import { type TFunction, useT } from "@/i18n";
import { languageName } from "@/i18n/format";
import { AUTO_LANGUAGE } from "@/lib/dictation";
import {
  errorMessage,
  listenToolsStatus,
  toolsInstall,
  transcriptionPickFile,
  transcriptionProbe,
  transcriptionStart,
} from "@/lib/ipc";

const SELECT =
  "h-8 max-w-64 min-w-0 rounded-lg border border-neutral-300 bg-white pr-7 pl-2 text-sm text-neutral-900 outline-none focus-visible:border-accent-fg focus-visible:ring-2 focus-visible:ring-accent-fg/40 disabled:opacity-50 dark:border-neutral-700 dark:bg-neutral-800 dark:text-neutral-100";
const INPUT =
  "w-full rounded-xl border border-neutral-200 bg-neutral-50 px-3 py-2 text-base text-neutral-900 outline-none placeholder:text-neutral-400 focus-visible:border-accent-fg focus-visible:ring-2 focus-visible:ring-accent-fg/40 dark:border-neutral-700 dark:bg-neutral-900 dark:text-neutral-100 dark:placeholder:text-neutral-500";

/**
 * Demande qui attendait les outils quand le formulaire a été quitté, par
 * client de requêtes (un par lancement de l'app, un par essai).
 */
const abandoned = new WeakMap<QueryClient, TranscriptionRequest>();

/** Fichier choisi ou déposé, lu par le moteur. */
interface ChosenFile {
  path: string;
  name: string;
  durationS: number | null;
  error: TranscriptionError | null;
}

export function NewTranscriptionForm() {
  const t = useT();
  const ids = { address: useId(), model: useId(), language: useId() };
  const navigate = useNavigate();
  const client = useQueryClient();
  const options = useTranscriptionOptions();
  const info = useDictationInfo();
  const status = useTranscriptionStatus();
  const tools = useToolsStatus();
  // Demande abandonnée en quittant la page : remise dans le formulaire.
  const [restored] = useState(() => abandoned.get(client) ?? null);
  const [lost, setLost] = useState(restored !== null);
  const [address, setAddress] = useState(() =>
    restored?.source.kind === "url" ? restored.source.url : "",
  );
  const [file, setFile] = useState<ChosenFile | null>(null);
  const [reading, setReading] = useState(false);
  const [model, setModel] = useState<string | null>(
    () => restored?.model ?? null,
  );
  const [language, setLanguage] = useState<string | null>(
    () => restored?.language ?? null,
  );
  const [starting, setStarting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [consent, setConsent] = useState(false);
  /** Demande en attente des outils (montrée), et la même pour les rappels. */
  const [pending, setPending] = useState<TranscriptionRequest | null>(null);
  const pendingRef = useRef<TranscriptionRequest | null>(null);
  /** Dernier dépôt ou choix : une lecture plus ancienne est ignorée. */
  const readId = useRef(0);

  const installed = (options.data?.models ?? []).filter((m) => m.installed);
  const chosenModel =
    installed.find((m) => m.id === model) ??
    installed.find((m) => m.id === options.data?.defaultModel) ??
    installed[0] ??
    null;
  const chosenLanguage = language ?? options.data?.language ?? AUTO_LANGUAGE;
  const languages = useMemo(() => {
    const collator = new Intl.Collator(t.lang);
    return (info.data?.languages ?? [])
      .map((lang) => ({ ...lang, label: languageName(t.lang, lang.code) }))
      .sort((a, b) => collator.compare(a.label, b.label));
  }, [info.data, t]);

  const url = address.trim();
  const source: "url" | "file" | null = file ? "file" : url ? "url" : null;
  const running = isRunning(status.data?.job);
  const noModel = options.data !== undefined && chosenModel === null;
  const waiting = pending !== null;
  const disabled =
    source === null ||
    running ||
    noModel ||
    starting ||
    waiting ||
    reading ||
    (file !== null && file.error !== null);
  const advice = engineAdvice({
    engine: chosenModel?.engine ?? null,
    source,
    durationS: file?.durationS ?? null,
    language: chosenLanguage,
    parakeetLanguages: (info.data?.languages ?? [])
      .filter((lang) => lang.parakeet)
      .map((lang) => lang.code),
  });

  const request = (): TranscriptionRequest | null => {
    const base = {
      ...(chosenModel ? { model: chosenModel.id } : {}),
      language: chosenLanguage,
    };
    if (file) return { source: { kind: "file", path: file.path }, ...base };
    if (url) return { source: { kind: "url", url }, ...base };
    return null;
  };

  const start = (request: TranscriptionRequest) => {
    setStarting(true);
    setError(null);
    transcriptionStart(request).then(
      (id) => {
        void client.invalidateQueries({ queryKey: transcriptionStatusKey });
        void client.invalidateQueries({ queryKey: transcriptionListKey });
        void navigate({
          to: "/transcription/$transcriptionId",
          params: { transcriptionId: id },
        });
      },
      (cause: unknown) => {
        setStarting(false);
        setError(
          t("transcription.form.startError", { message: errorMessage(cause) }),
        );
      },
    );
  };
  // Dernières fonctions pour le rappel des outils (inscrit une fois).
  const startRef = useRef(start);
  const requestRef = useRef(request);
  useEffect(() => {
    startRef.current = start;
    requestRef.current = request;
  });

  // Demande abandonnée reprise ; demande en attente gardée si l'on part.
  useEffect(() => {
    abandoned.delete(client);
    return () => {
      if (pendingRef.current) abandoned.set(client, pendingRef.current);
    };
  }, [client]);

  /** Changement de source : la demande en attente des outils est abandonnée. */
  const dropPending = () => {
    pendingRef.current = null;
    setPending(null);
    setLost(false);
  };

  // Outils prêts pendant qu'une demande les attend : elle part, avec le
  // modèle et la langue choisis à ce moment ; outils en échec : elle est
  // abandonnée (l'erreur est montrée sous le champ).
  useEffect(() => {
    let active = true;
    let stop: (() => void) | null = null;
    listenToolsStatus((next) => {
      const held = pendingRef.current;
      if (!held) return;
      if (next.ready) {
        pendingRef.current = null;
        setPending(null);
        const latest = requestRef.current();
        startRef.current(latest?.source.kind === "url" ? latest : held);
      } else if (!toolsBusy(next)) {
        pendingRef.current = null;
        setPending(null);
      }
    }).then(
      (unlisten) => {
        if (active) stop = unlisten;
        else unlisten();
      },
      (cause: unknown) =>
        console.error("Failed to listen to the tools:", cause),
    );
    return () => {
      active = false;
      stop?.();
    };
  }, []);

  const readFile = (path: string) => {
    const id = ++readId.current;
    dropPending();
    setReading(true);
    setError(null);
    setAddress("");
    transcriptionProbe(path).then(
      (probe) => {
        if (id !== readId.current) return;
        setReading(false);
        setFile({ path, ...probe });
      },
      (cause: unknown) => {
        if (id !== readId.current) return;
        setReading(false);
        setError(
          t("transcription.form.pickError", { message: errorMessage(cause) }),
        );
      },
    );
  };

  const dragging = useFileDrop((paths) => readFile(paths[0]));

  const pick = () => {
    transcriptionPickFile().then(
      (path) => {
        if (path) readFile(path);
      },
      (cause: unknown) =>
        setError(
          t("transcription.form.pickError", { message: errorMessage(cause) }),
        ),
    );
  };

  const submit = (event: FormEvent) => {
    event.preventDefault();
    const next = request();
    if (disabled || !next) return;
    setLost(false);
    if (next.source.kind === "file") {
      start(next);
      return;
    }
    if (!isWebAddress(url)) {
      setError(t("transcription.form.invalidAddress"));
      return;
    }
    const state = tools.data;
    if (state?.ready) {
      start(next);
    } else if (state && !state.supported) {
      setError(t("transcription.tools.unsupported"));
    } else if (toolsBusy(state)) {
      pendingRef.current = next;
      setPending(next);
    } else {
      pendingRef.current = next;
      setConsent(true);
    }
  };

  const accept = () => {
    setConsent(false);
    const next = pendingRef.current;
    if (next) setPending(next);
    toolsInstall(true).then(
      () => void client.invalidateQueries({ queryKey: toolsStatusKey }),
      (cause: unknown) => {
        pendingRef.current = null;
        setPending(null);
        setError(
          t("transcription.tools.actionError", {
            message: errorMessage(cause),
          }),
        );
      },
    );
  };

  const refuse = () => {
    setConsent(false);
    pendingRef.current = null;
  };

  const showTools =
    tools.data !== undefined &&
    (source === "url" || waiting || toolsBusy(tools.data));

  return (
    <form
      onSubmit={submit}
      // Adresse vérifiée par la page (message traduit), pas par le WebView.
      noValidate
      aria-label={t("transcription.form.label")}
      data-testid="transcription-form"
      className="rounded-3xl border border-neutral-200 bg-white p-5 shadow-sm dark:border-neutral-700/70 dark:bg-neutral-800"
    >
      <label
        htmlFor={ids.address}
        className="mb-2 block font-rounded text-lg font-medium text-neutral-900 dark:text-white"
      >
        {t("transcription.form.address")}
      </label>
      <input
        id={ids.address}
        type="url"
        inputMode="url"
        autoComplete="off"
        spellCheck={false}
        value={address}
        onChange={(event) => {
          setAddress(event.target.value);
          setError(null);
          dropPending();
          if (event.target.value !== "") {
            readId.current += 1;
            setReading(false);
            setFile(null);
          }
        }}
        placeholder={t("transcription.form.addressPlaceholder")}
        data-testid="transcription-address"
        className={INPUT}
      />
      {showTools && tools.data && (
        <ToolsPanel status={tools.data} onInstall={() => setConsent(true)} />
      )}
      {tools.isError && source === "url" && (
        <p role="alert" className="mt-2 text-sm text-red-600 dark:text-red-400">
          {t("transcription.tools.statusError", {
            message: errorMessage(tools.error),
          })}
        </p>
      )}

      <div className="mt-4 flex flex-wrap items-center gap-x-3 gap-y-2">
        <span className="text-sm text-neutral-500 dark:text-neutral-400">
          {t("transcription.form.or")}
        </span>
        <button
          type="button"
          className={QUIET_BUTTON}
          onClick={pick}
          data-testid="transcription-pick"
        >
          <DocumentArrowUpIcon aria-hidden="true" className="h-4 w-4" />
          {t("transcription.form.chooseFile")}
        </button>
        <span className="text-xs text-neutral-500 dark:text-neutral-400">
          {t("transcription.form.dropHint")}
        </span>
      </div>
      {reading && (
        <p
          aria-busy="true"
          className="mt-2 text-sm text-neutral-500 motion-safe:animate-pulse dark:text-neutral-400"
        >
          {t("transcription.form.reading")}
        </p>
      )}
      {file && (
        <div
          className="mt-3"
          aria-label={t("transcription.form.file")}
          role="group"
        >
          <div
            data-testid="transcription-file"
            className="inline-flex max-w-full items-center gap-2 rounded-full border border-neutral-200 bg-neutral-50 py-1 pr-1 pl-3 text-sm text-neutral-800 dark:border-neutral-700 dark:bg-neutral-900 dark:text-neutral-100"
          >
            <FilmIcon
              aria-hidden="true"
              className="h-4 w-4 flex-none text-neutral-500"
            />
            <span className="min-w-0 truncate">{file.name}</span>
            {file.durationS !== null && (
              <span className="flex-none text-neutral-500 tabular-nums dark:text-neutral-400">
                {clock(file.durationS)}
              </span>
            )}
            <button
              type="button"
              className="rounded-full p-0.5 text-neutral-500 outline-none hover:bg-neutral-200 hover:text-neutral-900 focus-visible:ring-2 focus-visible:ring-accent-fg dark:hover:bg-neutral-700 dark:hover:text-white"
              aria-label={t("transcription.form.removeFile", {
                name: file.name,
              })}
              title={t("transcription.form.removeFile", { name: file.name })}
              onClick={() => {
                dropPending();
                setFile(null);
              }}
            >
              <XMarkIcon aria-hidden="true" className="h-4 w-4" />
            </button>
          </div>
          {file.error && (
            <div className="mt-2">
              <ErrorNotice error={file.error} />
            </div>
          )}
        </div>
      )}

      <div className="mt-4 flex flex-wrap items-center gap-x-6 gap-y-3">
        <div className="flex min-w-0 items-center gap-2">
          <label
            htmlFor={ids.model}
            className="text-sm text-neutral-700 dark:text-neutral-300"
          >
            {t("transcription.form.model")}
          </label>
          <select
            id={ids.model}
            className={SELECT}
            value={chosenModel?.id ?? ""}
            disabled={installed.length === 0}
            onChange={(event) => setModel(event.target.value)}
            data-testid="transcription-model"
          >
            {installed.map((m) => (
              <option key={m.id} value={m.id}>
                {m.name}
              </option>
            ))}
          </select>
        </div>
        <div className="flex min-w-0 items-center gap-2">
          <label
            htmlFor={ids.language}
            className="text-sm text-neutral-700 dark:text-neutral-300"
          >
            {t("transcription.form.language")}
          </label>
          <select
            id={ids.language}
            className={SELECT}
            value={chosenLanguage}
            onChange={(event) => setLanguage(event.target.value)}
            data-testid="transcription-language"
          >
            <option value={AUTO_LANGUAGE}>
              {t("transcription.form.autoLanguage")}
            </option>
            {chosenLanguage !== AUTO_LANGUAGE &&
              !languages.some((lang) => lang.code === chosenLanguage) && (
                <option value={chosenLanguage}>
                  {languageName(t.lang, chosenLanguage)}
                </option>
              )}
            {languages.map((lang) => (
              <option key={lang.code} value={lang.code}>
                {lang.label}
              </option>
            ))}
          </select>
        </div>
        {installed.length > 0 && (
          <Link
            to="/hub/transcription"
            className="rounded text-sm text-accent-fg underline underline-offset-2 outline-none focus-visible:ring-2 focus-visible:ring-accent-fg"
          >
            {t("transcription.form.moreModels")}
          </Link>
        )}
      </div>
      {noModel && (
        <p
          data-testid="transcription-no-model"
          className="mt-3 text-sm text-neutral-600 dark:text-neutral-300"
        >
          {t("transcription.form.noModel")}{" "}
          <Link
            to="/hub/transcription"
            className="rounded font-medium text-accent-fg underline underline-offset-2 outline-none focus-visible:ring-2 focus-visible:ring-accent-fg"
          >
            {t("transcription.form.getModel")}
          </Link>
        </p>
      )}
      {advice.length > 0 && (
        <ul
          className="mt-3 flex flex-col gap-1"
          data-testid="transcription-advice"
        >
          {advice.map((item) => (
            <li
              key={item.kind}
              data-advice={item.kind}
              className="flex items-start gap-1.5 text-xs text-neutral-600 dark:text-neutral-300"
            >
              <ExclamationTriangleIcon
                aria-hidden="true"
                className="mt-0.5 h-3.5 w-3.5 flex-none text-amber-700 dark:text-amber-400"
              />
              {adviceText(t, item, chosenLanguage)}
            </li>
          ))}
        </ul>
      )}

      <div className="mt-4 flex flex-wrap items-center justify-end gap-3">
        {running && (
          <p className="mr-auto text-sm text-neutral-600 dark:text-neutral-300">
            {t("transcription.form.busy")}
          </p>
        )}
        {lost && !waiting && (
          <p
            className="mr-auto text-sm text-neutral-600 dark:text-neutral-300"
            data-testid="transcription-pending-lost"
          >
            {t("transcription.form.pendingLost")}
          </p>
        )}
        {waiting && (
          <p
            className="mr-auto text-sm text-neutral-600 dark:text-neutral-300"
            data-testid="transcription-waiting-tools"
          >
            {t("transcription.form.waitingTools")}
          </p>
        )}
        <button
          type="submit"
          className={PRIMARY_BUTTON}
          disabled={disabled}
          data-testid="transcription-start"
        >
          {starting
            ? t("transcription.form.starting")
            : t("transcription.form.start")}
          <ArrowRightIcon aria-hidden="true" className="h-4 w-4" />
        </button>
      </div>
      {error && (
        <p
          role="alert"
          className="mt-3 rounded-lg bg-red-50 px-3 py-2 text-sm break-words text-red-700 dark:bg-red-950/40 dark:text-red-300"
        >
          {error}
        </p>
      )}
      {consent && tools.data && (
        <ToolsConsent status={tools.data} onAccept={accept} onCancel={refuse} />
      )}
      {dragging && <DropVeil />}
    </form>
  );
}

function adviceText(t: TFunction, advice: EngineAdvice, language: string) {
  switch (advice.kind) {
    case "whisperSlow":
      return t("transcription.form.whisperSlow");
    case "whisperSlowFile":
      return t("transcription.form.whisperSlowFile", { count: advice.minutes });
    case "parakeetLanguage":
      return t("transcription.form.parakeetLanguage", {
        language: languageName(t.lang, language),
      });
    case "parakeetGuesses":
      return t("transcription.form.parakeetGuesses", {
        language: languageName(t.lang, language),
      });
    case "twoLanguages":
      return t("transcription.form.twoLanguages");
  }
}

/** Voile du dépôt d'un fichier sur la fenêtre. */
function DropVeil() {
  const t = useT();
  return createPortal(
    <div
      data-testid="transcription-drop"
      className="pointer-events-none fixed inset-0 z-[9999] flex items-center justify-center bg-neutral-50 dark:bg-neutral-900"
    >
      <div className="mx-4 max-w-sm rounded-2xl border border-neutral-200 bg-white p-10 text-center shadow-2xl dark:border-neutral-700 dark:bg-neutral-800">
        <DocumentArrowUpIcon
          aria-hidden="true"
          className="mx-auto mb-2 h-8 w-8 text-neutral-900 dark:text-white"
        />
        <p className="text-sm font-medium text-neutral-600 dark:text-neutral-300">
          {t("transcription.form.dropOverlay")}
        </p>
      </div>
    </div>,
    document.body,
  );
}
