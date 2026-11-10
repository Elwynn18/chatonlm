// Noms de modèles lisibles (propre à ChatonLM). Identifiants du moteur
// (contrats 100 § 1 et 102 § 1.1) : `<auteur>/<dépôt>:<QUANT>` pour un
// modèle de Hugging Face, `@ollama/<nom>:<étiquette>` pour un modèle repris
// d'Ollama, `@remote/<fournisseur>/<modèle>` pour un modèle distant. Les
// anciens noms d'Ollama (`qwen3:0.6b`, `hf.co/<auteur>/<dépôt>:<étiquette>`)
// restent lisibles : l'historique les garde. L'interface montre un titre et
// une étiquette séparés, l'étiquette toujours visible.

/** Sorte d'identifiant. */
export type ModelKind = "hf" | "ollama" | "remote" | "other";

/** Nom de modèle décomposé pour l'affichage. */
export interface ModelName {
  /** « SmolLM2-135M-Instruct » (dépôt sans suffixe GGUF), « qwen3 ». */
  title: string;
  /** « Q4_K_M », « 0.6b » ; vide : pas d'étiquette. */
  tag: string;
  /**
   * Auteur d'un modèle de Hugging Face, fournisseur d'un modèle distant,
   * espace de noms d'un ancien nom ; sinon null.
   */
  publisher: string | null;
  /** Dépôt Hugging Face « auteur/dépôt », sinon null. */
  repo: string | null;
  kind: ModelKind;
}

/** Préfixe des modèles repris d'Ollama. */
export const OLLAMA_PREFIX = "@ollama/";
/** Préfixe des modèles distants. */
export const REMOTE_PREFIX = "@remote/";

const HF_HOSTS = ["hf.co", "huggingface.co"];

/** Dépôt sans son suffixe GGUF (« Qwen3-8B-GGUF » → « Qwen3-8B »). */
export function withoutGgufSuffix(repo: string): string {
  return repo.replace(/[-_]gguf$/i, "") || repo;
}

/** Modèle distant (`@remote/…`, même mal formé : jamais local). */
export function isRemoteModel(model: string): boolean {
  return model.trim().startsWith(REMOTE_PREFIX);
}

/** Coupe au dernier « : » placé après le dernier « / » (un hôte peut porter un port). */
function splitTag(name: string): { path: string; tag: string } {
  const slash = name.lastIndexOf("/");
  const colon = name.lastIndexOf(":");
  return colon > slash
    ? { path: name.slice(0, colon), tag: name.slice(colon + 1) }
    : { path: name, tag: "" };
}

/**
 * Décompose un nom de modèle :
 * `bartowski/SmolLM2-135M-Instruct-GGUF:Q4_K_M` → titre
 * « SmolLM2-135M-Instruct », étiquette « Q4_K_M », publieur « bartowski » ;
 * `@ollama/qwen3:0.6b` → « qwen3 », « 0.6b » ;
 * `@remote/openrouter/meta-llama/llama-3.3-70b:free` → titre
 * « meta-llama/llama-3.3-70b:free » (le nom du fournisseur, tel quel),
 * publieur « openrouter » ; ancien nom `qwen3:0.6b` → « qwen3 », « 0.6b ».
 */
export function parseModelName(model: string): ModelName {
  const name = model.trim();
  if (name.startsWith(REMOTE_PREFIX)) {
    const rest = name.slice(REMOTE_PREFIX.length);
    const slash = rest.indexOf("/");
    return slash > 0 && slash < rest.length - 1
      ? {
          title: rest.slice(slash + 1),
          tag: "",
          publisher: rest.slice(0, slash),
          repo: null,
          kind: "remote",
        }
      : {
          title: rest || name,
          tag: "",
          publisher: null,
          repo: null,
          kind: "remote",
        };
  }
  if (name.startsWith(OLLAMA_PREFIX)) {
    const { path, tag } = splitTag(name.slice(OLLAMA_PREFIX.length));
    // Identifiant de repli `@ollama/<hôte>/<espace>/<modèle>:<étiquette>` :
    // le modèle seul en titre.
    return {
      title: path.split("/").at(-1) || path,
      tag,
      publisher: null,
      repo: null,
      kind: "ollama",
    };
  }
  const { path, tag } = splitTag(name);
  const parts = path.split("/");
  // Ancien nom de Hugging Face d'Ollama (`hf.co/<auteur>/<dépôt>`).
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
      kind: "hf",
    };
  }
  // Identifiant de la base locale : `<auteur>/<dépôt>:<QUANT>`.
  if (parts.length === 2 && parts[0] && parts[1] && tag) {
    return {
      title: withoutGgufSuffix(parts[1]),
      tag,
      publisher: parts[0],
      repo: path,
      kind: "hf",
    };
  }
  const title = parts.at(-1) || name;
  const namespace = parts.length >= 2 ? parts.at(-2) : undefined;
  return {
    title,
    tag,
    publisher: namespace && namespace !== "library" ? namespace : null,
    repo: null,
    kind: "other",
  };
}

/**
 * Étiquette à afficher : sans l'extension d'un nom de fichier complet
 * (« Qwen3.8-27B-Q4_K_L.gguf » → « Qwen3.8-27B-Q4_K_L »).
 */
export function displayTag(tag: string): string {
  return tag.replace(/\.gguf$/i, "");
}

/**
 * Nom court à citer dans une phrase : « SmolLM2-135M-Instruct:Q4_K_M » pour
 * un modèle de Hugging Face ou repris d'Ollama, le nom du fournisseur pour
 * un modèle distant, le nom tel quel sinon (« qwen3:0.6b »).
 */
export function shortModelName(model: string): string {
  const name = parseModelName(model);
  switch (name.kind) {
    case "hf":
    case "ollama":
      return name.tag ? `${name.title}:${displayTag(name.tag)}` : name.title;
    case "remote":
      return name.title;
    case "other":
      return model.trim();
  }
}

/**
 * Deux identifiants désignent-ils le même modèle ? Comme le moteur
 * (`llm::ids::same`) : la quantification sans tenir compte de la casse, le
 * reste exactement ; un modèle distant, exactement.
 */
export function sameModel(a: string, b: string): boolean {
  const left = a.trim();
  const right = b.trim();
  if (isRemoteModel(left) || isRemoteModel(right)) return left === right;
  const one = splitTag(left);
  const other = splitTag(right);
  return (
    one.path === other.path && one.tag.toUpperCase() === other.tag.toUpperCase()
  );
}
