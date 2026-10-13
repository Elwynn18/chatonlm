import { describe, expect, it } from "vitest";
import type { ChatInfo } from "@/bindings/ChatInfo";
import { translator } from "@/i18n";
import { chatLabel, groupChats, sortChats } from "./chatGroups";

const HOUR = 60 * 60 * 1000;
const DAY = 24 * HOUR;
const now = new Date(2026, 9, 3, 15, 0, 0);

function chat(id: string, ageMs: number, extra: Partial<ChatInfo> = {}) {
  const at = now.getTime() - ageMs;
  return {
    id,
    title: "",
    userExcerpt: `question ${id}`,
    createdAt: at,
    updatedAt: at,
    favorite: false,
    source: null,
    ...extra,
  } satisfies ChatInfo;
}

describe("chat history", () => {
  it("groups by Today, This week and Older, without empty groups", () => {
    const groups = groupChats(
      [
        chat("a", 1 * HOUR),
        chat("b", 2 * DAY),
        chat("c", 40 * DAY),
        chat("d", 6 * DAY),
      ],
      now,
    );
    expect(groups.map((g) => [g.id, g.chats.map((c) => c.id)])).toEqual([
      ["today", ["a"]],
      ["thisWeek", ["b", "d"]],
      ["older", ["c"]],
    ]);
    expect(groupChats([chat("a", 1 * HOUR)], now).map((g) => g.id)).toEqual([
      "today",
    ]);
  });

  it("puts favorites first, only there, sorted by activity", () => {
    const groups = groupChats(
      [
        chat("a", 1 * HOUR),
        chat("old-favorite", 40 * DAY, { favorite: true }),
        chat("b", 2 * DAY),
        chat("new-favorite", 2 * HOUR, { favorite: true }),
      ],
      now,
    );
    expect(groups.map((g) => [g.id, g.chats.map((c) => c.id)])).toEqual([
      ["favorites", ["new-favorite", "old-favorite"]],
      ["today", ["a"]],
      ["thisWeek", ["b"]],
    ]);
    // Sans favorite, pas de rubrique vide.
    expect(groupChats([chat("a", 1 * HOUR)], now)[0].id).toBe("today");
  });

  it("yesterday belongs to This week (no Yesterday group)", () => {
    expect(groupChats([chat("y", 20 * HOUR)], now)[0].id).toBe("thisWeek");
  });

  it("sorts by activity, then by id", () => {
    const same = chat("x", 3 * HOUR);
    const sorted = sortChats([
      same,
      { ...same, id: "z" },
      chat("new", 1 * HOUR),
    ]);
    expect(sorted.map((c) => c.id)).toEqual(["new", "z", "x"]);
  });

  it("shows the title, else the first message", () => {
    const en = translator("en");
    expect(chatLabel(en, chat("a", 0, { title: "Trip ideas" }))).toBe(
      "Trip ideas",
    );
    expect(chatLabel(en, chat("a", 0))).toBe("question a");
  });

  it("else the creation date, as the official toLocaleString in the language", () => {
    const untitled = chat("a", 0, { userExcerpt: "" });
    expect(chatLabel(translator("en"), untitled)).toBe("10/3/2026, 3:00:00 PM");
    expect(chatLabel(translator("fr"), untitled)).toBe("03/10/2026 15:00:00");
    expect(chatLabel(translator("es"), untitled)).toBe("3/10/2026, 15:00:00");
  });
});
