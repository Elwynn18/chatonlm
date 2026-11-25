// Page Tools (propre à ChatonLM) : une carte par outil, qui ouvre sa page.
// Le titre (h1) est celui du gabarit (SidebarLayout). Les outils gardent
// leurs pages, leurs adresses et leurs raccourcis ; l'entrée Tools de la
// barre latérale est surlignée sur chacune. Liste des outils : src/lib/tools.ts.
// Une carte peut porter une pastille (Atelier : un projet calcule ou attend
// l'utilisateur), tenue à jour par les événements du moteur.

import { Link } from "@tanstack/react-router";
import { useT } from "@/i18n";
import { TOOLS, type Tool } from "@/lib/tools";

export function ToolsPage() {
  const t = useT();
  return (
    <main
      data-testid="tools"
      className="flex min-h-0 w-full flex-1 flex-col overflow-y-auto dark:bg-neutral-900"
    >
      <div className="mx-auto flex w-full max-w-3xl flex-col gap-6 p-6 pt-2">
        <p className="text-sm/6 text-zinc-500 select-none dark:text-zinc-400">
          {t("tools.description")}
        </p>
        <ul className="grid grid-cols-1 gap-3 sm:grid-cols-2">
          {TOOLS.map((tool) => (
            <li key={tool.id}>
              <ToolCard tool={tool} />
            </li>
          ))}
        </ul>
      </div>
    </main>
  );
}

function ToolCard({ tool }: { tool: Tool }) {
  const t = useT();
  const Icon = tool.icon;
  const Badge = tool.badge;
  return (
    <Link
      to={tool.to}
      draggable={false}
      data-testid={`tool-card-${tool.id}`}
      className="flex h-full items-start gap-3 rounded-2xl border border-neutral-200 bg-white p-4 select-none hover:border-neutral-300 dark:border-neutral-700/70 dark:bg-neutral-800/60 dark:hover:border-neutral-600"
    >
      <span className="flex h-10 w-10 flex-none items-center justify-center rounded-full bg-neutral-100 text-accent-fg dark:bg-neutral-700/60">
        <Icon className="h-5 w-5" aria-hidden="true" />
      </span>
      <span className="flex min-w-0 flex-col gap-1">
        <span className="flex flex-wrap items-center gap-2 font-medium text-neutral-900 dark:text-white">
          {t(tool.name)}
          {Badge && <Badge />}
        </span>
        <span className="text-sm text-neutral-500 dark:text-neutral-400">
          {t(tool.description)}
        </span>
      </span>
    </Link>
  );
}
