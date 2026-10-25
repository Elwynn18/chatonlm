// @vitest-environment node
// Essais de la fabrication du site (build.mjs) : catalogues des trois langues,
// typographie française, gabarits, garde de la politique de sécurité, puis une
// fabrication complète dont chaque lien interne et chaque fichier cité existe,
// et le référencement (balises, données structurées, plan du site daté).
import { existsSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import path from "node:path";
import { afterAll, beforeAll, describe, expect, it, vi } from "vitest";
import {
  KOFI_URL,
  LANGS,
  PAGES,
  TOOLS,
  UNLISTED,
  assertCsp,
  build,
  checkLocales,
  contentHash,
  escapeHtml,
  flatten,
  jsonLd,
  pathOf,
  render,
  typographie,
} from "./build.mjs";

const ROOT = path.resolve(import.meta.dirname, "..");
const OUT = path.join(ROOT, ".tools", "site-test-dist");

// Chaque fabrication écrit une soixantaine de fichiers sous .tools/, sur un
// disque lent quand il est chargé : les délais par défaut (5 et 10 s) y sautent.
vi.setConfig({ testTimeout: 30_000, hookTimeout: 30_000 });

describe("catalogues", () => {
  it("aplatit les clés", () => {
    expect([...flatten({ a: { b: "x", c: { d: "y" } }, e: "z" })]).toEqual([
      ["a.b", "x"],
      ["a.c.d", "y"],
      ["e", "z"],
    ]);
  });

  it("refuse une clé absente d'une langue ou un texte vide", () => {
    const fr = new Map([
      ["a", "un"],
      ["b", "deux"],
    ]);
    expect(() => checkLocales({ fr, en: new Map([["a", "one"]]) })).toThrow(
      /en : clé absente « b »/,
    );
    expect(() =>
      checkLocales({
        fr,
        en: new Map([
          ["a", "one"],
          ["b", " "],
        ]),
      }),
    ).toThrow(/en : texte vide « b »/);
    expect(() =>
      checkLocales({
        fr,
        en: new Map([
          ["a", "one"],
          ["b", "two"],
        ]),
      }),
    ).not.toThrow();
  });

  it("pose les espaces insécables du français, hors des balises", () => {
    expect(
      typographie("fr", "Un mot : « oui » ; vraiment ? Oui ! 0 € et 5,2 Go"),
    ).toBe(
      "Un mot\u00a0: «\u00a0oui\u00a0»\u202f; vraiment\u202f? Oui\u202f! 0\u00a0€ et 5,2\u00a0Go",
    );
    expect(
      typographie(
        "fr",
        "<a href='/{{lang}}/x/?a=1' title='ah !'>Lire : ici</a>",
      ),
    ).toBe("<a href='/{{lang}}/x/?a=1' title='ah !'>Lire\u00a0: ici</a>");
    expect(typographie("en", "A word : yes ?")).toBe("A word : yes ?");
  });
});

describe("gabarits", () => {
  const context = () => ({
    texts: new Map([
      ["titre", "Chats & <chiens>"],
      ["riche", "Un <em>mot</em>, {{year}}"],
    ]),
    partials: new Map([["bloc", "[{{t:titre}}]"]]),
    assets: new Map([["site.css", "/assets/site.abc.css"]]),
    vars: { lang: "fr", year: "2026" },
    used: new Set(),
  });

  it("échappe {{t:}}, garde le HTML de {{h:}}, inclut les morceaux", () => {
    const c = context();
    expect(
      render(
        "<p>{{t:titre}}</p>{{h:riche}} {{>bloc}} {{asset:site.css}} {{lang}}",
        c,
      ),
    ).toBe(
      "<p>Chats &amp; &lt;chiens&gt;</p>Un <em>mot</em>, 2026 [Chats &amp; &lt;chiens&gt;] /assets/site.abc.css fr",
    );
    expect([...c.used].sort()).toEqual(["riche", "titre"]);
    expect(escapeHtml('"a"')).toBe("&quot;a&quot;");
  });

  it("s'arrête sur une clé, un morceau, un fichier ou une marque inconnus", () => {
    expect(() => render("{{t:absent}}", context())).toThrow(
      /Clé de texte absente/,
    );
    expect(() => render("{{>absent}}", context())).toThrow(
      /Inclusion introuvable/,
    );
    expect(() => render("{{asset:absent.js}}", context())).toThrow(
      /Fichier d'assets inconnu/,
    );
    expect(() => render("{{bidule}}", context())).toThrow(/Marque inconnue/);
  });

  it("refuse ce que la politique de sécurité bloquerait", () => {
    expect(() =>
      assertCsp(
        '<script type="module" src="/a.js"></script><p class="x">ok</p>',
        "p",
      ),
    ).not.toThrow();
    expect(() => assertCsp("<script>alert(1)</script>", "p")).toThrow(
      /<script> sans src/,
    );
    // Un bloc de données structurées n'est pas exécuté : il passe, pas ce qui le suit.
    expect(() =>
      assertCsp('<script type="application/ld+json">{}</script>', "p"),
    ).not.toThrow();
    expect(() =>
      assertCsp(
        '<script type="application/ld+json">{}</script><script>alert(1)</script>',
        "p",
      ),
    ).toThrow(/<script> sans src/);
    // Un attribut de plus, et le navigateur pourrait l'exécuter : refusé.
    for (const balise of [
      '<script data-type="application/ld+json">',
      '<script type="module" type="application/ld+json">',
      `<script title='type="application/ld+json"'>`,
      '<script type="application/ld+json" nonce="x">',
      '<script data-src="/a.js">',
    ])
      expect(() => assertCsp(`${balise}x()</script>`, "p"), balise).toThrow(
        /<script> sans src/,
      );
    expect(() => assertCsp("<style>p{}</style>", "p")).toThrow(/bloc <style>/);
    expect(() => assertCsp('<p style="color:red">x</p>', "p")).toThrow(
      /attribut style/,
    );
    expect(() => assertCsp('<a onclick="x()">x</a>', "p")).toThrow(
      /attribut on/,
    );
  });
});

const LISTED = Object.keys(PAGES).filter((page) => !UNLISTED.has(page));
const INDEXED = LISTED.length * LANGS.length;

describe("fabrication complète", () => {
  let files;
  // Une date impossible : une page que la fabrication redaterait la porterait.
  const JAMAIS = "9999-12-31";
  beforeAll(async () => {
    files = await build({ out: OUT, today: JAMAIS });
  });
  afterAll(() => rmSync(OUT, { recursive: true, force: true }));
  const read = (file) => readFileSync(path.join(OUT, file), "utf8");

  it("écrit chaque page dans les trois langues, sans marque oubliée", () => {
    for (const lang of LANGS) {
      for (const page of Object.keys(PAGES)) {
        const pagePath = pathOf(page, lang);
        const html = read(`${lang}/${pagePath}index.html`);
        expect(html).toContain(`<html lang="${lang}"`);
        // Hors des données structurées, dont le JSON se ferme par des accolades.
        expect(
          html.replace(
            /<script type="application\/ld\+json">.*?<\/script>/s,
            "",
          ),
        ).not.toMatch(/\{\{|\}\}/);
        expect(html.match(/<h1[\s>]/g), `${lang}/${pagePath}`).toHaveLength(1);
      }
    }
  });

  it("donne aux pages indexées leur adresse canonique, leurs autres langues et leur aperçu", () => {
    for (const lang of LANGS) {
      for (const page of Object.keys(PAGES)) {
        const pagePath = pathOf(page, lang);
        const html = read(`${lang}/${pagePath}index.html`);
        const nom = `${lang}/${pagePath}`;
        if (UNLISTED.has(page)) {
          // Hors de l'index : ni adresse canonique, ni autres langues, ni données.
          expect(html, nom).toContain(
            '<meta name="robots" content="noindex" />',
          );
          expect(html, nom).not.toMatch(
            /rel="canonical"|rel="alternate"|ld\+json|og:url/,
          );
          continue;
        }
        expect(html, nom).not.toContain("noindex");
        expect(html, nom).toContain(
          `<link rel="canonical" href="https://chatonlm.com/${lang}/${pagePath}" />`,
        );
        // Chaque langue a sa propre adresse pour les pages des outils.
        for (const other of LANGS) {
          expect(html, nom).toContain(
            `hreflang="${other}" href="https://chatonlm.com/${other}/${pathOf(page, other)}"`,
          );
        }
        expect(html, nom).toContain(
          `hreflang="x-default" href="https://chatonlm.com/en/${pathOf(page, "en")}"`,
        );
        expect(html, nom).toContain(
          `<meta property="og:url" content="https://chatonlm.com/${lang}/${pagePath}" />`,
        );
        expect(html, nom).toMatch(
          new RegExp(
            `og:image" content="https://chatonlm\\.com/assets/img/og-${lang}\\.[0-9a-f]+\\.png"`,
          ),
        );
        expect(html, nom).toMatch(/og:image:alt" content="[^"]{20,}"/);
        expect(html.match(/og:locale:alternate/g), nom).toHaveLength(2);
        // Longueurs lues en entier dans les résultats de recherche.
        const [, title] = /<title>([^<]+)<\/title>/.exec(html);
        const [, description] = /name="description" content="([^"]+)"/.exec(
          html,
        );
        expect(title.length, `${nom} : titre`).toBeLessThanOrEqual(60);
        if (page === "home" || TOOLS[page])
          expect(title.length, `${nom} : titre`).toBeGreaterThanOrEqual(30);
        expect(
          description.length,
          `${nom} : description`,
        ).toBeGreaterThanOrEqual(120);
        expect(description.length, `${nom} : description`).toBeLessThanOrEqual(
          160,
        );
      }
    }
  });

  it("décrit chaque page indexée en données structurées, sans rien inventer", () => {
    expect(jsonLd({ a: "</script><b>" })).toBe(
      '<script type="application/ld+json">{"a":"\\u003c/script>\\u003cb>"}</script>',
    );
    for (const lang of LANGS) {
      for (const page of LISTED) {
        const pagePath = pathOf(page, lang);
        const html = read(`${lang}/${pagePath}index.html`);
        const blocs = [
          ...html.matchAll(
            /<script type="application\/ld\+json">(.*?)<\/script>/gs,
          ),
        ];
        expect(blocs, `${lang}/${pagePath}`).toHaveLength(1);
        const data = JSON.parse(blocs[0][1]);
        expect(data["@context"]).toBe("https://schema.org");
        const graph = data["@graph"];
        const types = graph.map((node) => node["@type"]);
        expect(types).toEqual(
          expect.arrayContaining(["Organization", "WebSite"]),
        );
        // Chaque renvoi { "@id" } mène à une entité du même bloc.
        const ids = new Set(graph.map((node) => node["@id"]));
        const renvois = [];
        JSON.stringify(graph, (_, value) => {
          if (
            value &&
            typeof value === "object" &&
            Object.keys(value).join() === "@id"
          )
            renvois.push(value["@id"]);
          return value;
        });
        expect(renvois.length).toBeGreaterThan(2);
        for (const id of renvois) expect(ids, id).toContain(id);
        const url = `https://chatonlm.com/${lang}/${pagePath}`;
        const webpage = graph.find((node) => node.url === url && node.name);
        expect(webpage.inLanguage).toBe(lang);
        expect(webpage.dateModified).toMatch(/^\d{4}-\d{2}-\d{2}$/);
        expect(webpage.name).toBe(
          /<title>([^<]+)<\/title>/.exec(html)[1].replaceAll("&amp;", "&"),
        );
        // Ni note, ni avis, ni questions balisées (retirées des résultats de Google).
        expect(blocs[0][1]).not.toMatch(
          /aggregateRating|"review"|FAQPage|HowTo/,
        );
        if (page === "home") {
          const app = graph.find(
            (node) => node["@type"] === "SoftwareApplication",
          );
          expect(app.offers).toEqual({
            "@type": "Offer",
            price: "0",
            priceCurrency: "EUR",
          });
          expect(app.operatingSystem).toBe("Windows 11, Linux");
          // Un outil par page d'outil.
          expect(app.featureList).toHaveLength(Object.keys(TOOLS).length);
          expect(app.url).toBe(url);
          expect(app.installUrl).toBe(`${url}#telecharger`);
          expect(app.downloadUrl).toBeUndefined();
          expect(webpage.mainEntity).toEqual({ "@id": app["@id"] });
        } else {
          expect(types).toContain("BreadcrumbList");
          expect(webpage["@type"]).toBe(
            page === "contact" ? "ContactPage" : "WebPage",
          );
          const crumbs = graph.find(
            (node) => node["@type"] === "BreadcrumbList",
          ).itemListElement;
          expect(crumbs.map((item) => item.item)).toEqual([
            `https://chatonlm.com/${lang}/`,
            url,
          ]);
          // « Voz », le plus court des noms de page.
          expect(crumbs[1].name.length).toBeGreaterThanOrEqual(3);
        }
      }
    }
  });

  it("relie la page « Pourquoi ChatonLM » à l'en-tête, à l'accueil et aux outils", () => {
    for (const lang of LANGS) {
      const lien = `href="/${lang}/${pathOf("choose", lang)}"`;
      // En-tête, section des raisons et pied de page de l'accueil y mènent.
      expect(read(`${lang}/index.html`).split(lien).length - 1, lang).toBe(3);
      // Et l'en-tête de toute autre page.
      expect(read(`${lang}/${pathOf("images", lang)}index.html`)).toContain(
        lien,
      );
      const page = read(`${lang}/${pathOf("choose", lang)}index.html`);
      for (const outil of Object.keys(TOOLS))
        expect(page, `${lang} ${outil}`).toContain(
          `href="/${lang}/${pathOf(outil, lang)}"`,
        );
      // Sept sujets comparés, et le tableau plus les trois dessins s'animent.
      expect(page.match(/<div class="duel-ligne">/g)).toHaveLength(7);
      expect(page.match(/ data-anime/g)).toHaveLength(4);
    }
  });

  it("donne à chaque page d'outil son adresse par langue, ses liens et son écran", () => {
    for (const [page, { vue }] of Object.entries(TOOLS)) {
      const paths = LANGS.map((lang) => pathOf(page, lang));
      // Une adresse par langue, lisible dans cette langue.
      expect(new Set(paths).size, page).toBe(LANGS.length);
      for (const lang of LANGS) {
        const html = read(`${lang}/${pathOf(page, lang)}index.html`);
        // Le choix de langue mène à la même page dans l'autre langue.
        for (const other of LANGS)
          expect(html, `${lang} ${page}`).toContain(
            `<a href="/${other}/${pathOf(page, other)}" lang="${other}"`,
          );
        // La fenêtre dessinée est arrêtée sur l'écran de l'outil.
        expect(html).toContain(`<div class="app" data-app="${vue}">`);
        expect(html).toContain(`<div class="vue" data-vue="${vue}">`);
        expect(html.match(/<details>/g).length).toBeGreaterThanOrEqual(4);
        // Aucune page orpheline : l'accueil de la langue y mène.
        expect(read(`${lang}/index.html`), `${lang} ${page}`).toContain(
          `href="/${lang}/${pathOf(page, lang)}"`,
        );
      }
    }
  });

  it("rend cliquable, dans la fenêtre dessinée, chacun des sept outils", () => {
    const html = read("fr/index.html");
    const outils = [
      ...html.matchAll(/class="onglet[^"]*" data-vue="([a-z]+)"/g),
    ].map((m) => m[1]);
    expect(outils).toHaveLength(7);
    // Chaque outil de la fenêtre a sa page.
    expect(outils.sort()).toEqual(
      Object.values(TOOLS)
        .map(({ vue }) => vue)
        .sort(),
    );
    const scene = html.slice(
      html.indexOf('<div class="app"'),
      html.indexOf('<section class="reunis'),
    );
    for (const outil of outils)
      expect(scene, outil).toContain(`data-vers="${outil}"`);
    // Tout renvoi de la fenêtre vise un outil qui existe.
    for (const [, vers] of scene.matchAll(/data-vers="([^"]+)"/g))
      expect(outils).toContain(vers);
  });

  it("ne cite que des fichiers et des pages qui existent", () => {
    const pages = files.filter((file) => file.endsWith(".html"));
    expect(pages.length).toBe(LANGS.length * Object.keys(PAGES).length + 2);
    for (const page of pages) {
      const html = read(page);
      for (const [, ref] of html.matchAll(/(?:href|src)="(\/[^"#?]*)/g)) {
        if (/^\/(api|download)\//.test(ref)) continue;
        const target = ref.endsWith("/") ? `${ref}index.html` : ref;
        expect(existsSync(path.join(OUT, target)), `${page} → ${ref}`).toBe(
          true,
        );
      }
    }
    const css = read(
      files.find((file) => /^assets\/site\.[0-9a-f]+\.css$/.test(file)),
    );
    const fonts = [...css.matchAll(/url\(([^)]+)\)/g)].map((m) => m[1]);
    expect(fonts.length).toBeGreaterThanOrEqual(4);
    for (const ref of fonts)
      expect(existsSync(path.join(OUT, ref)), ref).toBe(true);
  });

  it("garde la typographie française et un plan du site complet", () => {
    expect(read("fr/index.html")).toMatch(/créer des images\u00a0: ChatonLM/);
    expect(read("en/index.html")).not.toMatch(/\u202f[;!?]/);
    const sitemap = read("sitemap.xml");
    expect(sitemap.match(/<loc>/g)).toHaveLength(INDEXED);
    expect(
      sitemap.match(/<lastmod>\d{4}-\d{2}-\d{2}<\/lastmod>/g),
    ).toHaveLength(INDEXED);
    expect(sitemap).not.toContain("/contact/sent/");
    // site/lastmod.json suit les sources : sinon, lancer « node site/build.mjs »
    // et commiter le fichier avec le changement.
    expect(sitemap, "site/lastmod.json est en retard").not.toContain(JAMAIS);
    expect(read("404.html")).toContain(
      '<meta name="robots" content="noindex" />',
    );
  });

  it("écrit les fichiers lus par les robots", () => {
    const robots = read("robots.txt");
    expect(robots).toMatch(/^User-agent: \*$/m);
    expect(robots).toMatch(/^Allow: \/$/m);
    expect(robots).toMatch(/^Disallow: \/api\/$/m);
    // Le script de la page lit l'état des téléchargements : un moteur aussi.
    expect(robots).toMatch(/^Allow: \/api\/release$/m);
    expect(robots).toMatch(/^Disallow: \/download\/$/m);
    expect(robots).toMatch(/^Sitemap: https:\/\/chatonlm\.com\/sitemap\.xml$/m);
    // Aucune page du plan du site n'est fermée aux robots.
    expect(robots.match(/^Disallow:/gm)).toHaveLength(2);
    const llms = read("llms.txt");
    expect(llms).toMatch(/^# ChatonLM\n\n> /);
    // Format llmstxt.org : sous un titre « ## », rien que des liens.
    for (const section of llms.split(/^## .*$/m).slice(1)) {
      for (const ligne of section.trim().split("\n"))
        expect(ligne).toMatch(/^- \[[^\]]+\]\(https:\/\/[^)]+\)(: .+)?$/);
    }
    expect(llms).not.toMatch(/\{\{|\}\}|&amp;|&quot;/);
    const liens = [...llms.matchAll(/\]\((https:\/\/chatonlm\.com[^)#]*)/g)];
    expect(liens.length).toBeGreaterThanOrEqual(6);
    for (const [, lien] of liens) {
      const fichier = `${new URL(lien).pathname}index.html`;
      expect(existsSync(path.join(OUT, fichier)), lien).toBe(true);
    }
    // Clé IndexNow : un fichier à la racine, qui porte son nom et la contient.
    const cles = files.filter((file) => /^[0-9a-f]{32}\.txt$/.test(file));
    expect(cles).toHaveLength(1);
    expect(`${read(cles[0]).trim()}.txt`).toBe(cles[0]);
  });

  it("n'oublie aucune langue dans les textes du formulaire lus par le script", () => {
    for (const lang of LANGS) {
      const html = read(`${lang}/contact/index.html`);
      for (const code of [
        "name",
        "email",
        "message",
        "rate",
        "too_fast",
        "send",
        "sending",
      ]) {
        expect(html).toMatch(new RegExp(`data-msg-${code}="[^"]+"`));
      }
      expect(html).toContain(
        `<input type="hidden" name="lang" value="${lang}" />`,
      );
    }
  });
});

describe("page des dons", () => {
  const OUT3 = path.join(ROOT, ".tools", "site-test-dist-don");
  afterAll(() => rmSync(OUT3, { recursive: true, force: true }));
  const lire = (file) => readFileSync(path.join(OUT3, file), "utf8");
  // Les deux boutons de la page (haut et fin de page).
  const boutons = (html) =>
    [...html.matchAll(/<a class="bouton[^"]*" data-don[^>]*>/g)].map(
      (m) => m[0],
    );

  it("est reliée au pied de page de chaque page, et marquée sur la sienne", async () => {
    await build({ out: OUT3, kofi: "" });
    for (const lang of LANGS) {
      const lien = `href="/${lang}/donate/"`;
      for (const page of Object.keys(PAGES)) {
        const html = lire(`${lang}/${pathOf(page, lang)}index.html`);
        expect(html, `${lang} ${page}`).toContain(
          page === "donate" ? `${lien} aria-current="page"` : lien,
        );
      }
    }
    expect(lire("llms.txt")).toContain("](https://chatonlm.com/en/donate/)");
  });

  it("sans adresse Ko-fi : boutons inactifs et avis « bientôt » visible", async () => {
    await build({ out: OUT3, kofi: "" });
    for (const lang of LANGS) {
      const html = lire(`${lang}/donate/index.html`);
      const liste = boutons(html);
      expect(liste, lang).toHaveLength(2);
      for (const bouton of liste) {
        expect(bouton).toContain('aria-disabled="true"');
        expect(bouton).not.toContain("href=");
      }
      expect(html).toMatch(/<p class="discret don-avis" data-don-bientot>/);
    }
  });

  it("avec une adresse Ko-fi : boutons vers elle et avis caché", async () => {
    await build({ out: OUT3, kofi: "https://ko-fi.com/essai?a=1&b=2" });
    for (const lang of LANGS) {
      const html = lire(`${lang}/donate/index.html`);
      const liste = boutons(html);
      expect(liste, lang).toHaveLength(2);
      for (const bouton of liste) {
        expect(bouton).toContain(
          'href="https://ko-fi.com/essai?a=1&amp;b=2" rel="noopener"',
        );
        expect(bouton).not.toContain("aria-disabled");
      }
      expect(html).toMatch(
        /<p class="discret don-avis" data-don-bientot hidden>/,
      );
    }
  });

  it("l'adresse de Ko-fi, si elle est posée, est une page Ko-fi", () => {
    if (KOFI_URL)
      expect(KOFI_URL).toMatch(/^https:\/\/ko-fi\.com\/[A-Za-z0-9_]+$/);
  });

  it("chatonlm.com/donate renvoie vers la page dans la langue du navigateur", () => {
    const nginx = readFileSync(
      path.join(ROOT, "site", "server", "nginx-chatonlm.conf"),
      "utf8",
    );
    expect(nginx).toMatch(
      /location ~ \^\/donate\/\?\$ \{\s*return 302 \/\$chatonlm_lang\/donate\/;\s*\}/,
    );
    // La réponse dépend de la langue : les caches le savent.
    expect(nginx).toContain('"~^/donate/?$" "Accept-Language";');
    expect(pathOf("donate", "fr")).toBe("donate/");
  });
});

describe("dates de dernière modification", () => {
  const DATES = path.join(ROOT, ".tools", "site-test-lastmod.json");
  const OUT2 = path.join(ROOT, ".tools", "site-test-dist-dates");
  afterAll(() => {
    rmSync(DATES, { force: true });
    rmSync(OUT2, { recursive: true, force: true });
  });
  const fabrique = (today, saveDates = true) =>
    build({ out: OUT2, dates: DATES, saveDates, today });
  const dates = () => JSON.parse(readFileSync(DATES, "utf8"));
  const plan = () => readFileSync(path.join(OUT2, "sitemap.xml"), "utf8");

  it("ne tient pas compte de l'empreinte des fichiers d'assets", () => {
    const page = (css) => `<link href="/assets/site.${css}.css" /><p>Texte</p>`;
    expect(contentHash(page("0123456789"))).toBe(
      contentHash(page("abcdef0123")),
    );
    expect(contentHash(page("0123456789"))).not.toBe(
      contentHash(page("0123456789").replace("Texte", "Autre")),
    );
  });

  it("date une page du jour où son contenu change, et la garde sinon", async () => {
    rmSync(DATES, { force: true });
    await fabrique("2026-01-01");
    const premier = dates();
    expect(Object.keys(premier)).toHaveLength(INDEXED);
    expect(new Set(Object.values(premier).map((d) => d.date))).toEqual(
      new Set(["2026-01-01"]),
    );
    // Rien n'a changé : les dates restent, la page et le plan du site les portent.
    await fabrique("2026-02-02");
    expect(dates()).toEqual(premier);
    expect(plan()).not.toContain("2026-02-02");
    expect(readFileSync(path.join(OUT2, "fr", "index.html"), "utf8")).toContain(
      '"dateModified":"2026-01-01"',
    );
    // Le contenu d'une page a changé depuis la dernière fabrication : elle seule est redatée.
    writeFileSync(
      DATES,
      JSON.stringify({
        ...premier,
        "es/contact/": { hash: "autre", date: "2026-01-01" },
      }),
    );
    await fabrique("2026-03-03", false);
    expect(plan().match(/<lastmod>2026-03-03<\/lastmod>/g)).toHaveLength(1);
    expect(plan()).toMatch(
      /<loc>https:\/\/chatonlm\.com\/es\/contact\/<\/loc>\s*<lastmod>2026-03-03</,
    );
    // Sans saveDates, le fichier des dates n'est pas récrit.
    expect(dates()["es/contact/"].hash).toBe("autre");
  });
});
