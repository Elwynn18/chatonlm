// Noms de modèles lisibles (propre à ChatonLM) : un nom Ollama de Hugging Face
// (`hf.co/<auteur>/<dépôt>:<étiquette>`) est long et sa fin, l'étiquette,
// distingue les versions d'un même dépôt ; l'interface le montre en titre et
// étiquette séparés, l'étiquette toujours visible.

/** Nom de modèle décomposé pour l'affichage. */
export interface ModelName {
  /** « SmolLM2-135M-Instruct » (dépôt sans suffixe GGUF), « qwen3 ». */
  title: string;
  /** « Q4_K_M », « 0.6b » ; vide : pas d'étiquette. */
  tag: string;
  /** Auteur d'un nom hf.co, espace de noms d'un autre nom ; sinon null. */
  publisher: string | null;
  /** Dépôt Hugging Face « auteur/dépôt » d'un nom hf.co, sinon null. */
  repo: string | null;
}

const HF_HOSTS = ["hf.co", "huggingface.co"];

/** Dépôt sans son suffixe GGUF (« Qwen3-8B-GGUF » → « Qwen3-8B »). */
export function withoutGgufSuffix(repo: string): string {
  return repo.replace(/[-_]gguf$/i, "") || repo;
}

/**
 * Décompose un nom de modèle : `hf.co/bartowski/SmolLM2-135M-Instruct-GGUF:Q4_K_M`
 * → titre « SmolLM2-135M-Instruct », étiquette « Q4_K_M », publieur
 * « bartowski » ; `qwen3:0.6b` → « qwen3 », « 0.6b ». L'étiquette est ce qui
 * suit le dernier « : » placé après le dernier « / » (un hôte peut porter un
 * port).
 */
export function parseModelName(model: string): ModelName {
  const name = model.trim();
  const slash = name.lastIndexOf("/");
  const colon = name.lastIndexOf(":");
  const hasTag = colon > slash;
  const path = hasTag ? name.slice(0, colon) : name;
  const tag = hasTag ? name.slice(colon + 1) : "";
  const parts = path.split("/");
  if (
    parts.length === 3 &&
    HF_HOSTS.includes(parts[0].toLowerCase()) &&
    parts[1] &&
    parts[2]
  ) {
    return {
      title: withoutGgufSuffix(parts[2]),
      tag,
      publisher: parts[1],
      repo: `${parts[1]}/${parts[2]}`,
    };
  }
  const title = parts.at(-1) || name;
  const namespace = parts.length >= 2 ? parts.at(-2) : undefined;
  return {
    title,
    tag,
    publisher: namespace && namespace !== "library" ? namespace : null,
    repo: null,
  };
}

/** Nom de Hugging Face (`hf.co/...`, sans tenir compte de la casse) ? */
export function isHuggingFaceName(model: string): boolean {
  return parseModelName(model).repo !== null;
}

/**
 * Étiquette à afficher : sans l'extension d'un nom de fichier complet
 * (« Qwen3.8-27B-Q4_K_L.gguf » → « Qwen3.8-27B-Q4_K_L »).
 */
export function displayTag(tag: string): string {
  return tag.replace(/\.gguf$/i, "");
}

/**
 * Deux noms désignent-ils le même modèle ? Ollama résout les noms sans tenir
 * compte de la casse.
 */
export function sameModel(a: string, b: string): boolean {
  return a.trim().toLowerCase() === b.trim().toLowerCase();
}
