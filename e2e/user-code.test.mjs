// Blocs de code entre ``` dans la zone de saisie de l'app réelle (WebKitGTK),
// avec de vraies touches du clavier de la session virtuelle :
// - curseur dans un bloc encore ouvert : Entrée ajoute une ligne, rien n'est
//   envoyé ; bloc refermé : Entrée envoie ;
// - le message envoyé montre son bloc comme ceux des réponses (étiquette du
//   langage, code coloré, bouton Copier), sans les « ``` », et son code en
//   ligne ; capture .tools/shots/user-code.png ;
// - bloc jamais refermé : Ctrl+Entrée envoie quand même.
import assert from "node:assert/strict";
import { withApp } from "./lib/app.mjs";
import { fakeModel, startEngine } from "./lib/fake.mjs";
import { pressKeys } from "./lib/keyboard.mjs";

const binary = process.argv[2];
const engine = startEngine([fakeModel("simple")]);

const TEXTAREA = "textarea[aria-label='Message']";

async function waitUntil(check, what, timeoutMs = 10000) {
  const deadline = Date.now() + timeoutMs;
  for (;;) {
    const value = await check();
    if (value) return value;
    if (Date.now() > deadline) throw new Error(`Délai dépassé : ${what}`);
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
}

/** Texte de la zone de saisie posé comme une frappe, curseur à la fin, focus. */
const setDraft = (driver, value) =>
  driver.execute(
    `const field = document.querySelector(arguments[0]);
     const set = Object.getOwnPropertyDescriptor(
       HTMLTextAreaElement.prototype, "value").set;
     set.call(field, arguments[1]);
     field.dispatchEvent(new Event("input", { bubbles: true }));
     field.focus();
     field.setSelectionRange(field.value.length, field.value.length);`,
    [TEXTAREA, value],
  );

const draft = (driver) =>
  driver.execute("return document.querySelector(arguments[0]).value", [
    TEXTAREA,
  ]);

const sent = (driver) =>
  driver.execute(`return [...document.querySelectorAll("[data-role=user]")]
    .map((bubble) => ({
      text: bubble.textContent,
      blocks: [...bubble.querySelectorAll("[data-user-code]")].map((block) => ({
        header: block.querySelector("[data-code-block-header]")?.textContent ?? "",
        code: block.querySelector("pre:not([hidden]) code")?.textContent ?? "",
        colored: block.querySelectorAll("pre code span[style]").length,
      })),
      inline: [...bubble.querySelectorAll(":scope > div > code")].map(
        (code) => code.textContent),
    }))`);

await withApp(
  binary,
  { name: "user-code", env: engine.env },
  async (driver) => {
    await driver.waitFor(TEXTAREA);

    // 1. Bloc ouvert : la vraie touche Entrée ajoute une ligne.
    const opened = "Why does `total` stay at 0?\n```python\ntotal = 0";
    await setDraft(driver, opened);
    await pressKeys("enter");
    assert.equal(await draft(driver), `${opened}\n`);
    assert.deepEqual(await sent(driver), [], "rien d'envoyé dans le bloc");

    // 2. Bloc refermé : Entrée envoie.
    const message = `${opened}\nfor n in [1, 2]:\n    total + n\n\`\`\`\nThanks!`;
    await setDraft(driver, message);
    await pressKeys("enter");
    const [bubble] = await waitUntil(async () => {
      const list = await sent(driver);
      return list.length === 1 && list[0].blocks[0]?.colored > 0 && list;
    }, "message envoyé avec son bloc coloré");
    assert.equal(await draft(driver), "");
    assert.equal(bubble.blocks.length, 1);
    assert.match(bubble.blocks[0].header, /^python/);
    assert.equal(
      bubble.blocks[0].code,
      "total = 0\nfor n in [1, 2]:\n    total + n\n",
    );
    assert.deepEqual(bubble.inline, ["total"]);
    assert.ok(!bubble.text.includes("```"), bubble.text);
    assert.ok(bubble.text.includes("Thanks!"), bubble.text);
    await waitUntil(
      () =>
        driver.execute(
          "return !!document.querySelector(\"button[aria-label='Send message']\")",
        ),
      "fin de la réponse",
    );
    await driver.screenshot(".tools/shots/user-code.png");

    // 3. Bloc jamais refermé : Ctrl+Entrée envoie quand même.
    await setDraft(driver, "```\nunfinished");
    await pressKeys("ctrl+enter");
    await waitUntil(
      async () => (await sent(driver)).length === 2,
      "envoi par Ctrl+Entrée",
    );
    const second = (await sent(driver))[1];
    assert.equal(second.blocks[0]?.code, "unfinished\n");
  },
);
console.log("ok user-code");
