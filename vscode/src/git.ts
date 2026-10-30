// Accès au dépôt par l'API de l'extension intégrée `vscode.git` (version 1) :
// diff indexé, sinon diff de travail ; message écrit dans la boîte de saisie
// du dépôt. Seuls les membres utilisés sont décrits ici.
import * as vscode from "vscode";

export type GitRepository = {
  readonly rootUri: vscode.Uri;
  readonly inputBox: { value: string };
  /** `cached` : changements indexés seulement. */
  diff(cached?: boolean): Promise<string>;
  /** Diff de travail d'un fichier par rapport à HEAD (chemin absolu). */
  diffWithHEAD?(path: string): Promise<string>;
};

type GitApi = {
  readonly repositories: GitRepository[];
  getRepository(uri: vscode.Uri): GitRepository | null;
};

type GitExtension = {
  readonly enabled: boolean;
  getAPI(version: 1): GitApi;
};

export async function gitApi(): Promise<GitApi | null> {
  const extension = vscode.extensions.getExtension<GitExtension>("vscode.git");
  if (!extension) return null;
  const exports = extension.isActive
    ? extension.exports
    : await extension.activate();
  return exports?.enabled ? exports.getAPI(1) : null;
}

/**
 * Dépôt visé : celui du bouton cliqué (menu `scm/title` : argument
 * `SourceControl` avec `rootUri`), sinon celui du fichier ouvert, sinon le
 * seul ouvert.
 */
export function pickRepository(
  api: GitApi,
  argument: unknown,
  activeUri: vscode.Uri | undefined,
): GitRepository | null {
  const rootUri = (argument as { rootUri?: vscode.Uri } | undefined)?.rootUri;
  if (rootUri) {
    const repository = api.getRepository(rootUri);
    if (repository) return repository;
  }
  if (activeUri) {
    const repository = api.getRepository(activeUri);
    if (repository) return repository;
  }
  return api.repositories.length === 1 ? api.repositories[0] : null;
}

/** Diff à décrire : indexé s'il y en a un, sinon les changements de travail. */
export async function changes(
  repository: GitRepository,
): Promise<{ diff: string; staged: boolean }> {
  const staged = await repository.diff(true);
  if (staged.trim()) return { diff: staged, staged: true };
  return { diff: await repository.diff(false), staged: false };
}
