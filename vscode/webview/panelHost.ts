// Hôte du rendu des réponses dans le panneau (voir src/lib/host.ts) : liens
// et presse-papiers passent par l'hôte de l'extension (vscode.env.openExternal,
// vscode.env.clipboard), la webview de VS Code n'ayant ni l'un ni l'autre de
// façon sûre. Le presse-papiers de VS Code ne prend que du texte : une copie
// riche y met le Markdown.
import type { Host } from "@/lib/host";
import type { Post } from "./vscodeApi";

export function panelHost(post: Post): Host {
  return {
    openLink(url) {
      post({ type: "openLink", url });
      return Promise.resolve();
    },
    copyText(text) {
      post({ type: "copy", text });
      return Promise.resolve();
    },
    copyRich(_html, text) {
      post({ type: "copy", text });
      return Promise.resolve();
    },
    // La copie passe par l'hôte, qui ne la refuse pas : rien à montrer.
    copyLinkFailed() {},
  };
}
