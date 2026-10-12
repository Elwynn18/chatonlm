import { isTauri } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";

/** Affiche la fenêtre principale, créée masquée par le backend. */
export async function showMainWindow(): Promise<void> {
  if (!isTauri()) return;
  await getCurrentWindow().show();
}
