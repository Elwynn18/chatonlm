// Canal vers l'hôte de l'extension : l'API que VS Code donne à la webview
// (une seule fois par page : acquireVsCodeApi lève au second appel).
import type { PanelMessage } from "../shared/protocol";

/** Envoie un message à l'hôte de l'extension. */
export type Post = (message: PanelMessage) => void;

declare const acquireVsCodeApi: () => { postMessage(message: unknown): void };

let post: Post | null = null;

export function vscodePost(): Post {
  if (!post) {
    const api = acquireVsCodeApi();
    post = (message) => api.postMessage(message);
  }
  return post;
}
