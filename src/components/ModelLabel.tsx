// Nom de modèle affiché (sélecteur, ModelHub) : la fin du nom, l'étiquette,
// reste toujours visible et seul le début est tronqué. Un identifiant de
// Hugging Face (`<auteur>/<dépôt>:<QUANT>`) ou d'un modèle repris d'Ollama
// (`@ollama/<nom>:<étiquette>`) devient « titre » + pastille d'étiquette ;
// un modèle distant montre le nom de son fournisseur (nom réglé, donné par
// le sélecteur, sinon son identifiant) ; un ancien nom
// d'Ollama garde son orthographe (« qwen3:0.6b »), coupé en deux morceaux.

import { displayTag, parseModelName } from "@/lib/modelNames";

export function ModelLabel({
  model,
  showPublisher = false,
  publisher,
}: {
  model: string;
  /**
   * Auteur d'un modèle de Hugging Face, fournisseur d'un modèle distant,
   * après le titre, estompé (tronqué en premier).
   */
  showPublisher?: boolean;
  /**
   * Publieur à montrer à la place de celui lu dans l'identifiant (nom réglé
   * d'un fournisseur distant).
   */
  publisher?: string | null;
}) {
  const parsed = parseModelName(model);
  const name = { ...parsed, publisher: publisher ?? parsed.publisher };
  if (name.kind !== "other") {
    return (
      <span className="flex min-w-0 items-center gap-1.5">
        <span className="min-w-0 truncate">
          {name.title}
          {showPublisher && name.publisher && (
            <span className="text-neutral-500">
              {" · "}
              {name.publisher}
            </span>
          )}
        </span>
        {name.tag && (
          <span className="flex-none rounded-md bg-neutral-100 px-1.5 font-mono text-[11px] leading-5 text-neutral-600 dark:bg-neutral-600/50 dark:text-neutral-200">
            {displayTag(name.tag)}
          </span>
        )}
      </span>
    );
  }
  const trimmed = model.trim();
  const colon = name.tag === "" ? -1 : trimmed.lastIndexOf(":");
  return (
    <span className="flex min-w-0 items-baseline">
      <span className="min-w-0 truncate">
        {colon === -1 ? trimmed : trimmed.slice(0, colon)}
      </span>
      {colon !== -1 && (
        <span className="flex-none">{trimmed.slice(colon)}</span>
      )}
    </span>
  );
}
