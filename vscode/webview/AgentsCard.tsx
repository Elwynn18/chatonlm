// Carte des appels des agents (lot 4b) : chaque appel d'un agent (Copilot,
// Cline…) par le serveur MCP de ChatonLM, avec l'outil, le début de la
// demande et où il en est ; « Stop » pour un appel en cours, « Turn off for
// agents » coupe le serveur pour tous (réglage `chatonlm.mcp.enabled`).
import { XMarkIcon } from "@heroicons/react/20/solid";
import { useT } from "@/i18n";
import type { AgentCall, AgentsView } from "../shared/protocol";
import {
  agentAnnounce,
  agentCallState,
  agentCallTitle,
  agentToolLabel,
} from "./labels";

const PILL =
  "cursor-pointer rounded-full border border-neutral-300 px-3 py-1 text-xs text-neutral-700 hover:bg-neutral-100 dark:border-neutral-600 dark:text-neutral-200 dark:hover:bg-neutral-800";

/** Appels montrés (les plus récents). */
export const SHOWN_CALLS = 5;

function CallRow({ call, onStop }: { call: AgentCall; onStop: () => void }) {
  const t = useT();
  return (
    <li
      data-testid="agent-call"
      data-state={call.state}
      className="flex items-center gap-2 py-1"
    >
      <div className="min-w-0 flex-1">
        <p className="truncate text-neutral-900 dark:text-white">
          {agentCallTitle(t, call)}
        </p>
        {call.detail && (
          <p className="truncate text-xs text-neutral-600 dark:text-neutral-300">
            {call.detail}
          </p>
        )}
        <p
          className={
            call.state === "failed"
              ? "text-xs text-red-600 dark:text-red-400"
              : "text-xs text-neutral-600 dark:text-neutral-300"
          }
        >
          {agentCallState(t, call)}
        </p>
      </div>
      {call.state === "running" && (
        <button
          type="button"
          className={PILL}
          onClick={onStop}
          // Plusieurs « Stop » : chacun nomme son outil.
          aria-label={t("vscode.agents.stopLabel", {
            tool: agentToolLabel(t, call.tool),
          })}
        >
          {t("vscode.agents.stop")}
        </button>
      )}
    </li>
  );
}

export function AgentsCard({
  agents,
  onStop,
  onTurnOff,
  onClear,
}: {
  agents: AgentsView;
  onStop: (callId: string) => void;
  onTurnOff: () => void;
  onClear: () => void;
}) {
  const t = useT();
  // Le plus récent des appels en cours (les appels arrivent du plus récent
  // au plus ancien) : un nouvel appel pendant un autre est aussi annoncé.
  const latest = agents.calls.find((call) => call.state === "running");
  const running = latest !== undefined;
  const shown = agents.calls.slice(0, SHOWN_CALLS);
  return (
    <section
      aria-label={t("vscode.agents.label")}
      data-testid="agents"
      className="mx-3 mt-2 shrink-0 rounded-2xl bg-neutral-100 px-3 py-2 text-sm dark:bg-neutral-800"
    >
      <div className="flex items-center gap-2">
        <p className="min-w-0 flex-1 truncate font-medium text-neutral-900 dark:text-white">
          {t("vscode.agents.title")}
        </p>
        {!running && (
          <button
            type="button"
            aria-label={t("vscode.agents.close")}
            title={t("vscode.agents.close")}
            onClick={onClear}
            className="flex h-7 w-7 shrink-0 cursor-pointer items-center justify-center rounded-full text-neutral-700 hover:bg-neutral-200 dark:text-neutral-200 dark:hover:bg-neutral-700/75"
          >
            <XMarkIcon className="h-4 w-4" />
          </button>
        )}
      </div>
      {/* Annonce du dernier appel en cours (lecteur d'écran). */}
      <p role="status" className="sr-only">
        {latest ? agentAnnounce(t, latest) : ""}
      </p>
      <ul aria-label={t("vscode.agents.calls")}>
        {shown.map((call) => (
          <CallRow key={call.id} call={call} onStop={() => onStop(call.id)} />
        ))}
      </ul>
      {agents.enabled ? (
        <div className="mt-1 flex flex-wrap gap-1.5">
          <button type="button" className={PILL} onClick={onTurnOff}>
            {t("vscode.agents.turnOff")}
          </button>
        </div>
      ) : (
        <p className="mt-1 text-xs text-neutral-600 dark:text-neutral-300">
          {t("vscode.agents.off")}
        </p>
      )}
    </section>
  );
}
