// Ressassement probable d'un modèle pendant son raisonnement (propre à
// ChatonLM) : il revient sans fin sur les mêmes vérifications, sans se répéter
// mot pour mot (une répétition exacte est arrêtée par le moteur, voir
// `src-tauri/src/chat/repetition.rs`). Rien n'est arrêté ici : la page le
// signale (bandeau avec Stop), une longue réflexion légitime pouvant y
// ressembler.
//
// Mesuré sur une vraie boucle (Qwen3.8 27B, 14 oct. 2026 : 149 paragraphes
// sur 689 ouverts par « Need maybe if », le même script réécrit dix fois) :
// le premier signe la repère vers 21 000 caractères, environ 4 min 30.

/** Raisonnement plus court : jamais signalé (environ 3 500 mots). */
const MIN_LENGTH = 20_000;

/** Fin du raisonnement examinée : coût borné à chaque fragment. */
const TAIL = 30_000;

/** Dernières lignes examinées pour les ouvertures de paragraphe. */
const LINES = 200;

/** Même ouverture de paragraphe (trois premiers mots) : fois, et part. */
const OPENER_COUNT = 15;
const OPENER_SHARE = 0.1;

/**
 * Même ligne (au moins 12 caractères, hors clôture de code) dans toute la fin
 * examinée : fois.
 */
const LINE_COUNT = 6;

function mostFrequent(items: readonly string[]): number {
  const counts = new Map<string, number>();
  let best = 0;
  for (const item of items) {
    const count = (counts.get(item) ?? 0) + 1;
    counts.set(item, count);
    if (count > best) best = count;
  }
  return best;
}

/** Le raisonnement `thinking`, reçu jusqu'ici, semble tourner en rond. */
export function isRuminating(thinking: string): boolean {
  if (thinking.length < MIN_LENGTH) return false;
  const lines = thinking
    .slice(-TAIL)
    .split("\n")
    .map((line) => line.trim().replace(/\s+/g, " "))
    .filter((line) => line !== "");
  // Phrases d'au moins six mots : une ligne de code ou de liste courte
  // n'ouvre pas un paragraphe de réflexion.
  const openers = lines
    .slice(-LINES)
    .map((line) => line.split(" "))
    .filter((words) => words.length >= 6)
    .map((words) => words.slice(0, 3).join(" ").toLowerCase());
  const opener = mostFrequent(openers);
  if (opener >= OPENER_COUNT && opener >= openers.length * OPENER_SHARE) {
    return true;
  }
  const repeated = lines.filter(
    (line) => line.length >= 12 && !line.startsWith("```"),
  );
  return mostFrequent(repeated) >= LINE_COUNT;
}
