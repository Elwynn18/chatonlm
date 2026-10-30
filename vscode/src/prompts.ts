// Texte envoyé par une action : consigne courte en tête, puis chaque morceau
// (chemin relatif et lignes, code dans un bloc). Construit dans l'extension :
// rien n'est modifié dans les fichiers (pas d'agent).
import type { Piece } from "./receipt";

/** Plus grand diff envoyé ; au-delà, il est coupé (et le texte le dit). */
export const MAX_DIFF_CHARS = 60_000;

/** Bloc de code dont la clôture ne peut pas se confondre avec le contenu. */
export function fence(text: string, language = ""): string {
  const longest = Math.max(
    2,
    ...Array.from(text.matchAll(/`{3,}/g), (m) => m[0].length),
  );
  const ticks = "`".repeat(longest + 1);
  const body = text.endsWith("\n") ? text : `${text}\n`;
  return `${ticks}${language}\n${body}${ticks}`;
}

/** Consigne, puis chaque morceau sous son titre (`heading`). */
export function compose(
  instruction: string,
  pieces: { piece: Piece; heading: string }[],
): string {
  const parts = [instruction.trim()];
  for (const { piece, heading } of pieces) {
    parts.push(`${heading}\n${fence(piece.text, piece.language ?? "")}`);
  }
  return parts.join("\n\n");
}

/** Diff gardé sous la limite, coupé à la fin d'une ligne. */
export function truncateDiff(
  diff: string,
  max = MAX_DIFF_CHARS,
): { text: string; truncated: boolean } {
  if (diff.length <= max) return { text: diff, truncated: false };
  const cut = diff.lastIndexOf("\n", max);
  return { text: diff.slice(0, cut > 0 ? cut + 1 : max), truncated: true };
}

/**
 * Message de commit tiré d'une réponse : sans raisonnement écrit en clair,
 * sans bloc de code ni guillemets autour.
 */
export function commitMessageFrom(reply: string): string {
  let text = reply.replace(/<think>[\s\S]*?<\/think>/g, "").trim();
  const block = /^(`{3,})[^\n]*\n([\s\S]*?)\n\1\s*$/.exec(text);
  if (block) text = block[2].trim();
  const quoted = /^"([\s\S]*)"$/.exec(text);
  if (quoted) text = quoted[1].trim();
  return text;
}
