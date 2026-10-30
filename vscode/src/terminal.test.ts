// @vitest-environment node
// Dernière commande échouée du terminal (intégration du shell).
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { until } from "./test/fakeBridge";
import { resetTestState, testState, window } from "./test/vscode";
import {
  BUFFER_CHARS,
  pushOutput,
  stripAnsi,
  TERMINAL_OUTPUT_CHARS,
  TerminalWatcher,
} from "./terminal";

const ESC = "\u001b";

let watcher: TerminalWatcher | null = null;
beforeEach(() => resetTestState());
afterEach(() => {
  watcher?.dispose();
  watcher = null;
});

/** Commande simulée : sa sortie est lue par `read()`, seulement après l'appel. */
function execution(command: string, chunks: string[]) {
  let reads = 0;
  return {
    commandLine: { value: command },
    get reads() {
      return reads;
    },
    read() {
      reads++;
      return (async function* () {
        for (const chunk of chunks) yield chunk;
      })();
    },
  };
}

describe("stripAnsi", () => {
  it("removes colours, titles and rewritten lines", () => {
    expect(
      stripAnsi(
        `${ESC}]633;C${ESC}\\${ESC}[1;31mFAIL${ESC}[0m src/a.test.ts\r\n` +
          `progress 10%\rprogress 100%\r\n${ESC}]0;title\u0007done${ESC}=`,
      ),
    ).toBe("FAIL src/a.test.ts\nprogress 100%\ndone");
  });

  it("removes the character set choices of tput sgr0", () => {
    expect(stripAnsi(`${ESC}[1mBold${ESC}(B${ESC}[m end${ESC})0`)).toBe(
      "Bold end",
    );
  });
});

describe("output buffer", () => {
  it("keeps a single huge chunk within the buffer", () => {
    const buffer = { chunks: [] as string[], size: 0 };
    pushOutput(buffer, "a".repeat(BUFFER_CHARS * 3) + "end");
    expect(buffer.size).toBe(BUFFER_CHARS);
    expect(buffer.chunks.join("")).toHaveLength(BUFFER_CHARS);
    expect(buffer.chunks.join("").endsWith("end")).toBe(true);
    pushOutput(buffer, "more");
    expect(buffer.size).toBeLessThanOrEqual(BUFFER_CHARS);
    expect(buffer.chunks.join("").endsWith("more")).toBe(true);
  });
});

describe("TerminalWatcher", () => {
  it("follows nothing before the editor is paired, and forgets on unpairing", async () => {
    watcher = new TerminalWatcher();
    const early = execution("npm test", ["secret output\n"]);
    testState.events.shellStart.fire({ execution: early });
    expect(early.reads).toBe(0);
    testState.events.shellEnd.fire({ execution: early, exitCode: 1 });
    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(watcher.last).toBeNull();

    watcher.watch(true);
    const failing = execution("make", ["boom\n"]);
    testState.events.shellStart.fire({ execution: failing });
    testState.events.shellEnd.fire({ execution: failing, exitCode: 2 });
    await until(() => watcher?.last !== null);
    let changes = 0;
    watcher.onChange(() => changes++);
    watcher.watch(false);
    expect(watcher.last).toBeNull();
    expect(changes).toBe(1);
    const later = execution("ls", ["x\n"]);
    testState.events.shellStart.fire({ execution: later });
    expect(later.reads).toBe(0);
  });

  it("keeps the command that failed last, even if an earlier one drains slower", async () => {
    watcher = new TerminalWatcher();
    watcher.watch(true);
    // Sortie qui ne finit jamais : sa fin attend le délai de vidange.
    const slow = {
      commandLine: { value: "slow" },
      read: () =>
        (async function* () {
          yield "slow output\n";
          await new Promise(() => {});
        })(),
    };
    const fast = execution("fast", ["fast output\n"]);
    testState.events.shellStart.fire({ execution: slow });
    testState.events.shellStart.fire({ execution: fast });
    testState.events.shellEnd.fire({ execution: slow, exitCode: 1 });
    testState.events.shellEnd.fire({ execution: fast, exitCode: 1 });
    await until(() => watcher?.last?.command === "fast");
    await new Promise((resolve) => setTimeout(resolve, 650));
    expect(watcher.last?.command).toBe("fast");
  });

  it("keeps the last command that failed, with the end of its output", async () => {
    watcher = new TerminalWatcher();
    watcher.watch(true);
    let changes = 0;
    watcher.onChange(() => changes++);
    const failing = execution("npm test", [
      `${ESC}[31mFAIL${ESC}[0m x\r\n`,
      "Expected 1, got 2\r\n",
    ]);
    testState.events.shellStart.fire({ execution: failing });
    // Lue dès le début : sinon la sortie serait perdue.
    expect(failing.reads).toBe(1);
    testState.events.shellEnd.fire({ execution: failing, exitCode: 1 });
    await until(() => watcher?.last !== null);
    expect(watcher.last).toEqual({
      command: "npm test",
      exitCode: 1,
      output: "FAIL x\nExpected 1, got 2",
    });
    expect(changes).toBe(1);

    // Réussite, ou code inconnu (Ctrl+C) : la dernière erreur reste.
    for (const exitCode of [0, undefined]) {
      const other = execution("ls", ["ok\n"]);
      testState.events.shellStart.fire({ execution: other });
      testState.events.shellEnd.fire({ execution: other, exitCode });
    }
    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(watcher.last?.command).toBe("npm test");
    expect(changes).toBe(1);
  });

  it("keeps only the last characters of a long output", async () => {
    watcher = new TerminalWatcher();
    watcher.watch(true);
    const long = execution("make", [
      "a".repeat(70_000),
      "b".repeat(TERMINAL_OUTPUT_CHARS),
    ]);
    testState.events.shellStart.fire({ execution: long });
    testState.events.shellEnd.fire({ execution: long, exitCode: 2 });
    await until(() => watcher?.last !== null);
    expect(watcher.last?.output).toBe("b".repeat(TERMINAL_OUTPUT_CHARS));
  });

  it("does nothing without shell integration in the editor", () => {
    const start = window.onDidStartTerminalShellExecution;
    (window as Record<string, unknown>).onDidStartTerminalShellExecution =
      undefined;
    try {
      watcher = new TerminalWatcher();
      watcher.watch(true);
      expect(watcher.last).toBeNull();
    } finally {
      window.onDidStartTerminalShellExecution = start;
    }
  });
});
