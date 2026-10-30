// Reçu de ce qui part au modèle, et ce qu'on retire avant un envoi hors de
// cette machine (modèle cloud, Ollama d'une autre machine, destination
// inconnue) : fichiers sensibles exclus, lignes à secret masquées, au mieux
// (motifs connus). En local, rien n'est retiré (tout reste sur la machine). Contrat :
// .tools/research/87-lot1b-contract.md (B, « Reçu »).
import path from "node:path";
import type { Receipt, ReceiptItem } from "../shared/protocol";

export type Destination = Receipt["destination"];
export type Blocked = Receipt["blocked"][number];

/** Morceau de l'espace de travail joint à un envoi. */
export type Piece = {
  kind: ReceiptItem["kind"];
  /** Nom montré dans le reçu (chemin relatif, ou « Staged changes »…). */
  label: string;
  path?: string;
  lines?: [number, number];
  text: string;
  /** Langage du bloc de code dans le texte envoyé. */
  language?: string;
};

/** Noms de fichiers qui portent souvent des secrets. */
const SECRET_FILES = [
  /^\.env(\..*)?$/i,
  /^\.envrc$/i,
  /\.env$/i,
  /\.pem$/i,
  /\.key$/i,
  /^id_(rsa|dsa|ecdsa|ed25519)/i,
  /\.p12$/i,
  /\.pfx$/i,
  /\.jks$/i,
  /\.keystore$/i,
  /^\.npmrc$/i,
  /^\.pypirc$/i,
  /^\.netrc$/i,
  /^\.git-credentials$/i,
  /\.tfvars(\.json)?$/i,
  /^kubeconfig/i,
  /^credentials/i,
  /^secrets\./i,
];

export function isSecretFile(file: string): boolean {
  const name = path.posix.basename(file.replaceAll("\\", "/"));
  return SECRET_FILES.some((pattern) => pattern.test(name));
}

/** Valeurs qui ressemblent à un secret, sur une ligne. */
const SECRET_VALUES = [
  /\bAKIA[0-9A-Z]{16}\b/,
  /\bgh[pousr]_[A-Za-z0-9]{36,}/,
  /\bgithub_pat_[A-Za-z0-9_]{20,}/,
  /\bglpat-[A-Za-z0-9_-]{20,}/,
  /\bsk-[A-Za-z0-9_-]{20,}/,
  /\b[sr]k_(live|test)_[A-Za-z0-9]{10,}/,
  /\bAIza[0-9A-Za-z_-]{35}/,
  /\bxox[baprs]-[A-Za-z0-9-]{10,}/,
  // Identifiants dans une adresse : postgres://user:mot-de-passe@hôte
  /\b[a-z][a-z0-9+.-]*:\/\/[^/\s:@]+:[^@\s]+@/i,
  // password = "…", "apiKey": '…', DB_TOKEN: "…", OPENAI_KEY="…"
  // (le nom finit par le mot : « tokenizer = "…" » n'est pas un secret)
  /(pass(word|wd)?|secret([_-]?key)?|token|api[_-]?key|private[_-]?key|access[_-]?key|[_-]key)["']?\s*[:=]\s*["'][^"'\s]{4,}["']/i,
  // password: hunter2 (YAML, INI, sans guillemets : une seule valeur jusqu'à
  // la fin de la ligne, sans parenthèse ni point-virgule, et ni un nom de
  // type, ni une valeur vide, ni un attribut comme `request.form` : pas du
  // code)
  /^[\s+-]*[\w.-]*(pass(word|wd)?|secret|token|api[_-]?key)\s*[:=]\s*(?!(null|none|nil|true|false|undefined|string|number|str|bool|boolean|int|required|optional)\s*$)(?![A-Za-z_]\w*(\.[A-Za-z_]\w*)+\s*$)[^\s"'(){};,]{4,}\s*$/i,
  // API_KEY=…, OPENAI_KEY=… (sans guillemets, en capitales comme dans un
  // fichier d'environnement)
  /\b[A-Z0-9_]*(PASSWORD|SECRET|TOKEN|_KEY)[A-Z0-9_]*=[^\s"']{4,}/,
];
const KEY_BEGIN = /-----BEGIN [A-Z0-9 ]*PRIVATE KEY( BLOCK)?-----/;
const KEY_END = /-----END [A-Z0-9 ]*PRIVATE KEY( BLOCK)?-----/;
/** Ligne pleine d'une clé en base64 (PEM : 64 caractères), préfixe de diff
 * permis ; trois d'affilée = un morceau de clé, même sans sa ligne BEGIN. */
const KEY_LINE = /^[+\- ]?\s*[A-Za-z0-9+/]{64}\s*$/;
const KEY_LAST_LINE = /^[+\- ]?\s*[A-Za-z0-9+/]{1,64}={0,2}\s*$/;
const KEY_RUN = 3;

export function hasSecretValue(line: string): boolean {
  return SECRET_VALUES.some((pattern) => pattern.test(line));
}

/** Lignes masquées (numéros à partir de 1 dans `text`). */
export function maskSecrets(
  text: string,
  marker: string,
): { text: string; masked: [number, number][] } {
  const lines = text.split("\n");
  const masked: [number, number][] = [];
  for (let i = 0; i < lines.length; i++) {
    if (KEY_BEGIN.test(lines[i])) {
      let end = i;
      while (end < lines.length - 1 && !KEY_END.test(lines[end])) end++;
      for (let j = i; j <= end; j++) lines[j] = marker;
      masked.push([i + 1, end + 1]);
      i = end;
      continue;
    }
    let run = i;
    while (run < lines.length && KEY_LINE.test(lines[run])) run++;
    if (run - i >= KEY_RUN) {
      // Corps d'une clé sélectionné sans ses lignes BEGIN et END.
      if (run < lines.length && KEY_LAST_LINE.test(lines[run])) run++;
      for (let j = i; j < run; j++) lines[j] = marker;
      masked.push([i + 1, run]);
      i = run - 1;
    } else if (hasSecretValue(lines[i])) {
      // Le début de la ligne reste (indentation, nom, préfixe d'un diff).
      const keep = /^[+\- ]?\s*/.exec(lines[i])?.[0] ?? "";
      lines[i] = keep + marker;
      masked.push([i + 1, i + 1]);
    }
  }
  return { text: lines.join("\n"), masked };
}

/**
 * Chemin cité par git (`"b/caf\303\251.txt"` : nom non ASCII, espaces…),
 * octets en octal décodés en UTF-8.
 */
export function unquoteGitPath(quoted: string): string {
  const bytes: number[] = [];
  const body = quoted.slice(1, -1);
  const escapes: Record<string, string> = {
    a: "\x07",
    b: "\b",
    f: "\f",
    n: "\n",
    r: "\r",
    t: "\t",
    v: "\v",
  };
  for (let i = 0; i < body.length; i++) {
    const c = body[i];
    if (c !== "\\" || i === body.length - 1) {
      bytes.push(...Buffer.from(c, "utf8"));
      continue;
    }
    const next = body[i + 1];
    const octal = /^[0-7]{3}/.exec(body.slice(i + 1, i + 4));
    if (octal) {
      bytes.push(parseInt(octal[0], 8));
      i += 3;
    } else {
      bytes.push(...Buffer.from(escapes[next] ?? next, "utf8"));
      i += 1;
    }
  }
  return Buffer.from(bytes).toString("utf8");
}

/** Chemin d'une section de diff, d'après son en-tête `diff --git`, cité
 * par git ou non. */
function diffPath(section: string): string {
  const plain = /^diff --git a\/(.+?) b\/(.+)$/m.exec(section);
  if (plain) return plain[2];
  const quoted =
    /^diff --git (?:"(?:[^"\\]|\\.)*"|\S+) ("(?:[^"\\]|\\.)*")$/m.exec(section);
  if (!quoted) return "";
  const path = unquoteGitPath(quoted[1]);
  return path.startsWith("b/") ? path.slice(2) : path;
}

/** Sections d'un diff git, par fichier. */
export function splitDiff(diff: string): { file: string; text: string }[] {
  const sections: { file: string; text: string }[] = [];
  for (const part of diff.split(/^(?=diff --git )/m)) {
    if (!part) continue;
    sections.push({ file: diffPath(part), text: part });
  }
  return sections;
}

export type Labels = {
  /** Texte qui remplace une ligne à secret. */
  marker: string;
  /** « chemin, ligne N » / « chemin, lignes N–M ». */
  lines(label: string, first: number, last: number): string;
};

/**
 * Ce qui part vraiment : en cloud, fichiers sensibles retirés et secrets
 * masqués ; en local, les morceaux tels quels.
 */
export function filterPieces(
  pieces: Piece[],
  destination: Destination,
  labels: Labels,
): { pieces: Piece[]; blocked: Blocked[] } {
  // Seule une destination sûre sur cette machine reçoit tout tel quel ;
  // une destination inconnue est traitée comme hors de la machine.
  if (destination === "local") return { pieces, blocked: [] };
  const kept: Piece[] = [];
  const blocked: Blocked[] = [];
  for (const piece of pieces) {
    if (piece.kind === "diff") {
      const sections: string[] = [];
      for (const section of splitDiff(piece.text)) {
        if (section.file && isSecretFile(section.file)) {
          blocked.push({ label: section.file, reason: "secretFile" });
          continue;
        }
        const { text, masked } = maskSecrets(section.text, labels.marker);
        // Numéros de lignes du diff, pas du fichier : le fichier seul.
        if (masked.length > 0) {
          blocked.push({
            label: section.file || piece.label,
            reason: "secretValue",
          });
        }
        sections.push(text);
      }
      if (sections.length > 0) kept.push({ ...piece, text: sections.join("") });
      continue;
    }
    if (piece.path && isSecretFile(piece.path)) {
      blocked.push({ label: piece.path, reason: "secretFile" });
      continue;
    }
    const { text, masked } = maskSecrets(piece.text, labels.marker);
    const offset = (piece.lines?.[0] ?? 1) - 1;
    for (const [first, last] of masked) {
      blocked.push({
        label: labels.lines(
          piece.path ?? piece.label,
          first + offset,
          last + offset,
        ),
        reason: "secretValue",
      });
    }
    kept.push({ ...piece, text });
  }
  return { pieces: kept, blocked };
}

/** Jetons estimés : caractères / 4, arrondi au-dessus. */
export function estimateTokens(chars: number): number {
  return Math.ceil(chars / 4);
}

/** Historique envoyé avec un message : messages précédents et caractères. */
export type History = { messages: number; chars: number };

/** Historique d'une conversation (texte, raisonnement et résultats d'outils
 * repartent au modèle). */
export function historyOf(
  messages: readonly {
    content: string;
    thinking?: string;
    toolCalls?: readonly { result?: string | null }[];
  }[],
): History {
  let chars = 0;
  for (const message of messages) {
    chars += message.content.length + (message.thinking?.length ?? 0);
    for (const call of message.toolCalls ?? []) {
      chars += call.result?.length ?? 0;
    }
  }
  return { messages: messages.length, chars };
}

/**
 * Reçu d'un envoi : éléments joints, caractères du nouveau texte, jetons
 * estimés pour tout ce qui part (historique compris), destination.
 */
export function buildReceipt(
  content: string,
  pieces: Piece[],
  blocked: Blocked[],
  destination: Destination,
  contextLength: number | null,
  history: History | null = null,
): Receipt {
  const items: ReceiptItem[] = pieces.map((piece) => ({
    kind: piece.kind,
    label: piece.label,
    ...(piece.path !== undefined ? { path: piece.path } : {}),
    ...(piece.lines ? { lines: piece.lines } : {}),
    chars: piece.text.length,
  }));
  const earlier = history && history.messages > 0 ? history : null;
  return {
    items,
    chars: content.length,
    tokens: estimateTokens(content.length + (earlier?.chars ?? 0)),
    contextLength,
    destination,
    ...(earlier ? { history: earlier } : {}),
    blocked,
  };
}

/** Modèle cloud d'après son nom, quand l'app n'a pas pu le dire. */
export function looksRemote(model: string): boolean {
  return /[:-]cloud$/i.test(model);
}
