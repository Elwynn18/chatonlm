#!/usr/bin/env node
// Résumé d'un relevé de contrastes (contrast.jsonl écrit par les captures e2e
// lancées avec E2E_THEME_AUDIT, voir e2e/lib/contrast.mjs) : les textes sous
// le rapport minimal, regroupés par thème, couleurs et classes.
//   node scripts/contrast-report.mjs <dossier du relevé> [--all]
// Sort avec le code 1 s'il reste un texte à lire sous 4,5:1 (les invites des
// champs vides et les contrôles désactivés sont listés à part, sans échec).
import { readFileSync } from "node:fs";

const [dir, option] = process.argv.slice(2);
if (!dir) {
  console.error("Usage : contrast-report.mjs <dossier du relevé> [--all]");
  process.exit(2);
}
const MIN = 4.5;
const entries = readFileSync(`${dir}/contrast.jsonl`, "utf8")
  .split("\n")
  .filter(Boolean)
  .map((line) => JSON.parse(line));

const groups = new Map();
for (const entry of entries) {
  if (entry.ratio >= MIN && option !== "--all") continue;
  const key = [
    entry.theme,
    entry.kind,
    entry.color,
    entry.background,
    entry.classes,
  ].join("|");
  const group = groups.get(key) ?? { ...entry, shots: new Set() };
  group.shots.add(entry.shot);
  group.ratio = Math.min(group.ratio, entry.ratio);
  groups.set(key, group);
}

const sorted = [...groups.values()].sort(
  (a, b) =>
    a.theme.localeCompare(b.theme) ||
    a.kind.localeCompare(b.kind) ||
    a.ratio - b.ratio,
);
for (const group of sorted) {
  const shots = [...group.shots];
  console.log(
    [
      group.theme.padEnd(5),
      group.kind.padEnd(11),
      group.ratio.toFixed(2).padStart(5),
      `${group.color} sur ${group.background}${group.gradient ? " (dégradé)" : ""}`,
      `${group.size}px/${group.weight}`,
      `« ${group.text} »`,
      `<${group.tag} class="${group.classes.slice(0, 150)}">`,
      `[${shots.slice(0, 3).join(", ")}${shots.length > 3 ? `, +${shots.length - 3}` : ""}]`,
    ].join("  "),
  );
}
const failing = sorted.filter(
  (group) => group.kind === "text" && group.ratio < MIN,
);
console.log(
  `${entries.length} textes mesurés, ${failing.length} groupe(s) de texte sous ${MIN}:1`,
);
process.exit(failing.length > 0 ? 1 : 0);
