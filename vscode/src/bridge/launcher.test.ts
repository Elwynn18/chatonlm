// @vitest-environment node
import { EventEmitter } from "node:events";
import { promises as fs } from "node:fs";
import path from "node:path";
import { afterAll, beforeAll, describe, expect, it, vi } from "vitest";
import {
  bridgeDir,
  compatible,
  readDiscovery,
  validEndpoint,
} from "./discovery";
import {
  awaitBridge,
  cleanEnv,
  dataDir,
  desktopExec,
  findApp,
  launch,
  sessionEnv,
  system32,
  parseRegQuery,
  type SystemProbe,
  UNINSTALL_KEY,
  waitFor,
} from "./launcher";

// Fichiers d'essai dans le projet (.tools/tmp), jamais dans /tmp.
const scratch = path.resolve(import.meta.dirname, "../../../.tools/tmp");
let dir = "";
beforeAll(async () => {
  await fs.mkdir(scratch, { recursive: true });
  dir = await fs.mkdtemp(path.join(scratch, "vscode-discovery-"));
});
afterAll(async () => {
  if (dir) await fs.rm(dir, { recursive: true, force: true });
});

describe("discovery", () => {
  it("lives in $XDG_RUNTIME_DIR on Linux and %LOCALAPPDATA% on Windows", () => {
    expect(bridgeDir("linux", { XDG_RUNTIME_DIR: "/run/user/1000" })).toBe(
      "/run/user/1000/com.chatonlm.chatonlm",
    );
    expect(bridgeDir("linux", {})).toBeNull();
    expect(bridgeDir("linux", { XDG_RUNTIME_DIR: "relative" })).toBeNull();
    expect(
      bridgeDir("windows", { LOCALAPPDATA: "C:\\Users\\a\\AppData\\Local" }),
    ).toBe("C:\\Users\\a\\AppData\\Local\\com.chatonlm.chatonlm");
    expect(bridgeDir("linux", {}, "/override")).toBe("/override");
  });

  it("looks in the session's folder from a Snap VS Code", () => {
    // snapd donne au Snap /run/user/<uid>/snap.code ; l'app écrit dans le
    // dossier de la session.
    const env = { XDG_RUNTIME_DIR: "/run/user/1000/snap.code" };
    expect(bridgeDir("linux", env)).toBe(
      "/run/user/1000/com.chatonlm.chatonlm",
    );
    expect(bridgeDir("linux", { XDG_RUNTIME_DIR: "/run/user/1000/" })).toBe(
      "/run/user/1000/com.chatonlm.chatonlm",
    );
  });

  it("never lets a folder override the bridge on Windows", () => {
    expect(bridgeDir("windows", { LOCALAPPDATA: "C:\\L" }, "C:\\repo")).toBe(
      "C:\\L\\com.chatonlm.chatonlm",
    );
    expect(bridgeDir("linux", {}, "relative")).toBeNull();
  });

  it("accepts only the app's own endpoint", () => {
    const pipe = `\\\\.\\pipe\\chatonlm-${"ab".repeat(16)}`;
    expect(validEndpoint(pipe, "C:\\L\\x", "windows")).toBe(true);
    // Tube d'une autre machine : session réseau, refusée.
    expect(
      validEndpoint(
        `\\\\attacker.example\\pipe\\chatonlm-${"ab".repeat(16)}`,
        "C:\\L\\x",
        "windows",
      ),
    ).toBe(false);
    expect(validEndpoint(`${pipe}0`, "C:\\L", "windows")).toBe(false);
    expect(validEndpoint("\\\\.\\pipe\\other", "C:\\L", "windows")).toBe(false);
    const dir = "/run/user/1000/com.chatonlm.chatonlm";
    expect(validEndpoint(`${dir}/bridge.sock`, dir, "linux")).toBe(true);
    expect(validEndpoint(`${dir}/../x/bridge.sock`, dir, "linux")).toBe(false);
    expect(validEndpoint("/tmp/bridge.sock", dir, "linux")).toBe(false);
    expect(validEndpoint("\0chatonlm", dir, "linux")).toBe(false);
  });

  const content = {
    protocol: [1, 1],
    endpoint: "",
    pid: 42,
    app: "0.9.4",
    instance: "abc",
    startedAt: 1,
  };

  it("reads a private bridge.json and ignores one others can read", async () => {
    content.endpoint = path.join(dir, "bridge.sock");
    const file = path.join(dir, "bridge.json");
    await fs.writeFile(file, JSON.stringify(content), { mode: 0o600 });
    await fs.chmod(file, 0o600);
    expect(await readDiscovery(dir, "linux")).toEqual(content);
    await fs.chmod(file, 0o644);
    expect(await readDiscovery(dir, "linux")).toBeNull();
    // Sous Windows, les droits sont ceux du dossier de l'utilisateur ; le
    // point d'accès doit être un tube local de l'app.
    expect(await readDiscovery(dir, "windows")).toBeNull();
    const pipe = {
      ...content,
      endpoint: `\\\\.\\pipe\\chatonlm-${"0".repeat(32)}`,
    };
    await fs.writeFile(file, JSON.stringify(pipe));
    expect(await readDiscovery(dir, "windows")).toEqual(pipe);
    await fs.writeFile(
      file,
      JSON.stringify({ ...content, endpoint: "/elsewhere/bridge.sock" }),
    );
    await fs.chmod(file, 0o600);
    expect(await readDiscovery(dir, "linux")).toBeNull();
    await fs.writeFile(file, JSON.stringify(content));
    await fs.chmod(file, 0o600);
    expect(await readDiscovery(dir, "linux", 12345)).toBeNull();
    await fs.writeFile(file, "{not json");
    expect(await readDiscovery(dir, "linux")).toBeNull();
    await fs.rm(file);
    expect(await readDiscovery(dir, "linux")).toBeNull();
  });

  it("knows which protocols it speaks", () => {
    const d = { ...content, protocol: [1, 1] as [number, number] };
    expect(compatible(d)).toBe(true);
    expect(compatible({ ...d, protocol: [1, 4] })).toBe(true);
    expect(compatible({ ...d, protocol: [2, 4] })).toBe(false);
  });
});

function probe(
  files: Record<string, string>,
  registry?: Record<string, string>,
): SystemProbe {
  return {
    readFile: async (file) => {
      if (file in files) return files[file];
      throw new Error("ENOENT");
    },
    isFile: async (file) => file in files,
    registry: async (key) => {
      if (key !== UNINSTALL_KEY || !registry) throw new Error("not found");
      return registry;
    },
  };
}

describe("finding the app", () => {
  const env = { XDG_DATA_HOME: "/home/a/.local/share" };

  it("prefers the launcher.json the app wrote", async () => {
    const files = {
      "/home/a/.local/share/com.chatonlm.chatonlm/launcher.json":
        JSON.stringify({
          path: "/home/a/Apps/ChatonLM.AppImage",
        }),
      "/home/a/Apps/ChatonLM.AppImage": "",
      "/usr/bin/chatonlm": "",
    };
    expect(await findApp("linux", env, "/home/a", probe(files))).toBe(
      "/home/a/Apps/ChatonLM.AppImage",
    );
  });

  it("falls back to /usr/bin, then to the menu entry of an AppImage", async () => {
    expect(
      await findApp("linux", {}, "/home/a", probe({ "/usr/bin/chatonlm": "" })),
    ).toBe("/usr/bin/chatonlm");
    const entry =
      "[Desktop Entry]\nName=ChatonLM\nTryExec=/opt/ChatonLM.AppImage\nExec=/opt/ChatonLM.AppImage %U\n";
    const files = {
      "/home/a/.local/share/applications/com.chatonlm.chatonlm.desktop": entry,
      "/opt/ChatonLM.AppImage": "",
    };
    expect(await findApp("linux", {}, "/home/a", probe(files))).toBe(
      "/opt/ChatonLM.AppImage",
    );
    // launcher.json vers un programme disparu : ignoré.
    const stale = {
      "/home/a/.local/share/com.chatonlm.chatonlm/launcher.json":
        JSON.stringify({
          path: "/gone",
        }),
    };
    expect(await findApp("linux", {}, "/home/a", probe(stale))).toBeNull();
  });

  it("reads the installer's registry key on Windows", async () => {
    const env = { LOCALAPPDATA: "C:\\Users\\a\\AppData\\Local" };
    const registry = {
      InstallLocation: '"C:\\Users\\a\\AppData\\Local\\ChatonLM"',
      MainBinaryName: "chatonlm",
    };
    const files = {
      "C:\\Users\\a\\AppData\\Local\\ChatonLM\\chatonlm.exe": "",
    };
    expect(await findApp("windows", env, "", probe(files, registry))).toBe(
      "C:\\Users\\a\\AppData\\Local\\ChatonLM\\chatonlm.exe",
    );
    expect(await findApp("windows", env, "", probe(files))).toBe(
      "C:\\Users\\a\\AppData\\Local\\ChatonLM\\chatonlm.exe",
    );
    expect(await findApp("windows", env, "", probe({}))).toBeNull();
  });

  it("parses reg.exe output and menu entries", () => {
    const output = [
      "",
      "HKEY_CURRENT_USER\\Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\ChatonLM",
      '    InstallLocation    REG_SZ    "C:\\Program Files\\ChatonLM"',
      "    MainBinaryName    REG_SZ    chatonlm.exe",
      "    EstimatedSize    REG_DWORD    0x1234",
      "",
    ].join("\r\n");
    expect(parseRegQuery(output)).toEqual({
      InstallLocation: '"C:\\Program Files\\ChatonLM"',
      MainBinaryName: "chatonlm.exe",
      EstimatedSize: "0x1234",
    });
    expect(desktopExec('[Desktop Entry]\nExec="/a b/c" --x\n')).toBe("/a b/c");
    expect(
      desktopExec("[Desktop Entry]\nExec=/a\n[Desktop Action x]\nTryExec=/b\n"),
    ).toBe("/a");
  });

  it("finds the data folder like the app", () => {
    expect(dataDir("linux", {}, "/home/a")).toBe(
      "/home/a/.local/share/com.chatonlm.chatonlm",
    );
    expect(dataDir("linux", { XDG_DATA_HOME: "/x" }, "/home/a")).toBe(
      "/x/com.chatonlm.chatonlm",
    );
    expect(dataDir("windows", { LOCALAPPDATA: "C:\\L" }, "")).toBe(
      "C:\\L\\com.chatonlm.chatonlm",
    );
    // VS Code en Snap : sa valeur d'origine, sinon le dossier par défaut.
    expect(
      dataDir(
        "linux",
        { XDG_DATA_HOME: "/home/a/snap/code/190/.local/share" },
        "/home/a",
      ),
    ).toBe("/home/a/.local/share/com.chatonlm.chatonlm");
    expect(
      dataDir(
        "linux",
        {
          XDG_DATA_HOME: "/home/a/snap/code/190/.local/share",
          XDG_DATA_HOME_VSCODE_SNAP_ORIG: "/data/a",
        },
        "/home/a",
      ),
    ).toBe("/data/a/com.chatonlm.chatonlm");
  });
});

describe("launching", () => {
  it("cleans the environment of a Snap or AppImage VS Code", () => {
    const env = cleanEnv({
      HOME: "/home/a",
      PATH: "/usr/bin",
      APPDIR: "/tmp/.mount_code",
      APPIMAGE: "/a/code.AppImage",
      LD_LIBRARY_PATH: "/snap/code/lib",
      GTK_PATH: "/snap/code/gtk",
      GIO_MODULE_DIR: "/snap/gio",
      GDK_PIXBUF_MODULE_FILE: "/snap/pixbuf",
      SNAP: "/snap/code/1",
      SNAP_NAME: "code",
      ELECTRON_RUN_AS_NODE: "1",
      VSCODE_IPC_HOOK: "/run/x.sock",
      XDG_DATA_DIRS: "/snap/code/usr/share:/usr/share",
      XDG_DATA_DIRS_VSCODE_SNAP_ORIG: "/usr/local/share:/usr/share",
      GTK_IM_MODULE_FILE_VSCODE_SNAP_ORIG: "",
      GTK_IM_MODULE_FILE: "/snap/im",
      CHATONLM_BRIDGE_DIR: "/kept",
      XDG_RUNTIME_DIR: "/run/user/1000/snap.code",
      XDG_DATA_HOME: "/home/a/snap/code/190/.local/share",
    });
    expect(env).toEqual({
      HOME: "/home/a",
      PATH: "/usr/bin",
      XDG_DATA_DIRS: "/usr/local/share:/usr/share",
      CHATONLM_BRIDGE_DIR: "/kept",
      // L'app ouvre son pont dans le dossier de la session.
      XDG_RUNTIME_DIR: "/run/user/1000",
    });
    expect(sessionEnv({ XDG_RUNTIME_DIR: "/run/user/1000" })).toEqual({
      XDG_RUNTIME_DIR: "/run/user/1000",
    });
  });

  it("runs Windows tools by their full path", () => {
    expect(system32({ SystemRoot: "D:\\Win" }, "reg.exe")).toBe(
      "D:\\Win\\System32\\reg.exe",
    );
    expect(system32({}, "tasklist.exe")).toBe(
      "C:\\Windows\\System32\\tasklist.exe",
    );
  });

  it("starts the app detached, hidden, with the cleaned environment", async () => {
    const child = Object.assign(new EventEmitter(), { unref: vi.fn() });
    const spawn = vi.fn(() => {
      queueMicrotask(() => child.emit("spawn"));
      return child;
    });
    await launch(
      "/usr/bin/chatonlm",
      true,
      { PATH: "/usr/bin", APPIMAGE: "/x" },
      spawn as never,
    );
    expect(spawn).toHaveBeenCalledWith("/usr/bin/chatonlm", ["--background"], {
      detached: true,
      stdio: "ignore",
      env: { PATH: "/usr/bin" },
      cwd: "/usr/bin",
    });
    expect(child.unref).toHaveBeenCalled();
    await launch("/usr/bin/chatonlm", false, {}, spawn as never);
    expect(spawn).toHaveBeenLastCalledWith(
      "/usr/bin/chatonlm",
      [],
      expect.anything(),
    );
  });

  it("does not wait for a bridge an app already running has turned off", async () => {
    let checks = 0;
    const ready = async () => (checks++, false);
    const started = Date.now();
    await expect(awaitBridge(true, ready, 10_000)).resolves.toBe(false);
    expect(checks).toBe(1);
    expect(Date.now() - started).toBeLessThan(1000);
    await expect(awaitBridge(false, ready, 30)).resolves.toBe(false);
    expect(checks).toBeGreaterThan(1);
  });

  it("waits for the bridge, then gives up", async () => {
    let n = 0;
    await expect(waitFor(async () => ++n === 3, 1000, 1)).resolves.toBe(true);
    await expect(waitFor(async () => false, 20, 5)).resolves.toBe(false);
  });
});
