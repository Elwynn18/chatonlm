// Panneau de l'extension VS Code (webview) : reçoit l'état et les messages
// de l'hôte de l'extension (vscode/shared/protocol.ts), lui envoie les gestes
// de l'utilisateur. Langue et thème : ceux de VS Code, donnés par l'hôte.
import { useEffect, useMemo, useReducer, useState } from "react";
import { setLanguage, useT } from "@/i18n";
import { HostContext } from "@/lib/host";
import type { HostMessage, Theme } from "../shared/protocol";
import { Composer } from "./Composer";
import { Conversation } from "./Conversation";
import { Header } from "./Header";
import {
  initialModel,
  isBusy,
  reduce,
  savedMessages,
  visibleTurns,
} from "./model";
import { panelHost } from "./panelHost";
import { ConnectionScreen, EmptyChat, NoModelsScreen } from "./Screens";
import { TourCard } from "./TourCard";
import { VoiceBar } from "./VoiceBar";
import type { Post } from "./vscodeApi";

// Table complète par construction : un nouveau type de message de l'hôte
// oublié ici ne compile pas (il serait sinon ignoré sans bruit).
const HOST_TYPES: Record<HostMessage["type"], true> = {
  init: true,
  state: true,
  chat: true,
  event: true,
  receipt: true,
  receipts: true,
  theme: true,
  tour: true,
  voice: true,
  voiceLevel: true,
};

function isHostMessage(data: unknown): data is HostMessage {
  return (
    typeof data === "object" &&
    data !== null &&
    Object.hasOwn(HOST_TYPES, String((data as { type?: unknown }).type))
  );
}

/** Thème de ChatonLM : classe « dark » sur <html>, comme useTheme dans l'app. */
function applyTheme(theme: Theme) {
  document.documentElement.classList.toggle("dark", theme === "dark");
}

export function Panel({ post }: { post: Post }) {
  const t = useT();
  const [model, dispatch] = useReducer(reduce, undefined, initialModel);
  const host = useMemo(() => panelHost(post), [post]);
  /** Visite finie dont la carte a été fermée. */
  const [closedTour, setClosedTour] = useState(0);
  const [prefill, setPrefill] = useState<{ id: number; text: string } | null>(
    null,
  );

  useEffect(() => {
    const onMessage = (event: MessageEvent) => {
      const message: unknown = event.data;
      if (!isHostMessage(message)) return;
      if (message.type === "init") {
        setLanguage(message.locale);
        applyTheme(message.theme);
      } else if (message.type === "theme") {
        applyTheme(message.theme);
      }
      dispatch({ type: "host", message, now: Date.now() });
    };
    window.addEventListener("message", onMessage);
    // Prêt à recevoir : l'hôte répond par « init ».
    post({ type: "ready" });
    return () => window.removeEventListener("message", onMessage);
  }, [post]);

  const { state, tour, voice } = model;
  const busy = isBusy(model);
  const connected = state.connection === "connected";
  const turns = visibleTurns(model);
  const lastError = model.lastError?.error ?? null;
  const hasMessages =
    turns.length > 0 ||
    savedMessages(model, turns).length > 0 ||
    lastError !== null;

  // Avant « init », rien : la langue et le thème ne sont pas encore connus.
  if (!model.initialized) return null;

  let body;
  if (!connected) {
    body = <ConnectionScreen state={state} post={post} />;
  } else if (state.models.length === 0) {
    body = <NoModelsScreen post={post} />;
  } else if (hasMessages) {
    body = <Conversation model={model} turns={turns} lastError={lastError} />;
  } else {
    body = <EmptyChat />;
  }

  return (
    <HostContext value={host}>
      <div className="flex h-screen flex-col bg-white text-neutral-900 dark:bg-neutral-900 dark:text-white">
        <Header
          state={state}
          busy={busy}
          post={post}
          onNewChat={(ephemeral) => post({ type: "newChat", ephemeral })}
          onKeep={() => post({ type: "keep" })}
        />
        <main className="flex min-h-0 flex-1 flex-col">
          {connected && tour && tour.id !== closedTour && (
            <TourCard
              tour={tour}
              voiceOn={voice !== null}
              onCommand={(command) => post({ type: "tour", command })}
              onGoto={(index) => post({ type: "tourGoto", index })}
              onAsk={(byVoice) => {
                post({ type: "tourAsk", voice: byVoice });
                if (!byVoice) {
                  setPrefill((last) => ({
                    id: (last?.id ?? 0) + 1,
                    text: `${t("vscode.tour.prefill")} `,
                  }));
                }
              }}
              onClose={() => setClosedTour(tour.id)}
            />
          )}
          {body}
        </main>
        {connected && voice && (
          <VoiceBar
            voice={voice}
            level={model.voiceLevel}
            onMute={(muted) =>
              post({ type: "voice", command: muted ? "mute" : "unmute" })
            }
            onStop={() => post({ type: "voice", command: "stop" })}
            onResume={() => post({ type: "voice", command: "resume" })}
          />
        )}
        {connected && !voice && state.models.length > 0 && (
          <Composer
            attachment={state.attachment ?? null}
            prefill={prefill}
            onClearAttachment={() => post({ type: "clearAttachment" })}
            onTalk={() => post({ type: "voice", command: "start" })}
            busy={busy}
            canSend={state.model !== null}
            ephemeral={state.ephemeral}
            keepable={state.ephemeral && state.chatId !== null}
            notice={state.notice}
            onKeep={() => post({ type: "keep" })}
            onSend={(text) => {
              dispatch({ type: "sent", text });
              post({ type: "send", text });
            }}
            onStop={() => {
              dispatch({ type: "stopped", now: Date.now() });
              post({ type: "stop" });
            }}
            onAction={(action) => post({ type: "action", action })}
          />
        )}
      </div>
    </HostContext>
  );
}
