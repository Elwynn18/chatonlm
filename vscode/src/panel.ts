// Panneau latéral : page React de `dist/webview/` (écrite par le build du
// panneau), sous une CSP stricte (scripts par nonce, aucune ressource
// distante, aucune connexion : la page ne parle qu'à l'extension).
import { randomBytes } from "node:crypto";
import * as vscode from "vscode";
import type {
  HostMessage,
  Locale,
  PanelMessage,
  Theme,
} from "../shared/protocol";

export const VIEW_ID = "chatonlm.chat";

/** Langue de VS Code ramenée à en/es/fr (anglais sinon). */
export function panelLocale(language: string): Locale {
  const base = language.toLowerCase().split(/[-_]/)[0];
  return base === "fr" || base === "es" ? base : "en";
}

/**
 * Thème du panneau : celui de ChatonLM, clair ou sombre. Les thèmes à
 * contraste élevé de VS Code prennent le sombre (ou le clair) de ChatonLM :
 * ses couleurs gardent 4,5:1 pour tout texte, mais pas les bordures épaisses
 * de ces thèmes (écart voulu, apparence propre à ChatonLM).
 */
export function panelTheme(kind: vscode.ColorThemeKind): Theme {
  return kind === vscode.ColorThemeKind.Dark ||
    kind === vscode.ColorThemeKind.HighContrast
    ? "dark"
    : "light";
}

export function newNonce(): string {
  return randomBytes(18).toString("base64url");
}

/** Politique de sécurité de la page. */
export function contentSecurityPolicy(
  cspSource: string,
  nonce: string,
): string {
  return [
    "default-src 'none'",
    `script-src 'nonce-${nonce}' ${cspSource}`,
    // Styles en ligne : mise en page des formules (KaTeX).
    `style-src ${cspSource} 'unsafe-inline'`,
    `font-src ${cspSource}`,
    `img-src ${cspSource} data:`,
    "connect-src 'none'",
    "form-action 'none'",
    "base-uri 'none'",
  ].join("; ");
}

const escapeAttribute = (value: string) =>
  value.replace(/[&"<>]/g, (c) => `&#${c.charCodeAt(0)};`);

export function panelHtml(options: {
  cspSource: string;
  nonce: string;
  script: string;
  style: string;
  locale: Locale;
  theme: Theme;
}): string {
  const { cspSource, nonce, script, style, locale, theme } = options;
  return `<!doctype html>
<html lang="${locale}"${theme === "dark" ? ' class="dark"' : ""}>
<head>
<meta charset="utf-8">
<meta http-equiv="Content-Security-Policy" content="${escapeAttribute(contentSecurityPolicy(cspSource, nonce))}">
<meta name="viewport" content="width=device-width, initial-scale=1">
<link rel="stylesheet" href="${escapeAttribute(style)}">
</head>
<body>
<div id="root"></div>
<script type="module" nonce="${nonce}" src="${escapeAttribute(script)}"></script>
</body>
</html>`;
}

/** Messages gardés pour une page pas encore prête (au plus). */
export const MAX_QUEUED = 5000;

/**
 * Messages gardés à rejouer après `init` : les envois en cours (reçu,
 * événements) et les conversations relues ; pas les états, dont `init`
 * donne le dernier, ni les reçus gardés, redonnés à `ready`.
 */
const REPLAYED = new Set<HostMessage["type"]>(["event", "receipt", "chat"]);

/** Fournit la vue et relaie ses messages. */
export class ChatPanel implements vscode.WebviewViewProvider {
  readonly #extensionUri: vscode.Uri;
  readonly #onMessage: (message: PanelMessage) => void;
  readonly #onVisible: () => void;
  readonly #locale: () => Locale;
  readonly #theme: () => Theme;
  #view: vscode.WebviewView | null = null;
  #ready = false;
  /** Messages partis avant que la page soit prête (action lancée panneau
   * fermé : sa réponse s'affiche quand même en direct). */
  #queue: HostMessage[] = [];

  constructor(options: {
    extensionUri: vscode.Uri;
    onMessage: (message: PanelMessage) => void;
    /** La vue redevient visible (modèles relus). */
    onVisible?: () => void;
    locale: () => Locale;
    theme: () => Theme;
  }) {
    this.#extensionUri = options.extensionUri;
    this.#onMessage = options.onMessage;
    this.#onVisible = options.onVisible ?? (() => {});
    this.#locale = options.locale;
    this.#theme = options.theme;
  }

  resolveWebviewView(view: vscode.WebviewView): void {
    this.#view = view;
    this.#ready = false;
    const root = vscode.Uri.joinPath(this.#extensionUri, "dist", "webview");
    view.webview.options = { enableScripts: true, localResourceRoots: [root] };
    const nonce = newNonce();
    view.webview.html = panelHtml({
      cspSource: view.webview.cspSource,
      nonce,
      script: view.webview
        .asWebviewUri(vscode.Uri.joinPath(root, "index.js"))
        .toString(),
      style: view.webview
        .asWebviewUri(vscode.Uri.joinPath(root, "index.css"))
        .toString(),
      locale: this.#locale(),
      theme: this.#theme(),
    });
    view.webview.onDidReceiveMessage((message: PanelMessage) => {
      if (typeof message !== "object" || message === null) return;
      if (message.type === "ready") this.#ready = true;
      this.#onMessage(message);
    });
    view.onDidChangeVisibility(() => {
      if (view.visible && this.#ready) this.#onVisible();
    });
    view.onDidDispose(() => {
      if (this.#view === view) {
        this.#view = null;
        this.#ready = false;
      }
    });
  }

  /**
   * Vers la page. Avant son `ready`, les messages d'un envoi sont gardés et
   * rejoués juste après `init` (qui porte le dernier état).
   */
  post(message: HostMessage): void {
    if (!this.#view || !this.#ready) {
      if (REPLAYED.has(message.type)) {
        this.#queue.push(message);
        if (this.#queue.length > MAX_QUEUED) this.#queue.shift();
      }
      return;
    }
    void this.#view.webview.postMessage(message);
    if (message.type === "init") {
      const queued = this.#queue;
      this.#queue = [];
      for (const kept of queued) void this.#view.webview.postMessage(kept);
    }
  }

  async reveal(): Promise<void> {
    if (this.#view) {
      this.#view.show(true);
      return;
    }
    await vscode.commands.executeCommand(`${VIEW_ID}.focus`);
  }
}
