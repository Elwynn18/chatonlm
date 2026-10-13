import { emit } from "@tauri-apps/api/event";
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";
import { afterEach, describe, expect, it, vi } from "vitest";
import {
  accountSignIn,
  accountSignOut,
  accountStatus,
  cloudStatus,
  dictationBindShortcut,
  dictationCancel,
  dictationConfigureShortcut,
  dictationCopyLast,
  dictationInfo,
  dictationStatus,
  dictationToggle,
  easyLinkCancelPairing,
  easyLinkConnect,
  easyLinkDisconnect,
  easyLinkPair,
  easyLinkSetEnabled,
  easyLinkState,
  easyLinkUnlink,
  getDictationSettings,
  getSettings,
  getThinkPref,
  hubBrowse,
  hubCancelDownload,
  hubDeleteModel,
  hubDismissDownload,
  hubDownload,
  hubDownloads,
  hubHardware,
  hubHome,
  hubInstalled,
  hubPauseDownload,
  hubPrioritizeDownload,
  hubReadme,
  hubRepo,
  hubResumeDownload,
  hubSearch,
  isAppError,
  listenChatsUpdated,
  listenEasyLink,
  modelDetails,
  modelExistsUpstream,
  outdatedModels,
  quitApp,
  recommendedModels,
  setThinkPref,
  speechCancelDownload,
  speechDeleteModel,
  speechDismissDownload,
  speechDownload,
  speechDownloads,
  speechModels,
  stageBytes,
  stagePaths,
  unstage,
  updateDictationSettings,
  updateSettings,
} from "./ipc";

afterEach(() => clearMocks());

describe("settings commands", () => {
  it("calls get_settings without arguments", async () => {
    const calls: [string, unknown][] = [];
    mockIPC((command, args) => {
      calls.push([command, args]);
      return { theme: "dark" };
    });
    await expect(getSettings()).resolves.toEqual({ theme: "dark" });
    expect(calls).toEqual([["get_settings", {}]]);
  });

  it("wraps the patch for update_settings", async () => {
    const calls: [string, unknown][] = [];
    mockIPC((command, args) => {
      calls.push([command, args]);
      return {};
    });
    await updateSettings({ theme: "light", contextLength: 8192 });
    expect(calls).toEqual([
      ["update_settings", { patch: { theme: "light", contextLength: 8192 } }],
    ]);
  });

  it("recognizes backend errors", async () => {
    mockIPC(() => {
      throw { kind: "invalid", message: "unsupported context length: 1" };
    });
    const error = await updateSettings({ contextLength: 1 }).catch(
      (e: unknown) => e,
    );
    expect(isAppError(error)).toBe(true);
    expect(isAppError(new Error("x"))).toBe(false);
  });
});

describe("account commands", () => {
  it("names the four commands of commands/account.rs, without arguments", async () => {
    const calls: [string, unknown][] = [];
    mockIPC((command, args) => {
      calls.push([command, args]);
      if (command === "account_status") return { state: "signedOut" };
      if (command === "cloud_status") return null;
      return null;
    });
    await expect(accountStatus()).resolves.toEqual({ state: "signedOut" });
    await accountSignIn();
    await accountSignOut();
    await expect(cloudStatus()).resolves.toBeNull();
    expect(calls).toEqual([
      ["account_status", {}],
      ["account_sign_in", {}],
      ["account_sign_out", {}],
      ["cloud_status", {}],
    ]);
  });
});

describe("model commands", () => {
  it("names the commands and arguments of the backend", async () => {
    const calls: [string, unknown][] = [];
    mockIPC((command, args) => {
      calls.push([command, args]);
      return command === "recommended_models" ? ["gemma4"] : true;
    });
    await expect(recommendedModels()).resolves.toEqual(["gemma4"]);
    await expect(modelExistsUpstream("llama3.2")).resolves.toBe(true);
    expect(calls).toEqual([
      ["recommended_models", {}],
      ["model_exists_upstream", { model: "llama3.2" }],
    ]);
  });

  it("asks which installed models have a newer version in the registry", async () => {
    const calls: [string, unknown][] = [];
    mockIPC((command, args) => {
      calls.push([command, args]);
      return ["qwen3:0.6b"];
    });
    await expect(outdatedModels(["qwen3:0.6b", "gemma4"])).resolves.toEqual([
      "qwen3:0.6b",
    ]);
    expect(calls).toEqual([
      ["outdated_models", { models: ["qwen3:0.6b", "gemma4"] }],
    ]);
  });
});

describe("think commands", () => {
  it("names the commands and arguments of the backend", async () => {
    const calls: [string, unknown][] = [];
    mockIPC((command, args) => {
      calls.push([command, args]);
      return command === "get_think_pref" ? "high" : null;
    });
    await modelDetails("gpt-oss:20b");
    await expect(getThinkPref("gpt-oss:20b")).resolves.toBe("high");
    await setThinkPref("qwen3:0.6b", false);
    expect(calls).toEqual([
      ["model_details", { model: "gpt-oss:20b" }],
      ["get_think_pref", { model: "gpt-oss:20b" }],
      ["set_think_pref", { model: "qwen3:0.6b", think: false }],
    ]);
  });
});

describe("attachment commands", () => {
  const staged = {
    status: "staged",
    file: {
      id: "0199b0c4-9a43-7c1e-8b5d-1f2e3d4c5b6a",
      filename: "notes.txt",
      kind: "text",
      mime: "text/plain",
      size: 5,
    },
  };

  it("names the commands and arguments of the backend", async () => {
    const calls: [string, unknown][] = [];
    mockIPC((command, args) => {
      calls.push([command, args]);
      return command === "stage_paths" ? [staged] : null;
    });
    await expect(stagePaths(["/home/me/notes.txt"])).resolves.toEqual([staged]);
    await unstage([staged.file.id]);
    expect(calls).toEqual([
      ["stage_paths", { paths: ["/home/me/notes.txt"] }],
      ["unstage", { ids: [staged.file.id] }],
    ]);
  });

  it("sends pasted bytes raw, with the name encoded in a header", async () => {
    mockIPC(() => staged);
    const internals = (
      window as unknown as {
        __TAURI_INTERNALS__: { invoke: (...args: unknown[]) => unknown };
      }
    ).__TAURI_INTERNALS__;
    const invoke = vi.spyOn(internals, "invoke");
    const data = new Uint8Array([104, 105]);

    await expect(stageBytes("résumé 1#2.txt", data)).resolves.toEqual(staged);

    expect(invoke).toHaveBeenCalledWith("stage_bytes", data, {
      headers: { "x-filename": "r%C3%A9sum%C3%A9%201%232.txt" },
    });
  });
});

describe("ModelHub commands", () => {
  it("names the 12 commands of commands/hub.rs, arguments in camelCase", async () => {
    const calls: [string, unknown][] = [];
    mockIPC((command, args) => {
      calls.push([command, args]);
      return null;
    });
    const filters = {
      uses: ["code" as const],
      size: null,
      openLicense: true,
      verifiedOnly: false,
    };
    const request = { model: "hf.co/a/b:Q4_K_M", repo: "a/b", label: "Q4_K_M" };
    await hubHome(filters);
    await hubBrowse({ kind: "family", id: "qwen" }, filters);
    await hubSearch({ text: "qwen", filters, cursor: "2" });
    await hubRepo("a/b");
    await hubReadme("a/b", "sha1");
    await hubHardware();
    await hubDownload(request);
    await hubCancelDownload("hf.co/a/b:Q4_K_M");
    await hubPauseDownload("hf.co/a/b:Q4_K_M");
    await hubResumeDownload("hf.co/a/b:Q4_K_M");
    await hubPrioritizeDownload("hf.co/a/b:Q4_K_M");
    await hubDownloads();
    await hubDismissDownload("hf.co/a/b:Q4_K_M");
    await hubInstalled();
    await hubDeleteModel("qwen3:0.6b");
    expect(calls).toEqual([
      ["hub_home", { filters }],
      ["hub_browse", { scope: { kind: "family", id: "qwen" }, filters }],
      ["hub_search", { search: { text: "qwen", filters, cursor: "2" } }],
      ["hub_repo", { id: "a/b" }],
      ["hub_readme", { id: "a/b", revision: "sha1" }],
      ["hub_hardware", {}],
      ["hub_download", { request }],
      ["hub_cancel_download", { model: "hf.co/a/b:Q4_K_M" }],
      ["hub_pause_download", { model: "hf.co/a/b:Q4_K_M" }],
      ["hub_resume_download", { model: "hf.co/a/b:Q4_K_M" }],
      ["hub_prioritize_download", { model: "hf.co/a/b:Q4_K_M" }],
      ["hub_downloads", {}],
      ["hub_dismiss_download", { model: "hf.co/a/b:Q4_K_M" }],
      ["hub_installed", {}],
      ["hub_delete_model", { model: "qwen3:0.6b" }],
    ]);
  });
});

describe("dictation commands", () => {
  it("names the commands of commands/dictation.rs and quit_app, arguments in camelCase", async () => {
    const calls: [string, unknown][] = [];
    mockIPC((command, args) => {
      calls.push([command, args]);
      return null;
    });
    await getDictationSettings();
    await updateDictationSettings({
      enabled: true,
      speechModel: "whisper-tiny",
    });
    await dictationInfo();
    await dictationStatus();
    await dictationToggle();
    await dictationCancel();
    await dictationBindShortcut();
    await dictationConfigureShortcut();
    await dictationCopyLast();
    await speechModels();
    await speechDownloads();
    await speechDownload("whisper-tiny");
    await speechCancelDownload("whisper-tiny");
    await speechDismissDownload("whisper-tiny");
    await speechDeleteModel("whisper-tiny");
    await quitApp();
    expect(calls).toEqual([
      ["get_dictation_settings", {}],
      [
        "update_dictation_settings",
        { patch: { enabled: true, speechModel: "whisper-tiny" } },
      ],
      ["dictation_info", {}],
      ["dictation_status", {}],
      ["dictation_toggle", {}],
      ["dictation_cancel", {}],
      ["dictation_bind_shortcut", {}],
      ["dictation_configure_shortcut", {}],
      ["dictation_copy_last", {}],
      ["speech_models", {}],
      ["speech_downloads", {}],
      ["speech_download", { id: "whisper-tiny" }],
      ["speech_cancel_download", { id: "whisper-tiny" }],
      ["speech_dismiss_download", { id: "whisper-tiny" }],
      ["speech_delete_model", { id: "whisper-tiny" }],
      ["quit_app", {}],
    ]);
  });
});

describe("Easy Link commands", () => {
  const state = { bots: [], accounts: [], pairing: null };

  it("names the seven commands of the backend, arguments in camelCase, each giving the state", async () => {
    const calls: [string, unknown][] = [];
    mockIPC((command, args) => {
      calls.push([command, args]);
      return state;
    });
    await expect(easyLinkState()).resolves.toEqual(state);
    await expect(easyLinkConnect("telegram", "123:abc")).resolves.toEqual(
      state,
    );
    await expect(easyLinkSetEnabled("telegram", false)).resolves.toEqual(state);
    await expect(easyLinkPair("discord")).resolves.toEqual(state);
    await expect(easyLinkCancelPairing()).resolves.toEqual(state);
    await expect(easyLinkUnlink(7)).resolves.toEqual(state);
    await expect(easyLinkDisconnect("discord")).resolves.toEqual(state);
    expect(calls).toEqual([
      ["easy_link_state", {}],
      ["easy_link_connect", { platform: "telegram", token: "123:abc" }],
      ["easy_link_set_enabled", { platform: "telegram", enabled: false }],
      ["easy_link_pair", { platform: "discord" }],
      ["easy_link_cancel_pairing", {}],
      ["easy_link_unlink", { accountId: 7 }],
      ["easy_link_disconnect", { platform: "discord" }],
    ]);
  });

  it("a refused token rejects with the error of the backend", async () => {
    const refused = { kind: "invalid", message: "This token was refused" };
    mockIPC(() => {
      throw refused;
    });
    await expect(easyLinkConnect("telegram", "bad")).rejects.toBe(refused);
  });

  it("listens to “easy-link” and “chats-updated”, giving their payloads", async () => {
    mockIPC(() => null, { shouldMockEvents: true });
    const states: unknown[] = [];
    const chats: unknown[] = [];
    const stopStates = await listenEasyLink((value) => states.push(value));
    const stopChats = await listenChatsUpdated((value) => chats.push(value));
    await emit("easy-link", state);
    await emit("chats-updated", { chatId: "c1" });
    expect(states).toEqual([state]);
    expect(chats).toEqual([{ chatId: "c1" }]);
    // Désinscrit : plus rien n'arrive.
    stopStates();
    stopChats();
    await emit("easy-link", state);
    await emit("chats-updated", { chatId: "c2" });
    expect(states).toHaveLength(1);
    expect(chats).toHaveLength(1);
  });
});
