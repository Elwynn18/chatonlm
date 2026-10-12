// Faux serveur de mises à jour (e2e/update.test.mjs) : `latest.json` (format
// statique de tauri-plugin-updater) et les paquets, sur 127.0.0.1. L'app le
// lit par CHATONLM_UPDATE_URL (builds d'essai seulement).
import { readFileSync } from "node:fs";
import { createServer } from "node:http";

/**
 * Démarre le serveur. Renvoie { url, manifestUrl, set, requests, count,
 * stop } :
 * - `set({ manifest, files })` : `manifest` (objet, ou null : 204, aucune
 *   mise à jour) et `files` ({ "/nom": Buffer | chemin }) servis ensuite ;
 * - `requests()` : chemins demandés, dans l'ordre ; `count(path)` : combien
 *   de fois.
 */
export async function startUpdateServer() {
  let manifest = null;
  let files = {};
  const requests = [];
  const server = createServer((request, response) => {
    const path = new URL(request.url, "http://127.0.0.1").pathname;
    requests.push(path);
    if (path === "/latest.json") {
      if (manifest === null) {
        response.writeHead(204).end();
        return;
      }
      const body = Buffer.from(JSON.stringify(manifest));
      response
        .writeHead(200, {
          "Content-Type": "application/json",
          "Content-Length": body.length,
        })
        .end(body);
      return;
    }
    const file = files[path];
    if (file === undefined) {
      response.writeHead(404).end();
      return;
    }
    const body = Buffer.isBuffer(file) ? file : readFileSync(file);
    response
      .writeHead(200, {
        "Content-Type": "application/octet-stream",
        "Content-Length": body.length,
      })
      .end(body);
  });
  await new Promise((resolve, reject) => {
    server.once("error", reject);
    server.listen(0, "127.0.0.1", resolve);
  });
  const url = `http://127.0.0.1:${server.address().port}`;
  return {
    url,
    manifestUrl: `${url}/latest.json`,
    set(next) {
      if ("manifest" in next) manifest = next.manifest;
      if ("files" in next) files = next.files;
    },
    requests: () => [...requests],
    count: (path) => requests.filter((p) => p === path).length,
    stop: () =>
      new Promise((resolve) => {
        server.closeAllConnections();
        server.close(() => resolve());
      }),
  };
}

/**
 * Manifeste d'une version : `url` du paquet (AppImage) et sa signature,
 * pour toutes les plateformes que le greffon cherche sous Linux.
 */
export function updateManifest({ version, url, signature, notes }) {
  const platform = { url, signature };
  return {
    version,
    notes: notes ?? `ChatonLM ${version} for the update test.`,
    pub_date: "2026-10-07T12:00:00Z",
    platforms: {
      "linux-x86_64-appimage": platform,
      "linux-x86_64-deb": platform,
      "linux-x86_64": platform,
    },
  };
}
