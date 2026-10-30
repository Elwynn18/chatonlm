// Autocomplétion locale (lot 3) : `InlineCompletionItemProvider` relié au pont
// (`complete.fim`), barre d'état « ChatonLM autocomplete » et état de la carte
// « Graphics card » du panneau (plan du chef de la carte graphique de l'app).
// Coupée par défaut. Aucune requête de complétion tant que le modèle n'est pas
// en mémoire selon le plan de l'app ; un refus « en pause » (-32007) arrête les
// requêtes jusqu'au prochain plan (`gpu.changed`), jamais de boucle. Rien ne
// part vers un serveur d'une autre machine ni vers un modèle cloud, ni depuis
// un fichier sensible. L'extension ne télécharge rien : « Install in
// ChatonLM » demande à l'app de montrer sa proposition (`models.offerPull`).
// Contrat : .tools/research/91-lot3-gpu-completion-contract.md (Principes, X).
// Retour d'inactivité (principe 1) : modèle évincé après 10 min, l'extension
// le redemande d'elle-même à la frappe, seulement quand le plan dit
// « ensemble » sans réponse en cours, au plus une fois toutes les 30 s.
import path from "node:path";
import * as vscode from "vscode";
import {
  type CompletionCommand,
  type CompletionMeasure,
  type CompletionOption,
  type CompletionPause,
  type CompletionView,
  canPrepare,
  type GpuPlan,
  type HostMessage,
} from "../shared/protocol";
import type { LinkLike } from "./controller";
import { type Notification, RpcError, TimeoutError } from "./bridge/rpc";
import { isSecretFile, looksRemote } from "./receipt";

/** Anti-rebond entre la frappe et la requête. */
export const DEBOUNCE_MS = 250;
/** Préfixe envoyé au plus (caractères), en lignes entières. */
export const PREFIX_CHARS = 4000;
/** Suffixe envoyé au plus (caractères). */
export const SUFFIX_CHARS = 1000;
/** Début du préfixe aligné sur un multiple de ces lignes : il ne bouge pas à
 * chaque frappe (cache de préfixe d'Ollama, `cachedTokens`). */
export const PREFIX_STEP_LINES = 8;
/** Propositions gardées (préfixe, suffixe). */
export const CACHE_SIZE = 32;
/** `app.state.local` relu s'il date de plus longtemps. */
export const LOCAL_TTL_MS = 5000;
/** Délai de `complete.fim` côté client : sans réponse, la requête est
 * arrêtée et oubliée (le moteur abandonne lui-même après 1,5 s sans premier
 * jeton ; 64 jetons sur une machine lente restent sous ce délai). */
export const FIM_TIMEOUT_MS = 10_000;
/** Délais de garde de suite (premier jeton trop lent) avant de dire « trop
 * lent » et d'espacer les requêtes. */
export const SLOW_AFTER = 3;
/** Anti-rebond multiplié d'autant quand les propositions arrivent trop tard. */
export const SLOW_DEBOUNCE_FACTOR = 4;
/** Retour d'inactivité : au plus une préparation demandée par la frappe dans
 * cet intervalle. */
export const WAKE_INTERVAL_MS = 30_000;

/** Codes du pont pour l'autocomplétion (contrat 1a, lot 3) : en pause,
 * refus du moteur (modèle sans FIM, erreur d'Ollama), remplacée ou annulée. */
export const CompletionCodes = {
  paused: -32007,
  refused: -32020,
  cancelled: -32800,
} as const;

const PAUSES: readonly CompletionPause[] = [
  "exclusive",
  "chatBusy",
  "doesNotFit",
  "notInstalled",
  "notLocal",
  "notLoaded",
];

/** Raison d'un refus « en pause » (donnée de l'app, sans confiance). */
export function pauseReason(data: unknown): CompletionPause {
  const reason = (data as { reason?: unknown } | undefined)?.reason;
  return PAUSES.includes(reason as CompletionPause)
    ? (reason as CompletionPause)
    : "notLoaded";
}

const sameModel = (a: string, b: string) =>
  a.replace(/:latest$/, "") === b.replace(/:latest$/, "");

type Lines = Pick<vscode.TextDocument, "lineAt" | "lineCount">;

/**
 * Code autour du curseur : préfixe en lignes entières (début stable, aligné
 * sur `PREFIX_STEP_LINES`), suffixe jusqu'à `SUFFIX_CHARS`.
 */
export function promptWindow(
  document: Lines,
  position: vscode.Position,
): { prefix: string; suffix: string; restOfLineEmpty: boolean } {
  const line = document.lineAt(position.line).text;
  let before = line.slice(0, position.character);
  const after = line.slice(position.character);
  if (before.length > PREFIX_CHARS) {
    // Jamais la seconde moitié d'une paire de substitution seule (emoji) :
    // la trame JSON serait illisible pour l'app.
    before = before.slice(-PREFIX_CHARS);
    if (isLowSurrogate(before.charCodeAt(0))) before = before.slice(1);
  }
  let size = before.length;
  let start = position.line;
  while (start > 0) {
    const next = document.lineAt(start - 1).text.length + 1;
    if (size + next > PREFIX_CHARS) break;
    size += next;
    start--;
  }
  if (start > 0) {
    // Arrondi vers le bas du fichier : la fenêtre reste sous la limite.
    start = Math.min(
      position.line,
      Math.ceil(start / PREFIX_STEP_LINES) * PREFIX_STEP_LINES,
    );
  }
  const above: string[] = [];
  for (let i = start; i < position.line; i++) {
    above.push(document.lineAt(i).text);
  }
  const prefix = [...above, before].join("\n");
  let suffix = after;
  for (let i = position.line + 1; i < document.lineCount; i++) {
    const next = `\n${document.lineAt(i).text}`;
    if (suffix.length + next.length > SUFFIX_CHARS) break;
    suffix += next;
  }
  if (suffix.length > SUFFIX_CHARS) {
    suffix = suffix.slice(0, SUFFIX_CHARS);
    if (isHighSurrogate(suffix.charCodeAt(suffix.length - 1))) {
      suffix = suffix.slice(0, -1);
    }
  }
  return { prefix, suffix, restOfLineEmpty: after.trim() === "" };
}

const isHighSurrogate = (code: number) => code >= 0xd800 && code <= 0xdbff;
const isLowSurrogate = (code: number) => code >= 0xdc00 && code <= 0xdfff;

/** Dernières propositions, par (modèle, plusieurs lignes ou une, préfixe,
 * suffixe). */
export class CompletionCache {
  readonly #items = new Map<
    string,
    { prefix: string; suffix: string; text: string }
  >();

  #scope(model: string, multiline: boolean) {
    return `${model}\u0000${multiline ? "n" : "1"}\u0000`;
  }

  #key(model: string, multiline: boolean, prefix: string, suffix: string) {
    return `${this.#scope(model, multiline)}${prefix}\u0000${suffix}`;
  }

  /** Proposition gardée, ou ce qu'il en reste quand on a tapé son début. */
  get(
    model: string,
    multiline: boolean,
    prefix: string,
    suffix: string,
  ): string | undefined {
    const key = this.#key(model, multiline, prefix, suffix);
    const exact = this.#items.get(key);
    if (exact) {
      this.#items.delete(key);
      this.#items.set(key, exact);
      return exact.text;
    }
    const scope = this.#scope(model, multiline);
    for (const [k, item] of this.#items) {
      if (!k.startsWith(scope) || item.suffix !== suffix) continue;
      if (!prefix.startsWith(item.prefix)) continue;
      const typed = prefix.slice(item.prefix.length);
      if (typed && item.text.length > typed.length) {
        if (item.text.startsWith(typed)) return item.text.slice(typed.length);
      }
    }
    return undefined;
  }

  set(
    model: string,
    multiline: boolean,
    prefix: string,
    suffix: string,
    text: string,
  ): void {
    const key = this.#key(model, multiline, prefix, suffix);
    this.#items.delete(key);
    this.#items.set(key, { prefix, suffix, text });
    while (this.#items.size > CACHE_SIZE) {
      const oldest = this.#items.keys().next().value;
      if (oldest === undefined) break;
      this.#items.delete(oldest);
    }
  }

  clear(): void {
    this.#items.clear();
  }
}

/** Autres complétions en ligne connues, et le réglage qui les règle
 * (`perLanguage` : objet par langage, `"*"` pour les autres). */
const OTHER_PROVIDERS = [
  {
    id: "GitHub.copilot",
    name: "GitHub Copilot",
    setting: "github.copilot.enable",
    perLanguage: true,
  },
  {
    id: "GitHub.copilot-chat",
    name: "GitHub Copilot",
    setting: "github.copilot.enable",
    perLanguage: true,
  },
  {
    id: "Codeium.codeium",
    name: "Windsurf",
    setting: "codeium.enableConfig",
    perLanguage: true,
  },
  {
    id: "Continue.continue",
    name: "Continue",
    setting: "continue.enableTabAutocomplete",
    perLanguage: false,
  },
  { id: "TabNine.tabnine-vscode", name: "Tabnine", setting: null },
  { id: "supermaven.supermaven", name: "Supermaven", setting: null },
] as const;

/** Valeur d'un réglage d'une autre extension pour ce langage : posée par
 * l'utilisateur (`explicit`), ou celle par défaut. */
function providerSetting(
  setting: string,
  perLanguage: boolean,
  languageId: string,
): { value: boolean; explicit: boolean } {
  const config = vscode.workspace.getConfiguration();
  const pick = (raw: unknown): boolean | undefined => {
    if (!perLanguage) return typeof raw === "boolean" ? raw : undefined;
    if (!raw || typeof raw !== "object") return undefined;
    const map = raw as Record<string, unknown>;
    const value = map[languageId] ?? map["*"];
    return typeof value === "boolean" ? value : undefined;
  };
  const inspected = config.inspect(setting);
  const explicit = [
    inspected?.workspaceFolderLanguageValue,
    inspected?.workspaceLanguageValue,
    inspected?.globalLanguageValue,
    inspected?.workspaceFolderValue,
    inspected?.workspaceValue,
    inspected?.globalValue,
  ]
    .map(pick)
    .find((value) => value !== undefined);
  if (explicit !== undefined) return { value: explicit, explicit: true };
  return { value: pick(config.get(setting)) ?? true, explicit: false };
}

/**
 * Autre complétion en ligne pour ce langage (VS Code fusionnerait ses
 * propositions avec les nôtres : deux calculs pour une frappe), ou null.
 * `certain` : extension active ET complétion allumée pour ce langage par un
 * réglage que l'utilisateur a posé ; sinon (réglage par défaut, ou aucun
 * réglage connu : Tabnine, Supermaven ; Copilot installé sans compte) on ne
 * peut pas le savoir, et ChatonLM propose quand même en le disant. Une
 * complétion coupée par son réglage ne compte pas, ni Copilot quand
 * `chat.disableAIFeatures` coupe ses fonctions.
 */
export function otherProvider(
  languageId: string,
): { name: string; certain: boolean } | null {
  let maybe: string | null = null;
  for (const other of OTHER_PROVIDERS) {
    if (!vscode.extensions.getExtension(other.id)?.isActive) continue;
    if (
      other.setting === "github.copilot.enable" &&
      vscode.workspace.getConfiguration().get("chat.disableAIFeatures") === true
    ) {
      continue;
    }
    if (other.setting === null) {
      maybe ??= other.name;
      continue;
    }
    const { value, explicit } = providerSetting(
      other.setting,
      other.perLanguage,
      languageId,
    );
    if (!value) continue;
    if (explicit) return { name: other.name, certain: true };
    maybe ??= other.name;
  }
  return maybe ? { name: maybe, certain: false } : null;
}

/** Un des motifs (`files.readonlyInclude`…) désigne ce document : chemin
 * absolu, ou relatif à un dossier ouvert. */
function matchesGlobs(globs: unknown, document: vscode.TextDocument): boolean {
  if (!globs || typeof globs !== "object") return false;
  for (const [glob, on] of Object.entries(globs as Record<string, unknown>)) {
    if (on !== true) continue;
    if (vscode.languages.match({ pattern: glob }, document) > 0) return true;
    for (const folder of vscode.workspace.workspaceFolders ?? []) {
      const pattern = new vscode.RelativePattern(folder, glob);
      if (vscode.languages.match({ pattern }, document) > 0) return true;
    }
  }
  return false;
}

/** Document en lecture seule : système de fichiers, ou motifs des réglages
 * `files.readonlyInclude` / `files.readonlyExclude`. Écart : un éditeur mis
 * en lecture seule pour la séance, ou par les droits du fichier
 * (`files.readonlyFromPermissions`), n'est pas visible par l'API. */
function isReadOnly(document: vscode.TextDocument): boolean {
  if (vscode.workspace.fs.isWritableFileSystem(document.uri.scheme) === false) {
    return true;
  }
  const files = vscode.workspace.getConfiguration("files");
  return (
    matchesGlobs(files.get("readonlyInclude"), document) &&
    !matchesGlobs(files.get("readonlyExclude"), document)
  );
}

/** Rien n'est jamais proposé dans ce document : `remoteWindow` (fichier
 * d'une fenêtre distante : WSL, SSH, conteneur), `notHere` (autre schéma :
 * réglages, sortie, notebook… ; fichier sensible ; lecture seule). */
export function excludedDocument(
  document: vscode.TextDocument,
): "notHere" | "remoteWindow" | null {
  const { scheme } = document.uri;
  if (scheme !== "file" && scheme !== "untitled") {
    return scheme === "vscode-remote" || vscode.env.remoteName
      ? "remoteWindow"
      : "notHere";
  }
  if (isSecretFile(document.uri.path) || isReadOnly(document)) return "notHere";
  return null;
}

/** Pourquoi ne rien proposer ici (null : rien ne s'y oppose). */
export function skipReason(
  document: vscode.TextDocument,
  context: Pick<vscode.InlineCompletionContext, "selectedCompletionInfo">,
): "scheme" | "secret" | "readOnly" | "selection" | "snippet" | null {
  const { scheme } = document.uri;
  if (scheme !== "file" && scheme !== "untitled") return "scheme";
  if (isSecretFile(document.uri.path)) return "secret";
  if (isReadOnly(document)) return "readOnly";
  // Liste de suggestions ouverte : c'est elle qui propose.
  if (context.selectedCompletionInfo) return "selection";
  const editor = vscode.window.activeTextEditor;
  if (editor && editor.document === document) {
    // Curseurs multiples : snippet à plusieurs arrêts liés, ou édition en
    // colonne ; un champ de snippet sélectionné est une sélection.
    const selections = editor.selections ?? [editor.selection];
    if (selections.length > 1) return "snippet";
    if (!editor.selection.isEmpty) return "selection";
  }
  return null;
}

/** Chemin envoyé avec la requête : relatif au dossier ouvert, sinon le seul
 * nom du fichier (jamais un chemin absolu, qui dirait le nom de la session). */
export function requestPath(uri: vscode.Uri): string {
  const relative = vscode.workspace.asRelativePath(uri, false);
  return /^(?:[/\\]|[A-Za-z]:)/.test(relative)
    ? path.posix.basename(uri.path.replaceAll("\\", "/"))
    : relative;
}

export type CompletionDeps = {
  link: LinkLike;
  post(message: HostMessage): void;
  /** Montre le panneau. */
  reveal(): Promise<void>;
  /** Anti-rebond (essais : plus court). */
  debounceMs?: number;
  /** Délai de `complete.fim` côté client (essais : plus court). */
  fimTimeoutMs?: number;
};

type Suggest = { recommended?: unknown; options?: unknown };

const config = () => vscode.workspace.getConfiguration("chatonlm");

const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));

/** Texte d'une pause, pour la barre d'état et les messages. */
export function pauseText(reason: CompletionPause): string {
  switch (reason) {
    case "exclusive":
      return vscode.l10n.t(
        "Paused: the chat model is using the graphics card.",
      );
    case "chatBusy":
      return vscode.l10n.t("Paused while a reply is being written.");
    case "doesNotFit":
      return vscode.l10n.t(
        "Paused: the autocomplete model does not fit in this computer's memory.",
      );
    case "notInstalled":
      return vscode.l10n.t(
        "Paused: the autocomplete model is not installed in ChatonLM.",
      );
    case "notLocal":
      return vscode.l10n.t("Autocomplete needs a model on this computer.");
    case "notLoaded":
      return vscode.l10n.t(
        "Paused: the autocomplete model is not loaded. Prepare it to start.",
      );
  }
}

/** État de l'autocomplétion en une phrase (infobulle, menu). */
export function statusText(view: CompletionView): string {
  const text = (() => {
    switch (view.status) {
      case "off":
        return vscode.l10n.t("Autocomplete is off.");
      case "disconnected":
        return vscode.l10n.t("ChatonLM is not connected.");
      case "notHere":
        return vscode.l10n.t("Autocomplete is not available in this file.");
      case "remoteWindow":
        return vscode.l10n.t(
          "Autocomplete is not available in remote windows: the files are not on this computer.",
        );
      case "otherProvider":
        return vscode.l10n.t(
          "{0} already suggests code in this file, so ChatonLM stays quiet here. To use ChatonLM, turn off {0} for this language, or choose Always suggest with ChatonLM.",
          view.otherProvider ?? "",
        );
      case "notLocal":
        return vscode.l10n.t("Autocomplete needs a model on this computer.");
      case "missing":
        return vscode.l10n.t(
          "{0} is not installed in ChatonLM.",
          view.model ?? "",
        );
      case "checking":
        return vscode.l10n.t("Looking for the autocomplete model…");
      case "paused":
        return pauseText(view.reason ?? "notLoaded");
      case "unusable":
        return view.error ?? vscode.l10n.t("ChatonLM is not connected.");
      case "slow":
        return vscode.l10n.t(
          "Too slow: the last suggestions came too late. Measure the speed, or choose a smaller model.",
        );
      case "ready":
        return vscode.l10n.t("Ready, with {0}.", view.model ?? "");
    }
  })();
  return view.alsoSuggests
    ? `${text} ${vscode.l10n.t("{0} may also suggest code here.", view.alsoSuggests)}`
    : text;
}

/** Verdict d'une mesure, en une phrase. */
export function measureText(result: CompletionMeasure): string {
  const ms = Math.round(result.firstTokenMs);
  const speed = Math.round(result.tokensPerSecond);
  const verdict =
    result.rating === "smooth"
      ? vscode.l10n.t(
          "Smooth: first word in {0} ms, {1} tokens per second.",
          ms,
          speed,
        )
      : result.rating === "usable"
        ? vscode.l10n.t(
            "Usable, a little slow: first word in {0} ms, {1} tokens per second.",
            ms,
            speed,
          )
        : vscode.l10n.t(
            "Too slow here: first word in {0} ms, {1} tokens per second. Choose a smaller model.",
            ms,
            speed,
          );
  return result.partialGpu
    ? `${verdict} ${vscode.l10n.t("Part of the model runs on the processor.")}`
    : verdict;
}

/** Texte précédé d'une icône de VS Code (`$(nom)`). */
const withIcon = (icon: string, text: string) => `$(${icon}) ${text}`;

const ICONS: Record<CompletionView["status"], string> = {
  off: "$(circle-slash)",
  disconnected: "$(debug-disconnect)",
  notHere: "$(circle-slash)",
  remoteWindow: "$(remote)",
  otherProvider: "$(info)",
  notLocal: "$(warning)",
  missing: "$(cloud-download)",
  checking: "$(loading~spin)",
  paused: "$(debug-pause)",
  unusable: "$(error)",
  slow: "$(watch)",
  ready: "$(check)",
};

/** Statuts où une proposition peut être demandée. */
const usable = (status: CompletionView["status"]) =>
  status === "ready" || status === "slow";

type Evaluation = Pick<
  CompletionView,
  "status" | "reason" | "otherProvider" | "alsoSuggests" | "error"
>;

export class Completion implements vscode.InlineCompletionItemProvider {
  readonly #deps: CompletionDeps;
  #plan: GpuPlan | null = null;
  #recommended: string | null = null;
  #options: CompletionOption[] = [];
  /** Refus « en pause » de l'app, jusqu'au prochain plan. */
  #paused: CompletionPause | null = null;
  /** Refus du moteur (-32020 : modèle sans FIM…) : plus de requête avec ce
   * modèle jusqu'à une autre liaison ou une préparation réussie. */
  #failure: { model: string; message: string } | null = null;
  #local: { value: boolean | null; at: number } = { value: null, at: 0 };
  /** `models.details.remote` par modèle. */
  readonly #remote = new Map<string, boolean>();
  #measure: CompletionView["measure"] = null;
  #measured: string | null = null;
  #preparing = false;
  /** La carte a été ouverte dans le panneau (le plan intéresse quelqu'un). */
  #cardOpened = false;
  /** Requête `complete.fim` en vol (numéro), une seule. */
  #inFlight: number | null = null;
  /** Dernière demande de VS Code : une plus ancienne n'envoie rien. */
  #seq = 0;
  /** Délais de garde de suite (premier jeton trop lent, ou pas de réponse). */
  #timeouts = 0;
  /** Dernière préparation demandée par la frappe (retour d'inactivité). */
  #lastWake = Number.NEGATIVE_INFINITY;
  /** Réglages vus en dernier : un changement fait ailleurs (éditeur des
   * réglages, settings.json) a le même effet que la barre d'état. */
  #knownEnabled: boolean;
  #knownSetting: string;
  readonly #cache = new CompletionCache();
  readonly #status: vscode.StatusBarItem;
  readonly #disposables: vscode.Disposable[] = [];
  readonly #unsubscribe: (() => void)[] = [];
  #lastPosted = "";

  constructor(deps: CompletionDeps) {
    this.#deps = deps;
    this.#knownEnabled = this.enabled;
    this.#knownSetting = this.setting;
    this.#status = vscode.window.createStatusBarItem(
      "chatonlm.completion",
      vscode.StatusBarAlignment.Right,
      100,
    );
    this.#status.name = vscode.l10n.t("ChatonLM autocomplete");
    this.#status.command = "chatonlm.completionMenu";
    this.#unsubscribe.push(
      deps.link.onNotification((n) => this.#onNotification(n)),
      deps.link.onConnected(() => void this.#onConnected()),
      deps.link.onState((state) => {
        if (state.connection !== "connected") this.#disconnected();
        this.#update();
      }),
    );
    this.#disposables.push(
      this.#status,
      vscode.workspace.onDidChangeConfiguration((event) => {
        if (!event.affectsConfiguration("chatonlm.completion")) return;
        this.#onSettings();
      }),
      // Une autre complétion en ligne peut suggérer dans ce fichier-ci.
      vscode.window.onDidChangeActiveTextEditor(() => this.#update()),
      // Retour dans la fenêtre : modèle peut-être téléchargé dans l'app.
      vscode.window.onDidChangeWindowState((state) => {
        if (state.focused && this.#watched) void this.refresh();
      }),
    );
    this.#update();
  }

  dispose(): void {
    for (const unsubscribe of this.#unsubscribe) unsubscribe();
    for (const disposable of this.#disposables) disposable.dispose();
    this.#seq++;
  }

  get enabled(): boolean {
    return config().get<boolean>("completion.enabled", false) === true;
  }

  /** Réglage du modèle (`auto` : celui que l'app recommande). */
  get setting(): string {
    const value = config().get<string>("completion.model", "auto");
    return typeof value === "string" && value.trim() ? value.trim() : "auto";
  }

  get model(): string | null {
    return this.setting === "auto" ? this.#recommended : this.setting;
  }

  get #multiline(): boolean {
    return config().get<boolean>("completion.multiline", true) !== false;
  }

  /** « Always suggest with ChatonLM » : les autres complétions en ligne ne
   * sont plus cherchées. */
  get #ignoreOthers(): boolean {
    return config().get<boolean>("completion.ignoreOthers", false) === true;
  }

  /** Quelqu'un regarde le plan : autocomplétion active ou carte montrée. */
  get #watched(): boolean {
    return this.#deps.link.connected && (this.enabled || this.#cardOpened);
  }

  get view(): CompletionView {
    const document = vscode.window.activeTextEditor?.document;
    return {
      enabled: this.enabled,
      ...this.#evaluate(document),
      model: this.model,
      setting: this.setting,
      plan: this.#plan,
      recommended: this.#recommended,
      options: this.#options,
      preparing: this.#preparing,
      measure: this.#measured === this.model ? this.#measure : null,
    };
  }

  #isRemote(model: string): boolean {
    return looksRemote(model) || this.#remote.get(model) === true;
  }

  #evaluate(document: vscode.TextDocument | undefined): Evaluation {
    const at = (
      status: CompletionView["status"],
      more: Partial<Evaluation> = {},
    ): Evaluation => ({
      status,
      reason: null,
      otherProvider: null,
      alsoSuggests: null,
      error: null,
      ...more,
    });
    if (!this.enabled) return at("off");
    if (!this.#deps.link.connected) return at("disconnected");
    const excluded = document ? excludedDocument(document) : null;
    if (excluded) return at(excluded);
    const other =
      document && !this.#ignoreOthers
        ? otherProvider(document.languageId)
        : null;
    if (other?.certain)
      return at("otherProvider", { otherProvider: other.name });
    const also = { alsoSuggests: other?.name ?? null };
    const model = this.model;
    if (this.#local.value === false || (model && this.#isRemote(model))) {
      return at("notLocal", also);
    }
    if (!model) return at("checking", also);
    const option = this.#options.find((o) => sameModel(o.model, model));
    if (option && !option.installed) return at("missing", also);
    if (this.#failure && sameModel(this.#failure.model, model)) {
      return at("unusable", { ...also, error: this.#failure.message });
    }
    if (this.#paused) return at("paused", { ...also, reason: this.#paused });
    // Raison du plan (absente : prête) : pause pendant une réponse, etc.
    const planReason = this.#plan?.reason;
    if (planReason === "notLocal") return at("notLocal", also);
    if (planReason) return at("paused", { ...also, reason: planReason });
    if (this.#plan?.chatBusy) {
      return at("paused", { ...also, reason: "chatBusy" });
    }
    const slot = this.#plan?.completion;
    if (!slot || !sameModel(slot.model, model) || !slot.loaded) {
      const reason: CompletionPause =
        this.#plan?.mode === "doesNotFit"
          ? "doesNotFit"
          : this.#plan?.mode === "exclusive" && this.#plan.chat?.loaded
            ? "exclusive"
            : "notLoaded";
      return at("paused", { ...also, reason });
    }
    if (this.#local.value !== true) return at("checking", also);
    return at(this.#timeouts >= SLOW_AFTER ? "slow" : "ready", also);
  }

  #update(): void {
    const view = this.view;
    const visible = view.enabled || this.#deps.link.connected;
    if (visible) {
      const name = vscode.l10n.t("ChatonLM autocomplete");
      const text = statusText(view);
      this.#status.text = `${ICONS[view.status]} ${name}`;
      this.#status.tooltip = `${text}\n${vscode.l10n.t("Click for autocomplete options.")}`;
      // L'état n'est sinon que dans l'icône : un lecteur d'écran le lit ici.
      this.#status.accessibilityInformation = {
        label: vscode.l10n.t("ChatonLM autocomplete: {0}", text),
      };
      this.#status.show();
    } else {
      this.#status.hide();
    }
    const serialized = JSON.stringify(view);
    if (serialized === this.#lastPosted) return;
    this.#lastPosted = serialized;
    this.#deps.post({ type: "completion", completion: view });
  }

  /** Envoie l'état au panneau (ouvert ou rechargé). */
  postView(): void {
    this.#lastPosted = "";
    this.#update();
  }

  /** Réglages changés : par la barre d'état ou le panneau (déjà suivis),
   * ou ailleurs (éditeur des réglages, settings.json) : même effet. */
  #onSettings(): void {
    const enabled = this.enabled;
    const setting = this.setting;
    const turnedOn = enabled && !this.#knownEnabled;
    const turnedOff = !enabled && this.#knownEnabled;
    const modelChanged = setting !== this.#knownSetting;
    this.#knownEnabled = enabled;
    this.#knownSetting = setting;
    // Autre modèle : sa mesure ne vaut plus, son plan sera relu.
    if (this.#measured !== this.model) this.#measure = null;
    this.#update();
    if (turnedOn) void this.#activate();
    else if (turnedOff) this.#stopInFlight();
    else if (modelChanged) void this.#modelChanged(null);
  }

  #stopInFlight(): void {
    if (this.#inFlight === null) return;
    this.#deps.link.cancel(this.#inFlight);
    this.#inFlight = null;
  }

  #disconnected(): void {
    this.#plan = null;
    this.#paused = null;
    this.#failure = null;
    this.#inFlight = null;
    this.#timeouts = 0;
    this.#local = { value: null, at: 0 };
    this.#preparing = false;
    if (this.#measure?.state === "running") this.#measure = null;
    this.#seq++;
  }

  async #onConnected(): Promise<void> {
    if (this.#watched) await this.refresh();
    else this.#update();
  }

  #onNotification({ method, params }: Notification): void {
    if (method !== "gpu.changed") return;
    const plan = (params as { plan?: GpuPlan } | undefined)?.plan;
    if (!plan || typeof plan !== "object") return;
    this.#plan = plan;
    // Nouveau plan : une requête peut de nouveau partir (une seule, si
    // l'app répond encore « en pause »).
    this.#paused = null;
    this.#update();
  }

  /** Plan, propositions et serveur relus (connexion, activation, carte). */
  async refresh(): Promise<void> {
    if (!this.#deps.link.connected) {
      this.#update();
      return;
    }
    const link = this.#deps.link;
    const [suggest] = await Promise.allSettled([
      link.request<Suggest>("completion.suggest"),
      this.#readLocal(),
    ]);
    if (suggest.status === "fulfilled" && suggest.value) {
      const { recommended, options } = suggest.value;
      this.#recommended = typeof recommended === "string" ? recommended : null;
      this.#options = Array.isArray(options)
        ? (options as CompletionOption[]).filter(
            (o) => typeof o?.model === "string",
          )
        : [];
    }
    // Plan calculé pour le modèle de cet éditeur (l'app le retient).
    const chosen = this.model;
    const [plan] = await Promise.allSettled([
      link.request<GpuPlan>("gpu.status", chosen ? { model: chosen } : {}),
    ]);
    if (plan.status === "fulfilled" && plan.value) {
      // Pause levée seulement par un plan qui a changé (comme `gpu.changed`).
      if (JSON.stringify(plan.value) !== JSON.stringify(this.#plan)) {
        this.#paused = null;
      }
      this.#plan = plan.value;
    }
    const model = this.model;
    if (model && !this.#remote.has(model) && !looksRemote(model)) {
      try {
        const details = await link.request<{ remote?: unknown }>(
          "models.details",
          { model },
        );
        if (typeof details?.remote === "boolean") {
          this.#remote.set(model, details.remote);
        }
      } catch {
        // Pas installé : l'app le dira à la préparation.
      }
    }
    this.#update();
  }

  async #readLocal(): Promise<void> {
    try {
      const app = await this.#deps.link.request<{ local?: unknown }>(
        "app.state",
      );
      this.#local = {
        value: typeof app?.local === "boolean" ? app.local : null,
        at: Date.now(),
      };
    } catch {
      this.#local = { value: null, at: Date.now() };
    }
  }

  #pause(reason: CompletionPause): void {
    this.#paused = reason;
    // Serveur passé sur une autre machine : la barre le dit tout de suite.
    if (reason === "notLocal") this.#local = { value: false, at: Date.now() };
    this.#update();
  }

  /**
   * Retour d'inactivité (principe 1, décidé par le chef) : modèle évincé
   * (10 min sans frappe, ou VS Code rouvert), le plan dit que les deux
   * tiennent ensemble et aucune réponse ne s'écrit : la frappe redemande la
   * préparation, au plus une fois toutes les `WAKE_INTERVAL_MS`. Jamais en
   * « un à la fois », « ne tient pas » ni « inconnu » : là, « Prepare » reste
   * un geste de l'utilisateur. L'app refuse de toute façon ce qui ne tient pas.
   */
  #wake(reason: CompletionPause | null): void {
    const plan = this.#plan;
    if (reason !== "notLoaded" || !plan || this.#preparing) return;
    if (plan.reason !== "notLoaded" || plan.mode !== "together") return;
    if (plan.chatBusy) return;
    const now = Date.now();
    if (now - this.#lastWake < WAKE_INTERVAL_MS) return;
    this.#lastWake = now;
    void this.prepare(false, true);
  }

  async provideInlineCompletionItems(
    document: vscode.TextDocument,
    position: vscode.Position,
    context: vscode.InlineCompletionContext,
    token: vscode.CancellationToken,
  ): Promise<vscode.InlineCompletionItem[]> {
    if (!this.enabled || skipReason(document, context)) return [];
    const state = this.#evaluate(document);
    if (state.status === "paused") this.#wake(state.reason);
    if (!usable(state.status)) return [];
    const model = this.model;
    if (!model) return [];
    const multiline = this.#multiline;
    const { prefix, suffix, restOfLineEmpty } = promptWindow(
      document,
      position,
    );
    const lines = multiline && restOfLineEmpty;
    const item = (text: string) =>
      text
        ? [
            new vscode.InlineCompletionItem(
              text,
              new vscode.Range(position, position),
            ),
          ]
        : [];
    const cached = this.#cache.get(model, lines, prefix, suffix);
    if (cached !== undefined) return item(cached);

    const seq = ++this.#seq;
    // Serveur sur cette machine, relu s'il date (réglage changé dans l'app),
    // pendant l'anti-rebond : pas d'aller-retour de plus avant la requête.
    const local =
      Date.now() - this.#local.at > LOCAL_TTL_MS
        ? this.#readLocal().then(() => this.#update())
        : null;
    const debounce = this.#deps.debounceMs ?? DEBOUNCE_MS;
    await sleep(
      state.status === "slow" ? debounce * SLOW_DEBOUNCE_FACTOR : debounce,
    );
    if (local) await local;
    if (token.isCancellationRequested || seq !== this.#seq) return [];
    // Tout a pu changer pendant l'attente (plan, pause, réglage).
    if (!usable(this.#evaluate(document).status) || this.model !== model) {
      return [];
    }

    const link = this.#deps.link;
    this.#stopInFlight();
    const call = link.call<{ text?: unknown; timedOut?: unknown }>(
      "complete.fim",
      {
        model,
        prefix,
        suffix,
        language: document.languageId,
        path: requestPath(document.uri),
        // Envoyé toujours : l'app suppose vrai sans lui.
        multiline: lines,
      },
      this.#deps.fimTimeoutMs ?? FIM_TIMEOUT_MS,
    );
    this.#inFlight = call.id;
    const cancel = token.onCancellationRequested(() => {
      if (this.#inFlight !== call.id) return;
      this.#inFlight = null;
      link.cancel(call.id);
    });
    try {
      const answer = await call.result;
      // Premier jeton trop lent : rien à proposer, pas une panne ; rien
      // n'est gardé (la frappe suivante redemande).
      if (answer?.timedOut === true) {
        this.#timedOut();
        return [];
      }
      this.#answered();
      const text = typeof answer?.text === "string" ? answer.text : "";
      this.#cache.set(model, lines, prefix, suffix, text);
      return token.isCancellationRequested ? [] : item(text);
    } catch (error) {
      if (error instanceof TimeoutError) {
        // Pas de réponse : l'app arrête la requête, elle est oubliée ici.
        link.cancel(call.id);
        this.#timedOut();
      } else if (
        error instanceof RpcError &&
        error.code === CompletionCodes.paused
      ) {
        this.#pause(pauseReason(error.data));
      } else if (
        error instanceof RpcError &&
        error.code === CompletionCodes.refused
      ) {
        this.#failure = {
          model,
          message: error.appMessage ?? error.message,
        };
        this.#update();
      }
      // -32800 (remplacée ou annulée), liaison perdue : rien à dire.
      return [];
    } finally {
      cancel.dispose();
      if (this.#inFlight === call.id) this.#inFlight = null;
    }
  }

  /** Délai de garde : dit « trop lent » après `SLOW_AFTER` de suite. */
  #timedOut(): void {
    this.#timeouts++;
    if (this.#timeouts === SLOW_AFTER) this.#update();
  }

  #answered(): void {
    const wasSlow = this.#timeouts >= SLOW_AFTER;
    this.#timeouts = 0;
    if (wasSlow) this.#update();
  }

  /** Commande du panneau. */
  async command(command: CompletionCommand): Promise<void> {
    switch (command) {
      case "enable":
        return this.setEnabled(true);
      case "disable":
        return this.setEnabled(false);
      case "prepare":
        return this.prepare(false);
      case "measure":
        return this.measure(false);
      case "refresh":
        this.#cardOpened = true;
        return this.refresh();
      case "ignoreOthers":
        return this.ignoreOthers();
    }
  }

  /** Active (puis prépare le modèle : geste explicite) ou coupe. */
  async setEnabled(on: boolean): Promise<void> {
    // Suivi ici : l'évènement de configuration ne le refait pas.
    this.#knownEnabled = on;
    this.#knownSetting = this.setting;
    await config().update(
      "completion.enabled",
      on,
      vscode.ConfigurationTarget.Global,
    );
    this.#update();
    if (!on) {
      this.#stopInFlight();
      return;
    }
    await this.#activate();
  }

  /** Autocomplétion allumée (barre d'état, panneau ou réglages) : plan relu,
   * puis préparation du modèle si elle peut réussir, sinon la raison. */
  async #activate(): Promise<void> {
    if (!this.#deps.link.connected) {
      void vscode.window.showWarningMessage(
        vscode.l10n.t("ChatonLM is not connected. See the ChatonLM panel."),
      );
      return;
    }
    await this.refresh();
    const view = this.view;
    if (view.status === "missing" && view.model) {
      await this.#offerInstall(view.model);
      return;
    }
    if (view.status === "notLocal") {
      void vscode.window.showWarningMessage(
        vscode.l10n.t("Autocomplete needs a model on this computer."),
      );
      return;
    }
    if (canPrepare(view)) await this.prepare(true);
    else if (view.status === "paused" && view.reason) {
      void vscode.window.showWarningMessage(pauseText(view.reason));
    }
  }

  /** « Always suggest with ChatonLM ». */
  async ignoreOthers(): Promise<void> {
    await config().update(
      "completion.ignoreOthers",
      true,
      vscode.ConfigurationTarget.Global,
    );
    this.#update();
  }

  /**
   * Charge le modèle si le plan le permet (`completion.prepare`). `notify` :
   * refus dit par un avis (menu) ; `quiet` : retour d'inactivité, aucun avis
   * même en cas d'échec (l'état de la barre et de la carte le dit).
   */
  async prepare(notify: boolean, quiet = false): Promise<void> {
    const model = this.model;
    if (!model || !this.#deps.link.connected || this.#preparing) return;
    if (this.#isRemote(model) || this.#local.value === false) {
      if (notify) {
        void vscode.window.showWarningMessage(
          vscode.l10n.t("Autocomplete needs a model on this computer."),
        );
      }
      return;
    }
    this.#preparing = true;
    this.#update();
    try {
      const answer = await this.#deps.link.request<{ plan?: GpuPlan }>(
        "completion.prepare",
        { model },
      );
      if (answer?.plan) this.#plan = answer.plan;
      this.#paused = null;
      this.#failure = null;
      this.#timeouts = 0;
    } catch (error) {
      if (error instanceof RpcError && error.code === CompletionCodes.paused) {
        const reason = pauseReason(error.data);
        this.#paused = reason;
        if (notify) void vscode.window.showWarningMessage(pauseText(reason));
      } else if (error instanceof RpcError && !quiet) {
        void vscode.window.showErrorMessage(
          vscode.l10n.t(
            "Couldn't prepare the autocomplete model: {0}",
            error.appMessage ?? error.message,
          ),
        );
      }
    } finally {
      this.#preparing = false;
      this.#update();
    }
  }

  /** Mesure la vitesse du modèle chargé (`completion.measure`). */
  async measure(notify: boolean): Promise<void> {
    const model = this.model;
    if (!model || !this.#deps.link.connected) return;
    if (this.#measured === model && this.#measure?.state === "running") return;
    this.#measured = model;
    this.#measure = { state: "running" };
    this.#update();
    try {
      const result = await this.#deps.link.request<CompletionMeasure>(
        "completion.measure",
        { model },
      );
      if (this.#measured !== model) return;
      this.#measure = { state: "done", result };
      if (notify)
        void vscode.window.showInformationMessage(measureText(result));
    } catch (error) {
      if (this.#measured !== model) return;
      const message =
        error instanceof RpcError && error.code === CompletionCodes.paused
          ? pauseText(pauseReason(error.data))
          : error instanceof RpcError
            ? (error.appMessage ?? error.message)
            : vscode.l10n.t("ChatonLM is not connected.");
      this.#measure = { state: "failed", message };
      if (notify) void vscode.window.showWarningMessage(message);
    } finally {
      this.#update();
    }
  }

  /** Modèle choisi (le recommandé redevient `auto` : il suit la machine). */
  async setModel(model: string): Promise<void> {
    const value =
      this.#recommended && sameModel(model, this.#recommended) ? "auto" : model;
    // Suivi ici : l'évènement de configuration ne le refait pas.
    this.#knownSetting = value;
    this.#knownEnabled = this.enabled;
    await config().update(
      "completion.model",
      value,
      vscode.ConfigurationTarget.Global,
    );
    await this.#modelChanged(model);
  }

  /**
   * Autre modèle : plan relu (et préparé si l'autocomplétion est active et
   * que ça peut réussir). `chosen` : choisi dans le menu ou le panneau (un
   * modèle absent est proposé à l'installation, un refus est dit) ; null :
   * changé dans les réglages (rien n'est montré, le texte peut être en cours
   * de frappe).
   */
  async #modelChanged(chosen: string | null): Promise<void> {
    this.#paused = null;
    this.#timeouts = 0;
    if (this.#measured !== this.model) this.#measure = null;
    if (chosen === null && !this.#watched) {
      this.#update();
      return;
    }
    await this.refresh();
    const view = this.view;
    if (chosen && view.status === "missing") await this.#offerInstall(chosen);
    else if (canPrepare(view)) await this.prepare(chosen !== null);
  }

  /** « Install in ChatonLM » : l'app montre sa proposition de téléchargement
   * (seulement un modèle proposé par `completion.suggest`). */
  async offerPull(model: string): Promise<void> {
    if (!this.#options.some((o) => sameModel(o.model, model))) return;
    try {
      const answer = await this.#deps.link.request<{ installed?: unknown }>(
        "models.offerPull",
        { model },
      );
      // Déjà installé (téléchargé entre-temps) : la liste est relue.
      if (answer?.installed === true) await this.refresh();
    } catch (error) {
      if (error instanceof RpcError && error.code === CompletionCodes.paused) {
        void vscode.window.showWarningMessage(
          pauseText(pauseReason(error.data)),
        );
        return;
      }
      void vscode.window.showErrorMessage(
        error instanceof RpcError
          ? (error.appMessage ?? error.message)
          : vscode.l10n.t("ChatonLM is not connected."),
      );
    }
  }

  async #offerInstall(model: string): Promise<void> {
    const install = vscode.l10n.t("Install in ChatonLM");
    const choice = await vscode.window.showInformationMessage(
      vscode.l10n.t("{0} is not installed in ChatonLM.", model),
      install,
    );
    if (choice === install) await this.offerPull(model);
  }

  /** Montre la carte « Graphics card » dans le panneau : demande ponctuelle
   * (un panneau recréé plus tard ne la rouvre pas tout seul). */
  async showCard(): Promise<void> {
    this.#cardOpened = true;
    await this.#deps.reveal();
    this.#deps.post({ type: "revealGpu" });
    await this.refresh();
    this.#update();
  }

  /** Menu de la barre d'état. */
  async menu(): Promise<void> {
    if (this.#deps.link.connected && this.#options.length === 0) {
      await this.refresh();
    }
    const view = this.view;
    type Item = vscode.QuickPickItem & { run(): Promise<void> };
    const items: Item[] = [];
    items.push(
      view.enabled
        ? {
            label: withIcon(
              "circle-slash",
              vscode.l10n.t("Turn off autocomplete"),
            ),
            run: () => this.setEnabled(false),
          }
        : {
            label: withIcon("check", vscode.l10n.t("Turn on autocomplete")),
            run: () => this.setEnabled(true),
          },
    );
    if (view.status === "missing" && view.model) {
      const model = view.model;
      items.push({
        label: withIcon(
          "cloud-download",
          vscode.l10n.t("Install {0} in ChatonLM", model),
        ),
        run: () => this.offerPull(model),
      });
    }
    // Seulement quand la préparation peut réussir (pas « ne tient pas »…).
    if (canPrepare(view)) {
      items.push({
        label: withIcon("play", vscode.l10n.t("Prepare the model")),
        description: view.model ?? undefined,
        run: () => this.prepare(true),
      });
    }
    if (usable(view.status)) {
      items.push({
        label: withIcon("dashboard", vscode.l10n.t("Measure speed")),
        description: view.model ?? undefined,
        run: () => this.measure(true),
      });
    }
    const other = view.otherProvider ?? view.alsoSuggests;
    if (other) {
      items.push({
        label: withIcon(
          "check-all",
          vscode.l10n.t("Always suggest with ChatonLM"),
        ),
        detail:
          view.status === "otherProvider"
            ? vscode.l10n.t("{0} already suggests code in this file.", other)
            : vscode.l10n.t("{0} may also suggest code here.", other),
        run: () => this.ignoreOthers(),
      });
    }
    items.push(
      {
        label: withIcon("list-selection", vscode.l10n.t("Choose the model…")),
        description: view.model ?? undefined,
        run: () => this.chooseModel(),
      },
      {
        label: withIcon(
          "chip",
          vscode.l10n.t("Show the graphics card in the panel"),
        ),
        run: () => this.showCard(),
      },
    );
    const pick = await vscode.window.showQuickPick(items, {
      title: vscode.l10n.t("ChatonLM autocomplete"),
      placeHolder: statusText(view),
    });
    await pick?.run();
  }

  /** Choix du modèle parmi ceux que l'app propose. */
  async chooseModel(): Promise<void> {
    if (this.#deps.link.connected) await this.refresh();
    if (this.#options.length === 0) {
      void vscode.window.showWarningMessage(
        vscode.l10n.t("ChatonLM is not connected. See the ChatonLM panel."),
      );
      return;
    }
    const current = this.model;
    const items = this.#options.map((option) => {
      const notes = [
        option.license,
        option.commercialUse ? null : vscode.l10n.t("Non-commercial use only"),
        option.fits === false
          ? vscode.l10n.t("May not fit on this computer")
          : null,
      ].filter((note): note is string => note !== null);
      const tags = [
        option.model === this.#recommended
          ? vscode.l10n.t("recommended")
          : null,
        option.installed ? null : vscode.l10n.t("not installed"),
      ].filter((tag): tag is string => tag !== null);
      return {
        label:
          current && sameModel(current, option.model)
            ? withIcon("check", option.model)
            : option.model,
        description: tags.join(" · ") || undefined,
        detail: notes.join(" · "),
        model: option.model,
      };
    });
    const pick = await vscode.window.showQuickPick(items, {
      title: vscode.l10n.t("Autocomplete model"),
    });
    if (pick) await this.setModel(pick.model);
  }
}
