// Essai lancé DANS VSCodium (--extensionTestsPath) par les essais
// e2e/vscode-*.test.mjs (outil e2e/lib/vscodium.mjs). Scénario choisi par
// CHATONLM_TEST_SCENARIO ; l'essai extérieur vérifie dans l'app, prend ses
// captures, puis écrit son verdict dans CHATONLM_TEST_DONE_FILE (« ok » ou la
// raison de l'échec). Les étapes atteintes ici sont écrites dans
// CHATONLM_TEST_STEP_FILE.
const fs = require("node:fs");
const vscode = require("vscode");

const delay = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

function step(name) {
  fs.writeFileSync(process.env.CHATONLM_TEST_STEP_FILE, name);
}

/** Attend la demande `name` de l'essai extérieur (fichier `<step>.<name>`). */
async function waitFor(name) {
  const file = `${process.env.CHATONLM_TEST_STEP_FILE}.${name}`;
  const deadline = Date.now() + 60_000;
  while (!fs.existsSync(file)) {
    if (Date.now() > deadline) throw new Error(`no request: ${name}`);
    await delay(200);
  }
}

async function verdict() {
  const done = process.env.CHATONLM_TEST_DONE_FILE;
  const deadline = Date.now() + 120_000;
  while (!fs.existsSync(done)) {
    if (Date.now() > deadline) throw new Error("no verdict from the app check");
    await delay(200);
  }
  const text = fs.readFileSync(done, "utf8");
  if (text !== "ok") throw new Error(text);
}

async function openSelected(file) {
  const folder = vscode.workspace.workspaceFolders?.[0]?.uri;
  if (!folder) throw new Error("no workspace folder");
  const document = await vscode.workspace.openTextDocument(
    vscode.Uri.joinPath(folder, file),
  );
  const editor = await vscode.window.showTextDocument(document);
  editor.selection = new vscode.Selection(0, 0, document.lineCount - 1, 0);
  return editor;
}

const scenarios = {
  // Appairage, « Explain » sur la sélection, panneau ouvert.
  async explain() {
    await openSelected("sample.js");
    await vscode.commands.executeCommand("chatonlm.explain");
    await vscode.commands.executeCommand("workbench.view.extension.chatonlm");
    step("explained");
  },
  // Visite guidée parlée du fichier : pause sur la première partie (capture
  // de l'essai extérieur), partie suivante, puis arrêt.
  async tour() {
    const editor = await openSelected("sample.rs");
    // Fichier entier : pas de sélection.
    editor.selection = new vscode.Selection(0, 0, 0, 0);
    await vscode.commands.executeCommand("workbench.view.extension.chatonlm");
    await vscode.commands.executeCommand("chatonlm.tour");
    step("touring");
    await waitFor("pause");
    await vscode.commands.executeCommand("chatonlm.tourTogglePause");
    step("paused");
    await waitFor("next");
    await vscode.commands.executeCommand("chatonlm.tourNext");
    step("next");
    await waitFor("stop");
    await vscode.commands.executeCommand("chatonlm.tourStop");
    step("stopped");
  },
  // Chaton en caoutchouc : séance vocale tenue par l'éditeur ; le faux micro
  // de l'app parle, l'essai extérieur vérifie la conversation et la capture.
  async duck() {
    await openSelected("sample.js");
    await vscode.commands.executeCommand("workbench.view.extension.chatonlm");
    await vscode.commands.executeCommand("chatonlm.talkItThrough");
    step("talking");
  },
  // Autocomplétion : activée et préparée, proposition demandée puis acceptée ;
  // rien depuis un fichier sensible ni après la coupure.
  async completion() {
    const folder = vscode.workspace.workspaceFolders?.[0]?.uri;
    const document = await vscode.workspace.openTextDocument(
      vscode.Uri.joinPath(folder, "completion.js"),
    );
    const editor = await vscode.window.showTextDocument(document);
    // Coupée par défaut : l'essai extérieur vérifie que rien n'est parti.
    // Ailleurs que la position essayée ensuite : l'éditeur garderait sinon
    // cette réponse vide pour la même version du texte.
    editor.selection = new vscode.Selection(0, 20, 0, 20);
    await vscode.commands.executeCommand("editor.action.inlineSuggest.trigger");
    await delay(1500);
    step("ready");
    await waitFor("enable");
    await vscode.commands.executeCommand("chatonlm.completionToggle");
    step("enabled");
    // L'essai extérieur attend que le modèle soit chargé.
    await waitFor("trigger");
    const before = document.lineAt(1).text;
    const deadline = Date.now() + 30_000;
    let shotTaken = false;
    while (document.lineAt(1).text === before) {
      if (Date.now() > deadline)
        throw new Error("no inline suggestion accepted");
      editor.selection = new vscode.Selection(1, 2, 1, 2);
      await vscode.commands.executeCommand(
        "editor.action.inlineSuggest.trigger",
      );
      await delay(1500);
      if (!shotTaken) {
        step("suggested");
        await waitFor("shot");
        shotTaken = true;
      }
      await vscode.commands.executeCommand(
        "editor.action.inlineSuggest.commit",
      );
      await delay(300);
    }
    step(`accepted:${document.lineAt(1).text.trim()}`);
    await waitFor("secret");
    const secret = await vscode.workspace.openTextDocument(
      vscode.Uri.joinPath(folder, ".env"),
    );
    const secretEditor = await vscode.window.showTextDocument(secret);
    secretEditor.selection = new vscode.Selection(0, 4, 0, 4);
    await vscode.commands.executeCommand("editor.action.inlineSuggest.trigger");
    await delay(1500);
    await vscode.commands.executeCommand("chatonlm.completionToggle");
    step("off");
  },
  // Lot 4a : fin d'une commande du terminal (notification par Easy Link),
  // correction des commentaires (vue de différences), image écrite.
  async notify() {
    const terminal = vscode.window.createTerminal({
      name: "build",
      shellPath: "/bin/bash",
      cwd: vscode.workspace.workspaceFolders[0].uri,
    });
    terminal.show();
    const deadline = Date.now() + 20_000;
    while (!terminal.shellIntegration) {
      if (Date.now() > deadline) throw new Error("no shell integration");
      await delay(200);
    }
    const ended = new Promise((resolve) => {
      const listener = vscode.window.onDidEndTerminalShellExecution((event) => {
        if (event.terminal === terminal) {
          listener.dispose();
          resolve(event.exitCode);
        }
      });
    });
    terminal.shellIntegration.executeCommand(
      "./build.sh production Hunter2Summer",
    );
    const code = await ended;
    step(`ended:${code}`);
  },
  async proofread() {
    const folder = vscode.workspace.workspaceFolders[0].uri;
    const file = vscode.Uri.joinPath(folder, "src/a.ts");
    const document = await vscode.workspace.openTextDocument(file);
    await vscode.window.showTextDocument(document);
    void vscode.commands.executeCommand("chatonlm.proofreadComments");
    const deadline = Date.now() + 60_000;
    let diff = null;
    while (!diff) {
      if (Date.now() > deadline) throw new Error("no proofreading diff");
      for (const group of vscode.window.tabGroups.all) {
        for (const tab of group.tabs) {
          if (
            tab.input instanceof vscode.TabInputTextDiff &&
            tab.input.modified.scheme === "chatonlm-proofread"
          ) {
            diff = tab.input;
          }
        }
      }
      await delay(300);
    }
    const proposed = (
      await vscode.workspace.openTextDocument(diff.modified)
    ).getText();
    const disk = fs.readFileSync(file.fsPath, "utf8");
    if (!proposed.includes('const s = "teh-server";')) {
      throw new Error(`code changed in the proposal: ${proposed}`);
    }
    if (disk !== document.getText() || document.isDirty) {
      throw new Error("the file changed without consent");
    }
    step("diff");
  },
  async image() {
    const folder = vscode.workspace.workspaceFolders[0].uri;
    const page = vscode.Uri.joinPath(folder, "site/index.html");
    await vscode.window.showTextDocument(page);
    void vscode.commands.executeCommand("chatonlm.generateImage", page, 0, 12);
    await delay(1500);
    await vscode.commands.executeCommand(
      "workbench.action.acceptSelectedQuickOpenItem",
    );
    const deadline = Date.now() + 120_000;
    while (!(await vscode.commands.executeCommand("chatonlm.saveImage"))) {
      if (Date.now() > deadline) throw new Error("image never saved");
      await delay(1000);
    }
    const written = fs.readFileSync(
      vscode.Uri.joinPath(folder, "site/img/hero.png").fsPath,
    );
    if (written.subarray(1, 4).toString() !== "PNG") {
      throw new Error("not a PNG");
    }
    step("written");
  },
  // Dictée insérée au curseur : le faux micro de l'app (WAV) parle, l'essai
  // extérieur demande l'arrêt, le texte doit arriver au curseur, sans toucher
  // au presse-papiers (rempli avant par l'essai extérieur).
  async dictate() {
    const folder = vscode.workspace.workspaceFolders[0].uri;
    const document = await vscode.workspace.openTextDocument(
      vscode.Uri.joinPath(folder, "notes.md"),
    );
    const editor = await vscode.window.showTextDocument(document);
    // Fin de la ligne 1 (« Quote: »).
    editor.selection = new vscode.Selection(0, 7, 0, 7);
    const clipboard = await vscode.env.clipboard.readText();
    await vscode.commands.executeCommand("chatonlm.dictate");
    step("listening");
    await waitFor("stop");
    await vscode.commands.executeCommand("chatonlm.dictateStop");
    const deadline = Date.now() + 90_000;
    while (!/country/i.test(document.lineAt(0).text)) {
      if (Date.now() > deadline) throw new Error("no dictated text");
      await delay(500);
    }
    if ((await vscode.env.clipboard.readText()) !== clipboard) {
      throw new Error("the clipboard changed");
    }
    step(`inserted:${document.lineAt(0).text}`);
  },
  // Serveur MCP : réglage `chatonlm.mcp.enabled` donné par l'essai extérieur
  // (réglages de l'utilisateur) ; une fois appairé, l'extension écrit les
  // justificatifs dans son stockage ; l'essai extérieur lance alors
  // vscode/dist/mcp.cjs avec un client MCP minimal.
  async mcp() {
    step("paired");
    await waitFor("off");
    // Coupure : les justificatifs disparaissent (vérifié dehors).
    await vscode.workspace
      .getConfiguration("chatonlm")
      .update("mcp.enabled", false, vscode.ConfigurationTarget.Global);
    step("off");
  },
};

exports.run = async function run() {
  const extension = vscode.extensions.getExtension("chatonlm.chatonlm");
  if (!extension) throw new Error("ChatonLM extension not found");
  await extension.activate();
  await vscode.commands.executeCommand(
    "chatonlm.pair",
    process.env.CHATONLM_TEST_PAIR_CODE,
  );
  const scenario = scenarios[process.env.CHATONLM_TEST_SCENARIO ?? "explain"];
  if (!scenario) throw new Error("unknown scenario");
  await scenario();
  await verdict();
};
