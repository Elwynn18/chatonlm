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
