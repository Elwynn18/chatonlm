// Conseils de l'écran du serveur sous Windows : pas de service systemd (l'app
// de bureau d'Ollama en tient lieu), commandes de PowerShell. Les conseils de
// Linux sont essayés par ServerStatusScreen.test.tsx.

import { describe, expect, it } from "vitest";
import type { ServerState } from "@/bindings/ServerState";
import type { ServerStatus } from "@/bindings/ServerStatus";
import { translator } from "@/i18n";
import { setPlatform } from "./platform";
import { adviceFor, WINDOWS_LOG_COMMAND } from "./serverAdvice";

const t = translator("en");

function status(
  state: ServerState,
  patch: Partial<ServerStatus> = {},
): ServerStatus {
  return {
    url: "http://localhost:11434",
    state,
    version: null,
    alternativeUrl: null,
    hostEnv: null,
    httpStatus: null,
    detail: null,
    service: null,
    builtin: false,
    builtinAvailable: false,
    explicitAddress: false,
    logPath: null,
    ...patch,
  };
}

const APP = { user: true, enabled: true };
const commands = (state: ServerState, patch: Partial<ServerStatus> = {}) =>
  adviceFor(t, status(state, patch), "windows").commands.map(
    ({ command }) => command,
  );

describe("server advice on Windows", () => {
  it("not installed: the download link, no install script", () => {
    const advice = adviceFor(t, status("notInstalled"), "windows");
    expect(advice.title).toBe("Ollama is not installed");
    expect(advice.download).toBe(true);
    expect(advice.commands).toEqual([]);
    // Linux : le script d'installation reste.
    expect(adviceFor(t, status("notInstalled"), "linux").commands).toHaveLength(
      1,
    );
  });

  it("app installed but not running: Start Ollama, no systemctl, no enable line", () => {
    const advice = adviceFor(
      t,
      status("serviceStopped", { service: { user: true, enabled: false } }),
      "windows",
    );
    expect(advice.title).toBe("Ollama is not running");
    expect(advice.description).toBe(
      "Ollama is installed, but it isn't running. Click Start Ollama, or open Ollama from the Start menu.",
    );
    expect(advice.start).toBe(true);
    expect(advice.commands).toEqual([]);
  });

  it("app starting: its log, not journalctl", () => {
    const advice = adviceFor(
      t,
      status("serviceStarting", { service: APP }),
      "windows",
    );
    expect(advice.title).toBe("Ollama is starting");
    expect(advice.description).toBe(
      "The Ollama app is starting. If this takes more than a few seconds, check its log:",
    );
    expect(advice.commands).toEqual([{ command: WINDOWS_LOG_COMMAND }]);
    expect(WINDOWS_LOG_COMMAND).toBe(
      'notepad "$env:LOCALAPPDATA\\Ollama\\server.log"',
    );
  });

  it("no app: ollama serve, with OLLAMA_HOST set the PowerShell way", () => {
    expect(commands("noService")).toEqual(["ollama serve"]);
    expect(
      commands("noService", {
        url: "http://localhost:11500",
        hostEnv: "127.0.0.1:11500",
      }),
    ).toEqual(['$env:OLLAMA_HOST="127.0.0.1:11500"; ollama serve']);
    // Linux : la forme de sh.
    expect(
      adviceFor(t, status("noService", { hostEnv: "127.0.0.1:11500" }), "linux")
        .commands,
    ).toEqual([{ command: "OLLAMA_HOST=127.0.0.1:11500 ollama serve" }]);
  });

  it("not responding: the log of the app when it is installed, never journalctl", () => {
    expect(commands("notResponding", { service: APP })).toEqual([
      WINDOWS_LOG_COMMAND,
    ]);
    expect(commands("notResponding")).toEqual([]);
    const proxied = adviceFor(
      t,
      status("notResponding", { service: APP, httpStatus: 502 }),
      "windows",
    );
    expect(proxied.description).toBe(
      "http://localhost:11434 responded with HTTP 502: the Ollama server behind it is not responding.",
    );
    expect(proxied.commands).toEqual([{ command: WINDOWS_LOG_COMMAND }]);
  });

  it("gives no systemd command in any state", () => {
    const states: ServerState[] = [
      "running",
      "notOllama",
      "accessDenied",
      "remoteUnreachable",
      "connectionFailed",
      "notInstalled",
      "serviceStopped",
      "serviceStarting",
      "noService",
      "notResponding",
      "builtinStarting",
      "builtinFailed",
    ];
    for (const state of states) {
      for (const service of [null, APP]) {
        const all = commands(state, { service }).join("\n");
        expect(all, state).not.toMatch(/systemctl|journalctl|curl|sudo/);
      }
    }
  });

  it("follows the platform of the app by default", () => {
    setPlatform("windows");
    expect(adviceFor(t, status("notInstalled")).commands).toEqual([]);
    setPlatform("linux");
    expect(adviceFor(t, status("notInstalled")).commands).toHaveLength(1);
  });
});
