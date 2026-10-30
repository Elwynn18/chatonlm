// Cours de l'Étude sur le projet ouvert ou sur un langage (lot 4a) : fichiers
// choisis par l'utilisateur dans une liste (README, manifeste, point d'entrée,
// fichiers les plus importés), toujours filtrés comme un envoi hors de cette
// machine (fichiers sensibles exclus, lignes à secret masquées : le modèle de
// l'Étude se choisit dans l'app, sa destination n'est pas connue ici), puis
// `study.create` ; la progression vient de `study.event`, le cours vit dans
// l'onglet Study de l'app. Contrat : .tools/research/93-lot4a-contract.md (2).
import path from "node:path";
import * as vscode from "vscode";
import type { HostMessage, StudyView } from "../shared/protocol";
import { fenceLanguage } from "./actions";
import { type Notification, RpcError } from "./bridge/rpc";
import type { LinkLike } from "./controller";
import { filterPieces, isSecretFile, type Piece } from "./receipt";

/** Limites du moteur de l'Étude (`study/mod.rs`, `sources.rs`). */
export const MAX_DOCUMENTS = 3;
export const MAX_STUDY_CHARS = 200_000;
/** Un fichier plus gros n'est pas proposé (texte rarement utile au cours). */
export const MAX_CANDIDATE_CHARS = 100_000;
/** Fichiers lus pour trouver les plus importés. */
const MAX_SCANNED = 400;
const SCAN_FILE_CHARS = 200_000;
/** Durée visée (minutes) : celle par défaut de l'onglet Study. */
export const COURSE_MINUTES = 20;
/** Relecture (`study.list`) d'un cours en préparation : filet si un
 * `study.event` se perd (reconnexion, rapport 94, M1). */
export const PLANNING_POLL_MS = 5_000;

/** Langue du cours : celle de VS Code si l'app la connaît (en, es, fr),
 * sinon rien (l'app prend la sienne). */
export function courseLanguage(vscodeLanguage: string): string | undefined {
  const base = vscodeLanguage.toLowerCase().split(/[-_]/)[0];
  return base === "en" || base === "es" || base === "fr" ? base : undefined;
}

const EXCLUDE =
  "**/{node_modules,dist,build,target,out,vendor,.git,.venv,venv,__pycache__,.next,coverage}/**";
const SOURCE_GLOB =
  "**/*.{ts,tsx,js,jsx,mjs,cjs,py,rs,go,java,kt,cs,rb,php,swift,dart,c,cc,cpp,h,hpp}";

const MANIFESTS = new Set(
  [
    "package.json",
    "cargo.toml",
    "pyproject.toml",
    "setup.py",
    "go.mod",
    "pom.xml",
    "build.gradle",
    "build.gradle.kts",
    "composer.json",
    "gemfile",
    "cmakelists.txt",
    "makefile",
    "deno.json",
    "pubspec.yaml",
    "mix.exs",
    "package.swift",
  ].map((name) => name.toLowerCase()),
);
const README = /^readme(\.(md|markdown|txt|rst|adoc))?$/i;
const ENTRY =
  /^(?:(?:src|lib|app|cmd\/[^/]+)\/)?(main|index|app|lib|program|server|cli|__main__)\.(ts|tsx|js|jsx|mjs|py|rs|go|java|kt|cs|rb|php|swift|dart|c|cpp)$/i;

export type Candidate = {
  /** Chemin relatif (séparateur « / »). */
  path: string;
  reason: "readme" | "manifest" | "entry" | "imported";
  picked: boolean;
};

const SOURCE_EXTENSIONS = [
  ".ts",
  ".tsx",
  ".js",
  ".jsx",
  ".mjs",
  ".cjs",
  ".py",
  ".rs",
  ".go",
];

/** Imports relatifs d'un fichier, résolus en chemins sans extension. */
function relativeImports(file: string, text: string): string[] {
  const dir = path.posix.dirname(file);
  const found: string[] = [];
  const js =
    /(?:\bfrom\s*|\bimport\s*\(?\s*|\brequire\s*\(\s*)["'](\.{1,2}\/[^"']+)["']/g;
  for (const match of text.matchAll(js)) {
    found.push(path.posix.join(dir, match[1]));
  }
  // Python : from .module import x ; from ..pkg.module import y
  for (const match of text.matchAll(/^\s*from\s+(\.+)([\w.]*)\s+import\b/gm)) {
    let base = dir;
    for (let i = 1; i < match[1].length; i++) base = path.posix.dirname(base);
    if (match[2]) found.push(path.posix.join(base, ...match[2].split(".")));
  }
  // Rust : mod name; (fichier voisin)
  if (file.endsWith(".rs")) {
    const parent = /(^|\/)(main|lib|mod)\.rs$/.test(file)
      ? dir
      : path.posix.join(dir, path.posix.basename(file, ".rs"));
    for (const match of text.matchAll(/^\s*(?:pub\s+)?mod\s+(\w+)\s*;/gm)) {
      found.push(path.posix.join(parent, match[1]));
    }
  }
  return found;
}

/**
 * Fichiers les plus importés par les autres (imports relatifs JS/TS,
 * Python, modules Rust), du plus au moins importé ; au moins deux fois.
 */
export function rankImports(files: { path: string; text: string }[]): string[] {
  const known = new Map<string, string>();
  for (const { path: file } of files) {
    const ext = path.posix.extname(file);
    const bare = file.slice(0, file.length - ext.length);
    known.set(bare, file);
    if (/\/(index|mod|__init__)$/.test(bare)) {
      known.set(bare.replace(/\/(index|mod|__init__)$/, ""), file);
    }
  }
  const counts = new Map<string, number>();
  for (const { path: file, text } of files) {
    const seen = new Set<string>();
    for (const target of relativeImports(file, text)) {
      const ext = path.posix.extname(target);
      const bare = SOURCE_EXTENSIONS.includes(ext)
        ? target.slice(0, target.length - ext.length)
        : target;
      const resolved = known.get(bare);
      if (!resolved || resolved === file || seen.has(resolved)) continue;
      seen.add(resolved);
      counts.set(resolved, (counts.get(resolved) ?? 0) + 1);
    }
  }
  return [...counts]
    .filter(([, count]) => count >= 2)
    .sort((a, b) => b[1] - a[1] || a[0].localeCompare(b[0]))
    .map(([file]) => file);
}

/**
 * Fichiers proposés pour le cours : README et manifeste de la racine, points
 * d'entrée, puis les plus importés ; jamais un fichier sensible.
 */
export function projectCandidates(
  paths: string[],
  imported: string[],
): Candidate[] {
  const safe = paths.filter((p) => !isSecretFile(p));
  const out: Candidate[] = [];
  const add = (p: string, reason: Candidate["reason"], picked: boolean) => {
    if (!out.some((c) => c.path === p)) out.push({ path: p, reason, picked });
  };
  const root = safe.filter((p) => !p.includes("/")).sort();
  for (const p of root.filter((p) => README.test(p)).slice(0, 1)) {
    add(p, "readme", true);
  }
  root
    .filter((p) => MANIFESTS.has(p.toLowerCase()) || /\.csproj$/i.test(p))
    .slice(0, 2)
    .forEach((p, i) => add(p, "manifest", i === 0));
  safe
    .filter((p) => ENTRY.test(p))
    .sort(
      (a, b) => a.split("/").length - b.split("/").length || a.localeCompare(b),
    )
    .slice(0, 3)
    .forEach((p, i) => add(p, "entry", i < 2));
  imported
    .filter((p) => safe.includes(p))
    .slice(0, 4)
    .forEach((p, i) => add(p, "imported", i < 2));
  return out;
}

export type StudyDocument = { name: string; text: string };

/**
 * Documents envoyés : le README seul (texte du projet), les autres fichiers
 * réunis dans un document (chaque fichier sous son chemin, en bloc de code) :
 * le moteur prend au plus trois documents.
 */
export function buildDocuments(
  pieces: Piece[],
  sourcesName: string,
): StudyDocument[] {
  const documents: StudyDocument[] = [];
  const sources: string[] = [];
  for (const piece of pieces) {
    const name = piece.path ?? piece.label;
    if (README.test(path.posix.basename(name)) && documents.length === 0) {
      documents.push({ name, text: piece.text });
      continue;
    }
    const fence = piece.text.includes("```") ? "````" : "```";
    sources.push(
      `### ${name}\n\n${fence}${piece.language ?? ""}\n${piece.text}\n${fence}`,
    );
  }
  if (sources.length > 0) {
    documents.push({ name: sourcesName, text: sources.join("\n\n") });
  }
  return documents.slice(0, MAX_DOCUMENTS);
}

/** Caractères de tous les documents. */
export const documentsChars = (documents: StudyDocument[]) =>
  documents.reduce((sum, d) => sum + d.name.length + d.text.length, 0);

/** Nom d'un langage d'après son identifiant VS Code. */
export function languageName(languageId: string): string {
  const names: Record<string, string> = {
    typescript: "TypeScript",
    typescriptreact: "TypeScript (React)",
    javascript: "JavaScript",
    javascriptreact: "JavaScript (React)",
    python: "Python",
    rust: "Rust",
    go: "Go",
    java: "Java",
    kotlin: "Kotlin",
    csharp: "C#",
    cpp: "C++",
    c: "C",
    php: "PHP",
    ruby: "Ruby",
    swift: "Swift",
    dart: "Dart",
    shellscript: "Shell",
    powershell: "PowerShell",
    sql: "SQL",
    html: "HTML",
    css: "CSS",
    scss: "SCSS",
    lua: "Lua",
    haskell: "Haskell",
    elixir: "Elixir",
    r: "R",
    markdown: "Markdown",
  };
  return (
    names[languageId] ??
    languageId.charAt(0).toUpperCase() + languageId.slice(1)
  );
}

/** Texte lisible d'un fichier (pas un binaire), ou null. */
function decode(bytes: Uint8Array): string | null {
  if (bytes.includes(0)) return null;
  return new TextDecoder("utf-8").decode(bytes);
}

export type StudyDeps = {
  link: LinkLike;
  post(message: HostMessage): void;
  reveal(): Promise<void>;
  /** Ouvre l'app sur une page. */
  show(route: string): Promise<void>;
  /** Délai de relecture d'un cours en préparation (essais). */
  pollMs?: number;
};

type CourseSummaryLike = {
  id?: unknown;
  status?: unknown;
  levelsReady?: unknown;
  levelsTotal?: unknown;
};

export class Study {
  readonly #deps: StudyDeps;
  #view: StudyView | null = null;
  #ids = 0;
  #poll: ReturnType<typeof setInterval> | null = null;
  #disposers: (() => void)[] = [];

  constructor(deps: StudyDeps) {
    this.#deps = deps;
    // Reconnexion : les `study.event` perdus entre-temps sont rattrapés.
    this.#disposers.push(deps.link.onConnected(() => this.resync()));
  }

  dispose(): void {
    for (const dispose of this.#disposers) dispose();
    this.#disposers = [];
    this.#stopPolling();
  }

  get view(): StudyView | null {
    return this.#view;
  }

  /** Cours en préparation relu (panneau rouvert, liaison revenue). */
  resync(): void {
    const view = this.#view;
    if (view?.status === "planning" && view.courseId) {
      void this.#refresh(view.courseId);
    }
  }

  #set(view: StudyView | null): void {
    this.#view = view;
    this.#deps.post({ type: "study", study: view });
    // Tant que le plan se prépare, relu régulièrement : l'app envoie
    // `study.event`, mais un évènement perdu ne figerait pas la carte.
    if (view?.status === "planning" && view.courseId) this.#startPolling();
    else this.#stopPolling();
  }

  #startPolling(): void {
    if (this.#poll) return;
    this.#poll = setInterval(() => {
      if (this.#deps.link.connected) this.resync();
    }, this.#deps.pollMs ?? PLANNING_POLL_MS);
  }

  #stopPolling(): void {
    if (this.#poll) clearInterval(this.#poll);
    this.#poll = null;
  }

  #update(id: number, change: Partial<StudyView>): void {
    if (this.#view?.id !== id) return;
    this.#set({ ...this.#view, ...change });
  }

  #connected(): boolean {
    if (this.#deps.link.connected) return true;
    void this.#deps.reveal();
    void vscode.window.showWarningMessage(
      vscode.l10n.t("ChatonLM is not connected. See the ChatonLM panel."),
    );
    return false;
  }

  /** « Make a course about this project ». */
  async fromProject(): Promise<void> {
    if (!this.#connected()) return;
    const folder =
      (vscode.window.activeTextEditor &&
        vscode.workspace.getWorkspaceFolder(
          vscode.window.activeTextEditor.document.uri,
        )) ??
      vscode.workspace.workspaceFolders?.[0];
    if (!folder) {
      void vscode.window.showInformationMessage(
        vscode.l10n.t("Open a folder first, then run this command again."),
      );
      return;
    }
    const candidates = await this.#candidates(folder.uri);
    if (candidates.length === 0) {
      void vscode.window.showInformationMessage(
        vscode.l10n.t("ChatonLM found no file to make a course from here."),
      );
      return;
    }
    const reasons: Record<Candidate["reason"], string> = {
      readme: vscode.l10n.t("Project description"),
      manifest: vscode.l10n.t("Project manifest"),
      entry: vscode.l10n.t("Entry point"),
      imported: vscode.l10n.t("Imported by many files"),
    };
    const picked = await vscode.window.showQuickPick(
      candidates.map((c) => ({
        label: c.path,
        description: reasons[c.reason],
        picked: c.picked,
      })),
      {
        canPickMany: true,
        title: vscode.l10n.t("Files for the course"),
        placeHolder: vscode.l10n.t(
          "Check the files the course is built from. Secrets are removed before sending.",
        ),
        ignoreFocusOut: true,
      },
    );
    if (!picked || picked.length === 0) return;
    const pieces: Piece[] = [];
    for (const item of picked) {
      const uri = vscode.Uri.joinPath(folder.uri, item.label);
      let text: string | null;
      try {
        text = decode(await vscode.workspace.fs.readFile(uri));
      } catch {
        text = null;
      }
      if (text === null) continue;
      pieces.push({
        kind: "file",
        label: item.label,
        path: item.label,
        text,
        language: fenceLanguage(path.posix.extname(item.label).slice(1)),
      });
    }
    const topic = await vscode.window.showInputBox({
      title: vscode.l10n.t("Course topic"),
      prompt: vscode.l10n.t("What should the course teach?"),
      value: vscode.l10n.t("How the {0} project works", folder.name),
      ignoreFocusOut: true,
    });
    if (topic === undefined) return;
    await this.#create(
      "project",
      topic.trim() || folder.name,
      pieces,
      vscode.l10n.t("{0} — project files", folder.name),
    );
  }

  /** « Make a course about this language » : le fichier actif sert d'exemple. */
  async fromLanguage(): Promise<void> {
    if (!this.#connected()) return;
    const editor = vscode.window.activeTextEditor;
    if (!editor) {
      void vscode.window.showInformationMessage(
        vscode.l10n.t("Open a file first, then run this command again."),
      );
      return;
    }
    const { document, selection } = editor;
    const name = languageName(document.languageId);
    const file = vscode.workspace.asRelativePath(document.uri);
    const text = selection.isEmpty
      ? document.getText()
      : document.getText(selection);
    const pieces: Piece[] = text.trim()
      ? [
          {
            kind: selection.isEmpty ? "file" : "selection",
            label: file,
            path: file,
            text,
            language: fenceLanguage(document.languageId),
          },
        ]
      : [];
    const topic = await vscode.window.showInputBox({
      title: vscode.l10n.t("Course topic"),
      prompt: vscode.l10n.t("What should the course teach?"),
      value: vscode.l10n.t("The {0} language", name),
      ignoreFocusOut: true,
    });
    if (topic === undefined) return;
    await this.#create(
      "language",
      topic.trim() || name,
      pieces,
      vscode.l10n.t("Example in {0}", name),
    );
  }

  async #create(
    kind: StudyView["kind"],
    topic: string,
    raw: Piece[],
    sourcesName: string,
  ): Promise<void> {
    const levels = [
      { label: vscode.l10n.t("Discover"), level: "beginner" },
      { label: vscode.l10n.t("I know the basics"), level: "intermediate" },
      { label: vscode.l10n.t("Go deeper"), level: "advanced" },
    ];
    const chosen = await vscode.window.showQuickPick(levels, {
      title: vscode.l10n.t("Your level"),
      ignoreFocusOut: true,
    });
    if (!chosen) return;
    // Toujours filtré : le modèle de l'Étude peut tourner hors de la machine.
    const { pieces, blocked } = filterPieces(raw, "unknown", {
      marker: vscode.l10n.t("[secret removed by ChatonLM]"),
      lines: (label, first, last) =>
        first === last
          ? vscode.l10n.t("{0}, line {1}", label, first)
          : vscode.l10n.t("{0}, lines {1}–{2}", label, first, last),
    });
    const documents = buildDocuments(pieces, sourcesName);
    const chars = documentsChars(documents);
    if (chars > MAX_STUDY_CHARS) {
      const count = chars.toLocaleString(vscode.env.language);
      const most = MAX_STUDY_CHARS.toLocaleString(vscode.env.language);
      // Cours sur un langage : aucune liste de fichiers n'a été montrée.
      void vscode.window.showWarningMessage(
        kind === "language"
          ? vscode.l10n.t(
              "This file holds {0} characters; a course takes at most {1}. Select a part of the file, then try again.",
              count,
              most,
            )
          : vscode.l10n.t(
              "These files hold {0} characters; a course takes at most {1}. Check fewer files.",
              count,
              most,
            ),
      );
      return;
    }
    const id = ++this.#ids;
    this.#set({
      id,
      kind,
      topic,
      status: "creating",
      courseId: null,
      levelsReady: 0,
      levelsTotal: 0,
      error: null,
      files: pieces.map((p) => p.path ?? p.label),
      blocked,
    });
    await this.#deps.reveal();
    try {
      const language = courseLanguage(vscode.env.language);
      const result = await this.#deps.link.request<{ courseId?: unknown }>(
        "study.create",
        {
          topic,
          level: chosen.level,
          duration: COURSE_MINUTES,
          documents,
          ...(language ? { language } : {}),
        },
      );
      const courseId =
        typeof result?.courseId === "string" ? result.courseId : null;
      if (!courseId) throw new Error("no course");
      this.#update(id, { status: "planning", courseId });
      await this.#refresh(courseId);
    } catch (error) {
      this.#update(id, {
        status: "failed",
        error:
          error instanceof RpcError
            ? (error.appMessage ??
              vscode.l10n.t("ChatonLM could not make this course."))
            : vscode.l10n.t("ChatonLM could not make this course."),
      });
    }
  }

  /** Fichiers proposés du dossier ouvert. */
  async #candidates(root: vscode.Uri): Promise<Candidate[]> {
    const base = new vscode.RelativePattern(root, "*");
    const top = await vscode.workspace.findFiles(base, null, 200);
    const sources = await vscode.workspace.findFiles(
      new vscode.RelativePattern(root, SOURCE_GLOB),
      new vscode.RelativePattern(root, EXCLUDE),
      MAX_SCANNED,
    );
    const rel = (uri: vscode.Uri) =>
      path.posix.relative(root.path, uri.path).replaceAll("\\", "/");
    const scanned: { path: string; text: string }[] = [];
    const sizes = new Map<string, number>();
    for (const uri of sources) {
      try {
        // Taille d'abord : un gros fichier (paquet minifié…) n'est pas lu.
        const { size } = await vscode.workspace.fs.stat(uri);
        sizes.set(rel(uri), size);
        if (size > SCAN_FILE_CHARS) continue;
        const text = decode(await vscode.workspace.fs.readFile(uri));
        if (text === null) continue;
        sizes.set(rel(uri), text.length);
        scanned.push({ path: rel(uri), text });
      } catch {
        // Fichier illisible : ignoré.
      }
    }
    const paths = [...new Set([...top.map(rel), ...sources.map(rel)])];
    const candidates = projectCandidates(paths, rankImports(scanned));
    const kept: Candidate[] = [];
    for (const candidate of candidates) {
      let size = sizes.get(candidate.path);
      if (size === undefined) {
        try {
          const stat = await vscode.workspace.fs.stat(
            vscode.Uri.joinPath(root, candidate.path),
          );
          size = stat.size;
        } catch {
          continue;
        }
      }
      if (size <= MAX_CANDIDATE_CHARS) kept.push(candidate);
    }
    return kept;
  }

  /** Progression relue (`study.list`) : statut et paliers du cours. */
  async #refresh(courseId: string): Promise<void> {
    let list: unknown;
    try {
      list = await this.#deps.link.request("study.list", {});
    } catch {
      return;
    }
    const courses = Array.isArray(list)
      ? list
      : (list as { courses?: unknown } | null)?.courses;
    if (!Array.isArray(courses)) return;
    const course = (courses as CourseSummaryLike[]).find(
      (c) => c?.id === courseId,
    );
    if (course) this.#apply(courseId, course);
  }

  #apply(courseId: string, course: CourseSummaryLike): void {
    const view = this.#view;
    if (!view || view.courseId !== courseId) return;
    const status = typeof course.status === "string" ? course.status : null;
    const change: Partial<StudyView> = {};
    if (typeof course.levelsReady === "number") {
      change.levelsReady = course.levelsReady;
    }
    if (typeof course.levelsTotal === "number") {
      change.levelsTotal = course.levelsTotal;
    }
    if (status === "planning") change.status = "planning";
    else if (status === "failed" || status === "unclear") {
      change.status = "failed";
      change.error =
        status === "unclear"
          ? vscode.l10n.t(
              "The topic was unclear to the model. Open the course in ChatonLM to make it clearer.",
            )
          : vscode.l10n.t("ChatonLM could not make this course.");
    } else if (status) {
      change.status = "ready";
    }
    this.#update(view.id, change);
  }

  /** `study.event` de l'app : le cours suivi est relu. */
  onNotification({ method, params }: Notification): void {
    if (method !== "study.event") return;
    const event = (params ?? {}) as CourseSummaryLike & {
      courseId?: unknown;
      course?: CourseSummaryLike;
    };
    const courseId =
      typeof event.courseId === "string"
        ? event.courseId
        : typeof event.id === "string"
          ? event.id
          : null;
    if (!courseId || this.#view?.courseId !== courseId) return;
    // Résumé du cours joint (`{ courseId, course }`) ; sinon relu.
    const course =
      event.course && typeof event.course === "object" ? event.course : event;
    if (typeof course.status === "string") this.#apply(courseId, course);
    else void this.#refresh(courseId);
  }

  /** Commande de la carte du panneau. */
  async command(command: "open" | "close"): Promise<void> {
    const view = this.#view;
    if (!view) return;
    if (command === "close") {
      this.#set(null);
      return;
    }
    await this.#deps.show(
      view.courseId ? `/study/${encodeURIComponent(view.courseId)}` : "/study",
    );
  }
}
