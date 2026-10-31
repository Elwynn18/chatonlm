// Captures du README de l'extension VS Code (vscode/media/screenshots/) :
// l'app réelle et VSCodium (e2e/lib/vscodium.mjs) dans la session virtuelle,
// avec l'instance Ollama de test (scripts/test-ollama.sh, 127.0.0.1:11435) et
// de vrais modèles : qwen3:8b pour la conversation et la visite guidée,
// qwen2.5-coder:0.5b-base pour l'autocomplétion ; voix de lecture de
// .tools/voice-models. Ce n'est pas un essai : check.sh ne le lance pas, et
// seule la forme des écrans est attendue.
//   scripts/test-ollama.sh start
//   E2E_TIMEOUT=1500 scripts/e2e-session.sh e2e/run.sh \
//     e2e/vscode-readme-screenshots.mjs "$(scripts/e2e-build.sh)"
// Captures brutes (PNG 1360 × 960) dans .tools/shots/ (readme-vscode-*),
// relues à l'œil : la meilleure de la rafale readme-vscode-chat-NN (question
// et début de la réponse visibles), readme-vscode-tour et la dernière
// readme-vscode-completion-N, rognées sous la barre de titre (VS Code y écrit
// « [Extension Development Host] » en mode essai) : y de 66 à 926 ; la page
// Settings › VS Code de l'app (.tools/shots/readme-vscode/app-settings.png),
// rognée à 530 px de haut. Sorties : vscode/media/screenshots/{chat,tour,
// completion,pairing}.png (hors du .vsix, lues en ligne par les magasins).
// Prérequis : builds de l'extension (vscode/dist).
import assert from "node:assert/strict";
import { mkdirSync } from "node:fs";
import { ROOT, withApp } from "./lib/app.mjs";
import { findDiscovery } from "./lib/bridge.mjs";
import { linkVoice } from "./lib/voice.mjs";
import { chooseModel, prepare, shot, waitUntil } from "./lib/vscodium.mjs";

const binary = process.argv[2];
const OLLAMA = "http://127.0.0.1:11435";
const MODEL = "qwen3:8b";
const COMPLETION = "qwen2.5-coder:0.5b-base";
const OUT = `${ROOT}.tools/shots/readme-vscode`;
const DATA = process.env.CHATONLM_DATA_DIR;
assert.ok(
  DATA,
  "CHATONLM_DATA_DIR absent : lancer dans scripts/e2e-session.sh",
);
mkdirSync(OUT, { recursive: true });
const delay = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

const ollama = (path) =>
  fetch(`${OLLAMA}${path}`)
    .then((response) => response.json())
    .catch(() => null);
const tags = await ollama("/api/tags");
for (const model of [MODEL, COMPLETION]) {
  if (!tags?.models.some((m) => m.name === model)) {
    throw new Error(
      `${model} absent ou instance arrêtée : scripts/test-ollama.sh start`,
    );
  }
}
for (const voice of ["pocket-tts-en", "pocket-tts-fr", "pocket-tts-es"]) {
  linkVoice(DATA, voice);
}

const codium = prepare("vscode-readme", {
  "src/slugify.ts": `/** Turns a title into a URL-friendly slug. */
export function slugify(title: string, maxLength = 60): string {
  const slug = title
    .normalize("NFKD")
    .replace(/[\\u0300-\\u036f]/g, "")
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "");
  if (slug.length <= maxLength) return slug;
  const cut = slug.slice(0, maxLength);
  const lastDash = cut.lastIndexOf("-");
  return lastDash > 0 ? cut.slice(0, lastDash) : cut;
}
`,
  "src/retry.ts": `/** Runs a task again after a failure, waiting longer each time. */
export async function retry<T>(
  task: () => Promise<T>,
  attempts = 3,
  delayMs = 500,
): Promise<T> {
  let lastError: unknown;
  for (let attempt = 1; attempt <= attempts; attempt++) {
    try {
      return await task();
    } catch (error) {
      lastError = error;
      if (attempt < attempts) {
        const wait = delayMs * 2 ** (attempt - 1);
        await new Promise((resolve) => setTimeout(resolve, wait));
      }
    }
  }
  throw lastError;
}
`,
  "src/cart.ts": `interface CartItem {
  name: string;
  price: number;
  quantity: number;
}

export function cartTotal(items: CartItem[]): number {

}
`,
  "package.json": `{\n  "name": "shop",\n  "private": true\n}\n`,
});

/** Réponse du modèle finie : texte non vide, inchangé pendant 5 s. */
async function replyDone(driver, chatId) {
  let last = null;
  let since = 0;
  await waitUntil(
    async () => {
      const chat = await driver.invoke("get_chat", { id: chatId });
      const text = chat.messages.at(-1)?.content ?? "";
      if (chat.messages.length < 2 || !text || text !== last) {
        last = text;
        since = Date.now();
        return false;
      }
      return Date.now() - since > 5000;
    },
    "fin de la réponse",
    300_000,
  );
}

try {
  await withApp(
    binary,
    { name: "vscode-readme", env: { OLLAMA_HOST: OLLAMA } },
    async (driver) => {
      await driver.waitFor("[data-testid=new-chat]");
      await chooseModel(driver, MODEL);
      // Modèle chargé d'avance (lecture lente du disque de .tools).
      await fetch(`${OLLAMA}/api/generate`, {
        method: "POST",
        body: JSON.stringify({ model: MODEL, keep_alive: "30m" }),
      }).then((response) => response.text());
      await driver.invoke("bridge_set_enabled", { enabled: true });
      const { pairing } = await driver.invoke("bridge_pair");
      const found = await waitUntil(findDiscovery, "bridge.json");
      const exited = codium.start({
        bridgeDir: found.dir,
        scenario: "readme",
        settings: {
          "chatonlm.completion.model": COMPLETION,
          "git.enabled": false,
          "git.openRepositoryInParentFolders": "never",
          "workbench.startupEditor": "none",
          "workbench.tips.enabled": false,
          "editor.minimap.enabled": false,
          // Corps de fonction vide avant la proposition : pas d'erreur
          // soulignée sur la capture.
          "typescript.validate.enable": false,
        },
        env: { CHATONLM_TEST_PAIR_CODE: pairing.code },
      });

      let verdict = "ok";
      try {
        // 1. « Explain » : vraie réponse dans le panneau.
        await waitUntil(
          () => codium.step() === "explained",
          "Explain lancé",
          120_000,
        );
        // Rafale de captures pendant la réponse (une par seconde environ) :
        // le panneau suit la fin du texte, la question et le début de la
        // réponse ne se voient qu'au début. La meilleure est choisie à l'œil.
        for (let i = 0; i < 25; i += 1) {
          await shot(`readme-vscode-chat-${String(i).padStart(2, "0")}`);
        }
        const chat = await waitUntil(async () => {
          const chats = await driver.invoke("list_chats");
          return chats.find((c) => c.source === "vscode");
        }, "conversation marquée VS Code");
        await replyDone(driver, chat.id);
        await delay(2000);
        await shot("readme-vscode-chat-end");

        // 2. Visite guidée : plan du vrai modèle, pause sur la 2e partie.
        codium.ask("tour");
        await waitUntil(() => codium.step() === "touring", "visite lancée");
        await waitUntil(
          async () =>
            (await driver.invoke("read_aloud_state"))?.phase === "playing",
          "lecture en cours dans l'app",
          300_000,
        );
        await delay(2500);
        codium.ask("pause");
        await waitUntil(() => codium.step() === "paused", "visite en pause");
        await delay(2000);
        await shot("readme-vscode-tour");
        codium.ask("stop");

        // 3. Autocomplétion : modèle chargé, proposition en gris.
        await waitUntil(
          () => codium.step() === "completion-on",
          "autocomplétion activée",
        );
        await waitUntil(
          async () =>
            (await ollama("/api/ps"))?.models?.some(
              (m) => m.name === COMPLETION,
            ),
          "modèle d'autocomplétion chargé",
          120_000,
        );
        codium.ask("trigger");
        for (let attempt = 0; ; attempt += 1) {
          const reached = await waitUntil(
            () => {
              const now = codium.step();
              return (
                (now === `suggested:${attempt}` || now === "accepted") && now
              );
            },
            "proposition",
            120_000,
          );
          if (reached === "accepted") break;
          await shot(`readme-vscode-completion-${attempt}`);
          codium.ask(`shot${attempt}`);
        }

        // 4. L'app : Settings › VS Code, éditeur appairé et connecté.
        await driver.execute(
          `document.querySelector("button[aria-label='Show sidebar']")?.click()`,
        );
        await waitUntil(
          () =>
            driver.execute(
              `const link = [...document.querySelectorAll("nav a")]
                 .find((a) => a.textContent.trim() === "Settings");
               link?.click();
               return link !== undefined;`,
            ),
          "Settings",
        );
        await driver.click(
          await driver.waitFor("[data-testid=settings-tab-vscode]"),
        );
        await driver.waitFor("[data-testid=vscode-settings]");
        await delay(1500);
        await driver.execute("document.activeElement?.blur()");
        await driver.screenshot(`${OUT}/app-settings.png`);
        codium.ask("end");
      } catch (error) {
        verdict = String(error?.stack ?? error);
        await shot("readme-vscode-failure").catch(() => {});
      }
      codium.done(verdict);
      const code = await exited();
      assert.equal(verdict, "ok");
      assert.equal(code, 0, "VSCodium exited with an error (see its log)");
    },
  );
  console.log("✓ vscode-readme-screenshots");
} finally {
  codium.finish();
}
