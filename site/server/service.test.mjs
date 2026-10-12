// @vitest-environment node
// Essais du service du site (service.mjs) : validation du formulaire, garde
// contre l'injection d'en-têtes, plafonds d'envoi, dialogue SMTP contre un faux
// serveur, routes HTTP (contact, version, téléchargements).
import { createServer as createTcpServer } from "node:net";
import { afterEach, describe, expect, it } from "vitest";
import {
  LIMITS,
  buildMessage,
  configFromEnv,
  createLimiter,
  visitorKey,
  createReleaseSource,
  createService,
  encodeHeader,
  pickAssets,
  sendMail,
  validateContact,
} from "./service.mjs";

const closers = [];
afterEach(async () => {
  await Promise.all(closers.splice(0).map((close) => close()));
});

function listen(server) {
  return new Promise((resolve) => {
    server.listen(0, "127.0.0.1", () => {
      closers.push(() => new Promise((done) => server.close(done)));
      resolve(server.address().port);
    });
  });
}

const VALID = {
  name: "Élodie Martin",
  email: "elodie@example.org",
  message: "Bonjour, une question sur la dictée.",
  lang: "fr",
  elapsed: 9000,
};

/** Décode les mots encodés (RFC 2047) d'un en-tête replié. */
function decodeHeader(value) {
  return value
    .split(/\r\n /)
    .map((word) =>
      Buffer.from(/^=\?UTF-8\?B\?(.*)\?=$/.exec(word)[1], "base64").toString(),
    )
    .join("");
}

describe("validateContact", () => {
  it("accepte un message correct et nettoie les champs", () => {
    const result = validateContact({
      ...VALID,
      name: "  Élodie\t Martin ",
      message: "  Bonjour à tous.\r\nMerci.  ",
    });
    expect(result).toEqual({
      ok: true,
      value: {
        name: "Élodie Martin",
        email: VALID.email,
        message: "Bonjour à tous.\nMerci.",
        lang: "fr",
      },
    });
  });

  it("refuse un nom vide ou trop long", () => {
    expect(validateContact({ ...VALID, name: " \n " })).toEqual({
      ok: false,
      error: "name",
    });
    expect(
      validateContact({ ...VALID, name: "a".repeat(LIMITS.name + 1) }).error,
    ).toBe("name");
  });

  it("refuse une adresse qui ouvrirait un autre en-tête", () => {
    for (const email of [
      "a@b.fr\r\nBcc: victime@example.org",
      "a@b.fr\nSubject: x",
      "a b@c.fr",
      "<a@b.fr>",
      "a@b",
      "sans-arobase.fr",
      "élodie@example.org",
      `${"a".repeat(250)}@b.fr`,
    ]) {
      expect(validateContact({ ...VALID, email }), email).toEqual({
        ok: false,
        error: "email",
      });
    }
  });

  it("refuse un message trop court ou trop long", () => {
    expect(validateContact({ ...VALID, message: "Salut" }).error).toBe(
      "message",
    );
    expect(
      validateContact({ ...VALID, message: "a".repeat(LIMITS.messageMax + 1) })
        .error,
    ).toBe("message");
    expect(validateContact({ ...VALID, message: undefined }).error).toBe(
      "message",
    );
  });

  it("prend l'anglais pour une langue inconnue", () => {
    expect(validateContact({ ...VALID, lang: "de" }).value.lang).toBe("en");
  });
});

describe("createLimiter", () => {
  it("plafonne par adresse et par heure, puis rouvre", () => {
    let now = Date.UTC(2026, 9, 11, 10);
    const allow = createLimiter(
      { perIp: 2, perIpWindow: 3600_000, perDay: 100 },
      () => now,
    );
    expect([allow("a"), allow("a"), allow("a")]).toEqual([true, true, false]);
    expect(allow("b")).toBe(true);
    now += 3600_000;
    expect(allow("a")).toBe(true);
  });

  it("plafonne tout le site par jour", () => {
    let now = Date.UTC(2026, 9, 11, 10);
    const allow = createLimiter(
      { perIp: 10, perIpWindow: 3600_000, perDay: 2 },
      () => now,
    );
    expect([allow("a"), allow("b"), allow("c")]).toEqual([true, true, false]);
    now += 24 * 3600_000;
    expect(allow("c")).toBe(true);
  });
});

describe("visitorKey", () => {
  it("garde une adresse IPv4 et réduit une adresse IPv6 à son réseau /64", () => {
    expect(visitorKey("203.0.113.7")).toBe("203.0.113.7");
    expect(visitorKey("::ffff:203.0.113.7")).toBe("203.0.113.7");
    expect(visitorKey("2001:db8:1:2:aaaa:bbbb:cccc:dddd")).toBe(
      "2001:db8:1:2::/64",
    );
    expect(visitorKey("2001:0DB8:0001:0002::1")).toBe("2001:db8:1:2::/64");
    expect(visitorKey("2001:db8::1")).toBe("2001:db8:0:0::/64");
    expect(visitorKey("2001:db8:1:3::1")).not.toBe(
      visitorKey("2001:db8:1:2::1"),
    );
  });
});

describe("createLimiter, par visiteur", () => {
  const HOUR = 3600_000;
  it("plafonne un visiteur par jour, tout un /64 ensemble", () => {
    let now = Date.UTC(2026, 9, 11, 0);
    const allow = createLimiter(
      { perIp: 2, perIpWindow: HOUR, perIpDay: 3, perDay: 100 },
      () => now,
    );
    expect([
      allow("2001:db8:1:2::1"),
      allow("2001:db8:1:2::2"),
      allow("2001:db8:1:2::3"),
    ]).toEqual([true, true, false]);
    now += HOUR;
    expect([allow("2001:db8:1:2::4"), allow("2001:db8:1:2::5")]).toEqual([
      true,
      false,
    ]);
    now += 2 * HOUR;
    expect(allow("2001:db8:1:2::6")).toBe(false);
    expect(allow("2001:db8:1:3::1")).toBe(true);
    now += 24 * HOUR;
    expect(allow("2001:db8:1:2::7")).toBe(true);
  });

  it("rend un envoi raté et oublie les visiteurs au bout d'un jour", () => {
    let now = Date.UTC(2026, 9, 11, 10);
    const allow = createLimiter(
      { perIp: 1, perIpWindow: HOUR, perIpDay: 5, perDay: 2 },
      () => now,
    );
    expect(allow("a")).toBe(true);
    expect(allow("a")).toBe(false);
    allow.refund("a");
    expect(allow("a")).toBe(true);
    expect(allow("b")).toBe(true);
    expect(allow("c")).toBe(false);
    expect(allow.purge()).toBe(2);
    now += 25 * HOUR;
    expect(allow.purge()).toBe(0);
  });
});

describe("courriel", () => {
  it("laisse un en-tête ASCII court tel quel et encode le reste", () => {
    expect(encodeHeader("[ChatonLM] Message de Ana")).toBe(
      "[ChatonLM] Message de Ana",
    );
    const subject = `[ChatonLM] Message de ${"Éléonore ".repeat(11)}🐈`;
    const encoded = encodeHeader(subject);
    expect(encoded).toMatch(/^[\x20-\x7e\r\n]+$/);
    for (const line of encoded.split("\r\n"))
      expect(line.length).toBeLessThanOrEqual(76);
    expect(decodeHeader(encoded)).toBe(subject);
  });

  it("compose un message dont le corps est en base64, lignes de 76 caractères", () => {
    const text = `De : Élodie\n\n${"Une ligne qui commence par un point.\n.\n".repeat(8)}`;
    const raw = buildMessage({
      from: '"ChatonLM (site)" <contact@example.org>',
      to: "contact@example.org",
      replyTo: "elodie@example.org",
      subject: "Sujet",
      text,
      date: new Date(Date.UTC(2026, 9, 11, 20, 31, 17)),
      id: "abc@chatonlm.com",
    });
    const [head, body] = raw.split("\r\n\r\n");
    expect(head.split("\r\n")).toEqual([
      'From: "ChatonLM (site)" <contact@example.org>',
      "To: <contact@example.org>",
      "Reply-To: <elodie@example.org>",
      "Subject: Sujet",
      "Date: Sun, 11 Oct 2026 20:31:17 +0000",
      "Message-ID: <abc@chatonlm.com>",
      "MIME-Version: 1.0",
      "Content-Type: text/plain; charset=utf-8",
      "Content-Transfer-Encoding: base64",
    ]);
    for (const line of body.split("\r\n"))
      expect(line).toMatch(/^[A-Za-z0-9+/=]{1,76}$/);
    expect(Buffer.from(body, "base64").toString()).toBe(
      text.replace(/\n/g, "\r\n"),
    );
  });
});

/** Faux serveur SMTP sans TLS : note les commandes et le message reçus. */
async function fakeSmtp(replies = {}) {
  const seen = { commands: [], data: null };
  const server = createTcpServer((socket) => {
    socket.setEncoding("utf8");
    socket.on("error", () => {});
    let buffer = "";
    let inData = false;
    socket.write("220 faux ESMTP\r\n");
    socket.on("data", (chunk) => {
      buffer += chunk;
      for (;;) {
        if (inData) {
          const end = buffer.indexOf("\r\n.\r\n");
          if (end === -1) return;
          seen.data = buffer.slice(0, end);
          buffer = buffer.slice(end + 5);
          inData = false;
          socket.write("250 2.0.0 OK\r\n");
          continue;
        }
        const end = buffer.indexOf("\r\n");
        if (end === -1) return;
        const line = buffer.slice(0, end);
        buffer = buffer.slice(end + 2);
        seen.commands.push(line);
        const verb = line.split(/[ :]/)[0].toUpperCase();
        if (verb === "EHLO")
          socket.write(
            "250-faux\r\n250-AUTH PLAIN LOGIN\r\n250 SIZE 35882577\r\n",
          );
        else if (verb === "AUTH")
          socket.write(replies.auth ?? "235 2.7.0 Accepted\r\n");
        else if (verb === "MAIL" || verb === "RCPT")
          socket.write("250 2.1.0 OK\r\n");
        else if (verb === "DATA") {
          socket.write("354 Go ahead\r\n");
          inData = true;
        } else if (verb === "QUIT") socket.end("221 Bye\r\n");
      }
    });
  });
  const port = await listen(server);
  return {
    seen,
    smtp: {
      host: "127.0.0.1",
      port,
      secure: false,
      user: "contact@example.org",
      pass: "motdepasse",
      timeout: 2000,
    },
  };
}

describe("sendMail", () => {
  const mail = {
    from: '"ChatonLM (site)" <contact@example.org>',
    to: "contact@example.org",
    replyTo: "elodie@example.org",
    subject: "[ChatonLM] Message de Élodie",
    text: "Bonjour.\n.\nUne ligne réduite à un point ne doit pas finir le message.\n",
  };

  it("s'identifie, puis remet le message entier", async () => {
    const { seen, smtp } = await fakeSmtp();
    await sendMail(mail, smtp);
    expect(seen.commands.slice(0, 5)).toEqual([
      "EHLO chatonlm.com",
      `AUTH PLAIN ${Buffer.from("\0contact@example.org\0motdepasse").toString("base64")}`,
      "MAIL FROM:<contact@example.org>",
      "RCPT TO:<contact@example.org>",
      "DATA",
    ]);
    const [head, body] = seen.data.split("\r\n\r\n");
    expect(head).toContain("Reply-To: <elodie@example.org>");
    expect(decodeHeader(/^Subject: (.*)$/m.exec(head)[1])).toBe(mail.subject);
    expect(Buffer.from(body, "base64").toString()).toBe(
      mail.text.replace(/\n/g, "\r\n"),
    );
  });

  it("échoue si le serveur refuse le mot de passe, sans le répéter", async () => {
    const { seen, smtp } = await fakeSmtp({
      auth: "535 5.7.8 Username and Password not accepted\r\n",
    });
    const error = await sendMail(mail, smtp).catch((e) => e);
    expect(error).toBeInstanceOf(Error);
    expect(error.message).toContain("AUTH");
    expect(error.message).not.toContain("motdepasse");
    expect(seen.data).toBeNull();
  });
});

const RELEASE = {
  tag_name: "v0.9.0",
  published_at: "2026-10-20T10:00:00Z",
  assets: [
    {
      name: "latest.json",
      size: 900,
      browser_download_url:
        "https://github.com/Elwynn18/chatonlm/releases/download/v0.9.0/latest.json",
    },
    {
      name: "chatonlm_0.9.0_amd64.AppImage.sig",
      size: 400,
      browser_download_url:
        "https://github.com/Elwynn18/chatonlm/releases/download/v0.9.0/chatonlm_0.9.0_amd64.AppImage.sig",
    },
    {
      name: "chatonlm_0.9.0_amd64.AppImage-sources.tar",
      size: 5,
      browser_download_url:
        "https://github.com/Elwynn18/chatonlm/releases/download/v0.9.0/chatonlm_0.9.0_amd64.AppImage-sources.tar",
    },
    {
      name: "chatonlm_0.9.0_amd64.AppImage",
      size: 190_000_000,
      browser_download_url:
        "https://github.com/Elwynn18/chatonlm/releases/download/v0.9.0/chatonlm_0.9.0_amd64.AppImage",
    },
    {
      name: "chatonlm_0.9.0_amd64.deb",
      size: 120_000_000,
      browser_download_url:
        "https://github.com/Elwynn18/chatonlm/releases/download/v0.9.0/chatonlm_0.9.0_amd64.deb",
    },
    {
      name: "chatonlm_0.9.0_x64-setup.exe",
      size: 34_000_000,
      browser_download_url:
        "https://github.com/Elwynn18/chatonlm/releases/download/v0.9.0/chatonlm_0.9.0_x64-setup.exe",
    },
  ],
};

describe("sendMail, échéance", () => {
  it("abandonne un envoi qui traîne, même si chaque étape répond à temps", async () => {
    // Serveur correct mais lent : 120 ms par réponse, soit plus de 700 ms pour
    // les six étapes, alors qu'aucune ne dépasse son propre délai.
    const replies = {
      EHLO: "250 lent",
      AUTH: "235 OK",
      MAIL: "250 OK",
      RCPT: "250 OK",
      DATA: "354 Go",
    };
    const server = createTcpServer((socket) => {
      socket.on("error", () => {});
      socket.setEncoding("utf8");
      socket.write("220 lent ESMTP\r\n");
      socket.on("data", (chunk) => {
        const reply =
          replies[chunk.trim().split(/[ :]/)[0].toUpperCase()] ?? "250 OK";
        setTimeout(() => socket.destroyed || socket.write(`${reply}\r\n`), 120);
      });
    });
    const port = await listen(server);
    const smtp = {
      host: "127.0.0.1",
      port,
      secure: false,
      user: "a@example.org",
      pass: "p",
      timeout: 2000,
    };
    const mail = {
      from: "a@example.org",
      to: "a@example.org",
      subject: "x",
      text: "x",
    };
    await expect(
      sendMail(mail, { ...smtp, deadline: 5000 }),
    ).resolves.toBeUndefined();
    await expect(sendMail(mail, { ...smtp, deadline: 300 })).rejects.toThrow(
      /envoi trop long/,
    );
  });
});

describe("dernière version", () => {
  it("retient les trois fichiers à télécharger, pas leurs signatures ni les sources", () => {
    const picked = pickAssets(RELEASE, "Elwynn18/chatonlm");
    expect(picked).toMatchObject({
      published: true,
      version: "0.9.0",
      date: "2026-10-20T10:00:00Z",
    });
    expect(picked.files.windows.name).toBe("chatonlm_0.9.0_x64-setup.exe");
    expect(picked.files.appimage).toEqual({
      name: "chatonlm_0.9.0_amd64.AppImage",
      size: 190_000_000,
      url: "https://github.com/Elwynn18/chatonlm/releases/download/v0.9.0/chatonlm_0.9.0_amd64.AppImage",
    });
    expect(picked.files.deb.name).toBe("chatonlm_0.9.0_amd64.deb");
  });

  it("écarte un fichier hébergé ailleurs que dans les Releases du dépôt", () => {
    const elsewhere = {
      ...RELEASE,
      assets: [
        {
          name: "x-setup.exe",
          size: 1,
          browser_download_url: "https://exemple.invalid/x-setup.exe",
        },
      ],
    };
    expect(pickAssets(elsewhere, "Elwynn18/chatonlm")).toEqual({
      published: false,
    });
    expect(pickAssets(null, "Elwynn18/chatonlm")).toEqual({ published: false });
  });

  it("n'interroge GitHub qu'une fois par période et garde la dernière réponse en cas de panne", async () => {
    let now = 0;
    let calls = 0;
    let fail = false;
    const source = createReleaseSource(
      { repo: "Elwynn18/chatonlm", ttl: 1000 },
      async () => {
        calls += 1;
        if (fail) throw new Error("panne");
        return RELEASE;
      },
      () => now,
    );
    expect((await Promise.all([source(), source()]))[1].version).toBe("0.9.0");
    await source();
    expect(calls).toBe(1);
    now = 2000;
    fail = true;
    expect((await source()).version).toBe("0.9.0");
    expect(calls).toBe(2);
  });
});

describe("service HTTP", () => {
  const ORIGIN = "https://chatonlm.com";
  async function start(deps = {}, release = RELEASE) {
    const sent = [];
    const config = configFromEnv({
      SMTP_USER: "contact@example.org",
      SMTP_PASS: "abcd efgh",
    });
    const service = createService(config, {
      send: async (mail) => void sent.push(mail),
      release: createReleaseSource(config, async () => release),
      log: () => {},
      ...deps,
    });
    const base = `http://127.0.0.1:${await listen(service)}`;
    const post = (fields, headers = {}) =>
      fetch(`${base}/api/contact`, {
        method: "POST",
        redirect: "manual",
        headers: {
          "Content-Type": "application/json",
          Origin: ORIGIN,
          ...headers,
        },
        body: JSON.stringify(fields),
      });
    return { base, post, sent, config };
  }

  it("lit le mot de passe d'application sans ses espaces", () => {
    expect(
      configFromEnv({ SMTP_USER: "a@b.fr", SMTP_PASS: "abcd efgh ijkl mnop" })
        .smtp.pass,
    ).toBe("abcdefghijklmnop");
    expect(configFromEnv({ SMTP_USER: "a@b.fr" }).to).toBe("a@b.fr");
  });

  it("relaie un message : réponse au visiteur par « Reply-To »", async () => {
    const { post, sent } = await start();
    const response = await post(VALID, { Origin: "https://chatonlm.com" });
    expect(response.status).toBe(200);
    expect(await response.json()).toEqual({ ok: true });
    expect(sent).toEqual([
      {
        from: '"ChatonLM (site)" <contact@example.org>',
        to: "contact@example.org",
        replyTo: "elodie@example.org",
        subject: "[ChatonLM] Message de Élodie Martin",
        text: "De : Élodie Martin <elodie@example.org>\nLangue du site : fr\n\nBonjour, une question sur la dictée.\n",
      },
    ]);
  });

  it("refuse un champ invalide, une autre origine, un envoi trop rapide", async () => {
    const { post, sent } = await start();
    const bad = await post({ ...VALID, email: "nope" });
    expect([bad.status, await bad.json()]).toEqual([
      422,
      { ok: false, error: "email" },
    ]);
    const origin = await post(VALID, { Origin: "https://exemple.invalid" });
    expect([origin.status, await origin.json()]).toEqual([
      403,
      { ok: false, error: "origin" },
    ]);
    const fast = await post({ ...VALID, elapsed: 300 });
    expect([fast.status, await fast.json()]).toEqual([
      422,
      { ok: false, error: "too_fast" },
    ]);
    expect(sent).toEqual([]);
  });

  it("répond « envoyé » au robot qui remplit le champ piège, sans rien envoyer", async () => {
    const { post, sent } = await start();
    const response = await post({
      ...VALID,
      champ_libre: "https://spam.invalid",
    });
    expect([response.status, await response.json()]).toEqual([
      200,
      { ok: true },
    ]);
    expect(sent).toEqual([]);
  });

  it("plafonne les envois d'une même adresse", async () => {
    const { post, sent } = await start();
    const statuses = [];
    for (let i = 0; i <= LIMITS.perIp; i += 1)
      statuses.push((await post(VALID, { "X-Real-IP": "203.0.113.7" })).status);
    expect(statuses).toEqual([...Array(LIMITS.perIp).fill(200), 429]);
    expect((await post(VALID, { "X-Real-IP": "203.0.113.8" })).status).toBe(
      200,
    );
    expect(sent).toHaveLength(LIMITS.perIp + 1);
  });

  it("signale un envoi impossible sans détailler la panne", async () => {
    const { post } = await start({
      send: async () => {
        throw new Error("SMTP : étape AUTH refusée");
      },
    });
    const response = await post(VALID);
    expect([response.status, await response.json()]).toEqual([
      502,
      { ok: false, error: "send" },
    ]);
  });

  it("sans mot de passe d'envoi, met le formulaire en pause et sert le reste", async () => {
    const config = configFromEnv({ SMTP_USER: "contact@example.org" });
    const journal = [];
    const service = createService(config, {
      release: createReleaseSource(config, async () => RELEASE),
      log: (entry) => journal.push(entry),
    });
    const base = `http://127.0.0.1:${await listen(service)}`;
    const post = (fields) =>
      fetch(`${base}/api/contact`, {
        method: "POST",
        headers: { "Content-Type": "application/json", Origin: ORIGIN },
        body: JSON.stringify(fields),
      });
    const response = await post(VALID);
    expect([response.status, await response.json()]).toEqual([
      503,
      { ok: false, error: "send" },
    ]);
    // Aucune connexion à la messagerie n'a été tentée.
    expect(journal).toEqual([{ ev: "contact", result: "send" }]);
    // Les refus ordinaires passent avant, et le reste du service répond.
    expect((await post({ ...VALID, email: "nope" })).status).toBe(422);
    expect((await fetch(`${base}/api/release`)).status).toBe(200);
    expect((await fetch(`${base}/api/health`)).status).toBe(200);
  });

  it("refuse un corps trop long ou d'un autre format", async () => {
    const { base, post } = await start();
    expect(
      (await post({ ...VALID, message: "a".repeat(LIMITS.body) })).status,
    ).toBe(413);
    const text = await fetch(`${base}/api/contact`, {
      method: "POST",
      headers: { "Content-Type": "text/plain" },
      body: "x",
    });
    expect(text.status).toBe(415);
    const broken = await fetch(`${base}/api/contact`, {
      method: "POST",
      headers: { "Content-Type": "application/json", Origin: ORIGIN },
      body: "{",
    });
    expect(broken.status).toBe(400);
    expect((await fetch(`${base}/api/contact`)).status).toBe(405);
  });

  it("sans JavaScript, renvoie le navigateur vers une page du site", async () => {
    const { base, sent } = await start();
    const form = (fields, headers = { Origin: ORIGIN }) =>
      fetch(`${base}/api/contact`, {
        method: "POST",
        redirect: "manual",
        headers: {
          "Content-Type": "application/x-www-form-urlencoded",
          ...headers,
        },
        body: new URLSearchParams(fields),
      });
    const ok = await form({ ...VALID, lang: "es", champ_libre: "" });
    expect([ok.status, ok.headers.get("location")]).toEqual([
      303,
      "/es/contact/sent/",
    ]);
    expect(sent).toHaveLength(1);
    const bad = await form({ ...VALID, lang: "fr", email: "nope" });
    expect([bad.status, bad.headers.get("location")]).toEqual([
      303,
      "/fr/contact/error/",
    ]);
  });

  it("annonce la dernière version et y redirige les téléchargements", async () => {
    const { base } = await start();
    const release = await fetch(`${base}/api/release`);
    expect(release.headers.get("cache-control")).toBe("public, max-age=300");
    expect(await release.json()).toMatchObject({
      published: true,
      version: "0.9.0",
    });
    const windows = await fetch(`${base}/download/windows`, {
      redirect: "manual",
    });
    expect([windows.status, windows.headers.get("location")]).toEqual([
      302,
      "https://github.com/Elwynn18/chatonlm/releases/download/v0.9.0/chatonlm_0.9.0_x64-setup.exe",
    ]);
    expect(
      (await fetch(`${base}/download/macos`, { redirect: "manual" })).status,
    ).toBe(404);
  });

  it("avant la première version, renvoie les téléchargements à la page « bientôt »", async () => {
    const { base } = await start({}, null);
    expect(await (await fetch(`${base}/api/release`)).json()).toEqual({
      published: false,
    });
    const deb = await fetch(`${base}/download/deb`, { redirect: "manual" });
    expect([deb.status, deb.headers.get("location")]).toEqual([
      302,
      "/en/soon/",
    ]);
    const windows = await fetch(`${base}/download/windows`, {
      redirect: "manual",
      headers: { "Accept-Language": "fr-FR,fr;q=0.9,en;q=0.8" },
    });
    expect(windows.headers.get("location")).toBe("/fr/soon/");
  });

  it("refuse un envoi qui ne vient pas d'une page du site", async () => {
    const { base, sent } = await start();
    const raw = (headers) =>
      fetch(`${base}/api/contact`, {
        method: "POST",
        redirect: "manual",
        headers: { "Content-Type": "application/json", ...headers },
        body: JSON.stringify(VALID),
      });
    expect((await raw({})).status).toBe(403);
    expect((await raw({ Origin: "null" })).status).toBe(403);
    expect(
      (await raw({ Referer: "https://chatonlm.com.exemple.invalid/fr/" }))
        .status,
    ).toBe(403);
    expect(sent).toEqual([]);
    expect(
      (await raw({ Referer: "https://chatonlm.com/fr/contact/" })).status,
    ).toBe(200);
    expect(sent).toHaveLength(1);
  });

  it("sans JavaScript, renvoie vers la page d'erreur même pour un refus précoce", async () => {
    const { base } = await start();
    const form = (body, headers) =>
      fetch(`${base}/api/contact`, {
        method: "POST",
        redirect: "manual",
        headers: {
          "Content-Type": "application/x-www-form-urlencoded",
          ...headers,
        },
        body,
      });
    const tooLong = await form(`lang=fr&message=${"a".repeat(LIMITS.body)}`, {
      Origin: ORIGIN,
    });
    expect([tooLong.status, tooLong.headers.get("location")]).toEqual([
      303,
      "/en/contact/error/",
    ]);
    const elsewhere = await form("lang=fr", {
      Origin: "https://exemple.invalid",
    });
    expect([elsewhere.status, elsewhere.headers.get("location")]).toEqual([
      303,
      "/en/contact/error/",
    ]);
  });

  it("ne compte pas un envoi raté dans les plafonds", async () => {
    let down = true;
    const delivered = [];
    const { post } = await start({
      send: async (mail) => {
        if (down) throw new Error("SMTP : panne");
        delivered.push(mail);
      },
    });
    const statuses = [];
    for (let i = 0; i < LIMITS.perIp + 2; i += 1)
      statuses.push((await post(VALID)).status);
    expect(statuses).toEqual(Array(LIMITS.perIp + 2).fill(502));
    down = false;
    expect((await post(VALID)).status).toBe(200);
    expect(delivered).toHaveLength(1);
  });

  it("quand GitHub ne répond pas au démarrage, ne dit pas « aucune version » pour longtemps", async () => {
    let now = 0;
    let fail = true;
    const config = configFromEnv({
      SMTP_USER: "contact@example.org",
      SMTP_PASS: "x",
    });
    const release = createReleaseSource(
      config,
      async () => {
        if (fail) throw new Error("GitHub : réponse 403");
        return RELEASE;
      },
      () => now,
    );
    const { base } = await start({ release });
    const first = await fetch(`${base}/api/release`);
    expect(first.headers.get("cache-control")).toBe("no-store");
    expect(await first.json()).toEqual({ published: false });
    const download = await fetch(`${base}/download/windows`, {
      redirect: "manual",
    });
    expect(download.headers.get("location")).toBe(
      "https://github.com/Elwynn18/chatonlm/releases/latest",
    );
    fail = false;
    now = 29_000;
    expect(await (await fetch(`${base}/api/release`)).json()).toEqual({
      published: false,
    });
    now = 31_000;
    expect(await (await fetch(`${base}/api/release`)).json()).toMatchObject({
      published: true,
      version: "0.9.0",
    });
  });
});
