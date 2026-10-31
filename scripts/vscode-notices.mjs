// Notices de l'extension VS Code, écrites dans vscode/dist/THIRD_PARTY_NOTICES.txt
// avant l'empaquetage (vsce package, .github/workflows/vscode-publish.yml) :
// THIRD_PARTY_LICENSES du dépôt (code repris d'Ollama, réutilisé par le
// panneau), puis les paquets npm compilés dans le panneau
// (vscode/dist/webview/.vite/license.json, écrit par son build) et ceux qu'y
// fait entrer le CSS, au format de licenses/npm-packages.txt des paquets de
// l'app. L'hôte et le serveur MCP n'embarquent aucun paquet : tout leur code
// est le nôtre.
//
// Usage, après les deux builds de l'extension :
//   node scripts/vscode-notices.mjs
import { existsSync, readFileSync, writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import {
  CSS_PACKAGES,
  nodePackage,
  renderNpmPackages,
} from "./package-notices.mjs";

const root = path.resolve(import.meta.dirname, "..");

export const LICENSE_LIST = "vscode/dist/webview/.vite/license.json";
export const NOTICES_FILE = "vscode/dist/THIRD_PARTY_NOTICES.txt";

const RULE = "=".repeat(78);

/** Texte complet des notices de l'extension. */
export function renderVscodeNotices({ thirdParty, npmPackages }) {
  return [
    "ChatonLM for VS Code - Third-party notices",
    RULE,
    "",
    "The extension is distributed under the MIT License (see LICENSE). Its",
    "chat panel reuses the user interface code of the ChatonLM app: the notices",
    "of the app's source code follow, then the npm packages compiled into the",
    "panel. The rest of the extension embeds no third-party package.",
    "",
    "",
    thirdParty.trimEnd(),
    "",
    "",
    npmPackages.trimEnd(),
    "",
  ].join("\n");
}

function main() {
  const list = path.join(root, LICENSE_LIST);
  if (!existsSync(list)) {
    console.error(
      `${LICENSE_LIST} introuvable : lancer d'abord vite build -c vscode/webview.vite.config.mts.`,
    );
    process.exit(1);
  }
  const read = (file) => readFileSync(path.join(root, file), "utf8");
  const text = renderVscodeNotices({
    thirdParty: read("THIRD_PARTY_LICENSES"),
    npmPackages: renderNpmPackages(JSON.parse(read(LICENSE_LIST)), {
      css: CSS_PACKAGES.map(nodePackage),
      apacheText: read("packaging/licenses/Apache-2.0.txt"),
    }),
  });
  writeFileSync(path.join(root, NOTICES_FILE), text);
  console.log(`Notices écrites : ${NOTICES_FILE}`);
}

if (process.argv[1] === fileURLToPath(import.meta.url)) main();
