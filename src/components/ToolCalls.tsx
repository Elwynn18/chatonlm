/*
 * SPDX-License-Identifier: MIT
 * SPDX-FileCopyrightText: Ollama
 * SPDX-FileCopyrightText: 2026 Elwynn18 and ChatonLM contributors
 *
 * Adapted from Ollama (https://github.com/ollama/ollama),
 * file app/ui/app/src/components/Message.tsx, commit 42e911bc3d05.
 * Modified for ChatonLM: tool calls and their results come from the message
 * of the assistant that made them (no separate "tool" messages); each result
 * is titled by the query or address of its own call (the official one takes
 * the last one seen, wrong when a turn makes several calls); no browser tool
 * display (ChatonLM gives every model the same two web tools), any other tool
 * shown as "Calling name: preview..."; Heroicons magnifying glass, globe,
 * code brackets and chevron instead of the SF Symbols and Feather glyphs;
 * the result header is a real button (aria-expanded, aria-controls,
 * keyboard, blue focus ring, chevron shown on keyboard focus too); the folded
 * body is hidden from keyboard and screen readers and marked (data-folded)
 * like the thinking block; no shared element id on results; texts translated
 * into English, Spanish and French (src/i18n); ChatonLM's own image tool
 * (`generate_image`): while it runs, "Creating an image..." with its prompt,
 * the sampling steps, the preview in a frame the size of the image (the size
 * asked by the call until the engine gives it), a processor warning,
 * "Waiting for another image..." when queued and, after 20 s in the same
 * preparation stage, why the first image takes longer (the same display,
 * without progress, while an Easy Link bot's reply runs in the chat); once
 * the reply has ended without a result, "Image stopped"; its error with a link to the
 * Images tab of the ModelHub; once done, "Created an image", unfolding its
 * prompt like the web results (the image itself: GeneratedImage.tsx).
 */

import {
  ChevronDownIcon,
  CodeBracketIcon,
  GlobeAltIcon,
  PhotoIcon,
} from "@heroicons/react/24/outline";
import { MagnifyingGlassIcon } from "@heroicons/react/20/solid";
import { Link } from "@tanstack/react-router";
import { useEffect, useId, useState } from "react";
import type { ImageProgress } from "@/bindings/ImageProgress";
import type { ToolCall } from "@/bindings/ToolCall";
import { useImageProgress } from "@/hooks/useSendMessage";
import { type TFunction, useT } from "@/i18n";
import { Trans } from "@/i18n/Trans";
import {
  IMAGE_TOOL,
  imageFrame,
  imagePrompt,
  requestedSide,
} from "@/lib/generatedImages";

/** Préparation (démarrage, chargement) au-delà de laquelle un conseil vient. */
export const SLOW_PREPARATION_MS = 20_000;

/**
 * Côté d'une image que ni le moteur (avancement) ni l'appel (arguments) ne
 * donne encore.
 */
const DEFAULT_SIDE = 512;

/** Arguments d'un appel (objet JSON en texte) ; null s'ils sont illisibles. */
function parseArguments(call: ToolCall): Record<string, unknown> | null {
  try {
    const parsed: unknown = JSON.parse(call.arguments);
    return typeof parsed === "object" && parsed !== null
      ? (parsed as Record<string, unknown>)
      : null;
  } catch {
    return null;
  }
}

/** Valeur texte d'un argument, sinon "". */
function stringArgument(call: ToolCall, key: string): string {
  const value = parseArguments(call)?.[key];
  return typeof value === "string" ? value : "";
}

function InlineSearchTerm({ term }: { term: string }) {
  return <span className="font-medium">{term}</span>;
}

const IMAGE_LINE =
  "relative text-neutral-600 select-text dark:text-neutral-400";

/**
 * Conseil d'une préparation longue, montré SLOW_PREPARATION_MS après son
 * montage : rendu sous une clé par étape, il repart de zéro à chaque entrée
 * en `starting` ou `loading` (pas de remise à zéro par un effet).
 */
function SlowPreparationHint() {
  const t = useT();
  const [slow, setSlow] = useState(false);
  useEffect(() => {
    const timer = setTimeout(() => setSlow(true), SLOW_PREPARATION_MS);
    return () => clearTimeout(timer);
  }, []);
  return slow ? (
    <p className="text-sm text-neutral-500 dark:text-neutral-400">
      {t("chat.image.slowHint")}
    </p>
  ) : null;
}

/**
 * Image en cours (flux vivant) : étapes, aperçu dans un cadre à la taille de
 * l'image, avertissements. Avant le premier avancement : préparation, dans
 * un cadre à la taille demandée par l'appel (sinon 512 de côté).
 */
function ImageInProgress({
  call,
  messageId,
  index,
}: {
  call: ToolCall;
  messageId: number;
  index: number;
}) {
  const t = useT();
  const progress: ImageProgress | null = useImageProgress(messageId, index);
  const stage = progress?.stage ?? "starting";
  const preparing = stage === "starting" || stage === "loading";
  const prompt = imagePrompt(call);
  const width = progress?.width ?? requestedSide(call, "width") ?? DEFAULT_SIDE;
  const height =
    progress?.height ?? requestedSide(call, "height") ?? DEFAULT_SIDE;
  const steps =
    stage === "generating" &&
    progress?.step != null &&
    progress.total != null &&
    progress.total > 0
      ? { step: progress.step, total: progress.total }
      : null;

  return (
    <div data-role="image-progress" className={IMAGE_LINE}>
      <PhotoIcon aria-hidden="true" className="absolute top-1 h-4 w-4" />
      <div className="ml-6 flex flex-col gap-2">
        <p>{t("chat.image.creating")}</p>
        {prompt && (
          <p className="text-sm break-words text-neutral-500 dark:text-neutral-400">
            {prompt}
          </p>
        )}
        {stage === "queued" && (
          <p className="text-sm text-neutral-500 dark:text-neutral-400">
            {t("chat.image.queued")}
          </p>
        )}
        {steps && (
          <div className="flex items-center gap-3">
            <div
              role="progressbar"
              aria-label={t("chat.image.progress")}
              aria-valuemin={0}
              aria-valuemax={steps.total}
              aria-valuenow={steps.step}
              className="h-1.5 w-40 overflow-hidden rounded-full bg-neutral-200 dark:bg-neutral-700"
            >
              <div
                className="h-full rounded-full bg-accent transition-[width] duration-300"
                style={{
                  width: `${Math.min(100, (steps.step / steps.total) * 100)}%`,
                }}
              />
            </div>
            <span className="text-sm text-neutral-500 tabular-nums dark:text-neutral-400">
              {t("chat.image.step", { step: steps.step, total: steps.total })}
            </span>
          </div>
        )}
        <div
          data-testid="image-frame"
          style={imageFrame(width, height)}
          className="overflow-hidden rounded-2xl bg-neutral-100 dark:bg-neutral-800"
        >
          {progress?.preview && (
            <img
              src={progress.preview}
              alt={t("chat.image.previewAlt")}
              draggable={false}
              className="m-0 block h-full w-full object-cover select-none"
            />
          )}
        </div>
        {progress?.cpu && (
          <p className="text-xs text-amber-700 dark:text-amber-400">
            {t("chat.image.cpu")}
          </p>
        )}
        {preparing && <SlowPreparationHint key={stage} />}
      </div>
    </div>
  );
}

/** Image finie : « Created an image », dont le corps déplie la description. */
function ImageDone({ call }: { call: ToolCall }) {
  const t = useT();
  const [isCollapsed, setIsCollapsed] = useState(true);
  const bodyId = useId();
  const prompt = imagePrompt(call);
  return (
    <div className="relative flex w-full flex-col text-neutral-600 transition-colors select-text hover:text-neutral-800 dark:text-neutral-400 dark:hover:text-neutral-200">
      <button
        type="button"
        className="group/image relative flex cursor-pointer self-start rounded-md text-left outline-none select-text focus-visible:ring-2 focus-visible:ring-accent-fg"
        aria-expanded={!isCollapsed}
        aria-controls={bodyId}
        onClick={() => setIsCollapsed(!isCollapsed)}
      >
        <PhotoIcon
          aria-hidden="true"
          className="absolute top-1 h-4 w-4 transition-opacity group-hover/image:opacity-0 group-focus-visible/image:opacity-0"
        />
        <ChevronDownIcon
          aria-hidden="true"
          strokeWidth={2}
          className={`absolute top-1 h-4 w-4 opacity-0 transition-all group-hover/image:opacity-100 group-focus-visible/image:opacity-100 ${
            isCollapsed ? "-rotate-90" : "rotate-0"
          } will-change-[opacity,transform]`}
        />
        <span className="ml-6">{t("chat.image.created")}</span>
      </button>
      <div
        id={bodyId}
        className="mt-2 ml-6 overflow-y-auto text-sm break-words whitespace-pre-wrap text-neutral-500 transition-[max-height,opacity,visibility] duration-300 ease-in-out dark:text-neutral-400"
        style={{
          maxHeight: isCollapsed ? "0px" : "20rem",
          opacity: isCollapsed ? 0 : 1,
          visibility: isCollapsed ? "hidden" : undefined,
        }}
        data-folded={isCollapsed ? "" : undefined}
      >
        {prompt}
      </div>
    </div>
  );
}

/**
 * Appel `generate_image` : en cours (flux vivant), arrêté (fini sans
 * résultat), en erreur (avec le lien vers les modèles d'images), ou fini.
 */
function ImageCallDisplay({
  call,
  messageId,
  index,
  isStreaming,
  isRemoteRunning,
}: {
  call: ToolCall;
  messageId: number;
  index: number;
  isStreaming: boolean;
  isRemoteRunning: boolean;
}) {
  const t = useT();
  if (call.error !== null) {
    return (
      <div data-role="image-error" className={IMAGE_LINE}>
        <PhotoIcon aria-hidden="true" className="absolute top-1 h-4 w-4" />
        <div className="ml-6 flex flex-col gap-1">
          <p className="break-words text-red-600 dark:text-red-400">
            {t("chat.image.failed", { error: call.error })}
          </p>
          <Link
            to="/hub/images"
            className="self-start rounded-sm text-sm underline underline-offset-2 outline-none hover:text-neutral-900 focus-visible:ring-2 focus-visible:ring-accent-fg dark:hover:text-white"
          >
            {t("chat.image.models")}
          </Link>
        </div>
      </div>
    );
  }
  if (call.result !== null) return <ImageDone call={call} />;
  // Réponse d'un bot d'Easy Link en cours : sans flux dans la page, donc
  // sans avancement (préparation, cadre, conseil après 20 s).
  if (isStreaming || isRemoteRunning) {
    return <ImageInProgress call={call} messageId={messageId} index={index} />;
  }
  return (
    <div data-role="image-stopped" className={IMAGE_LINE}>
      <PhotoIcon aria-hidden="true" className="absolute top-1 h-4 w-4" />
      <div className="ml-6">{t("chat.image.stopped")}</div>
    </div>
  );
}

/**
 * Ligne « Searching for… » d'un appel, comme ToolCallDisplay de l'officiel ;
 * l'appel `generate_image` a son propre affichage (`messageId` et `index`
 * désignent son avancement, `isStreaming` : sa réponse s'écrit encore ;
 * `isRemoteRunning` : réponse d'un bot d'Easy Link en cours, sans flux).
 */
export function ToolCallDisplay({
  call,
  messageId = 0,
  index = 0,
  isStreaming = false,
  isRemoteRunning = false,
}: {
  call: ToolCall;
  messageId?: number;
  index?: number;
  isStreaming?: boolean;
  isRemoteRunning?: boolean;
}) {
  if (call.name === IMAGE_TOOL) {
    return (
      <ImageCallDisplay
        call={call}
        messageId={messageId}
        index={index}
        isStreaming={isStreaming}
        isRemoteRunning={isRemoteRunning}
      />
    );
  }

  // frontend tool call display for web_search and web_fetch
  if (call.name === "web_search" || call.name === "web_fetch") {
    const search = call.name === "web_search";
    const term = stringArgument(call, search ? "query" : "url");
    return (
      <div className="relative text-neutral-600 select-text dark:text-neutral-400">
        {/* Magnifying Glass Icon */}
        <MagnifyingGlassIcon
          aria-hidden="true"
          className="absolute top-1 h-4 w-4"
        />
        <div className="ml-6">
          <Trans
            k={search ? "chat.tools.searching" : "chat.tools.fetching"}
            tags={{ term: () => <InlineSearchTerm term={term} /> }}
          />
        </div>
      </div>
    );
  }

  // preview from the tool's JSON arguments.
  const args = parseArguments(call);
  const preferredKey = ["query", "url", "pattern", "id", "file", "path"].find(
    (key) => args !== null && Object.prototype.hasOwnProperty.call(args, key),
  );
  const value = preferredKey ? args?.[preferredKey] : undefined;
  const preview = typeof value === "string" ? value : "";

  return (
    <div className="relative text-neutral-600 select-text dark:text-neutral-400">
      <CodeBracketIcon
        aria-hidden="true"
        strokeWidth={2}
        className="absolute top-1.5 h-4 w-4"
      />
      <div className="ml-6">
        <Trans
          k={preview ? "chat.tools.callingWith" : "chat.tools.calling"}
          tags={{
            name: () => <span className="font-mono">{call.name}</span>,
            preview: () => <InlineSearchTerm term={preview} />,
          }}
        />
      </div>
    </div>
  );
}

/** Titre d'un résultat, par la requête ou l'adresse de son propre appel. */
function resultTitle(t: TFunction, call: ToolCall, result: unknown) {
  const fromResult = (key: string) =>
    typeof result === "object" &&
    result !== null &&
    typeof (result as Record<string, unknown>)[key] === "string"
      ? ((result as Record<string, unknown>)[key] as string)
      : "";
  if (call.name === "web_search") {
    const q =
      stringArgument(call, "query").trim() || fromResult("query").trim();
    return q ? (
      <Trans
        k="chat.tools.searchResults"
        tags={{ term: () => <InlineSearchTerm term={q} /> }}
      />
    ) : (
      t("chat.tools.webSearchResults")
    );
  }
  if (call.name === "web_fetch") {
    const u = stringArgument(call, "url").trim() || fromResult("url");
    return u ? (
      <Trans
        k="chat.tools.fetchResults"
        tags={{ url: () => <span className="break-all">{u}</span> }}
      />
    ) : (
      t("chat.tools.webFetchResults")
    );
  }
  return t("chat.tools.rawResult");
}

/** Résultat relu : objet JSON, ou le texte tel quel s'il n'en est pas un. */
function parseResult(text: string): unknown {
  try {
    return JSON.parse(text) as unknown;
  } catch {
    return text;
  }
}

/**
 * Résultat d'un appel terminé, comme ToolRoleContent de l'officiel : erreur
 * en clair (« Error: … »), sinon bloc replié par défaut (globe, flèche au
 * survol) dont le corps montre le JSON indenté.
 */
export function ToolResult({ call }: { call: ToolCall }) {
  const t = useT();
  const [isCollapsed, setIsCollapsed] = useState(true);
  const bodyId = useId();

  if (call.error !== null) {
    return (
      <div className="space-y-2">
        <pre className="max-h-40 overflow-x-auto rounded-md bg-neutral-100 p-2 text-xs whitespace-pre-wrap text-neutral-800 dark:bg-neutral-800 dark:text-neutral-200">
          <code>{t("chat.tools.error", { error: call.error })}</code>
        </pre>
      </div>
    );
  }
  if (call.result === null) return null;

  const result = parseResult(call.result);
  return (
    // collapsible tool result with raw json
    <div className="space-y-2">
      <div className="relative flex w-full flex-col text-neutral-600 transition-colors select-text hover:text-neutral-800 dark:text-neutral-400 dark:hover:text-neutral-200">
        <button
          type="button"
          className="group/browser relative flex cursor-pointer self-start rounded-md text-left outline-none select-text focus-visible:ring-2 focus-visible:ring-accent-fg"
          aria-expanded={!isCollapsed}
          aria-controls={bodyId}
          onClick={() => setIsCollapsed(!isCollapsed)}
        >
          {/* Globe icon */}
          <GlobeAltIcon
            aria-hidden="true"
            className="absolute top-1 -left-0.5 h-5 w-5 transition-opacity group-hover/browser:opacity-0 group-focus-visible/browser:opacity-0"
          />
          {/* Arrow */}
          <ChevronDownIcon
            aria-hidden="true"
            strokeWidth={2}
            className={`absolute top-1.5 h-4 w-4 opacity-0 transition-all group-hover/browser:opacity-100 group-focus-visible/browser:opacity-100 ${
              isCollapsed ? "-rotate-90" : "rotate-0"
            } will-change-[opacity,transform]`}
          />
          <span className="ml-6">{resultTitle(t, call, result)}</span>
        </button>

        <div
          id={bodyId}
          className="mt-2 ml-6 overflow-y-auto rounded-md text-xs text-neutral-500 transition-[max-height,opacity,visibility] duration-300 ease-in-out"
          style={{
            maxHeight: isCollapsed ? "0px" : "20rem",
            opacity: isCollapsed ? 0 : 1,
            // Replié : ni tabulation ni lecteur d'écran.
            visibility: isCollapsed ? "hidden" : undefined,
          }}
          // Repli en cours ou fait : compté à sa hauteur finale (nulle) par
          // la liste des messages (useMessageAutoscroll).
          data-folded={isCollapsed ? "" : undefined}
        >
          <pre className="overflow-x-auto rounded-md border border-neutral-200 bg-neutral-50 p-2 text-xs text-neutral-800 dark:border-neutral-700 dark:bg-neutral-900 dark:text-neutral-200">
            <code>
              {typeof result === "string"
                ? result
                : JSON.stringify(result, null, 2)}
            </code>
          </pre>
        </div>
      </div>
    </div>
  );
}
