// Build du panneau de l'extension VS Code (vscode/webview) : React, Tailwind
// et rendu des réponses de l'app, avec les greffons et les alias de
// vite.config.ts (garde not-prose retirée, polices KaTeX en woff2, grammaire
// GLSL vide, bouchons de mermaid, shiki et rehype-raw, maths llm-math).
// Sortie à noms fixes, lue par l'hôte de l'extension : dist/webview/index.js
// et index.css ; le surligneur reste un morceau chargé à la demande
// (chunks/), les polices de KaTeX dans assets/. Rien de distant.
//
// Usage : node_modules/.bin/vite build -c vscode/webview.vite.config.mts
// Contrôle : node scripts/check-bundle.mjs --webview vscode/dist/webview
import path from "node:path";
import tailwindcss from "@tailwindcss/vite";
import react from "@vitejs/plugin-react";
import { type Alias, defineConfig, type Plugin, type PluginOption } from "vite";
import app from "../vite.config.ts";

const project = path.resolve(import.meta.dirname, "..");
const webview = path.join(project, "vscode/webview");

/** Greffons propres à ChatonLM de vite.config.ts (noms « chatonlm-… »). */
function appPlugins(): Plugin[] {
  const flat = (option: PluginOption): Plugin[] =>
    Array.isArray(option)
      ? option.flatMap(flat)
      : option && typeof option === "object" && "name" in option
        ? [option]
        : [];
  const plugins = (app.plugins ?? [])
    .flatMap(flat)
    .filter((plugin) => plugin.name.startsWith("chatonlm-"));
  const names = plugins.map((plugin) => plugin.name).sort();
  const expected = [
    "chatonlm-empty-glsl",
    "chatonlm-katex-woff2-only",
    "chatonlm-without-not-prose",
  ];
  if (JSON.stringify(names) !== JSON.stringify(expected)) {
    throw new Error(
      `greffons de vite.config.ts inattendus : ${names.join(", ")} au lieu de ${expected.join(", ")}`,
    );
  }
  return plugins;
}

/** Alias de l'app, précédés de l'hôte du panneau (rien de Tauri embarqué). */
function aliases(): Alias[] {
  const own = app.resolve?.alias;
  if (!Array.isArray(own)) {
    throw new Error("alias de vite.config.ts : tableau attendu");
  }
  return [
    {
      find: /^@\/lib\/appHost$/,
      replacement: path.join(webview, "noAppHost.ts"),
    },
    ...own,
  ];
}

export default defineConfig({
  root: webview,
  base: "./",
  publicDir: false,
  plugins: [react(), tailwindcss(), ...appPlugins()],
  resolve: { alias: aliases() },
  clearScreen: false,
  build: {
    outDir: path.join(project, "vscode/dist/webview"),
    emptyOutDir: true,
    sourcemap: false,
    // Pas d'index.html : la page est écrite par l'hôte (CSP à nonce).
    modulePreload: false,
    cssCodeSplit: false,
    // Surligneur ≈ 2 Mo, chargé à part : tailles contrôlées par
    // scripts/check-bundle.mjs --webview.
    chunkSizeWarningLimit: 2100,
    // Paquets embarqués, vérifiés par scripts/check-bundle.mjs.
    license: { fileName: ".vite/license.json" },
    rolldownOptions: {
      input: path.join(webview, "main.tsx"),
      output: {
        entryFileNames: "index.js",
        chunkFileNames: "chunks/[name]-[hash].js",
        assetFileNames: (asset) =>
          asset.names.some((name) => name.endsWith(".css"))
            ? "index.css"
            : "assets/[name]-[hash][extname]",
      },
      onwarn(warning, warn) {
        // Directive React Server Components de Streamdown, sans objet ici.
        if (
          warning.code === "MODULE_LEVEL_DIRECTIVE" &&
          warning.message.includes('"use client"')
        )
          return;
        warn(warning);
      },
    },
  },
});
