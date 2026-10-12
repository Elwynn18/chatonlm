// Rendu Markdown des réponses dans l'app réelle (build e2e : frontend
// embarqué, CSP de production appliquée par Tauri), avec les scénarios
// chatonlm-markdown, -slow et -long du faux serveur :
// - aucune violation de la CSP (témoin : un script en ligne est bien signalé)
//   ni erreur ou avertissement dans la console ;
// - surligneur : absent de index.html, chargé après l'affichage de la fenêtre
//   sans attendre un bloc de code (les entrées « resource » de la page sont
//   vides sous tauri:// : un bloc envoyé 4 s après le lancement arrive coloré) ;
//   toutes les expressions de ses grammaires compilent dans le JavaScriptCore
//   de l'app (son propre préchauffage, rejoué dans la page) ;
// - DOM : titres, listes, tableau, citations, code en ligne, blocs de code
//   (étiquette, deux <pre>, couleurs ; un bloc par langage coloré, les 19 de
//   l'officiel, sans aucune ligne laissée en texte brut ; langage inconnu et
//   mermaid en texte brut), maths KaTeX et leurs polices, HTML brut et image
//   neutralisés ;
// - thème clair puis sombre : un seul des deux <pre> affiché ;
// - liens : https et localhost ouverts par xdg-open (faux, qui note
//   l'adresse) au clic et au clic milieu ; infobulle = titre Markdown, sinon
//   l'adresse ouverte ; javascript: et lien vide en <a> sans href, inertes,
//   à la couleur des liens ; menu maison « Open Link » / « Copy Link » ;
//   window.open refusé ; échec de xdg-open : avis en haut de la zone de
//   contenu, qui ne couvre pas la zone de saisie ;
// - copie (bouton d'un bloc, bouton du message, sélection d'un bloc) relue
//   dans le presse-papiers de la session comme le ferait une autre
//   application ;
// - flux lent : bloc de code affiché et coloré pendant le flux, sans « ``` »
//   ni bouton copier du message, bouton présent à la fin ; le code affiché
//   n'est jamais que le début du code final (aucune réparation du Markdown
//   incomplet dans un bloc ouvert : ni « $$$$ » ni lien incomplet) ; pas de
//   remontage à la fin de la réponse (même <pre>, sélection et focus faits
//   pendant le flux gardés, zone de saisie laissée sans focus) ;
// - mesures avec seuils de régression : pire écart entre deux images pendant
//   un long bloc en flux et à la fin de la réponse, recalculs de style
//   forcés sur la longue réponse, images pendant le chargement et le
//   préchauffage du surligneur ; délai de coloration du premier bloc dans une
//   app fraîchement lancée, préchauffage fini ou non.
import assert from "node:assert/strict";
import {
  closeSync,
  existsSync,
  mkdtempSync,
  openSync,
  readFileSync,
  readSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { join } from "node:path";
import { ROOT, withApp } from "./lib/app.mjs";
import { readClipboard } from "./lib/clipboard.mjs";
import { startFake } from "./lib/fake.mjs";
import { PENDING_CALLS } from "./lib/page.mjs";

const binary = process.argv[2];
const fake = await startFake("ollama");

const FIXTURES = join(ROOT, "src-tauri/crates/fake-ollama/fixtures");
const MARKDOWN = readFileSync(join(FIXTURES, "markdown.md"), "utf8");
const STREAM = readFileSync(join(FIXTURES, "markdown-stream.md"), "utf8");
const LONG = readFileSync(join(FIXTURES, "markdown-long.md"), "utf8");

/** Blocs de code d'un texte Markdown : langage de la clôture et code (avec son "\n" final). */
const fences = (markdown) =>
  [...markdown.matchAll(/^```(.*)\n([\s\S]*?)^```$/gm)].map(
    ([, lang, code]) => ({ lang, code }),
  );

/** Les 19 langages colorés (ceux de l'officiel), nom de clôture sans casse. */
const COLORED_LANGS = new Set([
  "javascript",
  "typescript",
  "python",
  "bash",
  "shell",
  "json",
  "html",
  "css",
  "tsx",
  "jsx",
  "go",
  "rust",
  "java",
  "c",
  "cpp",
  "sql",
  "swift",
  "yaml",
  "markdown",
]);
const colored = (lang) => COLORED_LANGS.has(lang.toLowerCase());

// Seuils de régression, tirés des mesures dans la session virtuelle (après
// le retrait de la garde not-prose de typography et sans remontage en fin de
// réponse ; avant entre parenthèses), avec de la marge pour une machine
// chargée :
// - écart entre deux images pendant le flux du long bloc, puis jusqu'à 1 s
//   après la fin : 54-64 ms (518-591 ms) ;
// - recalcul de style forcé de la longue réponse (bloc masqué puis
//   réaffiché ; bloc qui cesse d'être le dernier enfant) : 28-34 ms
//   (512-541 ms) ; classe dark sur <html> : 62-94 ms (1 121-1 203 ms) ;
// - écart entre deux images pendant le chargement et le préchauffage du
//   surligneur : 90-120 ms.
const FRAME_LIMIT_MS = 300;
const RESTYLE_LIMIT_MS = 150;
const THEME_LIMIT_MS = 300;
const START_FRAME_LIMIT_MS = 300;

// Faux xdg-open : note le lien et réussit ; échoue (code 3, « aucune
// application pour les liens web ») tant que le fichier « fail » existe.
const fakeBin = mkdtempSync(join(ROOT, ".tools/tmp/xdg-open-"));
process.on("exit", () => rmSync(fakeBin, { recursive: true, force: true }));
writeFileSync(
  join(fakeBin, "xdg-open"),
  `#!/bin/sh
if [ -e "${fakeBin}/fail" ]; then echo "$1" >> "${fakeBin}/failed"; exit 3; fi
echo "$1" >> "${fakeBin}/opened"
`,
  { mode: 0o755 },
);
const linesOf = (name) =>
  existsSync(join(fakeBin, name))
    ? readFileSync(join(fakeBin, name), "utf8").trim().split("\n")
    : [];
const opened = () => linesOf("opened");

const delay = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

/** `what` : texte, ou fonction appelée à l'échec (dernier état observé). */
async function waitUntil(check, what, timeoutMs = 10000) {
  const deadline = Date.now() + timeoutMs;
  for (;;) {
    const value = await check();
    if (value) return value;
    if (Date.now() > deadline) {
      throw new Error(
        `Délai dépassé : ${typeof what === "function" ? what() : what}`,
      );
    }
    await delay(100);
  }
}

const exists = (driver, css) =>
  driver.execute("return document.querySelector(arguments[0]) !== null", [css]);

/**
 * Chemin, dans l'app, d'un morceau JavaScript du frontend embarqué dans le
 * binaire testé (nom haché par Vite) : lu dans la table des fichiers que
 * Tauri compile dans le binaire, donc toujours celui de ce build.
 */
function embeddedChunk(file, name) {
  const needle = Buffer.from(`/assets/${name}-`);
  const buffer = Buffer.alloc(1 << 24);
  const fd = openSync(file, "r");
  try {
    let carry = Buffer.alloc(0);
    for (let position = 0; ;) {
      const read = readSync(fd, buffer, 0, buffer.length, position);
      if (read === 0) return null;
      position += read;
      const data = Buffer.concat([carry, buffer.subarray(0, read)]);
      for (let at = data.indexOf(needle); at >= 0;) {
        const found = /^\/assets\/[\w-]+\.js/.exec(
          data.subarray(at, at + 200).toString("latin1"),
        );
        if (found) return found[0];
        at = data.indexOf(needle, at + 1);
      }
      // Un nom coupé entre deux lectures est retrouvé dans la suivante.
      carry = data.subarray(Math.max(0, data.length - 256));
    }
  } finally {
    closeSync(fd);
  }
}

// Violations de la CSP, erreurs et avertissements de la console : notés par
// la page (écouteurs posés à nouveau après chaque rechargement) et relevés
// avant chaque rechargement. console.warn : le surligneur y signale les
// expressions de ses grammaires qui ne compilent pas ici.
const violations = [];
const consoleErrors = [];
const consoleWarnings = [];
const WATCH_PAGE = `
  if (!window.__csp) {
    window.__csp = [];
    window.__errors = [];
    window.__warnings = [];
    document.addEventListener("securitypolicyviolation", (event) =>
      window.__csp.push(event.violatedDirective + " " + event.blockedURI));
    const error = console.error;
    console.error = (...args) => {
      window.__errors.push(args.map(String).join(" "));
      error.apply(console, args);
    };
    const warn = console.warn;
    console.warn = (...args) => {
      window.__warnings.push(args.map(String).join(" "));
      warn.apply(console, args);
    };
  }`;
async function collectViolations(driver) {
  const found = await driver.execute(
    `const found = { csp: window.__csp ?? [], errors: window.__errors ?? [],
       warnings: window.__warnings ?? [] };
     window.__csp = [];
     window.__errors = [];
     window.__warnings = [];
     return found;`,
  );
  violations.push(...found.csp);
  consoleErrors.push(...found.errors);
  consoleWarnings.push(...found.warnings);
}

/**
 * Recharge la page et attend la nouvelle (écouteur de la CSP reposé). Pas
 * pendant un appel au backend (réglages, modèles, recommandations, détails) :
 * sa réponse arriverait dans la nouvelle page, où Tauri écrit « Couldn't find
 * callback id » dans la console. Vérification et rechargement dans le même
 * script : aucun appel ne part entre les deux.
 */
async function reload(driver) {
  await collectViolations(driver);
  await waitUntil(
    () =>
      driver.execute(
        `${PENDING_CALLS}
         if (pending.length > 0) return false;
         window.__oldPage = true;
         location.reload();
         return true;`,
      ),
    "appels au backend toujours en cours avant le rechargement",
  );
  await waitUntil(
    async () => !(await driver.execute("return window.__oldPage === true")),
    "rechargement",
  );
  await driver.execute(WATCH_PAGE);
}

const shownModel = (driver) =>
  driver.execute(
    "return document.querySelector('[title=\"Select model\"]')?.textContent ?? null",
  );

/**
 * Choisit un modèle (absent de la liste), puis recharge la page. La page doit
 * déjà afficher un modèle : sinon, son modèle de secours, écrit dès qu'elle a
 * lu la liste, pourrait remplacer celui-ci (son cache des réglages ne voit
 * pas l'appel du test). Le réglage est relu après le rechargement.
 */
async function useModel(driver, model) {
  assert.ok(await shownModel(driver), "aucun modèle affiché avant useModel");
  await driver.invoke("update_settings", { patch: { selectedModel: model } });
  await reload(driver);
  await waitUntil(
    async () => (await shownModel(driver)) === model,
    `modèle ${model}`,
  );
  assert.equal(
    (await driver.invoke("get_settings")).selectedModel,
    model,
    "réglage du modèle remplacé",
  );
}

/** Nouvelle conversation, par le bouton « New chat ». */
async function newChat(driver) {
  await driver.click(await driver.waitFor("a[title='New chat']"));
  await driver.waitFor("[data-testid=new-chat]");
}

/** Tape un message et l'envoie avec Entrée ; renvoie l'instant de l'envoi (performance.now). */
async function send(driver, text) {
  const textarea = await driver.waitFor("textarea[aria-label='Message']");
  await driver.type(textarea, text);
  return driver.execute(
    `const sentAt = performance.now();
     document.querySelector("textarea[aria-label='Message']").dispatchEvent(
       new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true }));
     return sentAt;`,
  );
}

/** Attend la fin de la réponse : bouton d'envoi revenu et bouton copier du message. */
async function waitReplyDone(driver, timeoutMs = 15000) {
  await waitUntil(
    async () =>
      (await exists(driver, "button[aria-label='Send message']")) &&
      (await exists(driver, "button[title=Copy]")),
    "fin de la réponse",
    timeoutMs,
  );
}

/** fn(driver), avec une capture de la fenêtre en cas d'échec. */
const shotOnFailure = (name, fn) => async (driver) => {
  try {
    await fn(driver);
  } catch (error) {
    await driver.screenshot(`.tools/shots/${name}-failure.png`).catch(() => {});
    throw error;
  }
};

/** État du dernier bloc de code de la dernière réponse. */
const lastBlock = (driver) =>
  driver.execute(`
    const pre = [...document.querySelectorAll(
      "[data-role=assistant] pre[data-theme=light]")].at(-1);
    return pre && {
      text: pre.textContent,
      colored: pre.querySelectorAll("span[style*=color]").length,
      plain: pre.querySelectorAll("span:not([style])").length,
    };`);

/**
 * Attend que les blocs de la dernière réponse soient colorés : `count` <pre>
 * clairs avec des couleurs, et plus aucune ligne en texte brut dans les deux
 * thèmes (les lignes au-delà du budget de temps d'un rendu sont colorées
 * ensuite, pendant les temps morts).
 */
async function waitColored(driver, count, what) {
  let state;
  await waitUntil(
    async () => {
      state = await driver.execute(`
        const reply = [...document.querySelectorAll("[data-role=assistant]")].at(-1);
        return {
          colored: [...reply.querySelectorAll("pre[data-theme=light]")]
            .filter((pre) => pre.querySelector("span[style]")).length,
          plain: reply.querySelectorAll("pre code span:not([style])").length,
        };`);
      return state.colored === count && state.plain === 0;
    },
    () => `${what} : ${JSON.stringify(state)}`,
    15000,
  );
}

/**
 * Surveille le code affiché pendant un flux : à chaque changement du DOM, le
 * texte de chaque <pre> de la dernière réponse, sans son "\n" final ni une
 * clôture encore incomplète (« ` », « `` »), doit être un début de `final`
 * (le code du bloc à la fin) : aucun « $$$$ », aucun lien incomplet ni « ** »
 * ajouté à la fin d'un bloc ouvert, ni nulle part le marqueur
 * streamdown:incomplete-link. Note aussi le premier <pre>, pour vérifier à
 * la fin que c'est toujours le même nœud (pas de remontage).
 */
const WATCH_CODE = `
  const final = arguments[0];
  const state = (window.__code = { checks: 0, issues: [], first: null });
  const note = (what, text) => {
    if (state.issues.length < 5) {
      state.issues.push(what + " : " + JSON.stringify(text.slice(-120)));
    }
  };
  const check = () => {
    const reply = [...document.querySelectorAll("[data-role=assistant]")].at(-1);
    if (!reply) return;
    if (reply.textContent.includes("streamdown:incomplete-link")) {
      note("marqueur de lien incomplet", reply.textContent);
    }
    for (const pre of reply.querySelectorAll("pre")) {
      const text = pre.textContent;
      const shown = text.replace(/\\n$/, "").replace(/(^|\\n)\`{1,2}$/, "$1");
      state.checks += 1;
      if (text.includes("$$$$")) note("« $$$$ » dans le code", text);
      else if (!final.startsWith(shown)) note("code qui n'est pas un début du bloc final", text);
    }
    state.first ??= reply.querySelector("pre[data-theme=light]");
  };
  state.observer = new MutationObserver(check);
  state.observer.observe(document.body,
    { childList: true, subtree: true, characterData: true });`;
const STOP_WATCH_CODE = `
  const state = window.__code;
  state.observer.disconnect();
  const reply = [...document.querySelectorAll("[data-role=assistant]")].at(-1);
  const last = [...reply.querySelectorAll("pre[data-theme=light]")].at(-1);
  return {
    checks: state.checks,
    issues: state.issues,
    samePre: state.first !== null && state.first === last && last.isConnected,
  };`;

try {
  await withApp(
    binary,
    {
      name: "markdown",
      env: { OLLAMA_HOST: fake.url, PATH: `${fakeBin}:${process.env.PATH}` },
    },
    shotOnFailure("markdown", async (driver) => {
      await driver.waitFor("[data-testid=new-chat]");
      // Modèle de secours écrit par la page (session neuve : réglage vide)
      // avant tout choix du test, qu'il remplacerait sinon.
      await waitUntil(
        async () => (await shownModel(driver)) === "qwen3:0.6b",
        "modèle par défaut",
      );

      // Le surligneur (morceau de 2 Mo) n'est ni préchargé par index.html ni
      // importé statiquement : il est chargé après l'affichage de la fenêtre.
      assert.deepEqual(
        await driver.execute(
          `return [...document.querySelectorAll("link[href], script[src]")]
             .map((e) => e.getAttribute("href") ?? e.getAttribute("src"))
             .filter((url) => url.includes("highlighterCore"))`,
        ),
        [],
        "surligneur chargé avec la page",
      );

      // Écouteur de la CSP, vérifié sur un script en ligne (refusé par
      // script-src 'self').
      await driver.execute(WATCH_PAGE);
      await driver.execute(`
        const script = document.createElement("script");
        script.textContent = "window.__inline = true";
        document.head.append(script);`);
      await waitUntil(
        async () => (await driver.execute("return window.__csp.length")) > 0,
        "violation témoin de la CSP",
      );
      assert.equal(
        await driver.execute("return typeof window.__inline"),
        "undefined",
      );
      assert.match(
        (await driver.execute("return window.__csp"))[0],
        /^script-src/,
      );
      await driver.execute("window.__csp = []");

      // Réponse Markdown complète.
      await useModel(driver, "chatonlm-markdown");
      await send(driver, "Show me some Markdown.");
      await driver.waitFor("[data-testid=chat]");
      await waitReplyDone(driver);
      // Blocs colorés (le surligneur, chargé après le rechargement de la
      // page, peut arriver un peu après la réponse).
      const expected = fences(MARKDOWN);
      await waitColored(
        driver,
        expected.filter((f) => colored(f.lang)).length,
        "blocs de code colorés",
      );

      const dom = await driver.execute(`
        const root = document.querySelector("[data-role=assistant]");
        const all = (css) => [...root.querySelectorAll(css)];
        const texts = (css) => all(css).map((e) => e.textContent);
        return {
          headings: ["h1", "h2", "h3", "h4", "h5", "h6"].map((h) => texts(h)),
          lists: {
            ul3: all("ul ul ul").length,
            ol2: all("ol ol").length,
            tasks: all("li input[type=checkbox]").map((c) => c.checked),
          },
          table: {
            wrapped: all("div.overflow-x-auto > table").length,
            head: texts("th"),
            align: all("th").map((e) => e.style.textAlign),
            cells: all("td").length,
            nodeAttribute: all("[node]").length,
          },
          nestedQuotes: all("blockquote blockquote").length,
          inlineCode: texts("code[data-streamdown=inline-code]"),
          blocks: all("[data-code-block-header]").map((header) => {
            const block = header.parentElement;
            const light = block.querySelector("pre[data-theme=light]");
            const dark = block.querySelector("pre[data-theme=dark]");
            return {
              label: header.querySelector(":scope > div")?.textContent ?? "",
              pres: block.querySelectorAll("pre").length,
              text: light.textContent,
              sameText: light.textContent === dark.textContent,
              colored: light.querySelectorAll("span[style*=color]").length,
              plain: block.querySelectorAll("pre span:not([style])").length,
              spans: light.querySelectorAll("span").length,
              lightColor: light.querySelector("span[style]")?.style.color ?? "",
              darkColor: dark.querySelector("span[style]")?.style.color ?? "",
              svg: block.querySelectorAll("svg:not(button svg)").length,
            };
          }),
          math: {
            katex: all(".katex").length,
            display: all(".katex-display").length,
            errors: all(".katex-error").length,
            dollars: texts("p").find((t) => t.includes("$5")),
            dollarsInMath: all(".katex").some((k) => k.textContent.includes("5")),
          },
          dangerous: all("script, iframe, img, object, embed").length,
          rawHtmlAsText: root.textContent.includes(
            "<script>window.__pwned=1</script>"),
          imageAlt: all("span").some(
            (s) => s.textContent === "Ollama logo" && s.children.length === 0),
          pwned: typeof window.__pwned,
          links: all("[data-streamdown=link]").map((e) => [
            e.tagName, e.getAttribute("href"), e.getAttribute("target"),
            e.textContent, e.getAttribute("title")]),
        };`);

      assert.deepEqual(dom.headings, [
        ["Markdown check"],
        ["Lists", "Links", "Raw HTML"],
        ["Table"],
        ["Quote"],
        ["Code"],
        ["Math"],
      ]);
      assert.deepEqual(dom.lists, { ul3: 1, ol2: 1, tasks: [true, false] });
      assert.deepEqual(dom.table, {
        wrapped: 1,
        head: ["Model", "Size", "Context"],
        align: ["left", "right", "center"],
        cells: 6,
        nodeAttribute: 0,
      });
      assert.equal(dom.nestedQuotes, 1);
      assert.deepEqual(dom.inlineCode, ["inline code", "code", "ollama serve"]);

      // Blocs de code : étiquette telle qu'écrite, deux <pre> au même texte
      // (celui du fichier), couleurs pour les 19 langages (Python compris),
      // sans aucune ligne en texte brut ; texte brut sinon (ruby, mermaid
      // sans diagramme, bloc sans langage).
      assert.deepEqual(
        dom.blocks.map((b) => b.label),
        expected.map((f) => f.lang),
      );
      assert.deepEqual(
        [...COLORED_LANGS].filter(
          (lang) => !expected.some((f) => f.lang === lang),
        ),
        [],
        "langage coloré sans bloc dans markdown.md",
      );
      for (const [i, block] of dom.blocks.entries()) {
        const { lang, code } = expected[i];
        assert.equal(block.pres, 2, `${lang} : deux <pre>`);
        assert.equal(block.text, code, `${lang} : texte du bloc`);
        assert.ok(block.sameText, `${lang} : <pre> sombre différent`);
        assert.equal(block.svg, 0, `${lang} : diagramme ou image dans le bloc`);
        if (colored(lang)) {
          assert.ok(block.colored > 3, `${lang} : bloc non coloré`);
          assert.equal(
            block.plain,
            0,
            `${lang} : lignes laissées en texte brut`,
          );
          assert.notEqual(
            block.lightColor,
            block.darkColor,
            `${lang} : mêmes couleurs en clair et en sombre`,
          );
        } else {
          assert.equal(block.spans, 0, `${lang || "sans langage"} : coloré`);
        }
      }

      assert.deepEqual(dom.math, {
        katex: 4,
        display: 2,
        errors: 0,
        dollars: "Single dollars stay text: $5 and $10.",
        dollarsInMath: false,
      });
      assert.equal(dom.dangerous, 0, "script, iframe ou img dans la réponse");
      assert.equal(dom.rawHtmlAsText, true, "HTML brut non affiché en texte");
      assert.equal(dom.imageAlt, true, "texte de l'image absent");
      assert.equal(dom.pwned, "undefined");
      // Liens web : infobulle = titre Markdown, sinon l'adresse qui sera
      // ouverte (même quand le texte annonce une autre adresse) ; autres
      // liens : <a> sans href.
      assert.deepEqual(dom.links, [
        [
          "A",
          "https://example.com/ollama-docs",
          null,
          "Ollama docs",
          "https://example.com/ollama-docs",
        ],
        [
          "A",
          "http://localhost:11434",
          null,
          "local server",
          "http://localhost:11434/",
        ],
        ["A", null, null, "x", null],
        ["A", null, null, "empty link", null],
        [
          "A",
          "https://example.com/autolink",
          null,
          "https://example.com/autolink",
          "https://example.com/autolink",
        ],
        ["A", "https://example.com/titled", null, "Titled link", "Docs title"],
        [
          "A",
          "https://example.com/real",
          null,
          "https://example.com/shown",
          "https://example.com/real",
        ],
      ]);

      // Polices KaTeX chargées sous la CSP de production (woff2 de l'app).
      const fonts = await driver.executeAsync(`
        const done = arguments[0];
        document.fonts.ready.then(() => {
          const katex = [...document.fonts].filter((f) => f.family.includes("KaTeX"));
          done({
            check: document.fonts.check("16px KaTeX_Main"),
            main: katex
              .filter((f) => f.family.replace(/["']/g, "") === "KaTeX_Main"
                && f.weight === "400" && f.style === "normal")
              .map((f) => f.status),
            failed: katex.filter((f) => f.status === "error").map((f) => f.family),
            family: getComputedStyle(
              document.querySelector("[data-role=assistant] .katex")).fontFamily,
          });
        });`);
      assert.deepEqual(fonts.main, ["loaded"], "KaTeX_Main non chargée");
      assert.equal(fonts.check, true);
      assert.deepEqual(fonts.failed, []);
      assert.match(fonts.family, /KaTeX_Main/);

      // Liens. https : ouvert dans le navigateur, la page ne bouge pas.
      const page = await driver.execute("return location.href");
      const link = (href) =>
        driver.find(`[data-role=assistant] a[href="${href}"]`);
      await driver.click(await link("https://example.com/ollama-docs"));
      await waitUntil(
        () => opened().includes("https://example.com/ollama-docs"),
        "lien https ouvert par xdg-open",
      );
      await driver.click(await link("http://localhost:11434"));
      await waitUntil(
        () => opened().includes("http://localhost:11434/"),
        "lien localhost ouvert par xdg-open",
      );
      // Clic milieu (auxclick, bouton 1) : ouvert aussi ; le geste est annulé.
      assert.equal(
        await driver.execute(`
          return document.querySelector(
            '[data-role=assistant] a[href="https://example.com/autolink"]')
            .dispatchEvent(new MouseEvent("auxclick",
              { button: 1, bubbles: true, cancelable: true }));`),
        false,
        "clic milieu non annulé",
      );
      await waitUntil(
        () => opened().includes("https://example.com/autolink"),
        "lien ouvert au clic milieu",
      );
      // javascript: et lien vide : <a> sans href, à la couleur des liens
      // (styles .prose de « a ») mais sans curseur de lien, hors de la
      // tabulation ; rien ne s'ouvre ni ne s'exécute au clic.
      const inert = await driver.execute(`
        const links = [...document.querySelectorAll(
          "[data-role=assistant] a[data-streamdown=link]")];
        const web = links.find((a) => a.hasAttribute("href"));
        return links.filter((a) => !a.hasAttribute("href")).map((a) => {
          a.focus();
          return {
            text: a.textContent,
            focusable: document.activeElement === a,
            linkColor: getComputedStyle(a).color === getComputedStyle(web).color,
            pointer: getComputedStyle(a).cursor === "pointer",
            webPointer: getComputedStyle(web).cursor === "pointer",
          };
        });`);
      assert.deepEqual(
        inert,
        ["x", "empty link"].map((text) => ({
          text,
          focusable: false,
          linkColor: true,
          pointer: false,
          webPointer: true,
        })),
      );
      await driver.execute(`
        for (const a of document.querySelectorAll(
          "[data-role=assistant] a[data-streamdown=link]:not([href])")) a.click();`);
      await delay(1500);
      assert.deepEqual(opened(), [
        "https://example.com/ollama-docs",
        "http://localhost:11434/",
        "https://example.com/autolink",
      ]);
      assert.equal(
        await driver.execute("return typeof window.__pwned"),
        "undefined",
      );
      assert.equal(await driver.execute("return location.href"), page);

      // Clic droit : menu maison (celui de WebKit est annulé).
      const openMenu = (href) =>
        driver.execute(
          `const a = document.querySelector(
             '[data-role=assistant] a[href="' + arguments[0] + '"]');
           const box = a.getBoundingClientRect();
           return a.dispatchEvent(new MouseEvent("contextmenu", {
             bubbles: true, cancelable: true, button: 2,
             clientX: box.left + 5, clientY: box.top + 5 }));`,
          [href],
        );
      const menuItems = () =>
        driver.execute(
          "return [...document.querySelectorAll('[role=menu] [role=menuitem]')].map((e) => e.textContent)",
        );
      const menuItem = async (label) => {
        for (const item of await driver.findAll(
          "[role=menu] [role=menuitem]",
        )) {
          if ((await driver.text(item)) === label) return item;
        }
        throw new Error(`Élément de menu introuvable : ${label}`);
      };
      assert.equal(
        await openMenu("https://example.com/ollama-docs"),
        false,
        "menu de WebKit non annulé",
      );
      await driver.waitFor("[role=menu]");
      assert.deepEqual(await menuItems(), ["Open Link", "Copy Link"]);
      await driver.screenshot(".tools/shots/markdown-link-menu.png");
      await driver.click(await menuItem("Copy Link"));
      await waitUntil(
        async () => !(await exists(driver, "[role=menu]")),
        "menu fermé",
      );
      const copiedLink = await readClipboard();
      assert.equal(copiedLink.text, "https://example.com/ollama-docs");
      await openMenu("https://example.com/ollama-docs");
      await driver.click(await menuItem("Open Link"));
      await waitUntil(
        () =>
          opened().filter((u) => u === "https://example.com/ollama-docs")
            .length === 2,
        "« Open Link » du menu",
      );

      // window.open : aucune nouvelle fenêtre, refus noté dans le journal.
      assert.equal(
        await driver.execute(
          "return window.open('https://example.com/popup') === null",
        ),
        true,
      );
      const log = `${ROOT}.tools/logs/e2e-markdown.log`;
      await waitUntil(
        () =>
          readFileSync(log, "utf8").includes(
            "chatonlm: new window refused: https://example.com/popup",
          ),
        "refus de la nouvelle fenêtre dans le journal",
      );
      assert.deepEqual(
        await driver.request("GET", driver.session("/window/handles")),
        ["main"],
      );
      assert.ok(!opened().includes("https://example.com/popup"));

      // Échec de xdg-open : avis (raison et adresse) en haut de la zone de
      // contenu, sous la barre du haut ; la zone de saisie reste dégagée et
      // ses boutons reçoivent le pointeur ; Dismiss le ferme.
      writeFileSync(join(fakeBin, "fail"), "");
      await driver.click(await link("https://example.com/titled"));
      const notice = await waitUntil(
        () =>
          driver.execute(`
            const alert = [...document.querySelectorAll("[role=alert]")]
              .find((e) => e.textContent.startsWith("Couldn't open the browser"));
            if (!alert) return null;
            const box = (e) => {
              const r = e.getBoundingClientRect();
              return { top: r.top, bottom: r.bottom };
            };
            const reachable = (e) => {
              const r = e.getBoundingClientRect();
              return e.contains(document.elementFromPoint(
                r.left + r.width / 2, r.top + r.height / 2));
            };
            return {
              text: alert.querySelector("p").textContent,
              inMain: alert.closest("main") !== null,
              notice: box(alert),
              composer: box(document.querySelector("textarea[aria-label='Message']")),
              send: reachable(document.querySelector("button[aria-label='Send message']")),
              model: reachable(document.querySelector('[title="Select model"]')),
            };`),
        "avis d'échec du lien",
      );
      await driver.screenshot(".tools/shots/markdown-link-error.png");
      assert.match(
        notice.text,
        /^Couldn't open the browser \(.*xdg-open exited with 3.*\)\. Go to https:\/\/example\.com\/titled$/,
      );
      assert.equal(notice.inMain, true, "avis hors de la zone de contenu");
      assert.ok(notice.notice.top >= 50, `avis à ${notice.notice.top} px`);
      assert.ok(
        notice.notice.bottom < notice.composer.top,
        `avis sur la zone de saisie : ${JSON.stringify(notice)}`,
      );
      assert.equal(notice.send, true, "bouton d'envoi couvert par l'avis");
      assert.equal(notice.model, true, "choix du modèle couvert par l'avis");
      assert.deepEqual(linesOf("failed"), ["https://example.com/titled"]);
      rmSync(join(fakeBin, "fail"));
      await driver.click(
        await driver.find("[role=alert] button[aria-label=Dismiss]"),
      );
      await waitUntil(
        async () =>
          !(await driver.execute(
            `return [...document.querySelectorAll("[role=alert]")]
               .some((e) => e.textContent.startsWith("Couldn't open the browser"))`,
          )),
        "avis fermé par Dismiss",
      );

      // Copie d'un bloc de code : le code, sans le saut de ligne final.
      const codeButton = (
        await driver.findAll(
          "[data-role=assistant] [data-code-block-header] button",
        )
      )[0];
      await driver.click(codeButton);
      await waitUntil(
        async () => (await driver.text(codeButton)) === "Copied",
        "« Copied » sur le bouton du bloc",
      );
      const code = await readClipboard();
      assert.equal(code.text, expected[0].code.replace(/\n$/, ""));

      // Copie du message : Markdown en texte, HTML nettoyé (un seul <pre> par
      // bloc, sans en-tête, gras en <strong>, sans classes ni data-*). Le HTML
      // passé au presse-papiers est relevé dans la page ; celui relu est
      // retravaillé par WebKit (styles calculés, Apple-converted-space).
      await driver.execute(`
        window.__written = [];
        const write = navigator.clipboard.write.bind(navigator.clipboard);
        navigator.clipboard.write = (items) => {
          for (const item of items) {
            for (const type of item.types) {
              item.getType(type).then((blob) => blob.text())
                .then((text) => window.__written.push({ type, text }));
            }
          }
          return write(items);
        };`);
      await driver.click(await driver.find("button[title=Copy]"));
      await waitUntil(
        () =>
          driver.execute(
            "return document.querySelector('button[title=Copy] + [data-copy-status]').textContent === 'Copied'",
          ),
        "« Copied » annoncé pour le message",
      );
      const written = await waitUntil(async () => {
        const found = await driver.execute("return window.__written");
        return found.length === 2 && found;
      }, "HTML passé au presse-papiers");
      const html = written.find((w) => w.type === "text/html").text;
      assert.equal(written.find((w) => w.type === "text/plain").text, MARKDOWN);
      assert.ok(html.startsWith('<meta charset="utf-8">'));
      assert.equal(html.match(/<pre[\s>]/g)?.length, expected.length);
      assert.doesNotMatch(html, /\sclass=|\sdata-[\w-]+=|<button|katex-html/);
      assert.match(html, /<strong>bold<\/strong>/);
      assert.match(html, /<math/);
      assert.match(html, /style="color:/);

      const clip = await readClipboard();
      assert.equal(clip.text, MARKDOWN);
      assert.ok(clip.targets.includes("text/html"), clip.targets.join());
      assert.equal(clip.html.match(/<pre[\s>]/g)?.length, expected.length);
      assert.match(clip.html, /<strong[\s>]/);
      assert.doesNotMatch(clip.html, /\sdata-[\w-]+=|<script|<button/);
      assert.deepEqual(
        [
          ...new Set(
            [...clip.html.matchAll(/\sclass="([^"]*)"/g)].map((m) => m[1]),
          ),
        ].filter((c) => c !== "Apple-converted-space"),
        [],
        "classes dans le HTML copié",
      );
      assert.doesNotMatch(clip.html, /typescriptCopy|>Copied</);

      // Sélection d'un bloc entier copiée (comme Ctrl+C, dans un clic) : le
      // <pre> masqué du thème inactif n'est pas copié en double.
      await driver.execute(`
        const button = document.createElement("button");
        button.id = "select-and-copy";
        button.onclick = () => {
          const block = document.querySelector(
            "[data-role=assistant] [data-code-block-header]").parentElement;
          const range = document.createRange();
          range.selectNodeContents(block);
          getSelection().removeAllRanges();
          getSelection().addRange(range);
          window.__selectionCopied = document.execCommand("copy");
          getSelection().removeAllRanges();
          button.remove();
        };
        document.body.append(button);`);
      await driver.click(await driver.find("#select-and-copy"));
      assert.equal(
        await driver.execute("return window.__selectionCopied"),
        true,
      );
      const selection = await readClipboard();
      assert.equal(
        selection.text.split("interface Model {").length - 1,
        1,
        `bloc sélectionné copié : ${JSON.stringify(selection.text)}`,
      );

      // Thème clair puis sombre (réglage enregistré, page rechargée) : un
      // seul des deux <pre> de chaque bloc est affiché.
      const showCode = async () => {
        await driver.waitFor(
          "[data-role=assistant] pre[data-theme=light] span[style]",
        );
        await driver.execute(`
          document.querySelector("[data-role=assistant] h5")
            .scrollIntoView({ block: "start" });`);
        await delay(300);
      };
      const displays = () =>
        driver.execute(`
          const block = document.querySelector("[data-role=assistant] [data-code-block-header]").parentElement;
          return {
            dark: document.documentElement.classList.contains("dark"),
            light: getComputedStyle(block.querySelector("pre[data-theme=light]")).display,
            darkPre: getComputedStyle(block.querySelector("pre[data-theme=dark]")).display,
          };`);
      await driver.invoke("update_settings", { patch: { theme: "light" } });
      await reload(driver);
      await showCode();
      assert.deepEqual(await displays(), {
        dark: false,
        light: "block",
        darkPre: "none",
      });
      await driver.screenshot(".tools/shots/markdown-light.png");
      await driver.invoke("update_settings", { patch: { theme: "dark" } });
      await reload(driver);
      await showCode();
      assert.deepEqual(await displays(), {
        dark: true,
        light: "none",
        darkPre: "block",
      });
      await driver.screenshot(".tools/shots/markdown-dark.png");
      await driver.invoke("update_settings", { patch: { theme: "light" } });
      await reload(driver);

      // Flux lent : le bloc de code s'affiche (et se colore) pendant le flux,
      // sans « ``` » littéral ni bouton copier du message. Son bloc bash
      // contient « $$ », « [ » et « ** », que la réparation du Markdown
      // incomplet de Streamdown complétait dans un bloc ouvert.
      await newChat(driver);
      await useModel(driver, "chatonlm-markdown-slow");
      const streamed = fences(STREAM)[0].code;
      await driver.execute(WATCH_CODE, [streamed]);
      await send(driver, "Show me a deploy script.");
      await driver.waitFor("[data-testid=chat]");
      await waitUntil(
        async () => (await lastBlock(driver))?.text.includes("step 2"),
        "bloc de code pendant le flux",
      );
      const during = await driver.execute(`
        const root = document.querySelector("[data-role=assistant]");
        return {
          stop: document.querySelector("button[aria-label='Stop']") !== null,
          fences: root.textContent.includes("\`\`\`"),
          messageCopy: document.querySelector("button[title=Copy]") !== null,
          pres: root.querySelectorAll("pre").length,
          visiblePre: getComputedStyle(root.querySelector("pre[data-theme=light]")).display,
          colored: root.querySelectorAll("pre[data-theme=light] span[style*=color]").length,
          strong: root.querySelector("[data-streamdown=strong]")?.textContent,
        };`);
      assert.deepEqual(
        { ...during, colored: during.colored > 0 },
        {
          stop: true,
          fences: false,
          messageCopy: false,
          pres: 2,
          visiblePre: "block",
          colored: true,
          strong: "deployment",
        },
      );
      await driver.screenshot(".tools/shots/markdown-streaming.png");
      // Pendant le flux : texte sélectionné dans la réponse (titre déjà
      // fini) et focus sur le bouton copier du bloc, comme un utilisateur qui
      // lit la réponse.
      const held = await driver.execute(`
        const reply = document.querySelector("[data-role=assistant]");
        const heading = reply.querySelector("h2");
        const button = reply.querySelector("[data-code-block-header] button");
        button.focus();
        getSelection().selectAllChildren(heading);
        window.__held = { heading, button };
        return {
          streaming: document.querySelector("button[aria-label='Stop']") !== null,
          selection: getSelection().toString(),
          focused: document.activeElement === button,
        };`);
      assert.deepEqual(held, {
        streaming: true,
        selection: "Deploy script",
        focused: true,
      });
      await waitReplyDone(driver);
      // Effet de fin de réponse de la zone de saisie (après un setTimeout 0).
      await delay(500);
      const kept = await driver.execute(`
        const { heading, button } = window.__held;
        const selection = getSelection();
        const kept = {
          selection: selection.toString(),
          inHeading: heading.isConnected && heading.contains(selection.anchorNode),
          focused: button.isConnected && document.activeElement === button,
          composer: document.activeElement ===
            document.querySelector("textarea[aria-label='Message']"),
        };
        selection.removeAllRanges();
        return kept;`);
      assert.deepEqual(
        kept,
        {
          selection: "Deploy script",
          inHeading: true,
          focused: true,
          composer: false,
        },
        "sélection ou focus perdus à la fin de la réponse",
      );
      const slowCode = await driver.execute(STOP_WATCH_CODE);
      assert.deepEqual(slowCode.issues, [], "code affiché pendant le flux");
      assert.ok(slowCode.checks > 20, `${slowCode.checks} relevés du code`);
      assert.equal(slowCode.samePre, true, "bloc de code remonté à la fin");
      await waitColored(driver, 1, "bloc du flux lent coloré");
      assert.equal((await lastBlock(driver)).text, streamed);
      assert.equal(
        await driver.execute(
          "return document.querySelectorAll('[data-role=assistant] .katex-display').length",
        ),
        1,
      );

      // Long bloc en flux (238 lignes de TypeScript) : pire écart entre deux
      // images pendant le flux, puis à la fin de la réponse ; durée ; bloc
      // entièrement coloré à la fin.
      await newChat(driver);
      await useModel(driver, "chatonlm-markdown-long");
      const longCode = fences(LONG)[0].code;
      await driver.execute(WATCH_CODE, [longCode]);
      await driver.execute(`
        const frames = (window.__frames = { last: performance.now(), worst: 0,
          worstAt: 0, slow: 0, count: 0, doneAt: null, worstDuring: 0,
          firstPre: null, running: true, slowGaps: [] });
        const tick = (now) => {
          const gap = now - frames.last;
          frames.last = now;
          frames.count += 1;
          if (gap > 50) {
            frames.slow += 1;
            frames.slowGaps.push([Math.round(gap), Math.round(now)]);
          }
          if (gap > frames.worst) { frames.worst = gap; frames.worstAt = now; }
          if (frames.doneAt === null && gap > frames.worstDuring) frames.worstDuring = gap;
          if (frames.running) requestAnimationFrame(tick);
        };
        requestAnimationFrame(tick);
        const watch = new MutationObserver(() => {
          const now = performance.now();
          if (frames.firstPre === null &&
              document.querySelector("[data-role=assistant] pre")) frames.firstPre = now;
          if (document.querySelector("button[title=Copy]")) {
            frames.doneAt = now;
            watch.disconnect();
          }
        });
        watch.observe(document.body, { childList: true, subtree: true });`);
      const sentAt = await send(driver, "Show me a long module.");
      await waitReplyDone(driver, 30000);
      // Images de la fin de la réponse (bouton copier du message ajouté,
      // dernières lignes colorées pendant les temps morts) comprises.
      await delay(1000);
      const frames = await driver.execute(
        "window.__frames.running = false; return window.__frames",
      );
      const longWatch = await driver.execute(STOP_WATCH_CODE);
      assert.deepEqual(longWatch.issues, [], "code affiché pendant le flux");
      assert.equal(longWatch.samePre, true, "long bloc remonté à la fin");
      await waitColored(driver, 1, "long bloc coloré");
      const longBlock = await lastBlock(driver);
      assert.equal(longBlock.text, longCode);
      assert.ok(
        longBlock.colored > 1000,
        `${longBlock.colored} jetons colorés`,
      );
      // Recalculs de style forcés (synchrones, donc plus stables que les
      // images) sur la longue réponse : bloc masqué puis réaffiché, bloc qui
      // cesse puis redevient le dernier enfant (ce qui arrive quand le
      // paragraphe qui le suit commence), classe dark sur <html>.
      const restyle = await driver.execute(`
        const reply = [...document.querySelectorAll("[data-role=assistant]")].at(-1);
        const block = reply.querySelector("[data-code-block-header]").parentElement;
        const parent = block.parentElement;
        const tail = parent.lastElementChild;
        const root = document.documentElement;
        const force = () => document.body.offsetHeight;
        const time = (change) => {
          force();
          const started = performance.now();
          change();
          force();
          return performance.now() - started;
        };
        const worst = (a, b) => Math.round(Math.max(time(a), time(b)));
        return {
          elements: reply.querySelectorAll("*").length,
          tailAfterBlock: tail !== block,
          blockDisplay: worst(() => (block.style.display = "none"),
            () => (block.style.display = "")),
          lastChild: worst(() => tail.remove(), () => parent.append(tail)),
          dark: worst(() => root.classList.toggle("dark"),
            () => root.classList.toggle("dark")),
          stillLight: !root.classList.contains("dark"),
        };`);
      const total = frames.doneAt - sentAt;
      console.log(
        `Mesures (flux long, ${longCode.split("\n").length - 1} lignes) : ` +
          `réponse en ${Math.round(total)} ms (bloc à ${Math.round(frames.firstPre - sentAt)} ms) ; ` +
          `pire écart entre images pendant le flux (jusqu'au bouton copier du message) ` +
          `${Math.round(frames.worstDuring)} ms, ` +
          `fin de la réponse et 1 s après comprises ${Math.round(frames.worst)} ms ` +
          `(${Math.round(frames.worstAt - frames.doneAt)} ms après la fin) ; ` +
          `${frames.slow} écarts > 50 ms sur ${frames.count} images ` +
          `(les plus longs, en ms depuis l'envoi : ${frames.slowGaps
            .sort((a, b) => b[0] - a[0])
            .slice(0, 5)
            .map(([gap, at]) => `${gap} à ${Math.round(at - sentAt)}`)
            .join(
              ", ",
            )}) ; recalculs forcés (${restyle.elements} éléments) : ` +
          `bloc masqué ${restyle.blockDisplay} ms, dernier enfant ${restyle.lastChild} ms, ` +
          `classe dark ${restyle.dark} ms.`,
      );
      assert.ok(total < 20000, `flux long en ${Math.round(total)} ms`);
      assert.ok(
        frames.worstDuring < FRAME_LIMIT_MS,
        `image bloquée ${Math.round(frames.worstDuring)} ms pendant le flux`,
      );
      assert.ok(
        frames.worst < FRAME_LIMIT_MS,
        `image bloquée ${Math.round(frames.worst)} ms à la fin de la réponse ` +
          `(${Math.round(frames.worstAt - frames.doneAt)} ms après le bouton copier)`,
      );
      assert.equal(restyle.tailAfterBlock, true, "paragraphe après le bloc");
      assert.equal(restyle.stillLight, true);
      assert.ok(
        restyle.blockDisplay < RESTYLE_LIMIT_MS,
        `bloc masqué : recalcul de ${restyle.blockDisplay} ms`,
      );
      assert.ok(
        restyle.lastChild < RESTYLE_LIMIT_MS,
        `dernier enfant : recalcul de ${restyle.lastChild} ms`,
      );
      assert.ok(
        restyle.dark < THEME_LIMIT_MS,
        `classe dark : recalcul de ${restyle.dark} ms`,
      );

      // Prochain lancement : réponse Markdown d'emblée.
      await driver.invoke("update_settings", {
        patch: { selectedModel: "chatonlm-markdown" },
      });
      await collectViolations(driver);
    }),
  );

  /**
   * Premier bloc de code dans une app fraîchement lancée : envoyé après
   * `waitMs` (0 : dès l'affichage), il est observé de son arrivée à ses
   * premières couleurs. `after(driver)` : vérifications de plus, avant la
   * fermeture.
   */
  async function firstBlock(name, waitMs, after = async () => {}) {
    let result;
    await withApp(
      binary,
      { name, env: { OLLAMA_HOST: fake.url } },
      shotOnFailure(name, async (driver) => {
        await driver.waitFor("textarea[aria-label='Message']");
        await driver.execute(WATCH_PAGE);
        // Images depuis l'affichage : le chargement du surligneur et son
        // préchauffage ne doivent pas bloquer la page.
        await driver.execute(`
          const frames = (window.__startFrames = { last: performance.now(),
            from: performance.now(), worst: 0, worstAt: 0, running: true });
          const tick = (now) => {
            const gap = now - frames.last;
            frames.last = now;
            if (gap > frames.worst) { frames.worst = gap; frames.worstAt = now; }
            if (frames.running) requestAnimationFrame(tick);
          };
          requestAnimationFrame(tick);`);
        await waitUntil(
          () =>
            driver.execute(
              `return document.querySelector('[title="Select model"]')?.textContent
                 === "chatonlm-markdown"
                 && !document.querySelector("textarea[aria-label='Message']").disabled`,
            ),
          `${name} : zone de saisie prête`,
        );
        if (waitMs > 0) await delay(waitMs);
        await driver.execute(`
          const cold = (window.__cold = { firstPre: null, colored: null,
            coloredOnArrival: null });
          const watch = new MutationObserver(() => {
            const pre = document.querySelector(
              "[data-role=assistant] pre[data-theme=light]");
            if (!pre) return;
            const now = performance.now();
            const colored = pre.querySelector("span[style]") !== null;
            if (cold.firstPre === null) {
              cold.firstPre = now;
              cold.coloredOnArrival = colored;
            }
            if (colored) {
              cold.colored = now;
              watch.disconnect();
            }
          });
          watch.observe(document.body,
            { childList: true, subtree: true, characterData: true });`);
        const startFrames = await driver.execute(
          "window.__startFrames.running = false; return window.__startFrames",
        );
        const sentAt = await send(driver, "Show me some Markdown.");
        await waitReplyDone(driver);
        await waitUntil(
          () => driver.execute("return window.__cold.colored !== null"),
          `${name} : premier bloc coloré`,
        );
        result = {
          sentAt,
          startFrames,
          ...(await driver.execute(
            `return { ...window.__cold,
               paint: performance.getEntriesByType("paint")
                 .find((e) => e.name === "first-contentful-paint")?.startTime ?? null }`,
          )),
        };
        await after(driver);
        await collectViolations(driver);
      }),
    );
    return result;
  }

  /**
   * Toutes les expressions des grammaires compilent-elles dans le
   * JavaScriptCore de l'app ? Le préchauffage du surligneur est rejoué dans
   * la page, sur le module qu'elle a chargé (même adresse, donc même cache
   * d'expressions) : seules celles qu'il n'a pas encore compilées le sont,
   * et chaque échec est relevé, même déjà en cache. Puis le préchauffage de
   * l'app finit à son tour : il signalerait un échec par console.warn.
   */
  async function compilesEveryPattern(driver) {
    const chunk = embeddedChunk(binary, "highlighterCore");
    assert.ok(chunk, "morceau du surligneur introuvable dans le binaire");
    const result = await driver.executeAsync(
      `const done = arguments[arguments.length - 1];
       import(arguments[0]).then((core) => {
         const failures = [];
         const step = core.createRegexWarmUp((source, error) =>
           failures.push(String(error).slice(0, 200)));
         const started = performance.now();
         let compiled = 0;
         while (step()) compiled += 1;
         const ms = performance.now() - started;
         let total = 0;
         for (const _ of core.regexSources()) total += 1;
         done({ failures, compiled, total, ms });
       }, (error) => done({ error: String(error) }));`,
      [chunk],
    );
    assert.equal(result.error, undefined, `import de ${chunk}`);
    console.log(
      `Préchauffage du surligneur (${chunk}) : ${result.compiled} expressions ` +
        `sur ${result.total} restaient à compiler, faites en ${Math.round(result.ms)} ms.`,
    );
    assert.ok(result.total > 2000, `${result.total} expressions`);
    assert.deepEqual(result.failures, [], "expressions refusées par JSC");
    await delay(1000);
  }

  // Dès l'affichage : surligneur pas encore chargé ou en préchauffage.
  const cold = await firstBlock("markdown-cold", 0);
  // Après 4 s sans aucun bloc de code : le surligneur, chargé après
  // l'affichage de la fenêtre (et non par le secours, à 5 s), est déjà prêt ;
  // le bloc arrive coloré.
  const warm = await firstBlock("markdown-warm", 4000, compilesEveryPattern);
  const ms = (value) => Math.round(value);
  console.log(
    `Mesures (premier bloc, app fraîche) : envoyé à ${ms(cold.sentAt)} ms ` +
      `(premier dessin à ${ms(cold.paint)} ms), bloc à ${ms(cold.firstPre)} ms, ` +
      `coloré à ${ms(cold.colored)} ms (+${ms(cold.colored - cold.firstPre)} ms` +
      `${cold.coloredOnArrival ? ", dès son arrivée" : ""}) ; après 4 s : ` +
      `coloré ${warm.coloredOnArrival ? "dès son arrivée" : `+${ms(warm.colored - warm.firstPre)} ms`} ; ` +
      `pire écart entre images avant l'envoi : ${ms(cold.startFrames.worst)} ms à ` +
      `${ms(cold.startFrames.worstAt)} ms (mesuré de ${ms(cold.startFrames.from)} ms à l'envoi), ` +
      `${ms(warm.startFrames.worst)} ms à ${ms(warm.startFrames.worstAt)} ms sur les 4 s d'attente.`,
  );
  assert.ok(
    cold.colored - cold.firstPre < 5000,
    `premier bloc coloré après ${ms(cold.colored - cold.firstPre)} ms`,
  );
  assert.equal(
    warm.coloredOnArrival,
    true,
    "surligneur pas prêt 4 s après l'affichage, sans bloc de code",
  );
  // Chargement (2 Mo) et préchauffage du surligneur par tranches : la page
  // ne se fige pas.
  for (const [run, { startFrames }] of [
    ["dès l'affichage", cold],
    ["pendant 4 s", warm],
  ]) {
    assert.ok(
      startFrames.worst < START_FRAME_LIMIT_MS,
      `image bloquée ${ms(startFrames.worst)} ms au démarrage (${run})`,
    );
  }

  assert.deepEqual(violations, [], "violations de la CSP");
  assert.deepEqual(consoleErrors, [], "erreurs dans la console");
  assert.deepEqual(consoleWarnings, [], "avertissements dans la console");
} finally {
  await fake.stop();
}
