// Faux backend des pièces jointes : sélecteur natif, disque simulé, zone de
// préparation et contrôle des envois, tels que le contrat le décrit
// (.tools/research/25-step11bc-contract.md § 2) ; réglages : validation comme
// le moteur et échecs forcés (28-step13-contract.md § 2.3) ; dictée :
// réglages validés comme le moteur, raccourci lié à l'activation, phases
// (42-step17-contract.md § 4) ; modèles en mémoire : chargement et
// déchargement comme le faux serveur, durée avant déchargement validée
// (43-model-loading-contract.md § 5.6) ; favoris des conversations comme le
// moteur (date d'activité inchangée, conversation absente refusée).

import { Channel, invoke } from "@tauri-apps/api/core";
import { describe, expect, it } from "vitest";
import type { ChatEvent } from "@/bindings/ChatEvent";
import type { DictationPatch } from "@/bindings/DictationPatch";
import type { SendRequest } from "@/bindings/SendRequest";
import type { SettingsPatch } from "@/bindings/SettingsPatch";
import {
  accountSignIn,
  accountSignOut,
  accountStatus,
  cloudStatus,
  dictationBindShortcut,
  dictationConfigureShortcut,
  dictationStatus,
  dictationToggle,
  getChat,
  getSettings,
  listChats,
  loadModel,
  pickFiles,
  runningModels,
  sendMessage,
  setChatFavorite,
  speechDeleteModel,
  speechDownload,
  stageBytes,
  stagePaths,
  unloadAllModels,
  unloadModel,
  unstage,
  updateDictationSettings,
  updateSettings,
} from "@/lib/ipc";
import { fakeBackend, MAX_FILE_SIZE, SERVER_URL } from "@/test/backend";
import { DEFAULT_POLISH_PROMPT, WHISPER_TINY } from "@/test/dictation";
import {
  type Handler,
  installSimulator,
  sequence,
  SIMULATOR_MARKER,
  type Simulator,
} from "@/test/simulator";

/** Requête d'une séquence, avec un identifiant de flux neuf. */
function requestOf(name: string): SendRequest {
  return { ...sequence(name).request, streamId: crypto.randomUUID() };
}

const bytes = (...values: number[]) => new Uint8Array(values);

describe("attachments in the fake backend", () => {
  it("stage_paths stages each readable file in order, with the defaults of its name", async () => {
    const simulator = installSimulator();
    const backend = fakeBackend(simulator, {
      files: {
        "/home/u/notes.txt": {},
        "/home/u/Photo.JPG": { size: 2048 },
        "/home/u/b.jpeg": {},
        "/home/u/c.png": {},
        "/home/u/pic.webp": {},
        "/home/u/scan.pdf": { size: 10 },
        "/home/u/data.json": {},
        "/home/u/raw.png": { kind: "text", mime: "image/x-raw", size: 5 },
        "/home/u/big.log": { error: "File too large" },
      },
    });

    const results = await stagePaths([
      "/home/u/notes.txt",
      "/home/u/missing.md",
      "/home/u/Photo.JPG",
      "/home/u/big.log",
      "/home/u/b.jpeg",
      "/home/u/c.png",
      "/home/u/pic.webp",
      "/home/u/scan.pdf",
      "/home/u/data.json",
      "/home/u/raw.png",
    ]);

    const staged = (
      id: number,
      filename: string,
      kind: string,
      mime: string,
      size = 1,
    ) => ({
      status: "staged",
      file: { id: `staged-${id}`, filename, kind, mime, size },
    });
    expect(results).toEqual([
      staged(1, "notes.txt", "text", "text/plain"),
      {
        status: "rejected",
        filename: "missing.md",
        error: "Error reading file",
      },
      staged(2, "Photo.JPG", "image", "image/jpeg", 2048),
      { status: "rejected", filename: "big.log", error: "File too large" },
      staged(3, "b.jpeg", "image", "image/jpeg"),
      staged(4, "c.png", "image", "image/png"),
      staged(5, "pic.webp", "image", "image/webp"),
      staged(6, "scan.pdf", "text", "application/pdf", 10),
      staged(7, "data.json", "text", "text/plain"),
      staged(8, "raw.png", "text", "image/x-raw", 5),
    ]);
    expect([...backend.staged.values()]).toEqual(
      results.flatMap((r) => (r.status === "staged" ? [r.file] : [])),
    );
  });

  it("identifiers are never reused, whatever the command", async () => {
    const simulator = installSimulator();
    const backend = fakeBackend(simulator, { files: { "/a.txt": {} } });

    await stagePaths(["/a.txt"]);
    await unstage(["staged-1"]);
    expect(backend.staged.size).toBe(0);
    expect(await stageBytes("b.txt", bytes(1))).toMatchObject({
      file: { id: "staged-2" },
    });
    expect(await stagePaths(["/a.txt"])).toMatchObject([
      { file: { id: "staged-3" } },
    ]);
    expect([...backend.staged.keys()]).toEqual(["staged-2", "staged-3"]);

    // Identifiant inconnu : sans effet.
    await unstage(["staged-404", "staged-2"]);
    expect([...backend.staged.keys()]).toEqual(["staged-3"]);
    expect(simulator.callsTo("unstage")).toEqual([
      { ids: ["staged-1"] },
      { ids: ["staged-404", "staged-2"] },
    ]);
  });

  it("pick_files answers the queued picks, then a cancellation", async () => {
    const simulator = installSimulator();
    const backend = fakeBackend(simulator);
    backend.picks.push(["/a.txt", "/b.png"], ["/c.md"]);

    expect(await pickFiles()).toEqual(["/a.txt", "/b.png"]);
    expect(await pickFiles()).toEqual(["/c.md"]);
    expect(await pickFiles()).toEqual([]);
    expect(backend.picks).toEqual([]);
  });

  it("stage_bytes checks the decoded name's extension, then the declared or received size", async () => {
    const simulator = installSimulator();
    const backend = fakeBackend(simulator);
    const name = "résumé (v2)#1.MD";

    // Taille du fichier préparé : celle des octets reçus.
    expect(await stageBytes(name, bytes(35, 32, 104), 999)).toEqual({
      status: "staged",
      file: {
        id: "staged-1",
        filename: name,
        kind: "text",
        mime: "text/plain",
        size: 3,
      },
    });
    expect(await stageBytes("photo.webp", bytes(1, 2))).toEqual({
      status: "staged",
      file: {
        id: "staged-2",
        filename: "photo.webp",
        kind: "image",
        mime: "image/webp",
        size: 2,
      },
    });
    for (const refused of [
      "report.docx",
      "README",
      "archive.tar.gz",
      "photo.png.exe",
      "",
    ]) {
      expect(await stageBytes(refused, bytes(1))).toEqual({
        status: "rejected",
        filename: refused,
        error: "File type not supported",
      });
    }
    // Extension d'abord ; taille déclarée trop grande : corps vide accepté.
    expect(await stageBytes("huge.docx", bytes(), MAX_FILE_SIZE + 1)).toEqual({
      status: "rejected",
      filename: "huge.docx",
      error: "File type not supported",
    });
    expect(await stageBytes("huge.png", bytes(), MAX_FILE_SIZE + 1)).toEqual({
      status: "rejected",
      filename: "huge.png",
      error: "File too large",
    });
    expect(await stageBytes("edge.png", bytes(1), MAX_FILE_SIZE)).toMatchObject(
      { status: "staged", file: { id: "staged-3", size: 1 } },
    );
    expect([...backend.staged.keys()]).toEqual([
      "staged-1",
      "staged-2",
      "staged-3",
    ]);
  });

  it("stage_bytes refuses received bytes over 10 MiB", () => {
    // Gestionnaire appelé directement : un tableau creux de 10 Mio d'octets,
    // sans les copies de l'IPC simulé.
    const handlers = new Map<string, Handler>();
    fakeBackend({
      handle: (command: string, handler: Handler) =>
        handlers.set(command, handler),
      onEvent: () => {},
      onSend: () => {},
    } as unknown as Simulator);
    const stage = handlers.get("stage_bytes")!;
    const headers = { "x-filename": "big.txt" };

    expect(stage({ bytes: new Array(MAX_FILE_SIZE + 1), headers })).toEqual({
      status: "rejected",
      filename: "big.txt",
      error: "File too large",
    });
    expect(stage({ bytes: new Array(MAX_FILE_SIZE), headers })).toMatchObject({
      status: "staged",
      file: { size: MAX_FILE_SIZE },
    });
  });

  it("stage_bytes rejects an unreadable request like the engine", async () => {
    const simulator = installSimulator();
    const backend = fakeBackend(simulator);
    const invalid = (message: string) => ({ kind: "invalid", message });

    await expect(
      invoke(
        "stage_bytes",
        { name: "a.txt" },
        { headers: { "x-filename": "a.txt" } },
      ),
    ).rejects.toEqual(invalid("the file must be sent as raw bytes"));
    await expect(invoke("stage_bytes", bytes(1))).rejects.toEqual(
      invalid("missing x-filename header"),
    );
    await expect(
      invoke("stage_bytes", bytes(1), {
        headers: { "x-filename": "%E0%A4%A" },
      }),
    ).rejects.toEqual(invalid("invalid x-filename header"));
    await expect(
      invoke("stage_bytes", bytes(1), {
        headers: { "x-filename": "a.txt", "x-size": "12kB" },
      }),
    ).rejects.toEqual(invalid("invalid x-size header"));
    expect(backend.staged.size).toBe(0);
  });

  it("a send with a file that is not staged fails the test, unless the sequence is refused", async () => {
    const simulator = installSimulator();
    fakeBackend(simulator, { files: { "/home/u/notes.txt": {} } });
    await stagePaths(["/home/u/notes.txt"]);

    simulator.queue("attachments");
    await expect(
      sendMessage(requestOf("attachments"), new Channel<ChatEvent>()),
    ).rejects.toThrow(
      `[${SIMULATOR_MARKER}] send_message for "attachments": staged-2 not staged (staged: staged-1)`,
    );
    expect(simulator.problems.splice(0)).toHaveLength(1);

    // Fichier retiré entre-temps : le moteur refuse lui-même l'envoi.
    simulator.queue("refused-unknown-attachment");
    await expect(
      sendMessage(
        requestOf("refused-unknown-attachment"),
        new Channel<ChatEvent>(),
      ),
    ).rejects.toEqual({
      kind: "invalid",
      message: "unknown attachment: staged-unknown",
    });
    expect(simulator.problems).toEqual([]);
  });

  it("the saved user message takes its files out of the staging area", async () => {
    const simulator = installSimulator();
    const backend = fakeBackend(simulator, {
      files: {
        "/home/u/notes.txt": { size: 13 },
        "/home/u/red.png": { size: 120 },
        "/home/u/other.txt": {},
      },
    });
    await stagePaths([
      "/home/u/notes.txt",
      "/home/u/red.png",
      "/home/u/other.txt",
    ]);
    const replay = simulator.queue("attachments", { stepwise: true });
    const sending = sendMessage(
      requestOf("attachments"),
      new Channel<ChatEvent>(),
    );

    await replay.started;
    await replay.until("chatCreated");
    expect([...backend.staged.keys()]).toEqual([
      "staged-1",
      "staged-2",
      "staged-3",
    ]);
    await replay.until("userMessage");
    expect([...backend.staged.keys()]).toEqual(["staged-3"]);
    await replay.finish();
    await sending;

    const saved = sequence("attachments").events.find(
      (event) => event.type === "userMessage",
    );
    expect(backend.chats.get("chat-1")?.messages[0].attachments).toEqual(
      saved?.type === "userMessage" && saved.message.attachments,
    );
  });
});

describe("settings in the fake backend", () => {
  const invalid = (message: string) => ({ kind: "invalid", message });

  it("update_settings answers a new object and recomputes the effective address", async () => {
    const simulator = installSimulator();
    const backend = fakeBackend(simulator);
    const before = backend.settings;

    const updated = await updateSettings({
      serverUrl: "  http://10.0.0.2:11434  ",
      contextLength: 8192,
      theme: "dark",
    });
    expect(updated).toEqual({
      ...before,
      serverUrl: "http://10.0.0.2:11434",
      effectiveServerUrl: "http://10.0.0.2:11434",
      contextLength: 8192,
      theme: "dark",
    });
    // Nouvel objet : l'ancien est intact.
    expect(backend.settings).not.toBe(before);
    expect(before.serverUrl).toBe("");
    expect(await getSettings()).toEqual(updated);

    // Adresse vide : automatique.
    expect(await updateSettings({ serverUrl: " " })).toMatchObject({
      serverUrl: "",
      effectiveServerUrl: SERVER_URL,
    });
    expect(
      await updateSettings({ serverUrl: "localhost:8080/ollama" }),
    ).toMatchObject({
      serverUrl: "localhost:8080/ollama",
      effectiveServerUrl: "localhost:8080/ollama",
    });
    expect(await updateSettings({ serverUrl: "[::1]:11434" })).toMatchObject({
      serverUrl: "[::1]:11434",
    });
  });

  it("update_settings accepts every context length step", async () => {
    const simulator = installSimulator();
    fakeBackend(simulator);
    for (const contextLength of [
      0, 4096, 8192, 16384, 32768, 65536, 131072, 262144,
    ]) {
      expect(await updateSettings({ contextLength })).toMatchObject({
        contextLength,
      });
    }
  });

  it("starts with the server's keep alive and no preloading, and accepts every keep-alive step", async () => {
    const simulator = installSimulator();
    fakeBackend(simulator);
    expect(await getSettings()).toMatchObject({
      keepAlive: 0,
      preloadModel: false,
    });
    for (const keepAlive of [0, 60, 300, 900, 1800, 3600, 14400, -1]) {
      expect(await updateSettings({ keepAlive })).toMatchObject({ keepAlive });
    }
    expect(await updateSettings({ preloadModel: true })).toMatchObject({
      preloadModel: true,
    });
  });

  it("update_settings refuses what the engine refuses, without writing anything", async () => {
    const simulator = installSimulator();
    const backend = fakeBackend(simulator);
    const before = backend.settings;
    const refusals: [SettingsPatch, string][] = [
      [{ contextLength: 5000 }, "unsupported context length: 5000"],
      [{ contextLength: 524288 }, "unsupported context length: 524288"],
      [{ theme: "sepia" as SettingsPatch["theme"] }, "unknown theme: sepia"],
      [{ keepAlive: 120 }, "unsupported keep alive: 120"],
      [{ keepAlive: -2 }, "unsupported keep alive: -2"],
      [{ serverUrl: "not a url" }, "invalid server address: not a url"],
      [{ serverUrl: "http://" }, "invalid server address: http://"],
      [
        { serverUrl: "ftp://example.com" },
        "invalid server address: unsupported scheme ftp",
      ],
      [
        { serverUrl: "http://user:secret@example.com" },
        "invalid server address: credentials are not supported",
      ],
      [
        { serverUrl: "http://example.com/?a=1" },
        "invalid server address: query strings are not supported",
      ],
      // Atomique : une valeur refusée, rien d'écrit.
      [{ theme: "dark", contextLength: 7 }, "unsupported context length: 7"],
      [
        { contextLength: 8192, serverUrl: "ftp://x" },
        "invalid server address: unsupported scheme ftp",
      ],
    ];
    for (const [patch, message] of refusals) {
      await expect(updateSettings(patch)).rejects.toEqual(invalid(message));
    }
    expect(backend.settings).toBe(before);
    expect(await getSettings()).toEqual(before);
  });

  it("settingsErrors fails the commands while it is set", async () => {
    const simulator = installSimulator();
    const backend = fakeBackend(simulator);
    expect(backend.settingsErrors).toEqual({ get: null, update: null });
    const broken = { kind: "database" as const, message: "disk I/O error" };

    backend.settingsErrors.get = broken;
    await expect(getSettings()).rejects.toEqual(broken);
    await expect(getSettings()).rejects.toEqual(broken);
    // Seule la lecture échoue.
    expect(await updateSettings({ theme: "light" })).toMatchObject({
      theme: "light",
    });
    backend.settingsErrors.get = null;
    expect(await getSettings()).toMatchObject({ theme: "light" });

    backend.settingsErrors.update = broken;
    await expect(updateSettings({ theme: "dark" })).rejects.toEqual(broken);
    await expect(updateSettings({ theme: "dark" })).rejects.toEqual(broken);
    expect(await getSettings()).toMatchObject({ theme: "light" });
    backend.settingsErrors.update = null;
    expect(await updateSettings({ theme: "dark" })).toMatchObject({
      theme: "dark",
    });
  });
});

describe("models in memory in the fake backend", () => {
  it("loads an installed model with the settings, unloads one or all", async () => {
    const simulator = installSimulator();
    const backend = fakeBackend(simulator, {
      models: ["qwen3:0.6b", "smollm2", "gemma3:270m"],
    });
    expect(await runningModels()).toEqual([]);
    const before = Date.now();
    await loadModel("qwen3:0.6b");
    let [loaded] = await runningModels();
    expect(loaded).toMatchObject({
      name: "qwen3:0.6b",
      size: 1024 ** 3,
      sizeVram: 1024 ** 3,
      contextLength: 4096,
      kept: false,
    });
    // Défaut du serveur : 5 min.
    expect(loaded.expiresAt).toBeGreaterThanOrEqual(before + 300_000);
    expect(loaded.expiresAt).toBeLessThanOrEqual(Date.now() + 300_000);

    // Réglages de l'app : contexte et durée ; « :latest » comme /api/ps.
    Object.assign(backend.settings, { contextLength: 8192, keepAlive: -1 });
    await loadModel("smollm2");
    [, loaded] = await runningModels();
    expect(loaded).toMatchObject({
      name: "smollm2:latest",
      contextLength: 8192,
      expiresAt: null,
      kept: true,
    });
    // Rechargé : une seule ligne, durée et contexte nouveaux.
    backend.settings.keepAlive = 60;
    await loadModel("qwen3:0.6b");
    const names = (await runningModels()).map((m) => m.name);
    expect(names).toEqual(["smollm2:latest", "qwen3:0.6b"]);

    await unloadModel("smollm2");
    expect((await runningModels()).map((m) => m.name)).toEqual(["qwen3:0.6b"]);
    // Pas en mémoire : sans effet.
    await unloadModel("gemma3:270m");
    await loadModel("gemma3:270m");
    await unloadAllModels();
    expect(await runningModels()).toEqual([]);
  });

  it("refuses an unknown model like Ollama, and fails on demand", async () => {
    const simulator = installSimulator();
    const backend = fakeBackend(simulator);
    await expect(loadModel("nope:1b")).rejects.toEqual({
      kind: "notFound",
      message: "model 'nope:1b' not found",
    });
    const memory = { kind: "ollama" as const, message: "out of memory" };
    backend.runningErrors.load.set("qwen3:0.6b", memory);
    await expect(loadModel("qwen3:0.6b")).rejects.toEqual(memory);
    await loadModel("qwen3.5:0.8b");
    const down = { kind: "ollama" as const, message: "down" };
    backend.runningErrors.list = down;
    await expect(runningModels()).rejects.toEqual(down);
    backend.runningErrors.unload = down;
    await expect(unloadModel("qwen3.5:0.8b")).rejects.toEqual(down);
    await expect(unloadAllModels()).rejects.toEqual(down);
    expect(backend.running.map((m) => m.name)).toEqual(["qwen3.5:0.8b"]);
  });
});

describe("account and web search in the fake backend", () => {
  it("answers the account and the cloud status, signs out, fails on demand", async () => {
    const simulator = installSimulator();
    const backend = fakeBackend(simulator);
    await expect(accountStatus()).resolves.toEqual({ state: "signedOut" });
    await expect(cloudStatus()).resolves.toEqual({
      disabled: false,
      source: "none",
    });
    backend.account = {
      state: "signedIn",
      name: "ada",
      email: "ada@example.com",
      plan: "free",
      avatar: null,
    };
    await accountSignIn();
    await expect(accountStatus()).resolves.toMatchObject({ name: "ada" });
    await accountSignOut();
    await expect(accountStatus()).resolves.toEqual({ state: "signedOut" });

    const broken = { kind: "ollama" as const, message: "down" };
    backend.accountErrors = { status: broken, signIn: broken, signOut: broken };
    await expect(accountStatus()).rejects.toEqual(broken);
    await expect(accountSignIn()).rejects.toEqual(broken);
    await expect(accountSignOut()).rejects.toEqual(broken);
    backend.cloud = null;
    await expect(cloudStatus()).resolves.toBeNull();
  });

  it("saves the tool calls and their results of a recorded search", async () => {
    const simulator = installSimulator();
    const backend = fakeBackend(simulator);
    backend.record(sequence("web-search"));
    const chat = await getChat("chat-1");
    expect(chat.messages.map((m) => [m.id, m.role])).toEqual([
      [1, "user"],
      [2, "assistant"],
      [3, "assistant"],
    ]);
    const [call] = chat.messages[1].toolCalls;
    expect(call.name).toBe("web_search");
    expect(call.result).toContain('"results"');
    expect(call.error).toBeNull();
    // Raisonnement du tour d'outils clos, réponse dans le tour suivant.
    expect(chat.messages[1].thinkingEndedAt).not.toBeNull();
    expect(chat.messages[2].content).toMatch(/^Found: /);
    expect(chat.messages[2].toolCalls).toEqual([]);
  });
});

describe("chats in the fake backend", () => {
  it("set_chat_favorite marks a chat without touching its activity date", async () => {
    const simulator = installSimulator();
    const backend = fakeBackend(simulator);
    backend.record(sequence("web-search"));
    const [before] = await listChats();
    expect(before.favorite).toBe(false);

    await setChatFavorite("chat-1", true);
    const [after] = await listChats();
    expect(after).toEqual({ ...before, favorite: true });
    // get_chat ne le donne pas, comme le moteur.
    expect(Object.keys(await getChat("chat-1"))).not.toContain("favorite");

    await setChatFavorite("chat-1", false);
    expect((await listChats())[0]).toEqual(before);
    await expect(setChatFavorite("missing", true)).rejects.toEqual({
      kind: "notFound",
      message: "chat not found: missing",
    });
  });
});

describe("dictation in the fake backend", () => {
  const invalid = (message: string) => ({ kind: "invalid", message });

  it("update_dictation_settings cleans like the engine and refuses what it refuses", async () => {
    const simulator = installSimulator();
    const backend = fakeBackend(simulator);
    const updated = await updateDictationSettings({
      dictionary: [" ChatonLM ", "", "chatonlm", "Ollama"],
      polishPrompt: `  ${DEFAULT_POLISH_PROMPT}\n`,
      language: "fr",
    });
    expect(updated).toMatchObject({
      dictionary: ["ChatonLM", "Ollama"],
      polishPrompt: "",
      language: "fr",
    });
    for (const [patch, message] of [
      [{ speechModel: "whisper-huge" }, "unknown speech model: whisper-huge"],
      [{ language: "xx" }, "unsupported language: xx"],
      [{ polishModel: " gemma3:4b" }, "invalid model name:  gemma3:4b"],
      [
        { polishPrompt: "x".repeat(4001) },
        "the instructions hold at most 4000 characters",
      ],
      [
        { dictionary: ["a".repeat(65)] },
        `invalid dictionary word: ${"a".repeat(65)}`,
      ],
    ] satisfies [DictationPatch, string][]) {
      await expect(updateDictationSettings(patch)).rejects.toEqual(
        invalid(message),
      );
    }
    // Rien d'écrit par les correctifs refusés.
    expect(backend.dictation.settings).toEqual(updated);
  });

  it("turning dictation on starts binding the shortcut; toggle refused like the engine", async () => {
    const simulator = installSimulator();
    const backend = fakeBackend(simulator);
    await dictationToggle();
    expect((await dictationStatus()).lastResult?.error).toBe(
      "Dictation is turned off in Settings",
    );
    await updateDictationSettings({ enabled: true });
    // Liaison en tâche de fond : Binding, boîte du bureau pas encore là.
    expect(await dictationStatus()).toMatchObject({
      shortcut: { state: "binding" },
      canConfigureShortcut: false,
    });
    await backend.dictation.shortcutBound();
    expect(await dictationStatus()).toMatchObject({
      shortcut: { state: "bound", trigger: "Ctrl+Alt+D" },
      canConfigureShortcut: true,
    });
    await dictationToggle();
    expect((await dictationStatus()).lastResult?.error).toBe(
      "Choose a speech model in Settings",
    );
    await updateDictationSettings({ speechModel: WHISPER_TINY });
    // Déjà activée : pas de nouvelle liaison.
    expect((await dictationStatus()).shortcut.state).toBe("bound");
    await dictationToggle();
    const refused = await dictationStatus();
    expect(refused.phase).toBe("idle");
    expect(refused.lastResult).toMatchObject({
      text: "",
      polished: false,
      error: "Speech model not installed",
    });
    backend.dictation.install(WHISPER_TINY);
    await dictationToggle();
    expect((await dictationStatus()).phase).toBe("recording");
    await dictationToggle();
    expect((await dictationStatus()).phase).toBe("transcribing");
    // Ignorée pendant la transcription.
    await dictationToggle();
    expect((await dictationStatus()).phase).toBe("transcribing");
    await updateDictationSettings({ enabled: false });
    expect((await dictationStatus()).shortcut).toEqual({ state: "disabled" });
    await expect(dictationBindShortcut()).rejects.toEqual(
      invalid("Turn on dictation first"),
    );
    await expect(dictationConfigureShortcut()).rejects.toEqual(
      invalid("The shortcut is not set yet"),
    );
  });

  it("refuses to delete a model being downloaded, with the engine's words", async () => {
    const simulator = installSimulator();
    fakeBackend(simulator);
    await speechDownload(WHISPER_TINY);
    await expect(speechDeleteModel(WHISPER_TINY)).rejects.toEqual(
      invalid("This model is downloading: cancel the download first"),
    );
    await expect(speechDownload("whisper-huge")).rejects.toEqual(
      invalid("unknown speech model: whisper-huge"),
    );
  });
});
