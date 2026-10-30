// Point d'entrée de l'extension ChatonLM pour VS Code : liaison avec l'app
// par le pont local, panneau, commandes et actions. Contrats :
// .tools/research/87-lot1b-contract.md (B), 88-fix-contract.md. L'app n'est
// jamais lancée au démarrage de VS Code, seulement sur un geste de
// l'utilisateur (bouton du panneau, commande).
import { watch as watchDir } from "node:fs";
import net from "node:net";
import os from "node:os";
import type { Duplex } from "node:stream";
import * as vscode from "vscode";
import type { QuickAction } from "../shared/protocol";
import { FixProvider } from "./actions";
import {
  bridgeDir,
  currentPlatform,
  DISCOVERY,
  readDiscovery,
} from "./bridge/discovery";
import {
  appRunning,
  awaitBridge,
  findApp,
  launch,
  systemProbe,
} from "./bridge/launcher";
import { type Credentials, Link, PairError } from "./bridge/link";
import { Controller } from "./controller";
import { ChatPanel, panelLocale, panelTheme, VIEW_ID } from "./panel";
import { TerminalWatcher } from "./terminal";

const SECRET_KEY = "chatonlm.client";
const WEBSITE = "https://chatonlm.com";

let disposeAll: (() => Promise<void>) | null = null;

function connectSocket(endpoint: string): Promise<Duplex> {
  return new Promise((resolve, reject) => {
    const socket = net.connect({ path: endpoint });
    socket.once("error", reject);
    socket.once("connect", () => {
      socket.off("error", reject);
      resolve(socket);
    });
  });
}

function parseCredentials(value: string | undefined): Credentials | null {
  if (!value) return null;
  try {
    const parsed = JSON.parse(value) as Partial<Credentials>;
    return typeof parsed.clientId === "string" &&
      typeof parsed.secret === "string"
      ? { clientId: parsed.clientId, secret: parsed.secret }
      : null;
  } catch {
    return null;
  }
}

/** Message d'un appairage refusé, traduit. */
export function pairErrorText(error: PairError): string {
  switch (error.reason) {
    case "appClosed":
      return vscode.l10n.t("ChatonLM is not open. Open it, then try again.");
    case "bridgeOff":
      return vscode.l10n.t(
        "The link with VS Code is off. Turn it on in ChatonLM, under Settings › VS Code.",
      );
    case "incompatible":
      return vscode.l10n.t(
        "This version of ChatonLM does not match this extension. Update both.",
      );
    case "noPairing":
      return vscode.l10n.t(
        "ChatonLM is not waiting for an editor. In Settings › VS Code, click Pair an editor, then type the new code.",
      );
    case "wrongCode":
      return vscode.l10n.t("This code is not the one shown in ChatonLM.");
    case "tooManyAttempts":
      return vscode.l10n.t(
        "Too many wrong codes: this code is cancelled. Ask ChatonLM for a new one.",
      );
    default:
      return vscode.l10n.t("Pairing failed. Try again.");
  }
}

/**
 * Réglage caché `chatonlm.bridgeDir` (essais) : lu seulement dans les réglages
 * de l'utilisateur, jamais dans ceux d'un dossier ouvert (un dépôt ne peut pas
 * rediriger l'extension vers un faux pont).
 */
export function bridgeDirSetting(
  configuration: Pick<vscode.WorkspaceConfiguration, "inspect">,
): string | undefined {
  const value = configuration.inspect<unknown>("bridgeDir")?.globalValue;
  return typeof value === "string" ? value : undefined;
}

export function activate(context: vscode.ExtensionContext): void {
  const platform = currentPlatform();
  const dir = () =>
    bridgeDir(
      platform,
      process.env,
      bridgeDirSetting(vscode.workspace.getConfiguration("chatonlm")),
    );

  // Secret gardé par VS Code (trousseau du système) ; s'il ne peut pas
  // l'écrire, il reste en mémoire pour cette session, jamais en clair.
  let fallbackSecret: Credentials | null = null;
  const secrets = {
    async get() {
      try {
        return (
          parseCredentials(await context.secrets.get(SECRET_KEY)) ??
          fallbackSecret
        );
      } catch {
        return fallbackSecret;
      }
    },
    async set(credentials: Credentials) {
      fallbackSecret = credentials;
      try {
        await context.secrets.store(SECRET_KEY, JSON.stringify(credentials));
        fallbackSecret = null;
      } catch {
        // Pas de trousseau : appairage à refaire au prochain démarrage.
      }
    },
    async delete() {
      fallbackSecret = null;
      try {
        await context.secrets.delete(SECRET_KEY);
      } catch {
        // Rien de gardé.
      }
    },
  };

  // Sortie d'une commande lisible seulement si on la suit depuis son début
  // (chaton en caoutchouc) ; suivie seulement une fois l'éditeur appairé.
  const terminal = new TerminalWatcher();
  // Appairé (secret gardé) : « Fix with ChatonLM » n'est proposé qu'alors.
  let paired = false;
  const setPaired = (value: boolean) => {
    paired = value;
    terminal.watch(value);
  };
  const readPaired = async () => {
    setPaired((await secrets.get()) !== null);
  };
  void readPaired();

  const version = String(context.extension.packageJSON.version ?? "0");
  const link = new Link({
    dir,
    readDiscovery: (path) => readDiscovery(path, platform),
    connect: connectSocket,
    secrets,
    client: {
      name: "chatonlm-vscode",
      version,
      editor: `${vscode.env.appName} ${vscode.version}`,
    },
    watch: (path, changed) => {
      try {
        const watcher = watchDir(path, (_event, name) => {
          if (!name || name === DISCOVERY) changed();
        });
        watcher.on("error", () => watcher.close());
        return watcher;
      } catch {
        return null;
      }
    },
    appRunning: () => appRunning(platform),
  });

  const locale = () => panelLocale(vscode.env.language);
  const theme = () => panelTheme(vscode.window.activeColorTheme.kind);

  // Programme trouvé, lancé, puis attente de son pont (30 s) ; si l'app
  // tournait déjà sans pont (lien coupé dans ses réglages), rien à attendre :
  // la liaison le dit tout de suite.
  const launchApp = async (background: boolean) => {
    const path = dir();
    const wasRunning =
      path !== null &&
      (await readDiscovery(path, platform)) === null &&
      (await appRunning(platform)) === true;
    const exe = await findApp(platform, process.env, os.homedir(), systemProbe);
    if (!exe) {
      const download = vscode.l10n.t("Download");
      const choice = await vscode.window.showErrorMessage(
        vscode.l10n.t("ChatonLM is not installed on this computer."),
        download,
      );
      if (choice === download) {
        await vscode.env.openExternal(vscode.Uri.parse(WEBSITE));
      }
      return;
    }
    try {
      await launch(exe, background, process.env);
    } catch {
      void vscode.window.showErrorMessage(
        vscode.l10n.t("ChatonLM could not be started."),
      );
      return;
    }
    if (link.connected) return;
    // Le fichier de découverte suffit : la liaison s'y connecte ensuite (ou
    // dit aussitôt « pont coupé »).
    await awaitBridge(wasRunning, async () => {
      const path = dir();
      return path !== null && (await readDiscovery(path, platform)) !== null;
    });
    await link.retryNow();
  };

  // Code donné en argument de la commande (raccourci, essai), sinon demandé.
  const pair = async (preset?: unknown) => {
    const given =
      typeof preset === "string" && /^\s*\d{6}\s*$/.test(preset)
        ? preset
        : undefined;
    const code =
      given ??
      (await vscode.window.showInputBox({
        title: vscode.l10n.t("Pair with ChatonLM"),
        prompt: vscode.l10n.t(
          "In ChatonLM, open Settings › VS Code, click Pair an editor, then type the code shown.",
        ),
        placeHolder: "123456",
        ignoreFocusOut: true,
        validateInput: (value) =>
          /^\s*\d{6}\s*$/.test(value)
            ? null
            : vscode.l10n.t("The code has 6 digits."),
      }));
    if (!code) return;
    try {
      await link.pair(code.trim(), vscode.env.appName);
      setPaired(true);
      void vscode.window.showInformationMessage(
        vscode.l10n.t("VS Code is paired with ChatonLM."),
      );
    } catch (error) {
      void vscode.window.showErrorMessage(
        error instanceof PairError
          ? pairErrorText(error)
          : vscode.l10n.t("Pairing failed. Try again."),
      );
    }
  };

  let controller: Controller | null = null;
  const panel = new ChatPanel({
    extensionUri: context.extensionUri,
    onMessage: (message) => void controller?.handle(message),
    // Retour au panneau : modèles relus (téléchargés dans l'app entre-temps).
    onVisible: () => {
      if (link.connected) void controller?.refresh();
    },
    locale,
    theme,
  });
  // Sortie « ChatonLM » : erreurs du panneau (diagnostic), jamais de contenu
  // des conversations.
  const output = vscode.window.createOutputChannel("ChatonLM", { log: true });
  context.subscriptions.push(output, terminal);
  controller = new Controller({
    terminal,
    log: (message) => output.error(message),
    link,
    memento: context.globalState,
    post: (message) => panel.post(message),
    reveal: () => panel.reveal(),
    launchApp,
    pair,
    locale,
    theme,
  });
  const ctl = controller;

  const action = (name: QuickAction) =>
    vscode.commands.registerCommand(`chatonlm.${name}`, (...args: unknown[]) =>
      ctl.runAction(name, args),
    );

  context.subscriptions.push(
    vscode.window.registerWebviewViewProvider(VIEW_ID, panel, {
      webviewOptions: { retainContextWhenHidden: true },
    }),
    vscode.commands.registerCommand("chatonlm.pair", pair),
    vscode.commands.registerCommand("chatonlm.unpair", async () => {
      // La conversation éphémère de cet éditeur part avec lui.
      await ctl.forgetEphemeral();
      await link.unpair();
      setPaired(false);
      void vscode.window.showInformationMessage(
        vscode.l10n.t(
          "This editor is no longer paired. You can also remove it in ChatonLM, under Settings › VS Code.",
        ),
      );
    }),
    vscode.commands.registerCommand("chatonlm.openApp", () =>
      ctl.launch(false),
    ),
    vscode.commands.registerCommand("chatonlm.newChat", async () => {
      ctl.newChat(false);
      await panel.reveal();
    }),
    vscode.commands.registerCommand("chatonlm.newEphemeralChat", async () => {
      ctl.newChat(true);
      await panel.reveal();
    }),
    action("explain"),
    action("fix"),
    action("tests"),
    action("commitMessage"),
    // Visite guidée parlée (lot 2) et chaton en caoutchouc.
    vscode.commands.registerCommand("chatonlm.tour", () =>
      ctl.tour.start(false),
    ),
    vscode.commands.registerCommand(
      "chatonlm.tourChanges",
      (...args: unknown[]) => ctl.tour.start(true, args),
    ),
    vscode.commands.registerCommand("chatonlm.tourTogglePause", () =>
      ctl.tourCommand("toggle"),
    ),
    vscode.commands.registerCommand("chatonlm.tourPrevious", () =>
      ctl.tourCommand("previous"),
    ),
    vscode.commands.registerCommand("chatonlm.tourNext", () =>
      ctl.tourCommand("next"),
    ),
    vscode.commands.registerCommand("chatonlm.tourStop", () =>
      ctl.tourCommand("stop"),
    ),
    vscode.commands.registerCommand("chatonlm.talkItThrough", async () => {
      await panel.reveal();
      await ctl.startVoice("duck");
    }),
    // Micro tenu par VS Code : arrêtable depuis la palette (panneau replié).
    vscode.commands.registerCommand("chatonlm.stopTalking", () =>
      ctl.voice.stop(),
    ),
    vscode.languages.registerCodeActionsProvider(
      "*",
      new FixProvider(() => paired),
      { providedCodeActionKinds: FixProvider.kinds },
    ),
    vscode.window.onDidChangeActiveColorTheme(() =>
      panel.post({ type: "theme", theme: theme() }),
    ),
    // Appairage fait ou retiré dans une autre fenêtre.
    context.secrets.onDidChange((event) => {
      if (event.key !== SECRET_KEY) return;
      void readPaired();
      void link.secretsChanged();
    }),
    // Retour dans la fenêtre : liaison et modèles relus (app ouverte ou
    // modèle téléchargé entre-temps).
    vscode.window.onDidChangeWindowState((state) => {
      if (!state.focused) return;
      if (link.connected) void ctl.refresh();
      else void link.retryNow();
    }),
    {
      dispose: link.onState((state) => {
        if (state.connection === "connected") setPaired(true);
        else if (state.connection === "notPaired") setPaired(false);
      }),
    },
  );

  disposeAll = async () => {
    // Conversation éphémère effacée en quittant (comme en quittant l'app).
    await ctl.forgetEphemeral();
    ctl.dispose();
    link.dispose();
  };

  void link.start();
}

export async function deactivate(): Promise<void> {
  const dispose = disposeAll;
  disposeAll = null;
  await dispose?.();
}
