// Capacités du modèle choisi, lues dans les détails du modèle (/api/show)
// déjà demandés pour le bouton Think (même requête, même cache : relus avec
// la liste des modèles, voir useThink).

import { useQuery } from "@tanstack/react-query";
import { modelDetailsKey } from "@/hooks/useThink";
import { modelDetails } from "@/lib/ipc";

/**
 * Le modèle sait-il lire les images ? null quand on ne le sait pas : aucun
 * modèle, détails pas encore lus ou illisibles (modèle pas encore
 * téléchargé...), serveur trop ancien pour annoncer ses capacités (liste
 * vide). L'officiel prend ces cas pour « non » et retire les images ;
 * ChatonLM les laisse partir, le backend tranchant une fois les capacités
 * connues.
 */
export function useHasVision(model: string | null): boolean | null {
  const name = model ?? "";
  const { data } = useQuery({
    queryKey: modelDetailsKey(name),
    queryFn: () => modelDetails(name),
    enabled: name !== "",
    staleTime: Infinity,
  });
  if (!data || data.capabilities.length === 0) {
    return null;
  }
  return data.capabilities.includes("vision");
}

/**
 * Le modèle sait-il appeler des outils (recherche web) ? null quand on ne le
 * sait pas : aucun modèle, détails pas encore lus ou illisibles (modèle pas
 * encore téléchargé...), serveur trop ancien pour annoncer ses capacités
 * (liste vide).
 */
export function useToolsSupport(model: string | null): boolean | null {
  const name = model ?? "";
  const { data } = useQuery({
    queryKey: modelDetailsKey(name),
    queryFn: () => modelDetails(name),
    enabled: name !== "",
    staleTime: Infinity,
  });
  if (!data || data.capabilities.length === 0) {
    return null;
  }
  return data.capabilities.includes("tools");
}

/**
 * Le modèle sait-il appeler des outils (recherche web) ? Non tant que ses
 * capacités ne sont pas connues, comme l'officiel.
 */
export function useHasTools(model: string | null): boolean {
  return useToolsSupport(model) === true;
}
