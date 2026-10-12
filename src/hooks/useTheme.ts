import { useLayoutEffect } from "react";
import type { Theme } from "@/bindings/Theme";

const SYSTEM_DARK = "(prefers-color-scheme: dark)";

export function isDark(theme: Theme, systemDark: boolean): boolean {
  return theme === "dark" || (theme === "system" && systemDark);
}

/**
 * Applique le thème à la page : classe « dark » (variante Tailwind) et
 * color-scheme, pour que les contrôles natifs (barres de défilement, champs)
 * suivent aussi un thème forcé.
 */
export function applyTheme(theme: Theme, root = document.documentElement) {
  const systemDark = window.matchMedia(SYSTEM_DARK).matches;
  root.classList.toggle("dark", isDark(theme, systemDark));
  root.style.colorScheme = theme === "system" ? "light dark" : theme;
}

/** Garde la page au thème choisi ; en mode système, suit ses changements. */
export function useTheme(theme: Theme | undefined) {
  useLayoutEffect(() => {
    if (!theme) return;
    applyTheme(theme);
    if (theme !== "system") return;
    const media = window.matchMedia(SYSTEM_DARK);
    const follow = () => applyTheme(theme);
    media.addEventListener("change", follow);
    return () => media.removeEventListener("change", follow);
  }, [theme]);
}
