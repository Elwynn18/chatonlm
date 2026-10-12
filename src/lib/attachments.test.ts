import { clearMocks, mockConvertFileSrc } from "@tauri-apps/api/mocks";
import { afterEach, describe, expect, it } from "vitest";
import {
  attachmentImageUrl,
  basename,
  isLocalPath,
  stagedImageUrl,
} from "./attachments";
import { setPlatform } from "./platform";

afterEach(clearMocks);

const ID = "0b0e7a52-6f0c-4c56-9d53-1f2a3b4c5d6e";

describe("image addresses", () => {
  it("use the chatonlm:// protocol on Linux", () => {
    mockConvertFileSrc("linux");
    expect(stagedImageUrl(ID)).toBe(`chatonlm://localhost/staged/${ID}`);
    expect(attachmentImageUrl(8)).toBe("chatonlm://localhost/attachment/8");
  });

  it("use http://chatonlm.localhost on Windows, with the same path (not encoded)", () => {
    mockConvertFileSrc("windows");
    expect(stagedImageUrl(ID)).toBe(`http://chatonlm.localhost/staged/${ID}`);
    expect(attachmentImageUrl(8)).toBe(
      "http://chatonlm.localhost/attachment/8",
    );
  });
});

describe("basename", () => {
  it("cuts at slashes on Linux, where a backslash is part of a name", () => {
    expect(basename("/home/user/notes.txt")).toBe("notes.txt");
    expect(basename("/home/user/dir/")).toBe("dir");
    expect(basename("/home/user/a\\b.txt")).toBe("a\\b.txt");
  });

  it("cuts at backslashes and slashes on Windows", () => {
    setPlatform("windows");
    expect(basename("C:\\Users\\me\\My photo.png")).toBe("My photo.png");
    expect(basename("C:\\Users\\me\\dir\\")).toBe("dir");
    expect(basename("\\\\server\\share\\report.pdf")).toBe("report.pdf");
    expect(basename("C:/Users/me/notes.txt")).toBe("notes.txt");
  });
});

describe("isLocalPath", () => {
  it("is an absolute path on Linux", () => {
    expect(isLocalPath("/home/user/notes.txt")).toBe(true);
    expect(isLocalPath("https://example.com/a.png")).toBe(false);
    expect(isLocalPath("C:\\Users\\me\\notes.txt")).toBe(false);
  });

  it("is a drive or a network share on Windows", () => {
    setPlatform("windows");
    expect(isLocalPath("C:\\Users\\me\\notes.txt")).toBe(true);
    expect(isLocalPath("d:/models/a.gguf")).toBe(true);
    expect(isLocalPath("\\\\server\\share\\report.pdf")).toBe(true);
    expect(isLocalPath("https://example.com/a.png")).toBe(false);
    expect(isLocalPath("/home/user/notes.txt")).toBe(false);
  });
});
