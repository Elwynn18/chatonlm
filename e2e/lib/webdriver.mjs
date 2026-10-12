// Client W3C WebDriver minimal pour piloter l'app réelle (WebKitGTK), compilée
// avec la fonctionnalité Cargo « e2e » qui embarque tauri-plugin-webdriver.
// Le serveur du plugin écoute sur 127.0.0.1 (TAURI_WEBDRIVER_PORT, 4445).
import { writeFileSync } from "node:fs";
import { auditThemes } from "./contrast.mjs";

const ELEMENT_KEY = "element-6066-11e4-a52e-4f735466cecf";

/** Erreur W3C du plugin quand l'app n'a pas (encore) de fenêtre. */
const NO_SUCH_WINDOW = "no such window";

export class WebDriver {
  constructor(port = Number(process.env.TAURI_WEBDRIVER_PORT ?? 4445)) {
    this.base = `http://127.0.0.1:${port}`;
    this.sessionId = null;
  }

  async request(method, path, body) {
    const response = await fetch(this.base + path, {
      method,
      headers: body ? { "Content-Type": "application/json" } : undefined,
      body: body ? JSON.stringify(body) : undefined,
    });
    const data = await response.json();
    if (!response.ok || data.value?.error) {
      const error = new Error(
        `${method} ${path} : ${data.value?.error ?? response.status} ${data.value?.message ?? ""}`,
      );
      // Code d'erreur W3C, pour qui doit en reconnaître un précisément.
      error.code = data.value?.error;
      throw error;
    }
    return data.value;
  }

  session(path) {
    return `/session/${this.sessionId}${path}`;
  }

  /**
   * Attend le serveur du plugin, puis la fenêtre de l'app, et ouvre une
   * session.
   *
   * Le serveur répond dès l'initialisation du plugin, avant le `setup` de
   * l'app, qui ne crée la fenêtre qu'à la fin, après des écritures synchrones
   * (base neuve, dossiers des données). Sur un disque chargé (compilation en
   * cours), elles durent plus de 10 s ; or le plugin n'attend la fenêtre que
   * 10 s par demande de session (sondage toutes les 100 ms, puis « no such
   * window »), et c'est le seul endroit où son existence s'observe. La demande
   * est donc refaite, pour cette seule erreur, jusqu'à `windowTimeoutMs` : une
   * app qui n'ouvre pas de fenêtre fait toujours échouer le démarrage.
   */
  async start({ timeoutMs = 20000, windowTimeoutMs = 60000 } = {}) {
    const deadline = Date.now() + timeoutMs;
    for (;;) {
      try {
        await this.request("GET", "/status");
        break;
      } catch (error) {
        if (Date.now() > deadline) throw error;
        await new Promise((resolve) => setTimeout(resolve, 250));
      }
    }
    const serverUp = Date.now();
    for (;;) {
      try {
        const value = await this.request("POST", "/session", {
          capabilities: { alwaysMatch: {} },
        });
        this.sessionId = value.sessionId;
        return this;
      } catch (error) {
        if (error.code !== NO_SUCH_WINDOW) throw error;
        const waited = Date.now() - serverUp;
        const seconds = Math.round(waited / 1000);
        if (waited >= windowTimeoutMs) {
          throw new Error(
            `L'app n'a ouvert aucune fenêtre ${seconds} s après le démarrage de son serveur WebDriver (${error.message.trim()})`,
            { cause: error },
          );
        }
        console.error(
          `Pas encore de fenêtre ${seconds} s après le démarrage du serveur WebDriver de l'app : nouvelle attente`,
        );
        await new Promise((resolve) => setTimeout(resolve, 250));
      }
    }
  }

  async stop() {
    if (this.sessionId) await this.request("DELETE", this.session(""));
    this.sessionId = null;
  }

  /** Ferme la fenêtre : l'app se termine normalement, et la session avec. */
  async closeWindow() {
    const session = this.session("/window");
    this.sessionId = null;
    await this.request("DELETE", session);
  }

  async find(css) {
    const value = await this.request("POST", this.session("/element"), {
      using: "css selector",
      value: css,
    });
    return value[ELEMENT_KEY];
  }

  async findAll(css) {
    const value = await this.request("POST", this.session("/elements"), {
      using: "css selector",
      value: css,
    });
    return value.map((element) => element[ELEMENT_KEY]);
  }

  /** Attend qu'un élément existe (polling). */
  async waitFor(css, { timeoutMs = 10000 } = {}) {
    const deadline = Date.now() + timeoutMs;
    for (;;) {
      const found = await this.findAll(css);
      if (found.length > 0) return found[0];
      if (Date.now() > deadline) throw new Error(`Introuvable : ${css}`);
      await new Promise((resolve) => setTimeout(resolve, 100));
    }
  }

  click(element) {
    return this.request("POST", this.session(`/element/${element}/click`), {});
  }

  clear(element) {
    return this.request("POST", this.session(`/element/${element}/clear`), {});
  }

  type(element, text) {
    return this.request("POST", this.session(`/element/${element}/value`), {
      text,
    });
  }

  text(element) {
    return this.request("GET", this.session(`/element/${element}/text`));
  }

  execute(script, args = []) {
    return this.request("POST", this.session("/execute/sync"), {
      script,
      args,
    });
  }

  /** Script asynchrone : il reçoit en dernier argument la fonction de retour. */
  executeAsync(script, args = []) {
    return this.request("POST", this.session("/execute/async"), {
      script,
      args,
    });
  }

  /**
   * Attend que la page ait chargé l'API de Tauri (une session ouverte juste
   * après le lancement, ou après une relance, peut précéder le chargement de
   * la page) ; une fois par session.
   */
  async tauriReady({ timeoutMs = 20000 } = {}) {
    if (this.readySession === this.sessionId) return;
    const deadline = Date.now() + timeoutMs;
    for (;;) {
      const ready = await this.execute(
        "return typeof window.__TAURI_INTERNALS__?.invoke === 'function'",
      ).catch(() => false);
      if (ready) break;
      if (Date.now() > deadline) {
        throw new Error(
          `API de Tauri absente de la page après ${timeoutMs} ms`,
        );
      }
      await new Promise((resolve) => setTimeout(resolve, 100));
    }
    this.readySession = this.sessionId;
  }

  /** Appelle une commande du backend depuis la page, comme le fait le frontend. */
  async invoke(command, args = {}) {
    await this.tauriReady();
    return this.executeAsync(
      `const done = arguments[arguments.length - 1];
       window.__TAURI_INTERNALS__.invoke(arguments[0], arguments[1])
         .then((value) => done({ value }), (error) => done({ error }));`,
      [command, args],
    ).then((result) => {
      if (result && "error" in result) {
        throw new Error(`${command} : ${JSON.stringify(result.error)}`);
      }
      return result?.value;
    });
  }

  /**
   * Capture la page (rendu WebKitGTK) dans un fichier PNG. Si E2E_THEME_AUDIT
   * désigne un dossier, la page y est d'abord capturée et mesurée dans les
   * deux thèmes (e2e/lib/contrast.mjs).
   */
  async screenshot(file) {
    if (process.env.E2E_THEME_AUDIT) {
      await auditThemes(this, file, process.env.E2E_THEME_AUDIT);
    }
    const base64 = await this.request("GET", this.session("/screenshot"));
    writeFileSync(file, Buffer.from(base64, "base64"));
    return file;
  }
}
