// Scripts KWin exécutés dans la session virtuelle (redimensionner, fermer une
// fenêtre...), là où WebDriver ne sait pas agir. Refusé hors de la session :
// un test ne doit jamais piloter le KWin du vrai bureau.
import { execFileSync } from "node:child_process";
import { mkdirSync, rmSync, writeFileSync } from "node:fs";
import path from "node:path";

let counter = 0;

function gdbus(objectPath, method, ...args) {
  return execFileSync(
    "gdbus",
    [
      "call",
      "--session",
      "--dest",
      "org.kde.KWin",
      "--object-path",
      objectPath,
      "--method",
      method,
      ...args,
    ],
    { encoding: "utf8" },
  );
}

/** Exécute un script KWin (JavaScript de KWin 6) et attend `settleMs`. */
export async function runKWinScript(source, { settleMs = 500 } = {}) {
  if (process.env.CHATONLM_E2E_SESSION !== "1") {
    throw new Error("Script KWin refusé hors de la session virtuelle.");
  }
  const dir = path.resolve(".tools/tmp");
  mkdirSync(dir, { recursive: true });
  const name = `chatonlm-e2e-${process.pid}-${counter++}`;
  const file = path.join(dir, `${name}.js`);
  writeFileSync(file, source);
  try {
    const loaded = gdbus(
      "/Scripting",
      "org.kde.kwin.Scripting.loadScript",
      file,
      name,
    );
    // gdbus affiche « (0,) » ou, selon les options, « (int32 0,) ».
    const id = /\((?:int32 )?(-?\d+),\)/.exec(loaded.trim())?.[1];
    if (id === undefined || id === "-1") {
      throw new Error(`KWin a refusé le script : ${loaded.trim()}`);
    }
    gdbus(`/Scripting/Script${id}`, "org.kde.kwin.Script.run");
    await new Promise((resolve) => setTimeout(resolve, settleMs));
    gdbus("/Scripting", "org.kde.kwin.Scripting.unloadScript", name);
  } finally {
    rmSync(file, { force: true });
  }
}

/**
 * Titres des fenêtres de la session qui contiennent `query`, d'après le
 * service de recherche de fenêtres de KWin (/WindowsRunner, interface
 * org.kde.krunner1 : un script KWin ne peut rien renvoyer).
 */
export function windowCaptions(query) {
  if (process.env.CHATONLM_E2E_SESSION !== "1") {
    throw new Error(
      "Recherche de fenêtres refusée hors de la session virtuelle.",
    );
  }
  const reply = gdbus("/WindowsRunner", "org.kde.krunner1.Match", query);
  // ([('0_{uuid}', 'Select Files', 'wayland', 100, 0.8, {...})],) : titre en
  // deuxième position, entre apostrophes (ou guillemets s'il en contient).
  const captions = [];
  const match =
    /\('(?:[^'\\]|\\.)*', (?:'((?:[^'\\]|\\.)*)'|"((?:[^"\\]|\\.)*)")/g;
  for (const found of reply.matchAll(match)) {
    captions.push((found[1] ?? found[2]).replace(/\\(.)/g, "$1"));
  }
  return captions;
}

/** Script qui agit sur les fenêtres dont le titre vaut `caption`. */
export function forWindows(caption, body) {
  return `for (const w of workspace.windowList()) {
  if (w.caption === ${JSON.stringify(caption)}) { ${body} }
}`;
}
