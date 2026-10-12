#!/usr/bin/env node
// Prévient les moteurs qui suivent IndexNow (Bing, Yandex, Naver, Seznam… ; pas
// Google) que des pages du site ont changé, pour qu'ils repassent sans attendre.
// À lancer après la mise en ligne d'un changement de contenu :
//
//   node site/indexnow.mjs             envoie les adresses de site/dist/sitemap.xml
//   node site/indexnow.mjs --dry-run   montre l'envoi sans le faire
//
// La clé n'est pas un secret : c'est le nom du fichier src/racine/<clé>.txt, servi
// à la racine du site, par lequel le moteur vérifie que l'envoi vient bien du
// site. Rien n'est envoyé tant que ce fichier n'est pas en ligne.

import { readFile, readdir } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { ORIGIN } from "./build.mjs";

const ROOT = path.dirname(fileURLToPath(import.meta.url));
const ENDPOINT = "https://api.indexnow.org/indexnow";

/** Adresses d'un plan du site. */
export function sitemapUrls(xml) {
  return [...xml.matchAll(/<loc>([^<]+)<\/loc>/g)].map((match) => match[1]);
}

/** Corps de l'envoi : le protocole n'accepte que des adresses de l'hôte de la clé. */
export function payload(urls, key, origin = ORIGIN) {
  const { host } = new URL(origin);
  const foreign = urls.filter((url) => new URL(url).host !== host);
  if (foreign.length)
    throw new Error(`Adresses hors de ${host} : ${foreign.join(", ")}`);
  if (!urls.length) throw new Error("Aucune adresse à envoyer");
  if (!/^[a-zA-Z0-9-]{8,128}$/.test(key))
    throw new Error("Clé IndexNow invalide");
  return { host, key, keyLocation: `${origin}/${key}.txt`, urlList: urls };
}

/** La clé : le fichier de `dir` qui porte son nom et la contient. */
export async function readKey(dir = path.join(ROOT, "src", "racine")) {
  const file = (await readdir(dir)).find((name) =>
    /^[0-9a-f]{32}\.txt$/.test(name),
  );
  if (!file) throw new Error(`Pas de fichier-clé IndexNow dans ${dir}`);
  const key = (await readFile(path.join(dir, file), "utf8")).trim();
  if (`${key}.txt` !== file)
    throw new Error(`${file} doit contenir son propre nom, sans « .txt »`);
  return key;
}

if (
  process.argv[1] &&
  import.meta.url === pathToFileURL(process.argv[1]).href
) {
  const body = payload(
    sitemapUrls(
      await readFile(path.join(ROOT, "dist", "sitemap.xml"), "utf8").catch(
        () => {
          throw new Error(
            "site/dist/sitemap.xml absent : lancer d'abord node site/build.mjs",
          );
        },
      ),
    ),
    await readKey(),
  );
  if (process.argv.includes("--dry-run")) {
    console.log(
      `Envoi prévu à ${ENDPOINT} :\n${JSON.stringify(body, null, 2)}`,
    );
    process.exit(0);
  }
  const online = await fetch(body.keyLocation)
    .then(async (res) => res.ok && (await res.text()).trim() === body.key)
    .catch(() => false);
  if (!online) {
    console.error(
      `${body.keyLocation} ne répond pas avec la clé : téléverser d'abord site/dist/ sur le serveur.`,
    );
    process.exit(1);
  }
  const res = await fetch(ENDPOINT, {
    method: "POST",
    headers: { "Content-Type": "application/json; charset=utf-8" },
    body: JSON.stringify(body),
  });
  // 200 : adresses reçues ; 202 : reçues, clé en cours de vérification.
  if (res.status === 200 || res.status === 202) {
    console.log(
      `IndexNow : ${body.urlList.length} adresses envoyées (réponse ${res.status}).`,
    );
  } else {
    console.error(
      `IndexNow a refusé l'envoi (réponse ${res.status}) : ${(await res.text()).slice(0, 300)}`,
    );
    process.exit(1);
  }
}
