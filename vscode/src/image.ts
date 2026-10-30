// « Generate this image » (lot 4a) : sur un chemin d'image absent du dossier
// ouvert (`src="hero.png"`, `![](img/x.png)`, `url(…)`, `"icon": "media/x.png"`),
// une action rapide demande une description (pré-remplie : texte alternatif ou
// nom du fichier), l'app peint l'image (`image.generate`, sans conversation),
// le panneau la montre, et seulement sur « Save » le fichier PNG est écrit :
// jamais hors du dossier ouvert (liens symboliques compris), jamais par-dessus
// un fichier sans confirmation. Contrat : .tools/research/93-lot4a-contract.md (3).
import { lstat as fsLstat, realpath as fsRealpath } from "node:fs/promises";
import path from "node:path";
import * as vscode from "vscode";
import type { HostMessage, ImageView } from "../shared/protocol";
import { type Notification, RpcError } from "./bridge/rpc";
import type { LinkLike } from "./controller";
import { LinkCodes } from "./notify";

const IMAGE_EXTENSION = /\.(png|jpe?g|webp|gif|svg|avif|bmp|ico)$/i;
/** Signature d'un fichier PNG. */
const PNG_SIGNATURE = [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a];
/** Page des modèles d'images de l'app. */
export const IMAGE_MODELS_ROUTE = "/hub/images";

export type ImageRef = {
  /** Chemin tel qu'écrit (sans `?query` ni `#ancre`). */
  path: string;
  /** Texte alternatif (`![alt](…)`, `alt="…"`), ou null. */
  alt: string | null;
  /** Colonnes du chemin dans la ligne. */
  start: number;
  end: number;
};

/** Chemin local plausible (pas une adresse, ni un gabarit, ni un lecteur),
 * `%20` et autres échappements d'adresse décodés (`my%20cat.png`). */
function localPath(raw: string): string | null {
  let value = raw.trim().replace(/[?#].*$/, "");
  if (value.includes("%")) {
    try {
      value = decodeURIComponent(value);
    } catch {
      return null;
    }
  }
  if (!value || !IMAGE_EXTENSION.test(value)) return null;
  if (/^[a-z][\w+.-]*:/i.test(value) || value.startsWith("//")) return null;
  if (/\$\{|\{\{|<%|\\/.test(value)) return null;
  return value;
}

/**
 * Chemin d'image sous le curseur (`character`, colonne à partir de 0) : lien
 * d'image Markdown, `url(…)` de CSS, ou chaîne entre guillemets qui finit par
 * une extension d'image.
 */
export function findImageRef(line: string, character: number): ImageRef | null {
  const forms: { pattern: RegExp; group: number; alt?: number }[] = [
    {
      pattern: /!\[([^\]]*)\]\(\s*<?([^)\s>]+)>?(?:\s+"[^"]*")?\s*\)/g,
      group: 2,
      alt: 1,
    },
    { pattern: /url\(\s*["']?([^"')\s]+)["']?\s*\)/g, group: 1 },
    // Un motif par guillemet : une apostrophe dans un texte entre guillemets
    // (`alt="Bob's hero"`) n'avale pas le guillemet du chemin qui suit.
    { pattern: /"([^"\n]+?)"/g, group: 1 },
    { pattern: /'([^'\n]+?)'/g, group: 1 },
    { pattern: /`([^`\n]+?)`/g, group: 1 },
  ];
  for (const { pattern, group, alt } of forms) {
    for (const match of line.matchAll(pattern)) {
      const raw = match[group];
      const start = (match.index ?? 0) + match[0].indexOf(raw);
      const end = start + raw.length;
      // Le curseur sur le chemin (ou sur ses guillemets, ou sur `![alt]`).
      if (
        character < (match.index ?? 0) ||
        character > (match.index ?? 0) + match[0].length
      ) {
        continue;
      }
      const value = localPath(raw);
      if (!value) continue;
      const altText =
        alt !== undefined ? match[alt].trim() || null : (altOf(line) ?? null);
      return { path: value, alt: altText, start, end };
    }
  }
  return null;
}

/** Attribut `alt` de la ligne (guillemets ou apostrophes), ou null. */
function altOf(line: string): string | null {
  const match = /\balt\s*=\s*(?:"([^"]*)"|'([^']*)')/i.exec(line);
  return (match?.[1] ?? match?.[2] ?? "").trim() || null;
}

/** Description proposée : texte alternatif, sinon le nom du fichier en mots. */
export function promptFor(ref: ImageRef): string {
  if (ref.alt) return ref.alt;
  const name = path.posix.basename(ref.path).replace(IMAGE_EXTENSION, "");
  return name
    .replace(/[-_.]+/g, " ")
    .replace(/([a-z])([A-Z])/g, "$1 $2")
    .replace(/\s+/g, " ")
    .trim();
}

/** Côté arrondi au multiple de 64, entre 256 et 1024. */
const side = (value: number) =>
  Math.min(1024, Math.max(256, Math.round(value / 64) * 64));

/**
 * Taille déduite : attributs `width`/`height` de la ligne (proportions
 * gardées au mieux), sinon d'après le nom (icône carrée, bannière large) ;
 * vide : taille par défaut de l'app.
 */
export function sizeFor(
  ref: ImageRef,
  line: string,
): { width?: number; height?: number } {
  // Pixels seulement : `width="1600"`, `width: 800px` ; pas `100%`, `20em`.
  const read = (name: string) => {
    const match = new RegExp(
      `\\b${name}\\s*[=:]\\s*["'{]?\\s*(\\d{2,5})(?:px)?(?![\\d.%a-z])`,
      "i",
    ).exec(line);
    return match ? Number(match[1]) : null;
  };
  const width = read("width");
  const height = read("height");
  if (width && height) {
    const scale = 1024 / Math.max(width, height);
    return {
      width: side(width * Math.min(1, scale)),
      height: side(height * Math.min(1, scale)),
    };
  }
  if (width || height) {
    const one = side((width ?? height) as number);
    return { width: one, height: one };
  }
  const name = path.posix.basename(ref.path).toLowerCase();
  if (/(icon|favicon|logo|avatar)/.test(name))
    return { width: 512, height: 512 };
  if (/(hero|banner|cover|header|background|bg|wallpaper)/.test(name)) {
    return { width: 1024, height: 576 };
  }
  return {};
}

/** Chemin cible : depuis la racine du dossier pour « /x.png », sinon depuis
 * le dossier du document. */
export function targetUri(
  document: vscode.Uri,
  root: vscode.Uri,
  ref: string,
): vscode.Uri {
  if (ref.startsWith("/")) return vscode.Uri.joinPath(root, ref.slice(1));
  return vscode.Uri.joinPath(document, "..", ref);
}

/** Vrai si `child` (chemin) est dans `parent` (sans en être la racine). */
export function within(parent: string, child: string, win = false): boolean {
  const norm = (p: string) => {
    const clean = p.replaceAll("\\", "/").replace(/\/+$/, "");
    return win ? clean.toLowerCase() : clean;
  };
  const a = norm(parent);
  const b = norm(child);
  return b.startsWith(`${a}/`) && !b.slice(a.length).split("/").includes("..");
}

/** Accès au disque de `realWithin` (essais : disque simulé). */
export type PathFs = {
  /** Chemin réel (liens suivis) ; échoue si le chemin n'existe pas. */
  realpath(p: string): Promise<string>;
  /** Le chemin est-il un lien symbolique (sans le suivre) ? null s'il
   * n'existe pas ; une autre erreur est levée. */
  isLink(p: string): Promise<boolean | null>;
};

/** `isLink` du vrai disque. */
export async function isLinkOnDisk(p: string): Promise<boolean | null> {
  try {
    return (await fsLstat(p)).isSymbolicLink();
  } catch (error) {
    const code = (error as { code?: unknown }).code;
    if (code === "ENOENT" || code === "ENOTDIR") return null;
    throw error;
  }
}

/**
 * Le fichier à écrire reste-t-il dans le dossier ouvert une fois les liens
 * symboliques suivis ? Ni la cible ni un dossier entre la racine et elle ne
 * doit être un lien symbolique (un lien pendant, dont la cible n'existe pas
 * encore, ferait écrire ailleurs : rapport 94, S5) ; puis le plus proche
 * dossier existant de la cible est résolu et comparé à la racine résolue.
 */
export async function realWithin(
  root: string,
  target: string,
  fs: PathFs,
  win = process.platform === "win32",
): Promise<boolean> {
  if (!within(root, target, win)) return false;
  let realRoot: string;
  try {
    realRoot = await fs.realpath(root);
  } catch {
    return false;
  }
  // Cible et dossiers intermédiaires, jusqu'à la racine (exclue).
  for (let step = target; within(root, step, win); step = path.dirname(step)) {
    try {
      if ((await fs.isLink(step)) === true) return false;
    } catch {
      return false;
    }
  }
  const realpath = (p: string) => fs.realpath(p);
  let current = target;
  const rest: string[] = [];
  for (;;) {
    try {
      const real = await realpath(current);
      const full = path.join(real, ...rest.reverse());
      return within(realRoot, full, win);
    } catch {
      const parent = path.dirname(current);
      if (parent === current) return false;
      rest.push(path.basename(current));
      current = parent;
    }
  }
}

/** Octets d'un PNG en base64, ou null si ce n'en est pas un. */
export function decodePng(base64: unknown): Uint8Array | null {
  if (typeof base64 !== "string" || base64.length === 0) return null;
  const bytes = new Uint8Array(Buffer.from(base64, "base64"));
  if (bytes.length < PNG_SIGNATURE.length) return null;
  return PNG_SIGNATURE.every((b, i) => bytes[i] === b) ? bytes : null;
}

async function exists(uri: vscode.Uri): Promise<boolean> {
  try {
    await vscode.workspace.fs.stat(uri);
    return true;
  } catch {
    return false;
  }
}

export type ImageDeps = {
  link: LinkLike;
  post(message: HostMessage): void;
  reveal(): Promise<void>;
  show(route: string): Promise<void>;
  /** Éditeur appairé (l'action n'est proposée qu'alors). */
  paired(): boolean;
  /** Chemin réel (liens suivis) ; essais : chemin donné. */
  realpath?(p: string): Promise<string>;
  /** Lien symbolique ? (null : absent) ; essais : disque simulé. */
  isLink?(p: string): Promise<boolean | null>;
};

type Pending = {
  view: ImageView;
  target: vscode.Uri;
  root: vscode.Uri;
  callId: number | null;
  cancelled: boolean;
  bytes: Uint8Array | null;
};

export class ImageMaker implements vscode.CodeActionProvider {
  static readonly kinds = [vscode.CodeActionKind.QuickFix];
  readonly #deps: ImageDeps;
  #pending: Pending | null = null;
  #ids = 0;

  constructor(deps: ImageDeps) {
    this.#deps = deps;
  }

  get view(): ImageView | null {
    return this.#pending?.view ?? null;
  }

  #post(): void {
    this.#deps.post({ type: "image", image: this.view });
  }

  #update(pending: Pending, change: Partial<ImageView>): void {
    pending.view = { ...pending.view, ...change };
    if (this.#pending === pending) this.#post();
  }

  /** Disque vu par `realWithin`. */
  #fs(): PathFs {
    return {
      realpath: (p) => (this.#deps.realpath ?? fsRealpath)(p),
      isLink: (p) => (this.#deps.isLink ?? isLinkOnDisk)(p),
    };
  }

  /** Cible dans le dossier ouvert, ou null (hors du dossier, autre schéma). */
  async #locate(
    document: vscode.TextDocument,
    ref: ImageRef,
  ): Promise<{ target: vscode.Uri; root: vscode.Uri } | null> {
    if (document.uri.scheme !== "file") return null;
    const folder = vscode.workspace.getWorkspaceFolder(document.uri);
    if (!folder) return null;
    const target = targetUri(document.uri, folder.uri, ref.path);
    const inside = await realWithin(
      folder.uri.fsPath,
      target.fsPath,
      this.#fs(),
    );
    return inside ? { target, root: folder.uri } : null;
  }

  async provideCodeActions(
    document: vscode.TextDocument,
    range: vscode.Range,
  ): Promise<vscode.CodeAction[]> {
    if (!this.#deps.paired()) return [];
    const line = document.lineAt(range.start.line).text;
    const ref = findImageRef(line, range.start.character);
    if (!ref) return [];
    const place = await this.#locate(document, ref);
    if (!place || (await exists(place.target))) return [];
    const action = new vscode.CodeAction(
      vscode.l10n.t("Generate this image with ChatonLM"),
      vscode.CodeActionKind.QuickFix,
    );
    action.command = {
      command: "chatonlm.generateImage",
      title: vscode.l10n.t("Generate this image with ChatonLM"),
      arguments: [document.uri, range.start.line, range.start.character],
    };
    return [action];
  }

  /** Commande : arguments de l'action rapide, sinon curseur de l'éditeur. */
  async generate(args: unknown[] = []): Promise<void> {
    const [uriArg, lineArg, charArg] = args;
    let document: vscode.TextDocument | undefined;
    let line: number;
    let character: number;
    if (
      uriArg instanceof vscode.Uri &&
      typeof lineArg === "number" &&
      typeof charArg === "number"
    ) {
      document = await vscode.workspace.openTextDocument(uriArg);
      line = lineArg;
      character = charArg;
    } else {
      const editor = vscode.window.activeTextEditor;
      document = editor?.document;
      line = editor?.selection.active.line ?? 0;
      character = editor?.selection.active.character ?? 0;
    }
    if (!this.#deps.link.connected) {
      await this.#deps.reveal();
      void vscode.window.showWarningMessage(
        vscode.l10n.t("ChatonLM is not connected. See the ChatonLM panel."),
      );
      return;
    }
    if (this.#pending?.view.status === "generating") {
      void vscode.window.showWarningMessage(
        vscode.l10n.t("An image is being made. Wait for it, or stop it."),
      );
      return;
    }
    const text = document?.lineAt(line).text ?? "";
    const ref = findImageRef(text, character);
    if (!document || !ref) {
      void vscode.window.showInformationMessage(
        vscode.l10n.t(
          "Put the cursor on the path of a missing image, such as img/hero.png, then try again.",
        ),
      );
      return;
    }
    if (!/\.png$/i.test(ref.path)) {
      void vscode.window.showInformationMessage(
        vscode.l10n.t(
          "ChatonLM only makes PNG images. Change the file name to .png, then try again.",
        ),
      );
      return;
    }
    const place = await this.#locate(document, ref);
    if (!place) {
      void vscode.window.showWarningMessage(
        vscode.l10n.t(
          "This path is outside the open folder: ChatonLM only writes images inside it.",
        ),
      );
      return;
    }
    const prompt = await vscode.window.showInputBox({
      title: vscode.l10n.t("Describe the image"),
      prompt: vscode.l10n.t("ChatonLM makes it with your image model."),
      value: promptFor(ref),
      ignoreFocusOut: true,
    });
    if (!prompt?.trim()) return;
    const size = sizeFor(ref, text);
    const pending: Pending = {
      view: {
        id: ++this.#ids,
        target: vscode.workspace.asRelativePath(place.target),
        prompt: prompt.trim(),
        status: "generating",
        stage: "starting",
        step: null,
        total: null,
        cpu: false,
        preview: null,
        width: size.width ?? null,
        height: size.height ?? null,
        exists: await exists(place.target),
        error: null,
      },
      target: place.target,
      root: place.root,
      callId: null,
      cancelled: false,
      bytes: null,
    };
    this.#pending = pending;
    this.#post();
    await this.#deps.reveal();
    const call = this.#deps.link.call<{
      pngBase64?: unknown;
      seed?: unknown;
      model?: unknown;
    }>("image.generate", { prompt: pending.view.prompt, ...size });
    pending.callId = call.id;
    try {
      const result = await call.result;
      const bytes = decodePng(result?.pngBase64);
      if (!bytes) throw new Error("not a png");
      pending.bytes = bytes;
      this.#update(pending, {
        status: "ready",
        stage: null,
        preview: `data:image/png;base64,${result.pngBase64 as string}`,
      });
    } catch (error) {
      if (pending.cancelled) {
        if (this.#pending === pending) {
          this.#pending = null;
          this.#post();
        }
        return;
      }
      const noModel =
        error instanceof RpcError && error.code === LinkCodes.noImageModel;
      const message = noModel
        ? vscode.l10n.t("No image model is installed in ChatonLM.")
        : error instanceof RpcError
          ? (error.appMessage ??
            vscode.l10n.t("ChatonLM could not make this image."))
          : vscode.l10n.t("ChatonLM could not make this image.");
      this.#update(pending, { status: "failed", stage: null, error: message });
      if (noModel) {
        const open = vscode.l10n.t("Open ChatonLM");
        const choice = await vscode.window.showErrorMessage(message, open);
        if (choice === open) await this.#deps.show(IMAGE_MODELS_ROUTE);
      }
    } finally {
      pending.callId = null;
    }
  }

  /** `image.event` : avancement de l'image en cours. */
  onNotification({ method, params }: Notification): void {
    if (method !== "image.event") return;
    const pending = this.#pending;
    if (!pending || pending.view.status !== "generating") return;
    const { requestId, event, progress } = (params ?? {}) as {
      requestId?: unknown;
      event?: unknown;
      progress?: unknown;
    };
    // Une seule image à la fois ; numéro pas encore connu : avancement
    // arrivé avant le retour de l'envoi (même tour de boucle).
    if (pending.callId !== null && requestId !== pending.callId) return;
    const data = (event ?? progress ?? params) as Record<string, unknown>;
    const stages = ["starting", "queued", "loading", "generating"] as const;
    const stage = stages.find((s) => s === data.stage) ?? pending.view.stage;
    this.#update(pending, {
      stage,
      step: typeof data.step === "number" ? data.step : pending.view.step,
      total: typeof data.total === "number" ? data.total : pending.view.total,
      cpu: data.cpu === true || pending.view.cpu,
      preview:
        typeof data.preview === "string" &&
        data.preview.startsWith("data:image/png;base64,")
          ? data.preview
          : pending.view.preview,
    });
  }

  /** Commande de la carte du panneau. */
  async command(command: "save" | "discard" | "cancel"): Promise<void> {
    const pending = this.#pending;
    if (!pending) return;
    switch (command) {
      case "cancel":
        if (pending.callId !== null) {
          pending.cancelled = true;
          this.#deps.link.cancel(pending.callId);
        }
        return;
      case "discard":
        if (pending.view.status === "generating") return;
        this.#pending = null;
        this.#post();
        return;
      case "save":
        await this.#save(pending);
        return;
    }
  }

  /** Commande « Save the Generated Image » (essais dans un vrai éditeur) :
   * vrai une fois le fichier écrit. */
  async save(): Promise<boolean> {
    const pending = this.#pending;
    if (!pending) return false;
    await this.#save(pending);
    return pending.view.status === "saved";
  }

  async #save(pending: Pending): Promise<void> {
    if (pending.view.status !== "ready" || !pending.bytes) return;
    // Revérifié au moment d'écrire (lien symbolique posé entre-temps).
    const inside = await realWithin(
      pending.root.fsPath,
      pending.target.fsPath,
      this.#fs(),
    );
    if (!inside) {
      this.#update(pending, {
        status: "failed",
        error: vscode.l10n.t(
          "This path is outside the open folder: ChatonLM only writes images inside it.",
        ),
      });
      return;
    }
    if (await exists(pending.target)) {
      const replace = vscode.l10n.t("Replace");
      const choice = await vscode.window.showWarningMessage(
        vscode.l10n.t(
          "{0} already exists. Replace it with the new image?",
          pending.view.target,
        ),
        { modal: true },
        replace,
      );
      if (choice !== replace) return;
    }
    this.#update(pending, { status: "saving" });
    try {
      await vscode.workspace.fs.createDirectory(
        vscode.Uri.joinPath(pending.target, ".."),
      );
      await vscode.workspace.fs.writeFile(pending.target, pending.bytes);
    } catch {
      this.#update(pending, {
        status: "failed",
        error: vscode.l10n.t("The image could not be written."),
      });
      return;
    }
    this.#update(pending, { status: "saved", exists: true });
  }
}
