// Ce que l'écran d'accueil dit et propose pour chaque état du serveur, dans
// la langue de `t` (les commandes à copier restent telles quelles) : le
// bouton « Start Ollama » d'un service arrêté, l'Ollama intégré (démarrage,
// échec, et le lien qui y passe quand celui de la machine ne répond pas).
// Sous Windows il n'y a pas de service : l'app de bureau d'Ollama (zone de
// notification) en tient lieu (`status.service` la décrit), « Start Ollama »
// la lance, et les commandes sont celles de PowerShell (ni systemctl, ni
// journalctl, ni script d'installation).
import type { ServerStatus } from "@/bindings/ServerStatus";
import type { TFunction } from "@/i18n";
import { type Platform, platform } from "@/lib/platform";

export const DOWNLOAD_URL = "https://ollama.com/download";
/**
 * Adresse que prend l'Ollama de la machine sans réglage, montrée quand
 * l'intégré sert (la sienne est privée, et change à chaque lancement).
 */
export const DEFAULT_ADDRESS = "http://localhost:11434";
export const INSTALL_COMMAND = "curl -fsSL https://ollama.com/install.sh | sh";
/** Journal du serveur de l'app de bureau d'Ollama sous Windows (PowerShell). */
export const WINDOWS_LOG_COMMAND =
  'notepad "$env:LOCALAPPDATA\\Ollama\\server.log"';

export interface Advice {
  title: string;
  description: string;
  commands: { label?: string; command: string }[];
  download?: boolean;
  /** « Start Ollama » : démarre le service arrêté (polkit pour le système). */
  start?: boolean;
  /**
   * Passage à l'Ollama intégré : « instead » quand celui de la machine (à
   * l'adresse par défaut) ne répond pas, « use » quand une adresse réglée ne
   * répond pas ; null : pas proposé (déjà l'intégré, ou pas d'intégré dans ce
   * paquet).
   */
  builtin: "instead" | "use" | null;
  /** L'intégré démarre : pas de Retry (vérifié toutes les 3 s). */
  waiting?: boolean;
  /** L'intégré n'a pas démarré : chemin de son journal (status.logPath). */
  log?: boolean;
}

/** Adresse locale standard d'Ollama (le service y écoute). */
const STANDARD_LOCAL = /^http:\/\/(localhost|127\.0\.0\.1|\[::1\]):11434$/;

export function adviceFor(
  t: TFunction,
  status: ServerStatus,
  os: Platform = platform(),
): Advice {
  const advice =
    (os === "windows" ? windowsAdvice(t, status) : null) ??
    baseAdvice(t, status);
  return { ...advice, builtin: builtinOffer(status) };
}

/**
 * Ce qui diffère sous Windows ; null : comme sous Linux (baseAdvice).
 */
function windowsAdvice(
  t: TFunction,
  status: ServerStatus,
): Omit<Advice, "builtin"> | null {
  const { url, service, httpStatus } = status;
  switch (status.state) {
    case "notInstalled":
      // Pas de script d'installation : l'installateur de ollama.com.
      return {
        title: t("chat.advice.notInstalled.title"),
        description: t("chat.advice.notInstalled.description"),
        commands: [],
        download: true,
      };
    case "serviceStopped":
      // App de bureau installée, pas lancée ; sans elle (jamais attendu),
      // le serveur se lance à la main.
      return service
        ? {
            title: t("chat.advice.notRunning"),
            description: t("chat.adviceWindows.appStopped"),
            start: true,
            commands: [],
          }
        : {
            title: t("chat.advice.notRunning"),
            description: t("chat.advice.serve"),
            commands: [{ command: "ollama serve" }],
          };
    case "serviceStarting":
      return {
        title: t("chat.advice.serviceStarting.title"),
        description: t("chat.adviceWindows.appStarting"),
        commands: [{ command: WINDOWS_LOG_COMMAND }],
      };
    case "noService":
      // Variable posée à la façon de PowerShell, pas de sh.
      return status.hostEnv
        ? {
            title: t("chat.advice.notRunningHere"),
            description: t("chat.advice.serveHost"),
            commands: [
              {
                command: `$env:OLLAMA_HOST="${status.hostEnv}"; ollama serve`,
              },
            ],
          }
        : null;
    case "notResponding":
      // Comme sous Linux, avec le journal de l'app à la place de journalctl.
      return {
        title: t("chat.advice.notResponding"),
        description: httpStatus
          ? t("chat.advice.behindProxy", { url, status: String(httpStatus) })
          : t(
              service
                ? "chat.advice.noResponseLogs"
                : "chat.advice.noResponseAddress",
              { url },
            ),
        commands: service ? [{ command: WINDOWS_LOG_COMMAND }] : [],
      };
    default:
      return null;
  }
}

/** Lien vers l'Ollama intégré, s'il peut servir et ne sert pas déjà. */
function builtinOffer(status: ServerStatus): Advice["builtin"] {
  if (status.builtin || !status.builtinAvailable) return null;
  if (status.state === "running") return null;
  return status.explicitAddress ? "use" : "instead";
}

function baseAdvice(
  t: TFunction,
  status: ServerStatus,
): Omit<Advice, "builtin"> {
  const { url, service, httpStatus } = status;
  const systemctl = service?.user ? "systemctl --user" : "sudo systemctl";
  const logs = {
    command: service?.user
      ? "journalctl --user -e -u ollama"
      : "journalctl -e -u ollama",
  };
  // Code HTTP en texte : jamais écrit à la façon d'un nombre (« 1 000 »).
  const code = String(httpStatus ?? "");
  const behindProxy = t("chat.advice.behindProxy", { url, status: code });

  switch (status.state) {
    case "notInstalled":
      return {
        title: t("chat.advice.notInstalled.title"),
        description: t("chat.advice.notInstalled.description"),
        commands: [{ command: INSTALL_COMMAND }],
        download: true,
      };
    case "serviceStopped":
      return {
        title: t("chat.advice.notRunning"),
        description: t(
          service
            ? "chat.advice.serviceStoppedStart"
            : "chat.advice.serviceStopped",
        ),
        start: service !== null,
        commands: [
          { command: `${systemctl} start ollama` },
          ...(service && !service.enabled
            ? [
                {
                  label: t(
                    service.user
                      ? "chat.advice.enableAtLogin"
                      : "chat.advice.enableAtBoot",
                  ),
                  command: `${systemctl} enable ollama`,
                },
              ]
            : []),
        ],
      };
    case "noService":
      if (status.hostEnv) {
        return {
          title: t("chat.advice.notRunningHere"),
          description: t("chat.advice.serveHost"),
          commands: [{ command: `OLLAMA_HOST=${status.hostEnv} ollama serve` }],
        };
      }
      if (STANDARD_LOCAL.test(url)) {
        return {
          title: t("chat.advice.notRunning"),
          description: t("chat.advice.serve"),
          commands: [{ command: "ollama serve" }],
        };
      }
      return {
        title: t("chat.advice.notRunningHere"),
        description: t("chat.advice.nothingListening", { url }),
        commands: [],
      };
    case "serviceFailed":
      return {
        title: t("chat.advice.serviceFailed.title"),
        description: t("chat.advice.serviceFailed.description"),
        commands: [
          {
            command: service?.user
              ? "systemctl --user status ollama"
              : "systemctl status ollama",
          },
          logs,
        ],
      };
    case "serviceStarting":
      return {
        title: t("chat.advice.serviceStarting.title"),
        description: t("chat.advice.serviceStarting.description"),
        commands: [logs],
      };
    case "notResponding":
      return {
        title: t("chat.advice.notResponding"),
        description: httpStatus
          ? behindProxy
          : t(
              service
                ? "chat.advice.noResponseLogs"
                : "chat.advice.noResponseAddress",
              { url },
            ),
        commands: service ? [logs] : [],
      };
    case "remoteUnreachable":
      return {
        title: t("chat.advice.remoteUnreachable.title"),
        description: httpStatus
          ? behindProxy
          : t("chat.advice.remoteUnreachable.description", { url }),
        commands: [],
      };
    case "connectionFailed":
      return {
        title: t("chat.advice.connectionFailed.title"),
        description: t(
          url.startsWith("https://")
            ? "chat.advice.connectionFailed.https"
            : "chat.advice.connectionFailed.address",
          { url },
        ),
        commands: [],
      };
    case "notOllama":
      return {
        title: t("chat.advice.notOllama.title"),
        description: httpStatus
          ? t("chat.advice.notOllama.withStatus", { url, status: code })
          : t("chat.advice.notOllama.description", { url }),
        commands: [],
      };
    case "accessDenied":
      return {
        title: t("chat.advice.accessDenied.title"),
        description: t("chat.advice.accessDenied.description", {
          url,
          status: code,
        }),
        commands: [],
      };
    case "builtinStarting":
      return {
        title: t("chat.advice.builtinStarting.title"),
        description: t("chat.advice.builtinStarting.description"),
        commands: [],
        waiting: true,
      };
    case "builtinFailed":
      return {
        title: t("chat.advice.builtinFailed.title"),
        description: t("chat.advice.builtinFailed.description"),
        commands: [],
        log: true,
      };
    case "running":
      return {
        title: t("chat.advice.running.title"),
        description: t("chat.advice.running.description", {
          version: status.version ?? "",
          url,
        }),
        commands: [],
      };
  }
}
