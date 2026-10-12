import { Lexer } from "marked";
import { afterEach, describe, expect, it, vi } from "vitest";
import longReply from "../../src-tauri/crates/fake-ollama/fixtures/markdown-long.md?raw";
import streamReply from "../../src-tauri/crates/fake-ollama/fixtures/markdown-stream.md?raw";
import fullReply from "../../src-tauri/crates/fake-ollama/fixtures/markdown.md?raw";
import { INCOMPLETE_LINK, repairIncompleteMarkdown } from "./markdownRepair";

afterEach(() => {
  vi.restoreAllMocks();
});

/** Réparation sans le découpage gardé d'un appel précédent. */
function repairAlone(text: string) {
  repairIncompleteMarkdown("\u0001");
  return repairIncompleteMarkdown(text);
}

const LINK = `(${INCOMPLETE_LINK})`;
// Un bloc fini avant : seul le dernier est réparé.
const BEFORE = "A finished paragraph with 2 * 3 = 6.\n\n";

/** Vérifie la réparation du dernier bloc, et que le bloc d'avant ne change pas. */
function expectRepair(input: string, output: string) {
  expect(repairAlone(input)).toBe(output);
  expect(repairAlone(BEFORE + input)).toBe(BEFORE + output);
}

/** Vérifie que le texte est rendu tel quel (le même objet chaîne). */
function expectUnchanged(input: string) {
  expect(repairAlone(input)).toBe(input);
  expect(repairAlone(BEFORE + input)).toBe(BEFORE + input);
}

/** Début du dernier bloc (hors lignes vides) selon marked, comme Streamdown. */
function lastBlockStart(text: string) {
  let offset = 0;
  let start = -1;
  for (const token of Lexer.lex(text, { gfm: true })) {
    if (token.type !== "space") start = offset;
    offset += token.raw.length;
  }
  return start;
}

describe("repairIncompleteMarkdown", () => {
  it("closes what the last block leaves open, like Streamdown", () => {
    expectRepair("Du **gras", "Du **gras**");
    expectRepair("Un *mot", "Un *mot*");
    expectRepair("Un _mot", "Un _mot_");
    expectRepair("Un ***mot", "Un ***mot***");
    expectRepair("Un __mot", "Un __mot__");
    expectRepair("Un ~~mot", "Un ~~mot~~");
    expectRepair("attribut _private et", "attribut _private et_");
    expectRepair("* un **gras", "* un **gras**");
    expectRepair("Utilise `npm", "Utilise `npm`");
    expectRepair("Utilise `npm i", "Utilise `npm i`");
    // Une clôture coupée en ligne : les « ` » manquants seulement.
    expectRepair("```js``", "```js```");
  });

  it("turns an incomplete link into an inert one and cuts an incomplete image", () => {
    expectRepair("Voir [doc](https://ex.com/pa", `Voir [doc]${LINK}`);
    expectRepair("Voir [doc](", `Voir [doc]${LINK}`);
    expectRepair("Voir [doc", `Voir [doc]${LINK}`);
    expectRepair("Voir [", `Voir []${LINK}`);
    // Parenthèses dans l'adresse.
    expectRepair("[w](https://en.wikipedia.org/wiki/Foo_(bar)", `[w]${LINK}`);
    // Comme Streamdown : un crochet ouvert passe pour un lien.
    expectRepair("array[0", `array[0]${LINK}`);
    expectRepair("Voir ![img](https://x", "Voir");
    expectRepair("Voir ![al", "Voir");
    expectRepair("Voir ![a [b", "Voir");
    expectUnchanged("[a](b) fin");
    expectUnchanged("- [ ] tâche");
    expectUnchanged("Voir [doc] et");
  });

  it("closes display and inline math, never single dollars", () => {
    expectRepair("Soit $$x^2", "Soit $$x^2$$");
    expectRepair("$$\n\\int_0^1", "$$\n\\int_0^1\n$$");
    expectRepair("$$\n\\int_0^1\n", "$$\n\\int_0^1\n$$");
    // Rien dans les maths ne compte : « _ », « * », « [ ».
    expectRepair("$$\na_1 * b[i", "$$\na_1 * b[i\n$$");
    expectUnchanged("$$\nx^2\n$$");
    expectUnchanged("Coût $5 et $10");
    expectUnchanged("Soit $x_1$ et y_");
    expectUnchanged("Soit \\(x^2");
    // « $$ » sans contenu : rien (Streamdown affiche « echo $$$$ »).
    expectUnchanged("echo $$");
  });

  it("leaves alone what is not an opening mark", () => {
    expectUnchanged("Du **");
    expectUnchanged("Du ** gras");
    expectUnchanged("* un\n* deux");
    expectUnchanged("la variable my_var vaut");
    expectUnchanged("méthode __init__ ok");
    expectUnchanged("fichiers *.txt et *.md");
    expectUnchanged("Utilise `npm` ici");
    expectUnchanged("Use `**kwargs` and `*args`");
    expectUnchanged("Escaped \\*star and \\[bracket and \\`tick");
    expectUnchanged("~5 min");
    expectUnchanged("texte  ");
    expectUnchanged("");
    // Ni ouvrant ni fermant pour CommonMark ; Streamdown y ajoute « * ».
    expectUnchanged("Calcul : 2 * 3 = 6");
    expectUnchanged("a * b * c");
    // Ne fait que fermer : Streamdown donne « importante* ».
    expectUnchanged("Note* importante");
    // Au milieu d'un mot : un calcul plus souvent qu'une emphase.
    expectUnchanged("5*3=15");
    // Même délimiteur ouvert deux fois : du texte.
    expectUnchanged("**a **b");
  });

  it("closes from the most recent mark to the oldest", () => {
    expectRepair("**Note:*", "**Note:**");
    expectRepair("**bold *ital", "**bold *ital***");
    expectRepair("*ital **bold", "*ital **bold***");
    expectRepair("**bold _ital", "**bold _ital_**");
    expectRepair("**bold `code", "**bold `code`**");
    expectRepair("**a $$x", "**a $$x$$**");
    expectRepair("**Step 1:** do **this", "**Step 1:** do **this**");
    expectRepair("**See [docs](http://x", `**See [docs]${LINK}**`);
    expectRepair("[**bold", `[**bold**]${LINK}`);
    expectRepair("[see `code", `[see \`code\`]${LINK}`);
    // Emphase ouverte dans le texte d'un lien fini : elle ne se ferme plus.
    expectUnchanged("[**a](https://x) b");
  });

  it("repairs the last paragraph of a list, a quote, a table or a heading", () => {
    expectRepair("- a\n- **b\n  more", "- a\n- **b\n  more**");
    expectRepair("- a\n\n  **b", "- a\n\n  **b**");
    expectRepair("> **quote\n> more", "> **quote\n> more**");
    expectRepair("> a\n>\n> - **b", "> a\n>\n> - **b**");
    expectRepair("1. a\n   - nested *x", "1. a\n   - nested *x*");
    expectRepair(
      "| a | b |\n|---|---|\n| 1 | **2",
      "| a | b |\n|---|---|\n| 1 | **2**",
    );
    expectRepair("# **Title", "# **Title**");
    // Ce qui est ouvert dans un élément fini de la liste y reste.
    expectUnchanged("- **a\n- b");
    expectUnchanged("| a | b |\n|---|---|\n| **1 | 2");
    // Titre souligné : un ajout après le soulignement le déferait.
    expectUnchanged("Title **x\n---");
  });

  it("never touches the blocks before the last one", () => {
    expectUnchanged("Du **gras\n\nSuite");
    expectUnchanged("Note* importante\n\nSuite.");
    expectRepair("Du **gras\n\nSuite *x", "Du **gras\n\nSuite *x*");
    // Avec une note, Streamdown affiche tout d'un bloc : le lien coupé du
    // premier paragraphe n'avale plus la suite.
    expectRepair(
      "Text[^1] et [inc](https://in\n\nParagraphe **x",
      "Text[^1] et [inc](https://in\n\nParagraphe **x**",
    );
    // Lignes vides à la fin : le paragraphe d'avant reste le dernier bloc.
    expectRepair("Hello **world\n\n", "Hello **world**");
  });

  it("never repairs a code block, open or closed, even in a list or a quote", () => {
    for (const code of [
      "```python\nitems = [\n    1,\n    2,",
      "```bash\necho $$",
      "```py\nx = a * b",
      "```py\nx = _private",
      "```py\nx = arr[i",
      "```md\n[a](http://x",
      "```c\nint x = a * b;",
      "```py\nx = a * b\n```",
      "```py\nx = __init\n```",
      "~~~\n**not bold",
      "    indented **code\n    more",
      "1. Step:\n\n   ```bash\n   echo $$",
      "> ```py\n> x = a * b",
      "Intro **open\n\n```py\nx = [",
    ]) {
      expectUnchanged(code);
    }
    expectUnchanged("Text\r\nwith **CR");
  });

  it("never closes a code fence typed just after a paragraph", () => {
    // marked n'en fait un bloc de code qu'au saut de ligne suivant ; d'ici
    // là, ni code en ligne `python`, ni réparation du paragraphe d'avant (un
    // bloc fini dès que la clôture sera un bloc).
    expectUnchanged("Text:\n```python");
    expectUnchanged("> quote\n> ```py\n");
    expectUnchanged("- item\n  ```py");
    expectUnchanged("Some **bold\n```python");
    expectUnchanged("See [the docs\n````");
    expectUnchanged("Some **bold\n~~~python");
    // Suivie d'autres « ` » : pas une clôture, du code en ligne.
    expectRepair("Text:\n```js``", "Text:\n```js```");
    expectRepair("Text:\n``code", "Text:\n``code``");
    expectRepair("Text ```code", "Text ```code```");
  });

  it("only changes the end of a reply, at every point of its stream", () => {
    for (const reply of [fullReply, streamReply, longReply]) {
      for (let i = 1; i <= reply.length; i++) {
        const text = reply.slice(0, i);
        const repaired = repairIncompleteMarkdown(text);
        if (repaired === text) continue;
        // Les blocs d'avant restent mot pour mot.
        const start = lastBlockStart(text);
        expect(repaired.slice(0, start)).toBe(text.slice(0, start));
        // Un suffixe, ou la coupure d'un lien ou d'une image.
        let same = 0;
        while (repaired[same] === text[same]) same++;
        const removed = text.slice(same);
        expect(
          removed.trim() === "" ||
            repaired.slice(same).startsWith(`${INCOMPLETE_LINK})`) ||
            /^\s*!\[/.test(removed),
          `${JSON.stringify(text.slice(-60))} -> ${JSON.stringify(repaired.slice(-60))}`,
        ).toBe(true);
        // Jamais dans un bloc de code.
        const last = Lexer.lex(text, { gfm: true }).findLast(
          (t) => t.type !== "space",
        );
        expect(last?.type).not.toBe("code");
      }
    }
  });

  it("follows a last block that joins the one before it", () => {
    // « # » seul est un titre vide ; « #t » prolonge le paragraphe d'avant.
    // Une liste reprend après « 2 » devenu « 2. ».
    for (const reply of ["Note **important\n#tag", "1. a\n\n2. **b"]) {
      repairAlone("");
      for (let i = 1; i <= reply.length; i++) {
        const text = reply.slice(0, i);
        const streamed = repairIncompleteMarkdown(text);
        expect(streamed, JSON.stringify(text)).toBe(repairAlone(text));
      }
    }
    expect(repairAlone("Note **important\n#tag")).toBe(
      "Note **important\n#tag**",
    );
  });

  it("does not lex the whole reply again at each fragment of a stream", () => {
    let reply = "";
    while (reply.length < 30_000) {
      reply += `${fullReply}\n\n${streamReply}\n\n${longReply}\n\n`;
    }
    reply = reply.slice(0, 30_000);
    const lex = vi.spyOn(Lexer.prototype, "blockTokens");

    let longest = 0;
    for (let i = 24; i <= reply.length; i += 24) {
      lex.mockClear();
      const repaired = repairIncompleteMarkdown(reply.slice(0, i));
      longest = Math.max(longest, ...lex.mock.calls.map(([src]) => src.length));
      if (i % 2400 === 0) {
        // Même résultat qu'en découpant tout le texte.
        expect(repaired).toBe(repairAlone(reply.slice(0, i)));
        repairIncompleteMarkdown(reply.slice(0, i));
      }
    }
    // Seuls les derniers blocs sont relus (le plus long : le bloc de code de
    // markdown-long.md et le paragraphe qui le précède).
    expect(longest).toBeLessThan(longReply.length + 500);
  });

  it("only lexes the end of a long list or table at each fragment of a stream", () => {
    const list = Array.from(
      { length: 400 },
      (_, i) =>
        `${i + 1}. **Tip ${i}**: use \`cmd-${i}\` and read [doc ${i}](https://example.com/d/${i}) for *details* about item ${i}.`,
    ).join("\n");
    const bullets = Array.from(
      { length: 150 },
      (_, i) => `- Item ${i}\n  - sub **${i}**\n    1. deep \`${i}\``,
    ).join("\n");
    const table =
      "| # | Name | Value |\n|---|---|---|\n" +
      Array.from(
        { length: 500 },
        (_, i) => `| ${i} | **n${i}** | \`v${i}\` |`,
      ).join("\n");
    const lex = vi.spyOn(Lexer.prototype, "blockTokens");
    for (const reply of [
      list,
      bullets,
      table,
      // Paragraphe après la liste : elle reste l'avant-dernier bloc.
      `${list}\n\nTo sum up, these **tips** cover most cases.`,
      `${table}\n\nAfter the **table**.`,
    ]) {
      repairAlone("");
      let longest = 0;
      const repaired: string[] = [];
      for (let i = 24; i < reply.length + 24; i += 24) {
        lex.mockClear();
        repaired.push(repairIncompleteMarkdown(reply.slice(0, i)));
        longest = Math.max(
          longest,
          ...lex.mock.calls.map(([src]) => src.length),
        );
      }
      // Le dernier élément ou la dernière ligne et le fragment, pas toute la
      // liste ou tout le tableau (plus de 20 000 caractères).
      expect(longest).toBeLessThan(400);
      // Même résultat qu'en découpant tout le texte.
      for (let i = 24, k = 0; i < reply.length + 24; i += 24 * 50, k += 50) {
        expect(repaired[k]).toBe(repairAlone(reply.slice(0, i)));
      }
    }
  });

  it("gives the same result as when lexing the whole text, at every fragment of a stream", () => {
    // Textes tirés au hasard (graine fixe) dans des morceaux qui font
    // changer le dernier bloc : listes qui s'arrêtent ou reprennent, puces
    // qui deviennent des filets, tableaux interrompus.
    const pieces = [
      "- a **b\n- c `d\n- e [f](https://x.y/z",
      "1. one *x\n2. two\n3. three **y",
      "1) a\n2) b\n10) c",
      "* star **a\n* star b\n* * *",
      "- a\n- - -",
      "- a\n- b\n--",
      "- a\n- b\n---\nafter",
      "- a\n-\n- c",
      "- a\n- \n- c",
      "1. a\n1.5 is a number\n2. b",
      "- a\n  - nested **x\n    1. deep `y\n- back",
      "- [ ] task **a\n- [x] done\n- [ ] more",
      "- item\n\n  para in item **x\n\n- next",
      "- item\n  ```py\n  x = a * b\n  ```\n- after code **z",
      "- a\nlazy continuation **x\n- b",
      "+ plus a\n+ plus b\n- dash c\n- dash d",
      "   - indented a\n   - indented b",
      "- a\n    - four spaces\n- b",
      "| a | b |\n|---|---|\n| 1 | **2 |\n| 3 | `4 |\n| 5 | [l](https://x.y) |",
      "| h1 | h2 |\n|:--|--:|\n| x | y |\n- list after table\n- more",
      "| h |\n|---|\n| r1 |\n# heading after",
      "| h |\n|---|\n| r1 |\n---\nhr after",
      "| h |\n|---|\n| r1 |\n```js\nlet a = 1;\n```",
      "| h |\n|---|\n| r1 |\n> quote after",
      "| a | b |\n|---|---|\n| 1 \\| pipe | 2 |\n|-|-|\n| 3 | 4 |",
      "a | b\n--|--\n1 | 2\n3 | 4",
      "Para **x\n- list interrupts\n- second",
      "Para\n2. not a list\n3. still",
      "> quote\n- list\n- after quote",
      "> - a\n> - b **c",
      "Title\n---",
      "Text **open\n```python",
      "    indented code\n    more",
      "Some $$x^2$$ and **bold**",
    ];
    // Reçus caractère par caractère : la dernière ligne d'une liste est une
    // puce seule (un élément vide), puis n'en est plus une (« -* » prolonge
    // l'élément d'avant, « 1.5 » aussi, « - - - » est un filet).
    const byChar = [
      "- a\n- **b1\n  b2\n-*x",
      "1. a\n2. **b1\n   b2\n1.5*x",
      "- a\n- **b\n- - -\nc *d",
      "| a |\n|---|\n| **1 |\n---\n| **2",
    ];
    let seed = 12345;
    const random = () => (seed = (seed * 1103515245 + 12345) >>> 0) / 2 ** 32;
    const replies = [...byChar, fullReply, streamReply, longReply];
    for (let k = 0; k < 300; k++) {
      let reply = "";
      for (let n = 1 + Math.floor(random() * 10); n > 0; n--) {
        if (reply) reply += random() < 0.5 ? "\n" : "\n\n";
        reply += pieces[Math.floor(random() * pieces.length)];
      }
      replies.push(reply);
    }
    for (const reply of replies) {
      const ends: number[] = [];
      for (let i = 0; i < reply.length;) {
        const step = byChar.includes(reply) ? 1 : 1 + random() * 20;
        i = Math.min(reply.length, i + Math.floor(step));
        ends.push(i);
      }
      repairAlone("");
      const streamed = ends.map((end) =>
        repairIncompleteMarkdown(reply.slice(0, end)),
      );
      ends.forEach((end, i) => {
        expect(streamed[i], JSON.stringify(reply.slice(0, end))).toBe(
          repairAlone(reply.slice(0, end)),
        );
      });
    }
  });
});
