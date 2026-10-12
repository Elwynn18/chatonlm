// Historique : conversations de démonstration (CHATONLM_SEED_DEMO) groupées en
// Today / This week / Older, de la plus récemment active à la plus ancienne ;
// ouverture d'une conversation ; menu au clic droit ; renommage sur place
// (Entrée valide sans changer l'ordre, Échap annule) ; suppression annulée
// puis confirmée, avec retour à une nouvelle conversation quand la
// conversation ouverte disparaît.
import assert from "node:assert/strict";
import { withApp } from "./lib/app.mjs";
import { startFake } from "./lib/fake.mjs";

const binary = process.argv[2];
const fake = await startFake("ollama");
const DAY = 24 * 60 * 60 * 1000;

/** Groupes attendus d'après les dates, avec les règles de l'app officielle. */
function expectedGroups(chats, now = new Date()) {
  const sameDay = (date) =>
    date.getDate() === now.getDate() &&
    date.getMonth() === now.getMonth() &&
    date.getFullYear() === now.getFullYear();
  const groups = [
    ["Today", []],
    ["This week", []],
    ["Older", []],
  ];
  for (const chat of chats) {
    const date = new Date(chat.updatedAt);
    const index = sameDay(date)
      ? 0
      : date > new Date(now.getTime() - 7 * DAY)
        ? 1
        : 2;
    groups[index][1].push(chat.title || chat.userExcerpt);
  }
  return groups.filter(([, labels]) => labels.length > 0);
}

/** Groupes affichés : titre et libellés des conversations, dans l'ordre. */
const shownGroups = (driver) =>
  driver.execute(`
    return [...document.querySelectorAll("nav h3")].map((heading) => [
      heading.textContent,
      [...heading.parentElement.querySelectorAll("[data-chat-id]")].map(
        (row) => row.textContent,
      ),
    ]);`);

const rowSelector = (id) => `[data-chat-id='${id}']`;

/** Clic droit au milieu de la ligne d'une conversation. */
const rightClick = (driver, id) =>
  driver.execute(
    `const row = document.querySelector(arguments[0]);
     const box = row.getBoundingClientRect();
     row.dispatchEvent(new MouseEvent("contextmenu", {
       bubbles: true, cancelable: true, button: 2,
       clientX: box.left + 24, clientY: box.top + box.height / 2,
     }));`,
    [rowSelector(id)],
  );

/** Entrée de menu ou bouton désigné par son texte. */
const clickText = (driver, css, text) =>
  driver.execute(
    `const target = [...document.querySelectorAll(arguments[0])]
       .find((element) => element.textContent === arguments[1]);
     if (!target) throw new Error("introuvable : " + arguments[1]);
     target.click();`,
    [css, text],
  );

const press = (driver, css, key) =>
  driver.execute(
    `document.querySelector(arguments[0]).dispatchEvent(
       new KeyboardEvent("keydown", { key: arguments[1], bubbles: true, cancelable: true }));`,
    [css, key],
  );

const exists = (driver, css) =>
  driver.execute("return document.querySelector(arguments[0]) !== null", [css]);

async function waitUntil(check, what, timeoutMs = 5000) {
  const deadline = Date.now() + timeoutMs;
  while (!(await check())) {
    if (Date.now() > deadline) throw new Error(`Délai dépassé : ${what}`);
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
}

try {
  await withApp(
    binary,
    {
      name: "history",
      env: { OLLAMA_HOST: fake.url, CHATONLM_SEED_DEMO: "1" },
    },
    async (driver) => {
      await driver.waitFor("[data-testid=new-chat]");
      await driver.click(
        await driver.find("button[aria-label='Show sidebar']"),
      );
      await driver.waitFor("[data-chat-id]");

      const chats = await driver.invoke("list_chats");
      assert.equal(chats.length, 6);
      assert.deepEqual(await shownGroups(driver), expectedGroups(chats));
      assert.deepEqual(
        (await shownGroups(driver)).map(([name]) => name),
        ["Today", "This week", "Older"],
      );
      // Capture après l'animation d'ouverture de la barre (300 ms).
      await new Promise((resolve) => setTimeout(resolve, 600));
      await driver.screenshot(".tools/shots/history-groups.png");

      // Ouvrir une conversation : ligne active, messages affichés.
      const [, trip, tcp] = chats;
      await driver.click(await driver.find(`${rowSelector(tcp.id)} a`));
      await driver.waitFor("[data-testid=chat]");
      const userText = await driver.text(
        await driver.find("[data-role=user] .whitespace-pre-wrap"),
      );
      assert.equal(userText, "Explain the difference between TCP and UDP");
      assert.equal(
        await driver.text(await driver.find("[data-role=assistant]")),
        "TCP is connection-oriented; UDP is not.",
      );
      assert.match(
        await driver.execute(
          "return document.querySelector(arguments[0]).className",
          [rowSelector(tcp.id)],
        ),
        /bg-neutral-200/,
      );
      await driver.screenshot(".tools/shots/history-chat.png");

      // Menu au clic droit : favori, Rename et Delete ; Échap le ferme.
      await rightClick(driver, trip.id);
      await driver.waitFor("[role=menu]");
      const items = await driver.execute(
        "return [...document.querySelectorAll('[role=menuitem]')].map((i) => i.textContent)",
      );
      assert.deepEqual(items, ["Add to favorites", "Rename", "Delete"]);
      await driver.screenshot(".tools/shots/history-menu.png");
      await press(driver, "[role=menu]", "Escape");
      await waitUntil(
        async () => !(await exists(driver, "[role=menu]")),
        "fermeture du menu",
      );

      // Renommer : le champ reprend le titre ; Échap annule.
      const titleInput = "input[aria-label='Chat title']";
      await rightClick(driver, trip.id);
      await driver.waitFor("[role=menu]");
      await clickText(driver, "[role=menuitem]", "Rename");
      const input = await driver.waitFor(titleInput);
      assert.equal(
        await driver.execute(
          "return document.querySelector(arguments[0]).value",
          [titleInput],
        ),
        "Trip ideas",
      );
      await driver.clear(input);
      await driver.type(input, "Not kept");
      await press(driver, titleInput, "Escape");
      await waitUntil(
        async () => !(await exists(driver, titleInput)),
        "fin du renommage",
      );
      assert.equal(
        await driver.text(await driver.find(rowSelector(trip.id))),
        "Trip ideas",
      );

      // Entrée enregistre ; l'ordre et la date d'activité ne bougent pas.
      await rightClick(driver, trip.id);
      await driver.waitFor("[role=menu]");
      await clickText(driver, "[role=menuitem]", "Rename");
      const again = await driver.waitFor(titleInput);
      await driver.clear(again);
      await driver.type(again, "  Lyon weekend  ");
      await driver.screenshot(".tools/shots/history-rename.png");
      await press(driver, titleInput, "Enter");
      await waitUntil(
        async () =>
          (await driver.text(await driver.find(rowSelector(trip.id)))) ===
          "Lyon weekend",
        "nouveau titre affiché",
      );
      const renamed = await driver.invoke("list_chats");
      assert.deepEqual(
        renamed.map((chat) => chat.id),
        chats.map((chat) => chat.id),
      );
      assert.equal(renamed[1].title, "Lyon weekend");
      assert.equal(renamed[1].updatedAt, trip.updatedAt);

      // Supprimer : Cancel garde la conversation.
      await rightClick(driver, tcp.id);
      await driver.waitFor("[role=menu]");
      await clickText(driver, "[role=menuitem]", "Delete");
      const dialog = await driver.waitFor("[role=alertdialog]");
      assert.match(
        await driver.text(dialog),
        /Are you sure you want to remove this chat\?/,
      );
      await driver.screenshot(".tools/shots/history-confirm.png");
      await clickText(driver, "[role=alertdialog] button", "Cancel");
      await waitUntil(
        async () => !(await exists(driver, "[role=alertdialog]")),
        "fermeture de la confirmation",
      );
      assert.equal((await driver.invoke("list_chats")).length, 6);

      // OK supprime la conversation ouverte : retour à une nouvelle
      // conversation, ligne retirée de l'historique.
      await rightClick(driver, tcp.id);
      await driver.waitFor("[role=menu]");
      await clickText(driver, "[role=menuitem]", "Delete");
      await driver.waitFor("[role=alertdialog]");
      await clickText(driver, "[role=alertdialog] button", "OK");
      await driver.waitFor("[data-testid=new-chat]");
      await waitUntil(
        async () => !(await exists(driver, rowSelector(tcp.id))),
        "ligne supprimée",
      );
      const remaining = await driver.invoke("list_chats");
      assert.equal(remaining.length, 5);
      assert.ok(!remaining.some((chat) => chat.id === tcp.id));
      assert.deepEqual(await shownGroups(driver), expectedGroups(remaining));
    },
  );
} finally {
  await fake.stop();
}
