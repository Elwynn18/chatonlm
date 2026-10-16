/*
 * SPDX-License-Identifier: MIT
 * SPDX-FileCopyrightText: Ollama
 * SPDX-FileCopyrightText: 2026 Elwynn18 and ChatonLM contributors
 *
 * Adapted from Ollama (https://github.com/ollama/ollama),
 * file app/ui/app/src/routes/__root.tsx, commit 42e911bc3d05.
 * Modified for ChatonLM: no account or cloud status; no data preloaded here
 * (chats, models and model recommendations are read by the components that
 * show them; the settings are read here for the preloading of the chosen
 * model, and the models and the state of Ollama too, only while that setting
 * is on); Ctrl+, opens the settings (Cmd+, from the
 * macOS menu in the official app); Ctrl+N (Windows only in the official app),
 * zoom, navigation guard and context menu zones (official webview.go and
 * index.html scripts) wired here; the ModelHub downloads (ChatonLM's own) are
 * followed here, once for the whole app, like the dictation status and the
 * speech model downloads; Ctrl+Q quits (closing the window only hides it in
 * the system tray), once the drafts are saved; the "navigate" event of the
 * backend (system tray menu, "Show" action of the notification of the
 * corrected selection) opens a page of the settings, of the ModelHub or the
 * Proofread page, unless a dialog or a menu is open, each request being a
 * new history state (the page shows the requested part again); the model
 * chosen in the chat is loaded into Ollama's memory from here when the
 * "Preload the chosen model" setting is on (ModelPreloader.tsx); the status
 * of the system-wide shortcuts (ChatonLM's own) is followed here, like the
 * dictation status; the corrections of a reloaded page are stopped when it
 * loads; Ctrl+Z and Ctrl+Shift+Z (or Ctrl+Y) undo and redo in a
 * text field (WebKitGTK does not bind them, the webviews of the official
 * app do); the state of the reading aloud of replies and the voice
 * downloads (ChatonLM's own) are followed here, and the reading stops when
 * another conversation is opened; so is ChatonLM's own voice mode (state,
 * levels, transcribed sentences), which ends when the page of its
 * conversation is no longer shown; while the keys of a system-wide shortcut
 * are being recorded (Windows), the app shortcuts handled here stay quiet;
 * ChatonLM's own first-launch tour (onboarding/Tour.tsx) is shown from here
 * until it is finished or skipped, the app under it being inert meanwhile;
 * ChatonLM's own Easy Link (Telegram and Discord bots) is followed here: its
 * state, and the chats the backend creates or changes for a bot, read again
 * by the history and by the page of that chat.
 */

import type { QueryClient } from "@tanstack/react-query";
import {
  createRootRouteWithContext,
  Outlet,
  useNavigate,
  useRouter,
} from "@tanstack/react-router";
import { useEffect } from "react";
import { ModelPreloader } from "@/components/ModelPreloader";
import { Tour } from "@/components/onboarding/Tour";
import { useChatsEvents } from "@/hooks/useChats";
import { useDictationEvents } from "@/hooks/useDictation";
import { useDownloadEvents } from "@/hooks/useDownloads";
import { useEasyLinkEvents } from "@/hooks/useEasyLink";
import { useImageModelEvents } from "@/hooks/useImageModels";
import { useReadAloudEvents } from "@/hooks/useReadAloud";
import { useSettings } from "@/hooks/useSettings";
import { useShortcutsEvents } from "@/hooks/useShortcuts";
import { useVoiceModeEvents } from "@/hooks/useVoiceMode";
import { navigablePath } from "@/lib/dictation";
import { listenNavigate, proofreadCancelAll } from "@/lib/ipc";
import { preventPageSelectAll } from "@/lib/keyboard";
import { allowsNativeMenu } from "@/lib/contextMenu";
import { shouldBlockKey, shouldBlockMouse } from "@/lib/navigationGuard";
import { overlayOpen } from "@/lib/overlay";
import { quit } from "@/lib/quit";
import {
  isNewChatShortcut,
  isQuitShortcut,
  isSettingsShortcut,
  recordingShortcut,
  textEditingCommand,
  zoomShortcut,
} from "@/lib/shortcuts";
import { zoomIn, zoomOut, zoomReset } from "@/lib/zoom";

const ZOOM = { zoomIn, zoomOut, zoomReset };

declare module "@tanstack/react-router" {
  interface HistoryState {
    /**
     * Numéro d'une page demandée par le moteur : la même adresse demandée
     * de nouveau est une nouvelle entrée (la page remontre ce qui est
     * demandé, settings.tsx).
     */
    navigateRequest?: number;
  }
}

/** Pages demandées par le moteur depuis le lancement. */
let navigateRequests = 0;

function RootComponent() {
  const navigate = useNavigate();
  const router = useRouter();
  // Visite guidée du premier démarrage, tant qu'elle n'est ni finie ni
  // passée : l'app dessous ne répond pas (ni clic, ni focus).
  const touring = useSettings().settings?.onboardingDone === false;
  // Téléchargements du ModelHub suivis de toute page, barre latérale fermée
  // comprise : leur fin relit le sélecteur de modèles.
  useDownloadEvents();
  // Dictée : phase, raccourci et téléchargements des modèles de transcription.
  useDictationEvents();
  // Raccourcis du système (onglet Keyboard shortcuts des réglages).
  useShortcutsEvents();
  // Lecture à voix haute : état, téléchargements des voix ; arrêt quand une
  // autre conversation s'ouvre.
  useReadAloudEvents();
  // Téléchargements des modèles d'images (onglet Images du ModelHub).
  useImageModelEvents();
  // Mode vocal : état, niveaux, phrases ; fin quand sa page n'est plus
  // affichée.
  useVoiceModeEvents();
  // Easy Link : état des bots et des comptes liés (onglet des réglages).
  useEasyLinkEvents();
  // Conversation créée ou modifiée par le moteur (message reçu par un bot) :
  // historique et conversation relus, quelle que soit la page.
  useChatsEvents();

  // Page demandée par le moteur (menu de la zone de notification, action
  // « Show » de la correction de la sélection) : les réglages, le ModelHub
  // ou la page Proofread seulement.
  useEffect(() => {
    let active = true;
    let unlisten: (() => void) | null = null;
    listenNavigate((path) => {
      const href = navigablePath(path);
      if (!href) {
        console.error("Refused navigation to", path);
        return;
      }
      // Comme Ctrl+, : une boîte ou un menu ouvert garde la main.
      if (overlayOpen()) return;
      navigateRequests += 1;
      void navigate({
        href,
        state: { navigateRequest: navigateRequests },
        // Page déjà affichée : remplacée, pas empilée dans l'historique.
        replace: router.state.location.href === href,
      });
    }).then(
      (stop) => {
        if (active) unlisten = stop;
        else stop();
      },
      (error: unknown) =>
        console.error("Failed to listen for navigation requests:", error),
    );
    return () => {
      active = false;
      unlisten?.();
    };
  }, [navigate, router]);

  // Corrections d'une page rechargée : plus rien ne lit leurs évènements,
  // elles sont arrêtées au chargement (un envoi sur un canal Tauri n'échoue
  // pas pour autant).
  useEffect(() => {
    proofreadCancelAll().catch((error: unknown) =>
      console.error("Failed to stop the previous corrections:", error),
    );
  }, []);

  useEffect(() => {
    document.addEventListener("keydown", preventPageSelectAll);
    return () => document.removeEventListener("keydown", preventPageSelectAll);
  }, []);

  // Ctrl+, : réglages ; Ctrl+N : nouvelle conversation ; Ctrl+Q : quitter ;
  // depuis toute page, aussi depuis un champ de saisie. Rechargement et
  // navigation dans l'historique bloqués (Retour arrière hors d'un champ
  // compris : la revue de la page Proofread le prend avant, en capture, pour
  // passer en édition et effacer).
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      // Saisie d'une combinaison de raccourci global : la touche lui revient.
      if (recordingShortcut()) return;
      if (shouldBlockKey(event)) {
        event.preventDefault();
        return;
      }
      // Même avec une boîte ouverte ; les brouillons sont d'abord enregistrés.
      if (isQuitShortcut(event)) {
        event.preventDefault();
        quit().catch((error: unknown) =>
          console.error("Failed to quit:", error),
        );
        return;
      }
      const settings = isSettingsShortcut(event);
      if (!settings && !isNewChatShortcut(event)) return;
      event.preventDefault();
      // Une boîte ou un menu ouvert garde la main (pas de fermeture muette).
      if (overlayOpen()) return;
      void (settings
        ? navigate({ to: "/settings" })
        : navigate({ to: "/c/$chatId", params: { chatId: "new" } }));
    };
    const onMouse = (event: MouseEvent) => {
      if (shouldBlockMouse(event)) event.preventDefault();
    };
    document.addEventListener("keydown", onKeyDown);
    for (const type of ["mousedown", "mouseup", "auxclick"] as const) {
      document.addEventListener(type, onMouse);
    }
    return () => {
      document.removeEventListener("keydown", onKeyDown);
      for (const type of ["mousedown", "mouseup", "auxclick"] as const) {
        document.removeEventListener(type, onMouse);
      }
    };
  }, [navigate]);

  // Menu contextuel de WebKit seulement dans les zones permises ; en capture
  // comme l'officiel. Les menus dessinés par l'app s'ouvrent quand même.
  useEffect(() => {
    const onContextMenu = (event: MouseEvent) => {
      if (!allowsNativeMenu(event.target)) event.preventDefault();
    };
    document.addEventListener("contextmenu", onContextMenu, true);
    return () =>
      document.removeEventListener("contextmenu", onContextMenu, true);
  }, []);

  // Annuler et rétablir dans un champ de texte (pile d'édition native) :
  // WebKitGTK ne lie pas ces touches. Écouteur de document, en phase de
  // bouillonnement : il passe après les gestionnaires React (posés sur le
  // conteneur de l'app) et les écouteurs de capture, mais AVANT les écouteurs
  // `window` des composants, qui viennent en dernier. Une touche déjà prise
  // (defaultPrevented) est laissée : le Redo de la revue annulée de la page
  // Proofread passe avant, par un écouteur `window` en capture ; son Ctrl+Z
  // de la revue vient d'un élément qui n'est pas un champ, ignoré ici.
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.defaultPrevented) return;
      const command = textEditingCommand(event);
      if (!command) return;
      event.preventDefault();
      document.execCommand(command);
    };
    document.addEventListener("keydown", onKeyDown);
    return () => document.removeEventListener("keydown", onKeyDown);
  }, []);

  // Zoom, en capture comme l'officiel : avant tout composant.
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (recordingShortcut()) return;
      const action = zoomShortcut(event);
      if (!action) return;
      event.preventDefault();
      void ZOOM[action]();
    };
    window.addEventListener("keydown", onKeyDown, true);
    return () => window.removeEventListener("keydown", onKeyDown, true);
  }, []);

  return (
    <>
      <div inert={touring}>
        {/* Option « Preload the chosen model » : au démarrage et au choix. */}
        <ModelPreloader />
        <Outlet />
      </div>
      {touring && <Tour />}
    </>
  );
}

export const Route = createRootRouteWithContext<{
  queryClient: QueryClient;
}>()({
  component: RootComponent,
});
