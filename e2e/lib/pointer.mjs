// Pointeur de KWin (e2e/lib/keyboard.py) placé sur un point de la page : la
// fenêtre est quelque part sur l'écran virtuel, avec ses décorations.
import assert from "node:assert/strict";
import { pressKeys } from "./keyboard.mjs";

/**
 * Décalage entre l'écran de la session et la page : le pointeur est placé en
 * un point connu, la page note où elle l'a vu passer. À refaire si la
 * fenêtre bouge ou si le zoom change.
 */
export async function pointerOffset(driver) {
  await driver.execute(`window.__lastPointer = null;
    addEventListener("pointermove", (e) => {
      window.__lastPointer = [e.clientX, e.clientY];
    });`);
  await pressKeys(["pointer:600,500", "wait:100", "pointer:610,505"], {
    settleMs: 200,
  });
  const seen = await driver.execute("return window.__lastPointer");
  assert.ok(seen, "la page n'a pas vu le pointeur");
  return [610 - seen[0], 505 - seen[1]];
}

/** Centre (coordonnées de la page) du premier élément `css`. */
export const centerOf = (driver, css) =>
  driver.execute(
    `const r = document.querySelector(arguments[0]).getBoundingClientRect();
     return [Math.round(r.left + r.width / 2), Math.round(r.top + r.height / 2)];`,
    [css],
  );
