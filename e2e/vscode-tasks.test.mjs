// Travaux de l'app demandés par le lien VS Code (lot 4a, contrat
// .tools/research/93-lot4a-contract.md), dans l'app réelle (build e2e), avec
// le faux Ollama, les faux Telegram et Discord (fake-messengers), le faux
// Hugging Face et le faux moteur d'images (fake-sd-server), et le client Node
// du pont (e2e/lib/bridge.mjs) :
// - Easy Link prévient : coupé par défaut (-32035) ; la bascule de l'onglet
//   Easy Link l'active ; un message part au compte Telegram lié (à son
//   identifiant, sans message reçu avant), borné à un toutes les 30 s par
//   compte ; Discord : salon privé ouvert, puis le message ; jamais de chemin
//   complet ; `/status` montre la tâche en cours envoyée par `notify.task` ;
// - cours de l'Étude sur un texte envoyé par l'éditeur, deux de suite (le
//   second préparé par la file que le premier a lancée) : `study.event`
//   jusqu'au plan prêt pour chacun, cours visibles par la page
//   (`study_list`) ; `app.show` mène la page au cours, refuse une page hors
//   de sa liste ;
// - correction : « Auto » ne retombe jamais sur le modèle cloud du chat
//   (refus, rien ne part) ; passages en `proofread.event`, texte corrigé
//   rendu ;
// - image : aucun modèle d'images → -32036 ; modèle installé → PNG rendu en
//   base64, avancement en `image.event` ;
// - aucune violation de la CSP ni erreur dans la console.
import assert from "node:assert/strict";
import { existsSync } from "node:fs";
import { ROOT, withApp } from "./lib/app.mjs";
import { BridgeClient, readDiscovery } from "./lib/bridge.mjs";
import { startFake, startFakeHf } from "./lib/fake.mjs";
import { FAKE_TOKENS, startFakeMessenger } from "./lib/messenger.mjs";

const binary = process.argv[2];
const fake = await startFake("ollama");
const hf = await startFakeHf("hf");
const telegram = await startFakeMessenger("telegram");
const discord = await startFakeMessenger("discord");
const KLEIN = "chatonlm/klein-test-GGUF";
const FAMILIES = `${ROOT}src-tauri/crates/fake-ollama/fixtures/hf/image-families.json`;
const SD_SERVER = `${ROOT}.tools/e2e-target/debug/fake-sd-server`;
assert.ok(existsSync(SD_SERVER), `faux moteur d'images absent : ${SD_SERVER}`);
const env = {
  OLLAMA_HOST: fake.url,
  CHATONLM_TELEGRAM_URL: telegram.url,
  CHATONLM_DISCORD_URL: discord.url,
  CHATONLM_HF_URL: hf.url,
  CHATONLM_HF_STATUS_URL: hf.url,
  CHATONLM_IMAGE_FAMILIES: FAMILIES,
  CHATONLM_SD_SERVER_BIN: SD_SERVER,
};

const delay = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

async function waitUntil(check, what, timeoutMs = 20000) {
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

const WATCH_PAGE = `
  if (!window.__csp) {
    window.__csp = [];
    window.__errors = [];
    document.addEventListener("securitypolicyviolation", (event) =>
      window.__csp.push(event.violatedDirective + " " + event.blockedURI));
    const error = console.error;
    console.error = (...args) => {
      window.__errors.push(args.map(String).join(" "));
      error.apply(console, args);
    };
  }`;

const navigate = (driver, path) =>
  driver.execute(
    `const index = (history.state?.__TSR_index ?? 0) + 1;
     const key = Math.random().toString(36).slice(2);
     history.pushState({ key, __TSR_key: key, __TSR_index: index }, "", arguments[0]);`,
    [path],
  );

/** Notifications `method` reçues depuis `from` (index). */
const notificationsOf = (client, method, from = 0) =>
  client.notifications
    .slice(from)
    .filter((message) => message.method === method)
    .map((message) => message.params);

const code = (answer) => {
  assert.ok(answer.error, `pas d'erreur : ${JSON.stringify(answer)}`);
  return answer.error.code;
};

/** Relie le bot de `platform` et y lie le compte `user` par le code. */
async function linkBot(driver, platform, messenger, user, text) {
  await driver.invoke("easy_link_connect", {
    platform,
    token: FAKE_TOKENS[platform],
  });
  const index = platform === "telegram" ? 0 : 1;
  await waitUntil(
    async () =>
      (await driver.invoke("easy_link_state")).bots[index].link.state ===
      "connected",
    `${platform} connecté`,
  );
  const before = (await driver.invoke("easy_link_state")).accounts.length;
  const paired = await driver.invoke("easy_link_pair", { platform });
  await messenger.message({
    user,
    name: "Ana",
    text: text(paired.pairing.code),
  });
  await waitUntil(
    async () =>
      (await driver.invoke("easy_link_state")).accounts.length === before + 1,
    `compte ${platform} lié`,
  );
}

/** Installe le modèle d'essai de la famille klein et le choisit. */
async function installImageModel(driver) {
  const detail = await driver.invoke("image_repo", { repo: KLEIN });
  const file = detail.files.find((f) => f.recommended) ?? detail.files[0];
  const id = `${KLEIN}/${file.file}`;
  await driver.invoke("image_download", {
    repo: KLEIN,
    file: file.file,
    revision: detail.revision,
  });
  await waitUntil(
    async () => {
      const downloads = await driver.invoke("image_downloads");
      const state = downloads.find((d) => d.id === id)?.state ?? null;
      assert.notEqual(state, "failed", `téléchargement de ${id} en échec`);
      return state === "done";
    },
    "modèle d'images installé",
    60000,
  );
  await driver.invoke("image_use_model", { id });
}

try {
  await withApp(binary, { name: "vscode-tasks", env }, async (driver) => {
    await driver.waitFor("[data-testid=new-chat]");
    await driver.execute(WATCH_PAGE);
    await driver.invoke("update_settings", {
      patch: { selectedModel: "qwen3:0.6b" },
    });
    await driver.invoke("bridge_set_enabled", { enabled: true });
    const paired = await driver.invoke("bridge_pair");
    const discovery = await waitUntil(readDiscovery, "bridge.json");
    const pairing = await BridgeClient.connect(discovery.endpoint);
    await pairing.initialize();
    const { clientId, secret } = await pairing.pair(
      paired.pairing.code,
      "VS Code e2e",
    );
    await pairing.close();
    const client = await BridgeClient.connect(discovery.endpoint);
    await client.initialize();
    assert.ok((await client.auth(clientId, secret)).result);

    // --- Correction : « Auto » et le modèle cloud du chat -----------------
    // Aucun modèle en mémoire (« Auto » prendrait celui-là).
    await driver.invoke("update_settings", {
      patch: { selectedModel: "gpt-oss:20b-cloud", cloudModels: true },
    });
    const ps = await (await fetch(new URL("/api/ps", fake.url))).json();
    for (const { name } of ps.models ?? []) {
      await fetch(new URL("/api/generate", fake.url), {
        method: "POST",
        body: JSON.stringify({ model: name, keep_alive: 0 }),
      });
    }
    const elsewhere = await client.request("proofread.run", {
      text: "My notes, with a secret.",
      mode: "correct",
    });
    assert.equal(code(elsewhere), -32020);
    assert.equal(elsewhere.error.data.code, "noModel");
    assert.match(elsewhere.error.data.message, /runs outside this computer/);
    // Rien n'est parti au faux serveur (journal des /api/chat).
    const chats = await (
      await fetch(new URL("/fake/requests", fake.url))
    ).json();
    assert.ok(
      !JSON.stringify(chats).includes("a secret"),
      JSON.stringify(chats),
    );
    await driver.invoke("update_settings", {
      patch: { selectedModel: "qwen3:0.6b", cloudModels: false },
    });

    // --- Easy Link prévient ---------------------------------------------
    const done = {
      title: "npm run build",
      outcome: "success",
      durationMs: 125_000,
      workspace: "/home/ana/projets/chaton-web",
    };
    // Coupé par défaut.
    assert.equal(code(await client.request("notify.send", done)), -32035);
    // Activé, mais aucun compte lié.
    await navigate(driver, "/settings?section=easyLink");
    const toggle = "[data-testid=easy-link-vscode] [role=switch]";
    await driver.waitFor(toggle);
    assert.equal(
      await driver.execute(
        `return document.querySelector(arguments[0]).getAttribute("aria-checked")`,
        [toggle],
      ),
      "false",
    );
    await driver.execute(`document.querySelector(arguments[0]).click()`, [
      toggle,
    ]);
    await waitUntil(
      async () => (await driver.invoke("easy_link_state")).vscodeNotify,
      "réglage activé par la bascule",
    );
    assert.equal(code(await client.request("notify.send", done)), -32034);

    await linkBot(driver, "telegram", telegram, "1001", (c) => `/start ${c}`);
    const sendsTo = async (messenger) =>
      (await messenger.sent()).filter((action) => action.kind === "send");
    const before = (await sendsTo(telegram)).length;
    const sent = await client.request("notify.send", done);
    assert.deepEqual(sent.result, { sent: 1 }, JSON.stringify(sent));
    const sends = await sendsTo(telegram);
    assert.equal(sends.length, before + 1);
    const notice = sends.at(-1);
    assert.equal(notice.channel, "1001");
    assert.match(notice.text, /npm run build/);
    assert.match(notice.text, /chaton-web/);
    assert.match(notice.text, /2 min 05 s/);
    assert.ok(!notice.text.includes("/home"), notice.text);
    // Moins de 30 s après : rien ne part.
    assert.deepEqual((await client.request("notify.send", done)).result, {
      sent: 0,
    });
    assert.equal((await sendsTo(telegram)).length, before + 1);

    // Discord : salon privé ouvert, puis le message (Telegram attend).
    await linkBot(driver, "discord", discord, "3003", (c) => c);
    const failed = { ...done, outcome: "failure", workspace: undefined };
    assert.deepEqual((await client.request("notify.send", failed)).result, {
      sent: 1,
    });
    const actions = await discord.sent();
    const opened = actions.findIndex(
      (action) => action.kind === "dm" && action.recipient === "3003",
    );
    assert.ok(opened >= 0, JSON.stringify(actions));
    const discordNotice = actions
      .slice(opened)
      .find((action) => action.kind === "send");
    assert.equal(discordNotice.channel, "93003");
    assert.match(discordNotice.text, /Failed/);

    // « Où en est-on ? » : la tâche en cours dans `/status`.
    assert.deepEqual(
      (
        await client.request("notify.task", {
          id: "t1",
          title: "cargo build --release",
          state: "running",
          startedAt: Date.now() - 90_000,
        })
      ).result,
      {},
    );
    await telegram.message({ user: "1001", name: "Ana", text: "/status" });
    const status = await waitUntil(
      async () =>
        (await telegram.shown()).find((text) =>
          text.includes("VS Code tasks:"),
        ),
      "statut avec les tâches",
    );
    assert.match(status, /cargo build --release/);
    assert.match(status, /running for 1 min/);

    // --- Cours de l'Étude ---------------------------------------------
    // Modèle lent : le second cours arrive pendant que la file, lancée par
    // le premier, travaille encore.
    await driver.invoke("update_settings", {
      patch: { selectedModel: "chatonlm-study-slow" },
    });
    let from = client.notifications.length;
    const courseIds = [];
    for (const topic of ["This project", "Rust lifetimes"]) {
      const created = await client.request("study.create", {
        topic,
        level: "beginner",
        duration: 10,
        documents: [
          {
            name: "README.md",
            text: "# Chaton web\n\nA small site about photosynthesis.",
          },
        ],
      });
      assert.ok(created.result?.courseId, JSON.stringify(created));
      courseIds.push(created.result.courseId);
    }
    const [courseId] = courseIds;
    for (const id of courseIds) {
      await waitUntil(
        () =>
          notificationsOf(client, "study.event", from).some(
            (event) =>
              event.courseId === id && event.course.status === "planned",
          ),
        `plan du cours ${id} prêt`,
        60000,
      );
    }
    const courses = await driver.invoke("study_list");
    const listed = await client.request("study.list", {});
    for (const id of courseIds) {
      assert.ok(courses.some((course) => course.id === id));
      assert.ok(listed.result.some((course) => course.id === id));
    }

    // app.show : la page va au cours ; une page hors de la liste est refusée.
    const route = `/study/${courseId}`;
    assert.deepEqual((await client.request("app.show", { route })).result, {});
    await waitUntil(
      async () => (await driver.execute("return location.pathname")) === route,
      "page du cours montrée",
    );
    assert.equal(
      code(
        await client.request("app.show", {
          route: "/settings?section=general",
        }),
      ),
      -32602,
    );
    await delay(300);
    assert.equal(await driver.execute("return location.pathname"), route);

    // --- Correction ---------------------------------------------------
    // Modèle de la Correction réglé (« Auto » prendrait le modèle des cours,
    // encore en mémoire).
    await driver.invoke("update_settings", {
      patch: { proofreadModel: "chatonlm-proofread" },
    });
    from = client.notifications.length;
    const proofread = await client.request("proofread.run", {
      text: "I recieve teh letter.\n\nSecond paragraph.",
      mode: "correct",
    });
    assert.equal(
      proofread.result?.text,
      "I receive the letter.\n\nSecond paragraph.",
      JSON.stringify(proofread),
    );
    const events = notificationsOf(client, "proofread.event", from);
    assert.deepEqual(
      events.map((event) => event.event.event),
      ["started", "partDone", "partDone"],
    );

    // --- Image ----------------------------------------------------------
    const none = await client.request("image.generate", {
      prompt: "a red kitten",
    });
    assert.equal(code(none), -32036);
    assert.equal(none.error.data.reason, "noModel");
    await installImageModel(driver);
    from = client.notifications.length;
    const image = await client.request("image.generate", {
      prompt: "a red kitten on a cushion",
      width: 512,
      height: 512,
    });
    assert.ok(image.result, JSON.stringify(image).slice(0, 400));
    const png = Buffer.from(image.result.pngBase64, "base64");
    assert.deepEqual(
      [...png.subarray(0, 8)],
      [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a],
    );
    assert.equal(image.result.width, 512);
    assert.ok(image.result.model, "nom du modèle d'images");
    assert.ok(
      notificationsOf(client, "image.event", from).length > 0,
      "avancement de l'image",
    );

    await client.close();
    const csp = await driver.execute("return window.__csp");
    assert.deepEqual(csp, [], `CSP : ${csp}`);
    const errors = await driver.execute("return window.__errors");
    assert.deepEqual(errors, [], `console : ${errors}`);
  });
  console.log("vscode-tasks : OK");
} finally {
  await Promise.all([fake.stop(), hf.stop(), telegram.stop(), discord.stop()]);
}
