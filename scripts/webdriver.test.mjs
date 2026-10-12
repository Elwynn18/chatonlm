// @vitest-environment node
// Client WebDriver des tests de l'app réelle (e2e/lib/webdriver.mjs), contre
// un faux serveur du plugin : ouverture de la session quand la fenêtre de
// l'app tarde. Le plugin répond « no such window » après 10 s sans fenêtre ;
// le client refait alors la demande, pour cette seule erreur, pendant un temps
// borné.
import { createServer } from "node:http";
import { afterEach, describe, expect, it, vi } from "vitest";
import { WebDriver } from "../e2e/lib/webdriver.mjs";

/** Faux plugin : `answers(n)` donne la réponse de la n-ième demande de session. */
async function fakePlugin(answers) {
  const calls = { status: 0, session: 0 };
  const server = createServer((request, response) => {
    let status = 200;
    let value = { ready: true };
    if (request.method === "GET" && request.url === "/status") {
      calls.status += 1;
    } else if (request.method === "POST" && request.url === "/session") {
      calls.session += 1;
      [status, value] = answers(calls.session);
    } else {
      [status, value] = [404, { error: "unknown command", message: "" }];
    }
    request.resume();
    response.writeHead(status, { "content-type": "application/json" });
    response.end(JSON.stringify({ value }));
  });
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  return { calls, server, port: server.address().port };
}

const NO_WINDOW = [
  404,
  { error: "no such window", message: "No window could be found" },
];
const SESSION = [200, { sessionId: "s1", capabilities: {} }];

describe("WebDriver.start", () => {
  let plugin;
  afterEach(async () => {
    vi.restoreAllMocks();
    await new Promise((resolve) => plugin.server.close(resolve));
  });

  it("asks again for a session while the app has no window yet", async () => {
    const waits = vi.spyOn(console, "error").mockImplementation(() => {});
    plugin = await fakePlugin((n) => (n < 3 ? NO_WINDOW : SESSION));
    const driver = await new WebDriver(plugin.port).start();
    expect(driver.sessionId).toBe("s1");
    expect(plugin.calls.session).toBe(3);
    // Chaque nouvelle attente est écrite au journal de l'essai.
    expect(waits).toHaveBeenCalledTimes(2);
  });

  it("fails with a clear message when no window ever opens", async () => {
    vi.spyOn(console, "error").mockImplementation(() => {});
    plugin = await fakePlugin(() => NO_WINDOW);
    const started = Date.now();
    await expect(
      new WebDriver(plugin.port).start({ windowTimeoutMs: 600 }),
    ).rejects.toThrow(/L'app n'a ouvert aucune fenêtre .* no such window/);
    // Borné : pas d'attente au-delà du délai (plus une demande).
    expect(Date.now() - started).toBeLessThan(5000);
    expect(plugin.calls.session).toBeGreaterThan(1);
  });

  it("does not retry any other error", async () => {
    const waits = vi.spyOn(console, "error").mockImplementation(() => {});
    plugin = await fakePlugin(() => [
      500,
      { error: "session not created", message: "boom" },
    ]);
    await expect(new WebDriver(plugin.port).start()).rejects.toThrow(
      "POST /session : session not created boom",
    );
    expect(plugin.calls.session).toBe(1);
    expect(waits).not.toHaveBeenCalled();
  });
});
