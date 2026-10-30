// Contrôleur relié au faux pont, pour les essais du lot 2 (visite, voix) :
// éditeur visible, dépôt Git et terminal simulés.
import { vi } from "vitest";
import type { HostMessage } from "../../shared/protocol";
import { type Credentials, Link } from "../bridge/link";
import { Controller } from "../controller";
import type { TerminalFailure } from "../terminal";
import { FakeBridge, until } from "./fakeBridge";
import {
  Position,
  Selection,
  TextDocument,
  TextEditor,
  testState,
  window,
} from "./vscode";

const cleanups: (() => void)[] = [];

/** À appeler dans `afterEach`. */
export function cleanUp(): void {
  for (const cleanup of cleanups.splice(0)) cleanup();
}

/** Terminal simulé : dernière commande échouée réglée par l'essai. */
export function fakeTerminal() {
  const listeners = new Set<() => void>();
  return {
    last: null as TerminalFailure | null,
    onChange(listener: () => void) {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
    fail(failure: TerminalFailure) {
      this.last = failure;
      for (const listener of listeners) listener();
    },
  };
}

export async function setupController() {
  const fake = new FakeBridge();
  let credentials: Credentials | null = fake.addClient();
  const link = new Link({
    dir: () => "/run",
    readDiscovery: async () => fake.discovery,
    connect: fake.connect,
    secrets: {
      get: async () => credentials,
      set: async (c) => void (credentials = c),
      delete: async () => void (credentials = null),
    },
    client: { name: "t", version: "0", editor: "VS Code" },
    watch: () => null,
    appRunning: async () => false,
  });
  const posted: HostMessage[] = [];
  const memory = new Map<string, unknown>();
  const terminal = fakeTerminal();
  const deps = {
    link,
    memento: {
      get: <T>(key: string) => memory.get(key) as T | undefined,
      update: async (key: string, value: unknown) =>
        void memory.set(key, value),
    },
    post: (m: HostMessage) => posted.push(m),
    reveal: vi.fn(async () => {}),
    launchApp: vi.fn(async () => {}),
    pair: vi.fn(async () => {}),
    locale: () => "fr" as const,
    theme: () => "dark" as const,
    terminal,
    // Disque simulé : les chemins sont déjà réels.
    realpath: async (p: string) => p,
    isLink: async (p: string) =>
      testState.links.has(p) ? true : testState.files.has(p) ? false : null,
  };
  const controller = new Controller(deps);
  cleanups.push(() => {
    controller.dispose();
    link.dispose();
  });
  await link.start();
  await until(() => controller.state.models.length > 0);
  return { fake, link, controller, posted, deps, terminal };
}

/** Éditeur actif et visible sur un fichier de l'espace de travail. */
export function openEditor(
  file: string,
  text: string,
  selection?: { from: [number, number]; to: [number, number] },
  languageId = "typescript",
): TextEditor {
  const document = new TextDocument(file, text, languageId);
  testState.documents.set(file, document);
  const editor = new TextEditor(
    document,
    selection
      ? new Selection(
          new Position(...selection.from),
          new Position(...selection.to),
        )
      : undefined,
  );
  window.activeTextEditor = editor;
  window.visibleTextEditors = [editor];
  return editor;
}

/** Messages postés au panneau, d'un type. */
export const ofType = <T extends HostMessage["type"]>(
  posted: HostMessage[],
  type: T,
) =>
  posted.filter((m): m is Extract<HostMessage, { type: T }> => m.type === type);

/** Dix lignes numérotées (« line 1 »…). */
export const TEN_LINES = Array.from(
  { length: 10 },
  (_, i) => `line ${i + 1}`,
).join("\n");
