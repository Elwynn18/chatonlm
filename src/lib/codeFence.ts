// Blocs de code entre trois accents graves (```), comme dans Discord ou
// Claude : la zone de saisie garde Entrée pour une nouvelle ligne tant qu'un
// bloc est ouvert, et les messages envoyés montrent ces blocs comme ceux des
// réponses. Mêmes règles que CommonMark (celles du rendu des réponses et de
// ce que lit le modèle) : clôture en début de ligne (trois espaces au plus
// devant), trois accents graves ou plus, refermée par une ligne d'au moins
// autant d'accents graves et rien d'autre ; une ouverture dont la suite
// contient un accent grave n'en est pas une (« ```code``` » sur une ligne est
// du code en ligne).

const FENCE = /^ {0,3}(`{3,})(.*)$/;

/** Ouverture d'un bloc : longueur de la clôture et langage, sinon null. */
function opening(line: string): { size: number; language: string } | null {
  const match = FENCE.exec(line);
  if (!match || match[2].includes("`")) return null;
  return { size: match[1].length, language: match[2].trim().split(/\s+/)[0] };
}

/** La ligne referme-t-elle un bloc ouvert par `size` accents graves ? */
function closes(line: string, size: number): boolean {
  const match = FENCE.exec(line);
  return match !== null && match[1].length >= size && match[2].trim() === "";
}

/**
 * Un bloc de code est-il ouvert (et pas encore refermé) à la position du
 * curseur ? La ligne du curseur compte jusqu'au curseur : juste après
 * « ```python », le bloc est ouvert ; juste après la clôture, il est fermé.
 */
export function insideOpenFence(text: string, caret = text.length): boolean {
  let open: number | null = null;
  for (const line of text.slice(0, caret).split("\n")) {
    if (open === null) open = opening(line)?.size ?? null;
    else if (closes(line, open)) open = null;
  }
  return open !== null;
}

export type FencePart =
  | { kind: "text"; text: string }
  | { kind: "code"; language: string; code: string };

/**
 * Découpe un message en texte et en blocs de code. Le saut de ligne qui
 * précède un bloc et celui de sa clôture sont retirés (le bloc fait déjà
 * sa ligne) ; un bloc jamais refermé va jusqu'à la fin, comme dans
 * CommonMark. Le code garde son saut de ligne final, comme celui que
 * react-markdown donne aux blocs des réponses.
 */
export function splitFences(content: string): FencePart[] {
  const parts: FencePart[] = [];
  const lines = content.split("\n");
  let text: string[] = [];
  let block: { size: number; language: string; lines: string[] } | null = null;
  const flushText = () => {
    if (text.length > 0) parts.push({ kind: "text", text: text.join("\n") });
    text = [];
  };
  // Les lignes de clôture ne vont dans aucune partie : leurs sauts de ligne
  // disparaissent avec elles.
  for (const line of lines) {
    if (block === null) {
      const open = opening(line);
      if (open) {
        flushText();
        block = { ...open, lines: [] };
      } else {
        text.push(line);
      }
    } else if (closes(line, block.size)) {
      parts.push(codePart(block));
      block = null;
    } else {
      block.lines.push(line);
    }
  }
  if (block !== null) parts.push(codePart(block));
  else flushText();
  // Ligne vide laissée par la clôture en toute fin de message.
  return parts.filter((part) => part.kind === "code" || part.text !== "");
}

function codePart(block: { language: string; lines: string[] }): FencePart {
  const code = block.lines.join("\n");
  return {
    kind: "code",
    language: block.language,
    code: code === "" ? "" : `${code}\n`,
  };
}

export type InlinePart = { kind: "text" | "code"; text: string };

/**
 * Code en ligne entre accents graves (`ainsi`, ou ``avec ` dedans``) : une
 * suite d'accents graves s'ouvre et se referme par une suite de même
 * longueur ; sans fermeture, elle reste du texte. Une espace de chaque côté
 * est retirée quand les deux y sont, comme dans CommonMark.
 */
export function splitInlineCode(text: string): InlinePart[] {
  const parts: InlinePart[] = [];
  let rest = text;
  let plain = "";
  for (;;) {
    const start = /`+/.exec(rest);
    if (!start) break;
    const ticks = start[0];
    const after = rest.slice(start.index + ticks.length);
    // Fermeture : exactement autant d'accents graves, pas plus.
    const close = new RegExp(`(?<!\`)${ticks}(?!\`)`).exec(after);
    if (!close || close.index === 0) {
      plain += rest.slice(0, start.index + ticks.length);
      rest = after;
      continue;
    }
    plain += rest.slice(0, start.index);
    if (plain) parts.push({ kind: "text", text: plain });
    plain = "";
    let code = after.slice(0, close.index);
    if (/^ .*[^ ].* $/s.test(code)) code = code.slice(1, -1);
    parts.push({ kind: "code", text: code });
    rest = after.slice(close.index + ticks.length);
  }
  plain += rest;
  if (plain) parts.push({ kind: "text", text: plain });
  return parts;
}

/** Le message contient-il un bloc de code (bulle élargie) ? */
export function hasCodeBlock(content: string): boolean {
  return splitFences(content).some((part) => part.kind === "code");
}
