import { clearMocks, mockConvertFileSrc } from "@tauri-apps/api/mocks";
import { afterEach, describe, expect, it } from "vitest";
import { appPlatform } from "@/lib/ipc";
import { fakeBackend } from "@/test/backend";
import { installSimulator } from "@/test/simulator";
import {
  guessPlatform,
  isWindows,
  platform,
  protocolBase,
  setPlatform,
} from "./platform";

afterEach(clearMocks);

describe("platform", () => {
  it("is Linux until the engine says otherwise, with no attribute on <html>", () => {
    expect(platform()).toBe("linux");
    expect(isWindows()).toBe(false);
    expect(document.documentElement.dataset.platform).toBeUndefined();
  });

  it("is noted and set on <html>, for the styles of one system", () => {
    setPlatform("windows");
    expect(platform()).toBe("windows");
    expect(isWindows()).toBe(true);
    expect(document.documentElement.dataset.platform).toBe("windows");
    setPlatform("linux");
    expect(isWindows()).toBe(false);
    expect(document.documentElement.dataset.platform).toBe("linux");
  });

  it("takes an unknown value for Linux", () => {
    setPlatform("darwin" as never);
    expect(platform()).toBe("linux");
  });

  it("is answered by the engine (app_platform), Linux by default in the simulator", async () => {
    const backend = fakeBackend(installSimulator());
    expect(await appPlatform()).toBe("linux");
    backend.platform = "windows";
    expect(await appPlatform()).toBe("windows");
  });
});

describe("protocolBase", () => {
  it("is <protocol>://localhost/ outside the app and on Linux", () => {
    expect(protocolBase("chatonlm")).toBe("chatonlm://localhost/");
    mockConvertFileSrc("linux");
    expect(protocolBase("chatonlm")).toBe("chatonlm://localhost/");
    expect(guessPlatform()).toBe("linux");
  });

  it("is http://<protocol>.localhost/ on Windows, as Tauri serves it", () => {
    mockConvertFileSrc("windows");
    expect(protocolBase("chatonlm")).toBe("http://chatonlm.localhost/");
    // Sans réponse du moteur, le système se lit là.
    expect(guessPlatform()).toBe("windows");
  });
});
