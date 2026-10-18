#!/usr/bin/env node
// Fabrique le site statique de chatonlm.com dans site/dist/, prêt à téléverser.
//
//   node site/build.mjs            fabrique site/dist/
//   node site/preview.mjs          fabrique puis sert le site sur cette machine
//
// Sans dépendance ni cadre : des gabarits HTML (src/pages, src/partials), une
// feuille de style et un script (src/styles, src/scripts), et les textes des
// trois langues (src/locales/{fr,en,es}.json). Chaque page est écrite une fois
// par langue sous /<langue>/… ; les fichiers de /assets/ portent l'empreinte de
// leur contenu dans leur nom (le navigateur peut les garder un an).
//
// Marques d'un gabarit :
//   {{t:cle}}        texte de la langue, échappé
//   {{h:cle}}        texte de la langue, HTML gardé (<em>, <strong>, <br>, <a>)
//   {{>nom}}         inclusion de src/partials/nom
//   @                dans une clé ou un nom de morceau : la page en cours, pour
//                    les pages qui partagent un gabarit ({{t:pages.@.title}})
//   {{asset:nom}}    adresse d'un fichier de /assets/ (avec son empreinte)
//   {{lang}} {{page}} {{path}} {{url}} {{url:fr}} {{current:fr}} {{origin}} {{year}} {{og}}
//   {{jsonld}} {{ogalternates}}   référencement, écrits par la fabrication
// Une marque inconnue ou une clé absente arrête la fabrication.
//
// Référencement : chaque page indexée (toutes sauf UNLISTED) inclut
// src/partials/head-index.html (adresse canonique, autres langues, aperçu de
// partage, données structurées schema.org) ; la fabrication écrit aussi
// robots.txt, sitemap.xml (avec la date de dernière modification de chaque
// page, tenue dans lastmod.json), llms.txt (résumé pour les assistants) et le
// fichier-clé IndexNow (src/racine/, envoi par indexnow.mjs).

import { createHash } from "node:crypto";
import { mkdir, readFile, readdir, rm, writeFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

export const LANGS = ["fr", "en", "es"];
export const DEFAULT_LANG = "en";
export const ORIGIN = "https://chatonlm.com";

/**
 * Page → chemin sous /<langue>/ : le même dans les trois langues, ou un par
 * langue (les pages des outils, dont l'adresse se lit dans la langue du lecteur).
 */
export const PAGES = {
  home: "",
  contact: "contact/",
  "contact-sent": "contact/sent/",
  "contact-error": "contact/error/",
  legal: "legal/",
  privacy: "privacy/",
  "not-found": "404/",
  soon: "soon/",
  chat: {
    fr: "conversation-ia-locale/",
    en: "local-ai-chat/",
    es: "chat-ia-local/",
  },
  hub: {
    fr: "modeles-ia-locaux/",
    en: "local-ai-models/",
    es: "modelos-ia-locales/",
  },
  dictation: {
    fr: "dictee-vocale/",
    en: "voice-dictation/",
    es: "dictado-por-voz/",
  },
  voice: { fr: "mode-vocal/", en: "voice-mode/", es: "modo-de-voz/" },
  proofread: {
    fr: "correcteur-de-texte/",
    en: "proofreading/",
    es: "corrector-de-textos/",
  },
  study: {
    fr: "apprendre-avec-une-ia/",
    en: "learn-with-ai/",
    es: "aprender-con-ia/",
  },
  // Pourquoi ChatonLM plutôt qu'une autre app : comparaison et détails.
  choose: {
    fr: "pourquoi-chatonlm/",
    en: "why-chatonlm/",
    es: "por-que-chatonlm/",
  },
  images: {
    fr: "generateur-d-images-ia/",
    en: "local-ai-image-generator/",
    es: "generador-de-imagenes-ia/",
  },
};
/**
 * Pages des outils : elles partagent le gabarit src/pages/tool.html, leurs
 * textes sont sous `pages.<page>` et `vue` nomme l'écran de la fenêtre
 * dessinée qui les illustre (src/partials/vue-<page>.html).
 */
export const TOOLS = {
  chat: { vue: "chat" },
  hub: { vue: "hub" },
  dictation: { vue: "dictee" },
  voice: { vue: "voix" },
  proofread: { vue: "correction" },
  study: { vue: "etude" },
  images: { vue: "images" },
};

/** Chemin d'une page dans une langue. */
export function pathOf(page, lang) {
  const path = PAGES[page];
  return typeof path === "string" ? path : path[lang];
}
/** Pages hors du plan du site (elles ne sont pas à indexer). */
export const UNLISTED = new Set([
  "contact-sent",
  "contact-error",
  "not-found",
  "soon",
]);

/**
 * Adresses publiques du projet hors du site (dépôt du code, comptes) : le
 * `sameAs` des données structurées. À remplir quand le dépôt devient public.
 */
export const SAME_AS = [];
const CONTACT = "contact.chatonlm@gmail.com";
/** Nom d'une page dans son fil d'Ariane : la clé du lien qui y mène. */
const CRUMBS = {
  contact: "nav.contact",
  legal: "footer.notice",
  privacy: "footer.privacy",
  choose: "nav.why",
};
const OG_LOCALES = { fr: "fr_FR", en: "en_US", es: "es_ES" };

const ROOT = path.dirname(fileURLToPath(import.meta.url));

export function escapeHtml(text) {
  return String(text)
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;");
}

/** `{ a: { b: "x" } }` → `Map { "a.b" → "x" }`. */
export function flatten(tree, prefix = "", out = new Map()) {
  for (const [key, value] of Object.entries(tree)) {
    const name = prefix ? `${prefix}.${key}` : key;
    if (value && typeof value === "object") flatten(value, name, out);
    else out.set(name, String(value));
  }
  return out;
}

/**
 * Espaces insécables du français, posées à la fabrication (les catalogues
 * s'écrivent avec des espaces ordinaires) : espace insécable avant « : » et
 * dans les guillemets, espace fine insécable avant « ; », « ! » et « ? ».
 * Les balises d'un texte HTML ne sont pas touchées.
 */
export function typographie(lang, text) {
  if (lang !== "fr") return text;
  return text
    .split(/(<[^>]*>|\{\{[^}]*\}\})/)
    .map((part, index) =>
      index % 2
        ? part
        : part
            .replace(/ ([;!?])/g, "\u202f$1")
            .replace(/ :/g, "\u00a0:")
            .replace(/« /g, "«\u00a0")
            .replace(/ »/g, "\u00a0»")
            .replace(/(\d) (€|%|°C|Go|Mo)/g, "$1\u00a0$2"),
    )
    .join("");
}

/** Les trois langues doivent porter exactement les mêmes clés, toutes remplies. */
export function checkLocales(locales) {
  const problems = [];
  const all = new Set(
    Object.values(locales).flatMap((texts) => [...texts.keys()]),
  );
  for (const [lang, texts] of Object.entries(locales)) {
    for (const key of all) {
      if (!texts.has(key)) problems.push(`${lang} : clé absente « ${key} »`);
      else if (!texts.get(key).trim())
        problems.push(`${lang} : texte vide « ${key} »`);
    }
  }
  if (problems.length)
    throw new Error(`Textes du site incomplets :\n  ${problems.join("\n  ")}`);
}

/**
 * Rend un gabarit. `context` : `texts` (Map de la langue), `partials` (Map
 * nom → source), `assets` (Map nom → adresse), `vars` (objet des marques
 * simples), `used` (Set où noter les clés employées).
 */
export function render(template, context, depth = 0) {
  if (depth > 8) throw new Error("Inclusions trop profondes (boucle ?)");
  const { texts, partials, assets, vars, used } = context;
  return template.replace(/\{\{([^{}]+)\}\}/g, (mark, inner) => {
    const body = inner.trim().replaceAll("@", vars.page ?? "@");
    if (body.startsWith(">")) {
      const name = body.slice(1).trim();
      if (!partials.has(name))
        throw new Error(`Inclusion introuvable : ${mark}`);
      return render(partials.get(name), context, depth + 1);
    }
    const colon = body.indexOf(":");
    const kind = colon === -1 ? body : body.slice(0, colon);
    const arg = colon === -1 ? "" : body.slice(colon + 1).trim();
    if (kind === "t" || kind === "h") {
      if (!texts.has(arg)) throw new Error(`Clé de texte absente : ${mark}`);
      used?.add(arg);
      // Un texte peut lui-même porter des marques simples ({{year}}, {{url:fr}}…).
      const text = render(texts.get(arg), context, depth + 1);
      return kind === "t" ? escapeHtml(text) : text;
    }
    if (kind === "asset") {
      if (!assets.has(arg))
        throw new Error(`Fichier d'assets inconnu : ${mark}`);
      return assets.get(arg);
    }
    if (Object.hasOwn(vars, body)) return vars[body];
    throw new Error(`Marque inconnue : ${mark}`);
  });
}

/**
 * La politique de sécurité du site (nginx) interdit les scripts et les styles
 * écrits dans la page : une page qui en contient serait cassée en ligne.
 */
export function assertCsp(html, file) {
  const problems = [];
  // Un bloc de données structurées n'est pas un script : le navigateur ne
  // l'exécute pas, la politique ne le concerne pas. Seule passe la forme
  // exacte écrite par jsonLd() (un attribut de plus pourrait en faire un script).
  if (
    /<script(?![^>]*\ssrc=)(?!\s+type="application\/ld\+json"\s*>)[^>]*>/i.test(
      html,
    )
  )
    problems.push("un <script> sans src");
  if (/<style[\s>]/i.test(html)) problems.push("un bloc <style>");
  if (/<[a-z][^>]*\sstyle\s*=/i.test(html)) problems.push("un attribut style");
  if (/<[a-z][^>]*\son[a-z]+\s*=/i.test(html)) problems.push("un attribut on…");
  if (problems.length) {
    throw new Error(
      `${file} : interdit par la politique de sécurité : ${problems.join(", ")}`,
    );
  }
}

/**
 * Données structurées (schema.org) d'une page indexée : qui publie le site, le
 * site, la page, puis l'application sur l'accueil ou le fil d'Ariane ailleurs.
 * Tout vient des textes de la page : rien n'y est affirmé qu'elle ne dise pas
 * (ni note, ni avis, ni numéro de version).
 */
export function structuredData({ page, lang, texts, assets, modified }) {
  const home = `${ORIGIN}/${lang}/`;
  const url = `${home}${pathOf(page, lang)}`;
  const meta = `meta.${page.replaceAll("-", "_")}`;
  const organization = { "@id": `${ORIGIN}/#organization` };
  const website = { "@id": `${ORIGIN}/#website` };
  const image = `${ORIGIN}${assets.get(`img/og-${lang}.png`)}`;
  const webpage = {
    "@type": page === "contact" ? "ContactPage" : "WebPage",
    "@id": `${url}#webpage`,
    url,
    name: texts.get(`${meta}.title`),
    description: texts.get(`${meta}.description`),
    inLanguage: lang,
    isPartOf: website,
    primaryImageOfPage: { "@type": "ImageObject", url: image },
    ...(modified ? { dateModified: modified } : {}),
  };
  const graph = [
    {
      "@type": "Organization",
      ...organization,
      name: "ChatonLM",
      url: `${ORIGIN}/`,
      logo: {
        "@type": "ImageObject",
        url: `${ORIGIN}${assets.get("img/icone.png")}`,
        width: 256,
        height: 256,
      },
      email: CONTACT,
      contactPoint: {
        "@type": "ContactPoint",
        contactType: "customer support",
        email: CONTACT,
        url: `${home}contact/`,
        availableLanguage: LANGS,
      },
      ...(SAME_AS.length ? { sameAs: SAME_AS } : {}),
    },
    {
      "@type": "WebSite",
      ...website,
      url: `${ORIGIN}/`,
      name: "ChatonLM",
      inLanguage: LANGS,
      publisher: organization,
    },
    webpage,
  ];
  if (page === "home") {
    const application = { "@id": `${ORIGIN}/#application` };
    webpage.mainEntity = application;
    graph.push({
      "@type": "SoftwareApplication",
      ...application,
      name: "ChatonLM",
      description: texts.get(`${meta}.description`),
      url: home,
      image,
      applicationCategory: "UtilitiesApplication",
      operatingSystem: "Windows 11, Linux",
      processorRequirements: "x86-64",
      offers: { "@type": "Offer", price: "0", priceCurrency: "EUR" },
      isAccessibleForFree: true,
      license: "https://opensource.org/license/mit",
      inLanguage: LANGS,
      featureList: Object.keys(TOOLS).map(
        (tool) =>
          `${texts.get(`tools.${tool}.name`)}. ${texts.get(`tools.${tool}.text`)}`,
      ),
      // La page où l'on choisit son fichier (downloadUrl désignerait le fichier même).
      installUrl: `${home}#telecharger`,
      publisher: organization,
    });
  } else {
    const breadcrumb = { "@id": `${url}#breadcrumb` };
    webpage.breadcrumb = breadcrumb;
    graph.push({
      "@type": "BreadcrumbList",
      ...breadcrumb,
      itemListElement: [
        { "@type": "ListItem", position: 1, name: "ChatonLM", item: home },
        {
          "@type": "ListItem",
          position: 2,
          name: texts.get(CRUMBS[page] ?? `tools.${page}.name`),
          item: url,
        },
      ],
    });
  }
  return { "@context": "https://schema.org", "@graph": graph };
}

/** Le bloc à poser dans la page ; « < » y est écrit en code, rien ne peut le fermer. */
export function jsonLd(data) {
  return `<script type="application/ld+json">${JSON.stringify(data).replaceAll("<", "\\u003c")}</script>`;
}

/**
 * Empreinte du contenu d'une page, pour dater sa dernière modification : les
 * empreintes des fichiers d'assets en sont retirées (un changement de style
 * seul ne change pas ce que dit la page).
 */
export function contentHash(html) {
  return createHash("sha256")
    .update(
      html.replace(/(\/assets\/[^"')\s]*?)\.[0-9a-f]{10}(\.\w+)/g, "$1$2"),
    )
    .digest("hex")
    .slice(0, 16);
}

/** La date du jour, à l'heure de la machine (AAAA-MM-JJ). */
function localDate(now = new Date()) {
  const two = (n) => String(n).padStart(2, "0");
  return `${now.getFullYear()}-${two(now.getMonth() + 1)}-${two(now.getDate())}`;
}

function hashed(name, buffer) {
  const hash = createHash("sha256").update(buffer).digest("hex").slice(0, 10);
  const ext = path.extname(name);
  return `${name.slice(0, -ext.length)}.${hash}${ext}`;
}

async function readDir(dir, encoding) {
  const files = new Map();
  for (const name of (await readdir(dir)).sort()) {
    files.set(name, await readFile(path.join(dir, name), encoding));
  }
  return files;
}

async function write(file, content) {
  await mkdir(path.dirname(file), { recursive: true });
  await writeFile(file, content);
}

/**
 * Fabrique le site de `src` dans `out`. Rend la liste des fichiers écrits.
 * `dates` : fichier des dates de dernière modification des pages (empreinte du
 * contenu et date, par page) ; une page dont le contenu a changé prend la date
 * `today`. Le fichier n'est récrit que si `saveDates` est vrai (fabrication à
 * la main ; ni l'aperçu, ni les essais) : il se commite avec le changement, et
 * un essai refuse un fichier en retard sur les sources.
 */
export async function build({
  src = path.join(ROOT, "src"),
  out = path.join(ROOT, "dist"),
  dates = path.join(ROOT, "lastmod.json"),
  saveDates = false,
  today = localDate(),
} = {}) {
  const locales = {};
  for (const lang of LANGS) {
    const texts = flatten(
      JSON.parse(
        await readFile(path.join(src, "locales", `${lang}.json`), "utf8"),
      ),
    );
    locales[lang] = new Map(
      [...texts].map(([key, text]) => [key, typographie(lang, text)]),
    );
  }
  checkLocales(locales);

  await rm(out, { recursive: true, force: true });
  const written = [];
  const emit = async (file, content) => {
    await write(path.join(out, file), content);
    written.push(file);
  };

  // Polices et images, puis la feuille de style (qui les cite), puis le script.
  const assets = new Map();
  for (const [dir, files] of [
    ["fonts", await readDir(path.join(src, "fonts"))],
    ["img", await readDir(path.join(src, "img"))],
  ]) {
    for (const [name, buffer] of files) {
      // Licences des polices : servies à côté d'elles, sous leur nom.
      if (name.endsWith(".txt")) {
        await emit(`assets/${dir}/${name}`, buffer);
        continue;
      }
      const file = `assets/${dir}/${hashed(name, buffer)}`;
      assets.set(`${dir}/${name}`, `/${file}`);
      await emit(file, buffer);
    }
  }
  const styles = await readDir(path.join(src, "styles"), "utf8");
  const css = [...styles.values()]
    .join("\n")
    .replace(/url\((["']?)([^)"']+)\1\)/g, (whole, _quote, ref) => {
      if (ref.startsWith("data:")) return whole;
      if (!assets.has(ref))
        throw new Error(`Feuille de style : fichier inconnu « ${ref} »`);
      return `url(${assets.get(ref)})`;
    });
  const cssFile = `assets/${hashed("site.css", css)}`;
  assets.set("site.css", `/${cssFile}`);
  await emit(cssFile, css);
  for (const [name, source] of await readDir(
    path.join(src, "scripts"),
    "utf8",
  )) {
    const file = `assets/${hashed(name, source)}`;
    assets.set(name, `/${file}`);
    await emit(file, source);
  }

  const partials = await readDir(path.join(src, "partials"), "utf8");
  const used = new Set();
  const year = String(new Date().getFullYear());
  let known = {};
  try {
    known = JSON.parse(await readFile(dates, "utf8"));
  } catch {
    // Première fabrication : toutes les pages datent d'aujourd'hui.
  }
  const modified = {};
  for (const page of Object.keys(PAGES)) {
    const template = await readFile(
      path.join(src, "pages", `${TOOLS[page] ? "tool" : page}.html`),
      "utf8",
    );
    for (const lang of LANGS) {
      const pagePath = pathOf(page, lang);
      const vars = {
        // Image de partage (réseaux, messageries) dans la langue de la page.
        og: assets.get(`img/og-${lang}.png`),
        oglocale: OG_LOCALES[lang],
        ogalternates: LANGS.filter((other) => other !== lang)
          .map(
            (other) =>
              `<meta property="og:locale:alternate" content="${OG_LOCALES[other]}" />`,
          )
          .join("\n    "),
        lang,
        page,
        vue: TOOLS[page]?.vue ?? "",
        path: pagePath,
        url: `/${lang}/${pagePath}`,
        origin: ORIGIN,
        year,
      };
      // Chemin d'une autre page dans cette langue : {{path:dictation}} ; et la
      // marque de la page en cours dans une liste de liens : {{here:dictation}}.
      for (const other of Object.keys(PAGES)) {
        vars[`path:${other}`] = pathOf(other, lang);
        vars[`here:${other}`] = other === page ? ' aria-current="page"' : "";
      }
      for (const other of LANGS) {
        vars[`url:${other}`] = `/${other}/${pathOf(page, other)}`;
        vars[`current:${other}`] = other === lang ? ' aria-current="true"' : "";
      }
      const draw = (date, more = {}) =>
        render(template, {
          texts: locales[lang],
          partials,
          assets,
          vars: {
            ...vars,
            ...more,
            jsonld: UNLISTED.has(page)
              ? ""
              : jsonLd(
                  structuredData({
                    page,
                    lang,
                    texts: locales[lang],
                    assets,
                    modified: date,
                  }),
                ),
          },
          used,
        });
      let html = draw("");
      if (!UNLISTED.has(page)) {
        // La date fait partie de la page : l'empreinte se prend sans elle, et
        // sans l'année du pied de page (un 1er janvier ne redate rien).
        const id = `${lang}/${pagePath}`;
        const hash = contentHash(draw("", { year: "" }));
        const date = known[id]?.hash === hash ? known[id].date : today;
        modified[id] = { hash, date };
        html = draw(date);
      }
      const file = `${lang}/${pagePath}index.html`;
      assertCsp(html, file);
      await emit(file, html);
    }
  }
  if (saveDates)
    await writeFile(dates, `${JSON.stringify(modified, null, 2)}\n`);

  // Racine : nginx renvoie « / » vers la langue du navigateur ; ces fichiers
  // servent de repli (autre hébergeur, erreur 404 hors d'une langue).
  await emit(
    "404.html",
    await readFile(path.join(out, DEFAULT_LANG, "404", "index.html"), "utf8"),
  );
  const root = render(await readFile(path.join(src, "root.html"), "utf8"), {
    texts: locales[DEFAULT_LANG],
    partials,
    assets,
    vars: { origin: ORIGIN, year },
  });
  assertCsp(root, "index.html");
  await emit("index.html", root);
  // Fichiers servis tels quels à la racine (favicon.ico, que les navigateurs
  // demandent d'office).
  for (const [name, buffer] of await readDir(path.join(src, "racine"))) {
    await emit(name, buffer);
  }
  // Tout le site est ouvert à tous les robots, ceux des moteurs de réponse et
  // des assistants compris ; seules les adresses du service ne sont pas des pages.
  await emit(
    "robots.txt",
    [
      "# chatonlm.com: every page is open to every crawler, AI search and",
      "# assistants included. The form and download endpoints are not pages.",
      "User-agent: *",
      "Allow: /",
      "Disallow: /api/",
      // L'état des téléchargements : sans lui, un moteur qui exécute le script
      // de la page lirait « Télécharger » avant qu'une version soit publiée.
      "Allow: /api/release",
      "Disallow: /download/",
      "",
      `Sitemap: ${ORIGIN}/sitemap.xml`,
      "",
    ].join("\n"),
  );
  // Résumé du site pour les assistants (llmstxt.org : un titre, une citation,
  // du texte libre, puis des sections « ## » qui ne portent que des liens), en
  // anglais : mêmes textes que la page anglaise pour ce que font les outils, et
  // les adresses anglaises des pages ({{path:images}}).
  await emit(
    "llms.txt",
    render(await readFile(path.join(src, "llms.txt"), "utf8"), {
      texts: locales[DEFAULT_LANG],
      partials,
      assets,
      vars: {
        origin: ORIGIN,
        ...Object.fromEntries(
          Object.keys(PAGES).map((page) => [
            `path:${page}`,
            pathOf(page, DEFAULT_LANG),
          ]),
        ),
      },
      used,
    }),
  );
  const unused = [...locales[DEFAULT_LANG].keys()].filter(
    (key) => !used.has(key),
  );
  if (unused.length)
    throw new Error(`Textes jamais employés :\n  ${unused.join("\n  ")}`);

  const urls = Object.keys(PAGES)
    .filter((page) => !UNLISTED.has(page))
    .flatMap((page) =>
      LANGS.map((lang) => {
        const links = [...LANGS.map((l) => [l, l]), ["x-default", DEFAULT_LANG]]
          .map(
            ([code, l]) =>
              `    <xhtml:link rel="alternate" hreflang="${code}" href="${ORIGIN}/${l}/${pathOf(page, l)}"/>`,
          )
          .join("\n");
        const pagePath = pathOf(page, lang);
        return `  <url>\n    <loc>${ORIGIN}/${lang}/${pagePath}</loc>\n    <lastmod>${modified[`${lang}/${pagePath}`].date}</lastmod>\n${links}\n  </url>`;
      }),
    );
  await emit(
    "sitemap.xml",
    `<?xml version="1.0" encoding="UTF-8"?>\n<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9" xmlns:xhtml="http://www.w3.org/1999/xhtml">\n${urls.join("\n")}\n</urlset>\n`,
  );
  return written;
}

if (
  process.argv[1] &&
  import.meta.url === pathToFileURL(process.argv[1]).href
) {
  const files = await build({ saveDates: true });
  console.log(`Site fabriqué : ${files.length} fichiers dans site/dist/`);
}
