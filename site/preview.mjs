#!/usr/bin/env node
// Aperçu du site sur cette machine : fabrique site/dist/, puis le sert sur
// http://127.0.0.1:4321 comme le fera nginx (langue de l'accueil d'après le
// navigateur, page 404, service du formulaire et des téléchargements).
//
//   node site/preview.mjs [--port 4321] [--release]
//
// Rien n'est envoyé : un message du formulaire est affiché dans ce terminal.
// Sans --release, le site est celui d'avant la première version (« bientôt
// disponible ») ; avec --release, une version 0.9.0 fictive est annoncée.

import { createReadStream } from "node:fs";
import { stat } from "node:fs/promises";
import { createServer } from "node:http";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { build } from "./build.mjs";
import { configFromEnv, createService, pickAssets } from "./server/service.mjs";

const ROOT = path.dirname(fileURLToPath(import.meta.url));
const DIST = path.join(ROOT, "dist");
const TYPES = {
  ".html": "text/html; charset=utf-8",
  ".css": "text/css; charset=utf-8",
  ".js": "text/javascript; charset=utf-8",
  ".json": "application/json; charset=utf-8",
  ".svg": "image/svg+xml",
  ".png": "image/png",
  ".webp": "image/webp",
  ".ico": "image/x-icon",
  ".woff2": "font/woff2",
  ".xml": "application/xml; charset=utf-8",
  ".txt": "text/plain; charset=utf-8",
};
// La politique de sécurité de nginx-chatonlm.conf, pour voir ici ce qu'elle bloquerait.
const CSP =
  "default-src 'self'; img-src 'self' data:; object-src 'none'; base-uri 'self'; form-action 'self'; frame-ancestors 'none'";

const args = process.argv.slice(2);
const port = Number(args[args.indexOf("--port") + 1]) || 4321;
const origin = `http://127.0.0.1:${port}`;

function fakeRelease() {
  const base = "https://github.com/Elwynn18/chatonlm/releases/download/v0.9.0/";
  const asset = (name, size) => ({
    name,
    size,
    browser_download_url: base + name,
  });
  return pickAssets(
    {
      tag_name: "v0.9.0",
      published_at: new Date().toISOString(),
      assets: [
        asset("chatonlm_0.9.0_amd64.AppImage", 196_000_000),
        asset("chatonlm_0.9.0_amd64.deb", 121_000_000),
        asset("chatonlm_0.9.0_x64-setup.exe", 34_200_000),
      ],
    },
    "Elwynn18/chatonlm",
  );
}

const service = createService(
  {
    ...configFromEnv({
      SMTP_USER: "contact.chatonlm@gmail.com",
      SITE_ORIGINS: origin,
    }),
    repo: "Elwynn18/chatonlm",
  },
  {
    send: async (mail) =>
      console.log(
        `\n--- Message du formulaire (non envoyé) ---\n${mail.subject}\nRépondre à : ${mail.replyTo}\n\n${mail.text}`,
      ),
    release: async () =>
      args.includes("--release") ? fakeRelease() : { published: false },
    log: () => {},
  },
);

async function file(res, status, name) {
  const info = await stat(name);
  res.writeHead(status, {
    "Content-Type": TYPES[path.extname(name)] ?? "application/octet-stream",
    "Content-Length": info.size,
    "Content-Security-Policy": CSP,
    "Cache-Control": "no-store",
  });
  createReadStream(name).pipe(res);
}

await build();
createServer(async (req, res) => {
  const { pathname } = new URL(req.url ?? "/", origin);
  if (/^\/(api|download)\//.test(pathname))
    return service.emit("request", req, res);
  if (pathname === "/") {
    const first = String(req.headers["accept-language"] ?? "").toLowerCase();
    const lang = first.startsWith("fr")
      ? "fr"
      : first.startsWith("es")
        ? "es"
        : "en";
    res.writeHead(302, { Location: `/${lang}/`, Vary: "Accept-Language" });
    return res.end();
  }
  // Page introuvable dans la langue du dossier demandé, comme nginx.
  const [, dossier] = /^\/(fr|es)\//.exec(pathname) ?? [];
  const absent = path.join(
    DIST,
    dossier ? `${dossier}/404/index.html` : "404.html",
  );
  const target = path.join(DIST, path.normalize(decodeURIComponent(pathname)));
  if (!target.startsWith(DIST)) return file(res, 404, absent);
  for (const candidate of [target, path.join(target, "index.html")]) {
    try {
      if ((await stat(candidate)).isFile())
        return await file(res, 200, candidate);
    } catch {
      // essai suivant
    }
  }
  return file(res, 404, absent);
}).listen(port, "127.0.0.1", () =>
  console.log(`Aperçu du site : ${origin}  (Ctrl+C pour arrêter)`),
);
