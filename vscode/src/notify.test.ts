// @vitest-environment node
// Easy Link prévient à la fin d'une tâche longue (lot 4a, 1) : titre sans
// arguments ni secrets, coupé par défaut, seuil, nom du dossier sur
// permission, commande armée, « /status » tenu à jour, tâches de VS Code (sans
// doublon avec leur terminal), réponse longue, refus de l'app.
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { type Credentials, Link } from "./bridge/link";
import { commandTitle, Notifier, taskTitle } from "./notify";
import { FakeBridge, until } from "./test/fakeBridge";
import { cleanUp, setupController } from "./test/setup";
import { resetTestState, testState, window } from "./test/vscode";
import { TerminalWatcher } from "./terminal";

const cleanups: (() => void)[] = [];
beforeEach(() => resetTestState());
afterEach(() => {
  for (const cleanup of cleanups.splice(0)) cleanup();
  cleanUp();
});

const pause = (ms = 40) => new Promise((resolve) => setTimeout(resolve, ms));

/** Liaison au faux pont, suivi du terminal et notificateur à horloge réglée. */
async function setup() {
  const fake = new FakeBridge();
  let credentials: Credentials | null = fake.addClient();
  const link = new Link({
    dir: () => "/run",
    readDiscovery: async () => fake.discovery,
    connect: fake.connect,
    secrets: {
      get: async () => credentials,
      set: async (c) => void (credentials = c),
      delete: async () => void (credentials = null),
    },
    client: { name: "t", version: "0", editor: "VS Code" },
    watch: () => null,
    appRunning: async () => false,
  });
  const watcher = new TerminalWatcher();
  watcher.watch(true);
  const clock = { now: 1_000_000 };
  const notifier = new Notifier({
    link,
    terminal: watcher,
    show: async (route) => {
      await link.request("app.show", { route });
    },
    now: () => clock.now,
    delays: { reportMs: 20, dedupeMs: 10 },
  });
  cleanups.push(() => {
    notifier.dispose();
    watcher.dispose();
    link.dispose();
  });
  await link.start();
  await until(() => link.connected);
  const terminal = { name: "bash", shellIntegration: {} };
  /** Commande lancée dans `terminal`, finie après `ms` (horloge réglée). */
  const run = (command: string) => {
    const execution = {
      commandLine: { value: command },
      read: () => (async function* () {})(),
    };
    testState.events.shellStart.fire({ execution, terminal });
    return {
      execution,
      end(ms: number, exitCode: number | undefined) {
        clock.now += ms;
        testState.events.shellEnd.fire({ execution, exitCode });
      },
    };
  };
  return { fake, link, notifier, clock, terminal, run };
}

const enable = (minDurationSeconds = 60) => {
  testState.config["chatonlm.notify.enabled"] = true;
  testState.config["chatonlm.notify.minDurationSeconds"] = minDurationSeconds;
};

describe("commandTitle", () => {
  it("keeps the program and plain subcommands, never arguments", () => {
    expect(commandTitle("cargo build --release -p chatonlm")).toBe(
      "cargo build",
    );
    expect(commandTitle("npm run build")).toBe("npm run build");
    expect(commandTitle('git commit -m "fix the bug"')).toBe("git commit");
    expect(commandTitle("FOO=bar ./scripts/check.sh --full")).toBe("check.sh");
    expect(commandTitle("sudo pacman -Syu")).toBe("pacman");
    expect(commandTitle("python3 /home/me/secret/train.py")).toBe("python3");
  });

  it("gives only the program for a command that holds a secret", () => {
    expect(
      commandTitle(
        "curl -H 'Authorization: Bearer abcdefghijklmnopqrstuv' https://x",
      ),
    ).toBe("curl");
    expect(commandTitle("mysql -pHunter2Secret db")).toBe("mysql");
    expect(commandTitle("export TOKEN=abcdef123456 deploy")).toBe("export");
  });

  it("never keeps a free argument: host, password, token, script argument", () => {
    // Exemples du rapport 94 (S1, M3) : seul le nom du programme part.
    expect(commandTitle("./deploy.sh production Hunter2Summer")).toBe(
      "deploy.sh",
    );
    expect(commandTitle("echo MySecretPassw0rd")).toBe("echo");
    expect(commandTitle("openssl passwd Tr0ub4dor")).toBe("openssl");
    expect(commandTitle("ssh admin.clientname-prod.internal")).toBe("ssh");
    expect(commandTitle("ssh prod-db.corp.internal")).toBe("ssh");
    expect(commandTitle("vault login hvs.CAESIJx7Rk3abcdEFGH12345678")).toBe(
      "vault",
    );
    expect(commandTitle("psql customers_prod")).toBe("psql");
    // Outil à sous-commandes : la sous-commande, jamais ce qui la suit.
    expect(commandTitle("docker login registry.acme.io")).toBe("docker login");
    expect(commandTitle("docker login registry")).toBe("docker login");
    expect(commandTitle("git clone admin.client-prod.internal:repo")).toBe(
      "git clone",
    );
    expect(commandTitle("make deploy-prod PASSWORD=x")).toBe(
      "make deploy-prod",
    );
    // Une « sous-commande » qui n'en a pas la forme n'est pas gardée.
    expect(commandTitle("git Hunter2Summer")).toBe("git");
    expect(commandTitle("cargo hvs.CAESIJx7Rk3")).toBe("cargo");
    // `sudo -u nom` : l'option pourrait prendre une valeur.
    expect(commandTitle("sudo -u admin make")).toBe("sudo");
  });

  it("keeps the subcommands of known tools", () => {
    expect(commandTitle("npm run build:prod -- --token x")).toBe(
      "npm run build:prod",
    );
    expect(commandTitle("pnpm run lint")).toBe("pnpm run lint");
    expect(commandTitle("docker compose up -d")).toBe("docker compose up");
    expect(commandTitle("deno task dev")).toBe("deno task dev");
    expect(commandTitle("go mod tidy")).toBe("go mod tidy");
    expect(commandTitle("./gradlew assemble")).toBe("gradlew assemble");
    expect(commandTitle("mvn clean install")).toBe("mvn clean");
    expect(commandTitle("dotnet test")).toBe("dotnet test");
    expect(commandTitle("pytest tests/test_api.py -k login")).toBe("pytest");
    expect(commandTitle("cargo.exe build --release")).toBe("cargo.exe build");
    expect(commandTitle(".\\build.ps1 -Target prod")).toBe("build.ps1");
  });

  it("titles VS Code tasks with their source", () => {
    expect(taskTitle("build", "npm")).toBe("npm: build");
    expect(taskTitle("Compile all", "Workspace")).toBe("Compile all");
    expect(taskTitle("password=hunter22", undefined)).toBe("");
  });
});

describe("terminal commands", () => {
  it("sends nothing while notifications are off (the default)", async () => {
    const { fake, run } = await setup();
    run("cargo build").end(10 * 60_000, 1);
    await pause(80);
    expect(fake.calls("notify.send")).toEqual([]);
    expect(fake.calls("notify.task")).toEqual([]);
  });

  it("sends only the title, duration and outcome of a long command", async () => {
    enable();
    const { fake, run } = await setup();
    run("cargo build --release /home/me/projet").end(90_000, 2);
    await until(() => fake.calls("notify.send").length === 1);
    expect(fake.calls("notify.send")[0]).toEqual({
      title: "cargo build",
      outcome: "failure",
      durationMs: 90_000,
    });
  });

  it("adds the folder name only when allowed", async () => {
    enable();
    testState.config["chatonlm.notify.includeFolderName"] = true;
    const { fake, run } = await setup();
    run("make").end(61_000, 0);
    await until(() => fake.calls("notify.send").length === 1);
    expect(fake.calls("notify.send")[0]).toEqual({
      title: "make",
      outcome: "success",
      durationMs: 61_000,
      workspace: "work",
    });
  });

  it("ignores commands shorter than the threshold", async () => {
    enable(60);
    const { fake, run } = await setup();
    run("ls -la").end(59_000, 0);
    await pause(80);
    expect(fake.calls("notify.send")).toEqual([]);
  });

  it("reports a running command to /status, then its end", async () => {
    enable();
    const { fake, run, clock } = await setup();
    const started = clock.now;
    const command = run("npm test");
    await until(() => fake.calls("notify.task").length === 1);
    expect(fake.calls("notify.task")[0]).toEqual({
      id: "vscode-1",
      title: "npm test",
      state: "running",
      startedAt: started,
    });
    command.end(5_000, 1);
    await until(() => fake.calls("notify.task").length === 2);
    expect(fake.calls("notify.task")[1]).toMatchObject({
      id: "vscode-1",
      state: "failure",
    });
  });
});

describe("Notify me when this finishes", () => {
  it("notifies the next command of the active terminal, even when off", async () => {
    const { fake, notifier, run, terminal } = await setup();
    window.activeTerminal = terminal;
    await notifier.armTerminal();
    expect(testState.statusItems[0]?.visible).toBe(true);
    run("cargo test").end(2_000, 0);
    await until(() => fake.calls("notify.send").length === 1);
    expect(fake.calls("notify.send")[0]).toEqual({
      title: "cargo test",
      outcome: "success",
      durationMs: 2_000,
    });
    expect(testState.statusItems[0]?.visible).toBe(false);
    // Une seule commande : la suivante ne prévient plus.
    run("cargo test").end(2_000, 0);
    await pause(80);
    expect(fake.calls("notify.send")).toHaveLength(1);
  });

  it("arms the command already running in the active terminal", async () => {
    const { fake, notifier, run, terminal } = await setup();
    window.activeTerminal = terminal;
    const command = run("./long-job.sh");
    await notifier.armTerminal();
    command.end(3_000, undefined);
    await until(() => fake.calls("notify.send").length === 1);
    expect(fake.calls("notify.send")[0]).toMatchObject({
      title: "long-job.sh",
      outcome: "done",
    });
  });

  it("says so when the terminal has no shell integration", async () => {
    const { fake, notifier, run } = await setup();
    const plain = { name: "cmd" };
    window.activeTerminal = plain;
    await notifier.armTerminal();
    expect(testState.shown.at(-1)?.level).toBe("warning");
    expect(notifier.armed).toBe(false);
    run("dir").end(2_000, 0);
    await pause(80);
    expect(fake.calls("notify.send")).toEqual([]);
  });

  it("forgets the notification when its terminal is closed", async () => {
    const { fake, notifier, run, terminal } = await setup();
    window.activeTerminal = terminal;
    await notifier.armTerminal();
    expect(testState.statusItems[0]?.visible).toBe(true);
    testState.events.terminalClose.fire(terminal);
    expect(notifier.armed).toBe(false);
    expect(testState.statusItems[0]?.visible).toBe(false);
    // Un autre terminal, même nom : rien d'armé.
    run("make").end(2_000, 0);
    await pause(80);
    expect(fake.calls("notify.send")).toEqual([]);
  });

  it("ends the armed command of a closed terminal, outcome unknown", async () => {
    const { fake, notifier, run, terminal } = await setup();
    window.activeTerminal = terminal;
    run("cargo build");
    await notifier.armTerminal();
    testState.events.terminalClose.fire(terminal);
    await until(() => fake.calls("notify.send").length === 1);
    expect(fake.calls("notify.send")[0]).toMatchObject({
      title: "cargo build",
      outcome: "done",
    });
    expect(notifier.armed).toBe(false);
  });

  it("says when the armed notification could not go out", async () => {
    const { fake, notifier, run, terminal } = await setup();
    fake.handlers["notify.send"] = () => ({ sent: 0 });
    window.activeTerminal = terminal;
    await notifier.armTerminal();
    run("cargo test").end(2_000, 0);
    await until(() => testState.shown.some((s) => s.level === "warning"));
    expect(testState.shown.at(-1)?.text).toContain("Easy Link sent no message");
    expect(testState.shown.at(-1)?.text).toContain("cargo test");
  });

  it("says when the link is lost before the armed command ends", async () => {
    const { fake, link, notifier, run, terminal } = await setup();
    window.activeTerminal = terminal;
    const command = run("cargo test");
    await notifier.armTerminal();
    link.dispose();
    await until(() => !link.connected);
    command.end(2_000, 0);
    await until(() => testState.shown.some((s) => s.level === "warning"));
    expect(testState.shown.at(-1)?.text).toContain("ChatonLM is not connected");
    expect(fake.calls("notify.send")).toEqual([]);
  });

  it("stays quiet about a full window for automatic notifications", async () => {
    enable(0);
    const { fake, run } = await setup();
    fake.handlers["notify.send"] = () => ({ sent: 0 });
    run("make").end(1_000, 0);
    await until(() => fake.calls("notify.send").length === 1);
    await pause();
    expect(testState.shown).toEqual([]);
  });

  it("cancels from the status bar", async () => {
    const { fake, notifier, run, terminal } = await setup();
    window.activeTerminal = terminal;
    await notifier.armTerminal();
    notifier.disarm();
    expect(testState.statusItems[0]?.visible).toBe(false);
    run("make").end(2_000, 0);
    await pause(80);
    expect(fake.calls("notify.send")).toEqual([]);
  });
});

describe("app refusals", () => {
  it("says once that the app setting is off, with a way to it", async () => {
    enable(0);
    const { fake, run } = await setup();
    fake.notifyRefusal = -32035;
    testState.choice = "Open ChatonLM";
    run("make").end(1_000, 0);
    await until(() => fake.shown.length === 1);
    expect(testState.shown.at(-1)?.text).toContain("Let VS Code notify me");
    expect(fake.shown).toEqual(["/settings?section=easyLink"]);
    run("make").end(1_000, 0);
    await until(() => fake.calls("notify.send").length === 2);
    await pause();
    expect(testState.shown).toHaveLength(1);
  });

  it("explains a missing Easy Link", async () => {
    enable(0);
    const { fake, run } = await setup();
    fake.notifyRefusal = -32034;
    run("make").end(1_000, 0);
    await until(() => testState.shown.length === 1);
    expect(testState.shown[0].text).toContain("set up Easy Link");
  });
});

describe("VS Code tasks", () => {
  it("notifies a long task once, not again for its terminal", async () => {
    enable();
    const { fake, run, clock } = await setup();
    const execution = { task: { name: "build", source: "npm" } };
    testState.events.taskStart.fire({ execution });
    // La même tâche vue aussi par l'intégration du shell de son terminal.
    const command = run("npm run build");
    clock.now += 120_000;
    testState.events.taskEnd.fire({ execution, exitCode: 0 });
    testState.events.shellEnd.fire({
      execution: command.execution,
      exitCode: 0,
    });
    await until(() => fake.calls("notify.send").length === 1);
    await pause(80);
    expect(fake.calls("notify.send")).toEqual([
      { title: "npm: build", outcome: "success", durationMs: 120_000 },
    ]);
  });
});

describe("long replies", () => {
  it("notifies the end of a ChatonLM reply, without its text", async () => {
    enable(0);
    const { fake, controller } = await setupController();
    fake.handlers["chat.send"] = (_params, { id, notify }) => {
      notify("chat.event", {
        requestId: id,
        event: { type: "content", text: "secret reply" },
      });
      notify("chat.event", {
        requestId: id,
        event: { type: "done", messageId: 2 },
      });
      return { chatId: "c1", outcome: "done" };
    };
    fake.handlers["chat.get"] = () => ({
      id: "c1",
      messages: [],
      replyRunning: null,
    });
    await controller.send("Explain this", null);
    await until(() => fake.calls("notify.send").length === 1);
    // Fin d'une réponse : « done » (pas un succès de commande, rapport 94, B8).
    expect(fake.calls("notify.send")[0]).toMatchObject({
      title: "ChatonLM reply in VS Code",
      outcome: "done",
    });
    expect(JSON.stringify(fake.calls("notify.send"))).not.toContain("secret");
  });

  it("stays quiet when the user stopped the reply", async () => {
    enable(0);
    const { fake, controller } = await setupController();
    fake.handlers["chat.send"] = () => ({ chatId: "c1", outcome: "cancelled" });
    fake.handlers["chat.get"] = () => ({
      id: "c1",
      messages: [],
      replyRunning: null,
    });
    await controller.send("Explain this", null);
    await pause(80);
    expect(fake.calls("notify.send")).toEqual([]);
  });
});
