// Essai lancé DANS VSCodium (--extensionTestsPath) par e2e/vscode-extension.test.mjs :
// appairage par la commande (code donné en argument), puis « Explain » sur
// une sélection ; l'essai extérieur vérifie dans l'app et écrit son verdict
// dans CHATONLM_TEST_DONE_FILE (« ok » ou la raison de l'échec).
const fs = require("node:fs");
const vscode = require("vscode");

const delay = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

exports.run = async function run() {
  const extension = vscode.extensions.getExtension("chatonlm.chatonlm");
  if (!extension) throw new Error("ChatonLM extension not found");
  await extension.activate();
  await vscode.commands.executeCommand(
    "chatonlm.pair",
    process.env.CHATONLM_TEST_PAIR_CODE,
  );
  const folder = vscode.workspace.workspaceFolders?.[0]?.uri;
  if (!folder) throw new Error("no workspace folder");
  const document = await vscode.workspace.openTextDocument(
    vscode.Uri.joinPath(folder, "sample.js"),
  );
  const editor = await vscode.window.showTextDocument(document);
  editor.selection = new vscode.Selection(0, 0, document.lineCount - 1, 0);
  await vscode.commands.executeCommand("chatonlm.explain");
  // Panneau visible pour la capture de l'essai extérieur.
  await vscode.commands.executeCommand("workbench.view.extension.chatonlm");

  const done = process.env.CHATONLM_TEST_DONE_FILE;
  const deadline = Date.now() + 90_000;
  while (!fs.existsSync(done)) {
    if (Date.now() > deadline) throw new Error("no verdict from the app check");
    await delay(200);
  }
  const verdict = fs.readFileSync(done, "utf8");
  if (verdict !== "ok") throw new Error(verdict);
};
