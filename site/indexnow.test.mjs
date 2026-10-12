// @vitest-environment node
// Essais de l'envoi IndexNow (indexnow.mjs) : lecture du plan du site, corps de
// l'envoi, fichier-clé. Rien n'est envoyé.
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import path from "node:path";
import { describe, expect, it } from "vitest";
import { payload, readKey, sitemapUrls } from "./indexnow.mjs";

const ROOT = path.resolve(import.meta.dirname, "..");
const KEY = "0123456789abcdef0123456789abcdef";

describe("IndexNow", () => {
  it("lit les adresses du plan du site", () => {
    expect(
      sitemapUrls(
        '<urlset><url><loc>https://chatonlm.com/fr/</loc><xhtml:link href="https://chatonlm.com/en/"/></url><url><loc>https://chatonlm.com/en/</loc></url></urlset>',
      ),
    ).toEqual(["https://chatonlm.com/fr/", "https://chatonlm.com/en/"]);
  });

  it("compose l'envoi et refuse une adresse d'un autre site, une liste vide ou une clé invalide", () => {
    expect(payload(["https://chatonlm.com/fr/"], KEY)).toEqual({
      host: "chatonlm.com",
      key: KEY,
      keyLocation: `https://chatonlm.com/${KEY}.txt`,
      urlList: ["https://chatonlm.com/fr/"],
    });
    expect(() =>
      payload(["https://chatonlm.com/fr/", "https://exemple.org/"], KEY),
    ).toThrow(/hors de chatonlm\.com : https:\/\/exemple\.org\//);
    expect(() => payload([], KEY)).toThrow(/Aucune adresse/);
    expect(() => payload(["https://chatonlm.com/fr/"], "court")).toThrow(
      /Clé IndexNow invalide/,
    );
  });

  it("lit la clé du site, et refuse un fichier qui ne contient pas son nom", async () => {
    expect(await readKey()).toMatch(/^[0-9a-f]{32}$/);
    const dir = mkdtempSync(path.join(ROOT, ".tools", "indexnow-test-"));
    try {
      await expect(readKey(dir)).rejects.toThrow(/Pas de fichier-clé/);
      writeFileSync(path.join(dir, `${KEY}.txt`), "autre chose");
      await expect(readKey(dir)).rejects.toThrow(/son propre nom/);
      writeFileSync(path.join(dir, `${KEY}.txt`), `${KEY}\n`);
      expect(await readKey(dir)).toBe(KEY);
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  });
});
