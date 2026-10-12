import path from "node:path";
import tailwindcss from "@tailwindcss/vite";
import { tanstackRouter } from "@tanstack/router-plugin/vite";
import react from "@vitejs/plugin-react";
import type { Plugin } from "vite";
import { defineConfig } from "vitest/config";

// Réglages recommandés pour Tauri : https://v2.tauri.app/start/frontend/vite/
const host = process.env.TAURI_DEV_HOST;

const src = path.resolve(import.meta.dirname, "src");
const stub = (file: string) => path.join(src, "lib/stubs", file);

/**
 * Polices KaTeX en woff2 seulement (WebKitGTK les lit) : la feuille de style
 * importée par Streamdown déclare aussi woff et ttf, soit ~900 Ko de plus.
 * Les fichiers de police restent ceux de KaTeX, sans modification (OFL).
 */
function katexWoff2Only(): Plugin {
  const css = /[\\/]node_modules[\\/]katex[\\/]dist[\\/]katex(\.min)?\.css$/;
  const fallbacks =
    /,url\(fonts\/KaTeX_[\w-]+\.woff\) format\("woff"\),url\(fonts\/KaTeX_[\w-]+\.ttf\) format\("truetype"\)/g;
  return {
    name: "chatonlm-katex-woff2-only",
    enforce: "pre",
    transform(code, id) {
      if (!css.test(id.split("?")[0])) return null;
      const out = code.replace(fallbacks, "");
      if (out === code || /format\("(woff|truetype)"\)/.test(out)) {
        this.error(`format de police inattendu dans ${id}`);
      }
      return { code: out, map: null };
    },
  };
}

/**
 * Grammaire GLSL (licence inconnue) remplacée par une grammaire vide : cpp la
 * réclame pour colorer les chaînes R"glsl(...)". Effet visible : ces chaînes
 * et les blocs ```glsl restent d'une seule couleur.
 */
function emptyGlslGrammar(): Plugin {
  const id = "\0chatonlm-empty-glsl";
  return {
    name: "chatonlm-empty-glsl",
    enforce: "pre",
    resolveId(source, importer) {
      const fromLangs =
        importer && /[\\/]@shikijs[\\/]langs[\\/]dist[\\/]/.test(importer);
      return source === "./glsl.mjs" && fromLangs ? id : undefined;
    },
    load(loaded) {
      if (loaded !== id) return undefined;
      return 'export default [{ name: "glsl", scopeName: "source.glsl", patterns: [], repository: {} }];';
    },
  };
}

/**
 * Règles de @tailwindcss/typography sans leur garde « not-prose » :
 * `.prose :where(p):not(:where([class~="not-prose"],[class~="not-prose"] *))`
 * devient `.prose :where(p)`. WebKitGTK évalue cette garde pour chaque
 * élément d'une réponse et chacune des ~200 règles, en remontant tous ses
 * ancêtres : restyler une réponse avec un bloc de 240 lignes (~4 000
 * éléments) prend ~0,5 s avec elle, ~30 ms sans (mesuré dans l'app : bloc
 * suivi d'un paragraphe, fin de réponse, réouverture ; changement de thème
 * 1,2 s → 80 ms). Même rendu : la garde a une spécificité nulle, et la classe
 * not-prose n'est jamais posée (ni dans src/, ni par Streamdown, KaTeX ou le
 * Markdown, sans HTML brut). scripts/check-bundle.mjs vérifie qu'aucune garde
 * ne reste dans la CSS et que src/ n'emploie pas la classe. Appliqué à la CSS
 * que Tailwind vient de générer, en dev comme en build ; une autre forme de
 * la garde (mise à jour de typography) arrête le build.
 */
export function withoutNotProse(): Plugin {
  const guard =
    /:not\(:where\(\[class~=(["']?)not-prose\1\],\s*\[class~=(["']?)not-prose\2\]\s+\*\)\)/g;
  return {
    name: "chatonlm-without-not-prose",
    enforce: "pre",
    transform(code, id) {
      if (!/\.css$/.test(id.split("?")[0]) || !code.includes("not-prose")) {
        return null;
      }
      const out = code.replace(guard, "");
      if (out.includes("not-prose")) {
        this.error(`garde not-prose de forme inattendue dans ${id}`);
      }
      return { code: out, map: null };
    },
  };
}

export default defineConfig({
  plugins: [
    // Routes par fichiers (src/routes), comme l'app officielle ; génère
    // src/routeTree.gen.ts, versionné pour que tsc le voie avant tout build.
    tanstackRouter({ target: "react", quoteStyle: "double" }),
    react(),
    tailwindcss(),
    // Après tailwindcss() : agit sur la CSS qu'il vient de générer.
    withoutNotProse(),
    katexWoff2Only(),
    emptyGlslGrammar(),
  ],
  resolve: {
    // Expressions régulières : une clé « shiki » toucherait aussi shiki/core.
    alias: [
      { find: "@", replacement: src },
      { find: /^shiki$/, replacement: stub("shiki.ts") },
      { find: /^mermaid$/, replacement: stub("mermaid.ts") },
      { find: /^rehype-raw$/, replacement: stub("rehype-raw.ts") },
      // Maths des modèles : \(…\) et \[…\] en plus de $$…$$, comme l'officiel.
      {
        find: /^micromark-extension-math$/,
        replacement: "micromark-extension-llm-math",
      },
    ],
  },
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host ? { protocol: "ws", host, port: 1421 } : undefined,
    watch: { ignored: ["**/src-tauri/**", "**/.tools/**"] },
    fs: {
      // Valeurs par défaut de Vite, plus .tools/ (jeton GitHub, clés de
      // l'instance Ollama de test, données de dev) : le serveur de dev ne
      // doit jamais les servir.
      deny: [
        ".env",
        ".env.*",
        "*.{crt,pem,key,p12,pfx,cer,der}",
        ".npmrc",
        ".yarnrc.yml",
        "**/.git/**",
        "**/.tools/**",
      ],
    },
  },
  // Sans cela, Vite parcourt tous les .html du projet, y compris ceux de .tools/.
  optimizeDeps: { entries: ["index.html"] },
  envPrefix: ["VITE_", "TAURI_ENV_"],
  build: {
    sourcemap: process.env.TAURI_ENV_DEBUG === "true",
    // Morceau du surligneur ≈ 2 Mo (grammaires), entrée ≈ 1 Mo : tailles
    // contrôlées par scripts/check-bundle.mjs, l'avertissement de Vite ne
    // serait que du bruit.
    chunkSizeWarningLimit: 2100,
    // Liste des paquets npm embarqués, vérifiée par scripts/check-npm-licenses.mjs.
    license: { fileName: ".vite/license.json" },
    rolldownOptions: {
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
  test: {
    environment: "jsdom",
    include: [
      "src/**/*.test.{ts,tsx}",
      "scripts/**/*.test.mjs",
      "site/**/*.test.mjs",
    ],
    setupFiles: ["src/test/setup.ts"],
    // Passés par Vite : CSS de KaTeX importée par Streamdown, alias llm-math
    // vu par remark-math.
    server: { deps: { inline: ["streamdown", "remark-math"] } },
  },
});
