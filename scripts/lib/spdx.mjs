// Évaluation des expressions de licence SPDX contre la liste blanche du projet.

/** Licences acceptées pour du code livré avec ChatonLM. */
export const ALLOWED = new Set([
  "MIT",
  "Apache-2.0",
  "BSD-2-Clause",
  "BSD-3-Clause",
  "ISC",
  "0BSD",
  "Zlib",
  "Unicode-3.0",
  "CC0-1.0",
  "Unlicense",
  "BSL-1.0",
  "MPL-2.0",
  "OFL-1.1",
  "BlueOak-1.0.0",
  "Python-2.0",
]);

const OPERATORS = new Set(["AND", "OR", "WITH", "(", ")"]);

/**
 * Vrai si l'expression est autorisée : OR suffit qu'une branche le soit, AND
 * exige les deux. Toute expression mal formée, ou utilisant WITH, un suffixe +
 * ou LicenseRef-, est refusée (du côté sûr).
 */
export function isAllowed(expression, allowed = ALLOWED) {
  const tokens = expression.match(/\(|\)|[^\s()]+/g) ?? [];
  let position = 0;
  let malformed = false;

  function parseOr() {
    let value = parseAnd();
    while (tokens[position] === "OR") {
      position += 1;
      const right = parseAnd();
      value = value || right;
    }
    return value;
  }

  function parseAnd() {
    let value = parseTerm();
    while (tokens[position] === "AND") {
      position += 1;
      const right = parseTerm();
      value = value && right;
    }
    return value;
  }

  function parseTerm() {
    const token = tokens[position++];
    if (token === "(") {
      const value = parseOr();
      if (tokens[position] !== ")") {
        malformed = true;
        return false;
      }
      position += 1;
      return value;
    }
    if (token === undefined || OPERATORS.has(token)) {
      malformed = true;
      return false;
    }
    return allowed.has(token);
  }

  if (tokens.length === 0) return false;
  const value = parseOr();
  return value && !malformed && position === tokens.length;
}
