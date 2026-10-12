import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, cleanup, renderHook } from "@testing-library/react";
import type { Channel } from "@tauri-apps/api/core";
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";
import type { ReactNode } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { Chat } from "@/bindings/Chat";
import type { ChatEvent } from "@/bindings/ChatEvent";
import type { SendRequest } from "@/bindings/SendRequest";
import type { Think } from "@/bindings/Think";
import { chatKey, chatsKey } from "@/hooks/useChats";
import { modelsKey } from "@/hooks/useModels";
import { modelDetailsKey } from "@/hooks/useThink";
import * as streams from "@/lib/streams";
import { useChatError, useSendMessage } from "./useSendMessage";

const navigate = vi.hoisted(() => vi.fn());
vi.mock("@tanstack/react-router", () => ({ useNavigate: () => navigate }));

afterEach(() => {
  cleanup();
  clearMocks();
  streams.resetStreams();
  navigate.mockReset();
});

function setup(
  sendMessage: (onEvent: Channel<ChatEvent>, request: SendRequest) => unknown,
) {
  mockIPC((command, args) => {
    if (command === "send_message") {
      const { onEvent, request } = args as {
        onEvent: Channel<ChatEvent>;
        request: SendRequest;
      };
      return sendMessage(onEvent, request);
    }
    if (command === "list_chats") return [];
    if (command === "get_chat") throw new Error("get_chat not expected");
    throw new Error(`unexpected command ${command}`);
  });
  const client = new QueryClient({
    defaultOptions: { queries: { networkMode: "always", retry: false } },
  });
  const wrapper = ({ children }: { children: ReactNode }) => (
    <QueryClientProvider client={client}>{children}</QueryClientProvider>
  );
  const { result } = renderHook(
    () => ({ ...useSendMessage(), newChatError: useChatError("new") }),
    { wrapper },
  );
  return { client, result };
}

describe("useSendMessage", () => {
  it("fills the new chat's page before opening it", async () => {
    let atNavigation: Chat | undefined;
    const { client, result } = setup((onEvent) => {
      onEvent.onmessage({ type: "chatCreated", chatId: "c1" });
      onEvent.onmessage({ type: "cancelled" });
      return null;
    });
    navigate.mockImplementation(() => {
      atNavigation = client.getQueryData<Chat>(chatKey("c1"));
    });

    await act(() =>
      result.current.send({ chatId: "new", content: "Hi", model: "m" }),
    );

    expect(navigate).toHaveBeenCalledWith({
      to: "/c/$chatId",
      params: { chatId: "c1" },
    });
    // Déjà en cache : la page ne passe pas par « Loading chat... ».
    expect(atNavigation).toMatchObject({ id: "c1", messages: [] });
  });

  it("keeps the error of a new chat refused before its creation", async () => {
    const { result } = setup(() => {
      throw { kind: "database", message: "database is locked" };
    });

    await act(() =>
      result.current.send({ chatId: "new", content: "Hi", model: "m" }),
    );

    expect(navigate).not.toHaveBeenCalled();
    expect(result.current.newChatError).toEqual({
      kind: "internal",
      message: "database is locked",
    });
    expect(streams.pendingNewStream()).toBeNull();
  });

  it.each([
    [true, true],
    [false, false],
    ["high", "high"],
    [undefined, "absent"],
  ] as [Think | undefined, Think | "absent"][])(
    "passes think %s on to the engine (%s)",
    async (think, expected) => {
      const requests: SendRequest[] = [];
      const { result } = setup((onEvent, request) => {
        requests.push(request);
        onEvent.onmessage({ type: "cancelled" });
        return null;
      });

      await act(() =>
        result.current.send({
          chatId: "new",
          content: "Hi",
          model: "m",
          think,
        }),
      );

      expect(requests).toHaveLength(1);
      // Absent, pas undefined : le moteur lit un champ manquant.
      expect("think" in requests[0] ? requests[0].think : "absent").toBe(
        expected,
      );
    },
  );
  it.each([
    [true, true],
    [false, "absent"],
    [undefined, "absent"],
  ] as [boolean | undefined, true | "absent"][])(
    "webSearch %s: %s in the request",
    async (webSearch, expected) => {
      const requests: SendRequest[] = [];
      const { result } = setup((onEvent, request) => {
        requests.push(request);
        onEvent.onmessage({ type: "cancelled" });
        return null;
      });

      await act(() =>
        result.current.send({
          chatId: "new",
          content: "Hi",
          model: "m",
          webSearch,
        }),
      );

      // Absent pour false : les séquences sans recherche web n'en ont pas.
      expect(
        "webSearch" in requests[0] ? requests[0].webSearch : "absent",
      ).toBe(expected);
    },
  );
  it.each([
    [false, "thinking", true],
    [true, "thinking", false],
    [undefined, "thinking", false],
    [false, "content", false],
  ] as [Think | undefined, "thinking" | "content", boolean][])(
    "think %s and a %s fragment: details re-read = %s (model that always thinks)",
    async (think, type, reread) => {
      const { client, result } = setup((onEvent) => {
        onEvent.onmessage({ type: "chatCreated", chatId: "c1" });
        onEvent.onmessage({ type, text: "Hmm." });
        onEvent.onmessage({ type: "done", messageId: 1 });
        return null;
      });
      const invalidated: unknown[] = [];
      vi.spyOn(client, "invalidateQueries").mockImplementation((filters) => {
        invalidated.push(filters?.queryKey);
        return Promise.resolve();
      });

      await act(() =>
        result.current.send({
          chatId: "new",
          content: "Hi",
          model: "lfm",
          think,
        }),
      );

      const details = JSON.stringify(modelDetailsKey("lfm"));
      expect(invalidated.some((key) => JSON.stringify(key) === details)).toBe(
        reread,
      );
    },
  );
  it.each([
    ["webSearchSignedOut", true],
    ["model", false],
  ] as const)("a %s error re-reads the account: %s", async (kind, reread) => {
    const { client, result } = setup((onEvent) => {
      onEvent.onmessage({ type: "chatCreated", chatId: "c1" });
      onEvent.onmessage({ type: "error", kind, message: "x" });
      return null;
    });
    const invalidated: unknown[] = [];
    vi.spyOn(client, "invalidateQueries").mockImplementation((filters) => {
      invalidated.push(filters?.queryKey);
      return Promise.resolve();
    });
    await act(() =>
      result.current.send({ chatId: "new", content: "Hi", model: "m" }),
    );
    expect(
      invalidated.some((key) => JSON.stringify(key) === '["account"]'),
    ).toBe(reread);
  });
  it("re-reads the models and the downloaded model's details once the download is done", async () => {
    const timeline: unknown[] = [];
    const { client, result } = setup((onEvent) => {
      onEvent.onmessage({ type: "chatCreated", chatId: "c1" });
      const progress = { type: "download", total: 4096 } as const;
      onEvent.onmessage({ ...progress, completed: 1024, done: false });
      timeline.push("progress");
      onEvent.onmessage({ ...progress, completed: 4096, done: true });
      timeline.push("done");
      onEvent.onmessage({ type: "cancelled" });
      return null;
    });
    vi.spyOn(client, "invalidateQueries").mockImplementation((filters) => {
      timeline.push(filters?.queryKey);
      return Promise.resolve();
    });

    await act(() =>
      result.current.send({ chatId: "new", content: "Hi", model: "gemma4" }),
    );

    expect(timeline.slice(0, timeline.indexOf("done"))).toEqual([
      chatsKey,
      "progress",
      modelsKey,
      modelDetailsKey("gemma4"),
    ]);
  });
});
