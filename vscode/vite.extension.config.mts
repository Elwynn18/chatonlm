// Build de l'hôte de l'extension : bibliothèque Node en CommonJS
// (`dist/extension.cjs`), `vscode` et les modules de Node laissés externes.
// Aucune dépendance à l'exécution : tout le code est le nôtre.
import { builtinModules } from "node:module";
import path from "node:path";
import { defineConfig } from "vite";

const dir = import.meta.dirname;

export default defineConfig({
  root: dir,
  publicDir: false,
  logLevel: "warn",
  build: {
    outDir: path.join(dir, "dist"),
    // dist/webview est écrit par le build du panneau.
    emptyOutDir: false,
    target: "node20",
    minify: false,
    sourcemap: true,
    lib: {
      entry: path.join(dir, "src/extension.ts"),
      formats: ["cjs"],
      fileName: () => "extension.cjs",
    },
    rolldownOptions: {
      external: [
        "vscode",
        ...builtinModules,
        ...builtinModules.map((name) => `node:${name}`),
      ],
    },
  },
});
