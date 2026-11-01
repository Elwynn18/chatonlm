// Pilotage du banc de recette e2e/vscode-recipe.mjs, depuis l'extérieur de la
// session virtuelle (rien n'est exécuté ici : on dépose une commande et on
// attend sa réponse).
//   node e2e/lib/recipe-ctl.mjs codium '<corps async (vscode, h)>'
//   node e2e/lib/recipe-ctl.mjs app '<corps async (ctx)>'
//   node e2e/lib/recipe-ctl.mjs codium - < fichier.js
//   node e2e/lib/recipe-ctl.mjs end
// Variable : RECIPE_TIMEOUT (secondes, 600 par défaut).
import {
  existsSync,
  readFileSync,
  readdirSync,
  renameSync,
  writeFileSync,
} from "node:fs";
import { fileURLToPath } from "node:url";

const CTL = fileURLToPath(new URL("../../.tools/recipe/ctl", import.meta.url));
const [target, source] = process.argv.slice(2);
const delay = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

if (!existsSync(`${CTL}/ready`)) {
  console.error("Banc non prêt (.tools/recipe/ctl/ready absent).");
  process.exit(2);
}
if (target === "end") {
  writeFileSync(`${CTL}/end`, "");
  process.exit(0);
}
if (!["codium", "app"].includes(target) || source === undefined) {
  console.error("Usage : recipe-ctl.mjs codium|app '<code>' | - ; ou end");
  process.exit(2);
}
const body = source === "-" ? readFileSync(0, "utf8") : source;
const dir = `${CTL}/${target}`;
const n = readdirSync(dir).filter((f) => /^\d+\.js$/.test(f)).length + 1;
writeFileSync(`${dir}/${n}.js.part`, body);
renameSync(`${dir}/${n}.js.part`, `${dir}/${n}.js`);

const deadline = Date.now() + Number(process.env.RECIPE_TIMEOUT ?? 600) * 1000;
while (!existsSync(`${dir}/${n}.json`)) {
  if (existsSync(`${CTL}/exited`)) {
    console.error("Le banc s'est arrêté.");
    process.exit(3);
  }
  if (Date.now() > deadline) {
    console.error(`Pas de réponse à ${target}/${n}.js dans le délai.`);
    process.exit(4);
  }
  await delay(200);
}
const result = JSON.parse(readFileSync(`${dir}/${n}.json`, "utf8"));
const value = result.ok ? result.value : result.error;
console.log(typeof value === "string" ? value : JSON.stringify(value, null, 2));
process.exit(result.ok ? 0 : 1);
