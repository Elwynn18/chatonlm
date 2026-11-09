import { describe, expect, it } from "vitest";
import { isCloudError, upgradeUrl } from "./cloud";

describe("cloud errors", () => {
  it("are the three kinds of the cloud", () => {
    expect(isCloudError("remoteOff")).toBe(true);
    expect(isCloudError("remoteKey")).toBe(true);
    expect(isCloudError("remoteLimit")).toBe(true);
    expect(isCloudError("webSearchUnavailable")).toBe(false);
    expect(isCloudError("model")).toBe(false);
    expect(isCloudError(undefined)).toBe(false);
  });

  it("upgrade on ollama.com, the billing page for a Pro account", () => {
    expect(upgradeUrl("pro")).toBe("https://ollama.com/settings/billing");
    expect(upgradeUrl("free")).toBe("https://ollama.com/upgrade");
    expect(upgradeUrl(undefined)).toBe("https://ollama.com/upgrade");
  });
});
