// Build de l'hôte de l'extension : bibliothèque Node en CommonJS
// (`dist/extension.cjs`), `vscode` et les modules de Node laissés externes ;
// puis le serveur MCP lancé par les agents (`dist/mcp.cjs`, sans `vscode`),
// construit à part pour rester un fichier autonome (sans morceau partagé
// avec l'hôte). Aucune dépendance à l'exécution : tout le code est le nôtre.
import { builtinModules } from "node:module";
import path from "node:path";
import { build, defineConfig, type InlineConfig, type Plugin } from "vite";

const dir = import.meta.dirname;

/** Build d'une entrée en un seul fichier CommonJS pour Node 20. */
function nodeLibrary(entry: string, fileName: string, external: string[]) {
  return {
    root: dir,
    publicDir: false,
    logLevel: "warn",
    build: {
      outDir: path.join(dir, "dist"),
      // dist/webview est écrit par le build du panneau, dist/mcp.cjs et
      // dist/extension.cjs chacun par le leur.
      emptyOutDir: false,
      target: "node20",
      minify: false,
      sourcemap: true,
      lib: {
        entry: path.join(dir, entry),
        formats: ["cjs"],
        fileName: () => fileName,
      },
      rolldownOptions: {
        external: [
          ...external,
          ...builtinModules,
          ...builtinModules.map((name) => `node:${name}`),
        ],
      },
    },
  } satisfies InlineConfig;
}

/** Serveur MCP construit après l'hôte, sans `vscode` (il tourne hors de
 * l'éditeur : une importation de `vscode` y échouerait). */
function mcpServer(): Plugin {
  return {
    name: "chatonlm-mcp-server",
    apply: "build",
    async closeBundle() {
      await build({
        configFile: false,
        ...nodeLibrary("src/mcp/main.ts", "mcp.cjs", []),
      });
    },
  };
}

export default defineConfig({
  ...nodeLibrary("src/extension.ts", "extension.cjs", ["vscode"]),
  plugins: [mcpServer()],
});
