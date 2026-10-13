// Faux Telegram ou faux Discord (crate easy-link, binaire fake-messengers)
// lancé depuis un test d'Easy Link.
import { spawn } from "node:child_process";
import { createInterface } from "node:readline";
import { ROOT } from "./app.mjs";

export const FAKE_MESSENGERS = `${ROOT}.tools/e2e-target/debug/fake-messengers`;

/** Seuls jetons acceptés par les faux serveurs. */
export const FAKE_TOKENS = {
  telegram: "424242:fake-telegram-token",
  discord: "fake.discord.token",
};

/**
 * Démarre le faux serveur de `kind` (« telegram » ou « discord ») ; `port`
 * 0 = port libre. Donner `url` à l'app par `CHATONLM_TELEGRAM_URL` ou
 * `CHATONLM_DISCORD_URL` (env de `withApp`). Renvoie { url, message, sent,
 * shown, stop }.
 */
export async function startFakeMessenger(kind, port = 0) {
  const child = spawn(FAKE_MESSENGERS, [kind, String(port)], {
    stdio: ["ignore", "pipe", "inherit"],
  });
  const exited = new Promise((resolve) => {
    child.once("exit", resolve);
    child.once("error", () => resolve(null));
  });
  const url = await new Promise((resolve, reject) => {
    const lines = createInterface({ input: child.stdout });
    lines.once("line", resolve);
    child.once("error", reject);
    child.once("exit", (code) =>
      reject(new Error(`fake-messengers s'est arrêté (code ${code})`)),
    );
  });
  // Le pilotage est à la racine de l'hôte (l'adresse du faux Discord porte
  // le chemin de son API).
  const control = (path) => new URL(path, new URL(url).origin);
  const sent = async () => (await fetch(control("/fake/sent"))).json();
  return {
    url,
    /**
     * Fait arriver un message privé : { user, name, text }, et en option
     * image, voice, private, bot, age (voir `fake/mod.rs` du crate).
     */
    async message(message) {
      const response = await fetch(control("/fake/message"), {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify(message),
      });
      if (!response.ok) {
        throw new Error(`/fake/message : ${response.status}`);
      }
    },
    /** Ce que le bot a fait, dans l'ordre : { kind, channel, id, text }. */
    sent,
    /** Textes montrés : dernier texte de chaque message du bot. */
    async shown() {
      const messages = [];
      for (const action of await sent()) {
        if (action.kind === "send") {
          messages.push({ id: action.id, text: action.text });
        } else if (action.kind === "edit") {
          const message = messages.find((known) => known.id === action.id);
          if (message) message.text = action.text;
        }
      }
      return messages.map((message) => message.text);
    },
    async stop() {
      child.kill("SIGTERM");
      await exited;
    },
  };
}
