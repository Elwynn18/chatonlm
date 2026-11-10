// Scripts exécutés dans la page de l'app réelle (WebDriver execute).

/**
 * Déclare `pending` : identifiants des rappels de Tauri qui attendent la
 * réponse d'un appel au backend. Les écouteurs d'évènements (dépôt de
 * fichiers du composer...) restent inscrits tant que la page vit : ils sont
 * écartés, d'après le registre où Tauri les note (propriétés non
 * énumérables de window.__internal_unstable_listeners_object_id__). Pour
 * recharger la page seulement quand aucun appel n'est en cours : la réponse
 * arriverait dans la nouvelle page (« Couldn't find callback id »).
 */
export const PENDING_CALLS = `
  const listening = new Set();
  const registry = window.__internal_unstable_listeners_object_id__ ?? {};
  for (const event of Object.getOwnPropertyNames(registry)) {
    for (const id of Object.getOwnPropertyNames(registry[event])) {
      listening.add(registry[event][id].handlerId);
    }
  }
  const pending = [...window.__TAURI_INTERNALS__.callbacks.keys()]
    .filter((id) => !listening.has(id));`;

/**
 * Identifiant du modèle montré par le sélecteur de la zone de saisie
 * (`data-model-id` du bouton ; « » sans modèle, null sans sélecteur). Le
 * texte du bouton n'est que le titre et l'étiquette du modèle.
 */
export const shownModel = (driver) =>
  driver.execute(
    "return document.querySelector('[title=\"Select model\"]')?.dataset.modelId ?? null",
  );
