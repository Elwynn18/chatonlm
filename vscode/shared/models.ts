// Modèles de l'app vus par l'extension (`models.list`) : `ChatModel` depuis
// ChatonLM 0.10 (moteur llama.cpp : identifiants de Hugging Face, modèles
// repris d'Ollama, modèles distants `@remote/…`), ou l'ancien `ModelInfo`
// d'une app 0.9.x (noms d'Ollama), encore en circulation. Pur (ni `vscode`
// ni Node) : lu par l'hôte et par le serveur MCP.

/** Préfixe des identifiants des modèles distants (chez un fournisseur). */
export const REMOTE_PREFIX = "@remote/";

/**
 * Le modèle tourne hors de cette machine, d'après son seul identifiant
 * (quand l'app n'a pas pu le dire) : modèle distant de l'app, ou nom
 * « cloud » d'Ollama d'une app 0.9.x.
 */
export function looksRemote(model: string): boolean {
  return model.startsWith(REMOTE_PREFIX) || /[:-]cloud$/i.test(model);
}

/** Un modèle de `models.list`, quelle que soit la version de l'app. */
export type ListedModel = {
  /** Identifiant à renvoyer à l'app (envoi, détails). */
  id: string;
  /** Nom à montrer. */
  name: string;
  /** Distant (`origin: "remote"`), ou nom d'un modèle cloud (app 0.9.x). */
  remote: boolean;
  /** Contexte maximal du modèle, s'il est connu. */
  contextMax: number | null;
  /** « 0.6B »… */
  parameters: string | null;
  /** « Q4_K_M »… */
  quantization: string | null;
  capabilities: string[];
};

const text = (value: unknown): string | null =>
  typeof value === "string" && value.trim() ? value : null;

const positive = (value: unknown): number | null =>
  typeof value === "number" && Number.isFinite(value) && value > 0
    ? value
    : null;

/**
 * Nom montré et mémorisé : un ancien nom d'Ollama (app 0.9.x) perd
 * `:latest`, comme dans l'app ; un identifiant (`auteur/dépôt:QUANT`,
 * `@ollama/nom:étiquette`, `@remote/…`) reste entier : l'app ne
 * reconnaîtrait pas `@ollama/qwen3` pour `@ollama/qwen3:latest`.
 */
export function shortName(model: string): string {
  return model.includes("/") || model.startsWith("@")
    ? model
    : model.replace(/:latest$/, "");
}

/** Liste reçue de `models.list` : les entrées illisibles sont écartées. */
export function listedModels(value: unknown): ListedModel[] {
  if (!Array.isArray(value)) return [];
  const listed: ListedModel[] = [];
  for (const raw of value as Record<string, unknown>[]) {
    if (!raw || typeof raw !== "object") continue;
    const id = text(raw.id);
    if (id) {
      // App 0.10 et suivantes : `ChatModel`.
      listed.push({
        id,
        name: text(raw.displayName) ?? id,
        remote: raw.origin === "remote",
        contextMax: positive(raw.contextMax),
        parameters: text(raw.paramsLabel),
        quantization: text(raw.quant),
        capabilities: strings(raw.capabilities),
      });
      continue;
    }
    const model = text(raw.model);
    if (!model) continue;
    // App 0.9.x : `ModelInfo` d'Ollama.
    const name = shortName(model);
    listed.push({
      id: name,
      name,
      remote: looksRemote(name),
      contextMax: positive(raw.contextLength),
      parameters: text(raw.parameterSize),
      quantization: text(raw.quantizationLevel),
      capabilities: strings(raw.capabilities),
    });
  }
  return listed;
}

function strings(value: unknown): string[] {
  return Array.isArray(value)
    ? value.filter((item): item is string => typeof item === "string")
    : [];
}
