#!/usr/bin/env node
// Service du site chatonlm.com, lancé sur le VPS derrière nginx (il n'écoute
// que sur 127.0.0.1) :
//   POST /api/contact         formulaire de contact, relayé par courriel (SMTP) ;
//   GET  /api/release         dernière version publiée de ChatonLM, en JSON ;
//   GET  /download/<fichier>  redirection vers le fichier de cette version
//                             (windows, appimage ou deb) ;
//   GET  /api/health          état du service.
//
// Sans dépendance : un seul fichier à téléverser, lancé par « node service.mjs »
// (Node 18 ou plus récent). Réglages par variables d'environnement, voir
// chatonlm-site.env.example ; installation : chatonlm-site.service.
//
// Vie privée : rien n'est écrit sur le disque. Le journal ne contient ni le
// message, ni le nom, ni l'adresse du visiteur ; son adresse IP ne sert qu'à
// limiter les envois et reste en mémoire un jour au plus.

import { randomBytes } from "node:crypto";
import { realpathSync } from "node:fs";
import { createServer } from "node:http";
import { connect as netConnect } from "node:net";
import { connect as tlsConnect } from "node:tls";
import { pathToFileURL } from "node:url";

export const LANGS = ["fr", "en", "es"];
export const DOWNLOADS = ["windows", "appimage", "deb"];
/** Nom du champ piège du formulaire (voir contact-form.html). */
export const TRAP = "champ_libre";

export const LIMITS = {
  /** Taille maximale du corps d'une requête, en octets. */
  body: 16 * 1024,
  name: 100,
  email: 254,
  messageMin: 10,
  messageMax: 5000,
  /** Envois acceptés par adresse IP et par heure, puis par jour pour tout le site. */
  perIp: 5,
  perIpWindow: 60 * 60 * 1000,
  perIpDay: 10,
  perDay: 200,
  /** Temps minimal entre l'affichage du formulaire et l'envoi (robots), en ms. */
  minElapsed: 2500,
};

// ---------------------------------------------------------------------------
// Formulaire : validation

const CONTROL = /\p{Cc}/gu;
// Adresse en ASCII imprimable, sans rien qui puisse fermer ou ouvrir un
// en-tête : elle est recopiée telle quelle dans « Reply-To ».
const EMAIL = /^[^\s@<>()[\]\\,;:"]+@[^\s@<>()[\]\\,;:"]+\.[a-z0-9-]{2,}$/i;

/**
 * Vérifie les champs du formulaire. Rend `{ ok: true, value }` avec les champs
 * nettoyés, ou `{ ok: false, error }` avec le nom du champ fautif.
 */
export function validateContact(fields) {
  const text = (value) => (typeof value === "string" ? value : "");
  const name = text(fields.name)
    .replace(CONTROL, " ")
    .replace(/\s+/g, " ")
    .trim();
  if (!name || name.length > LIMITS.name) return { ok: false, error: "name" };
  const email = text(fields.email).trim();
  if (
    email.length > LIMITS.email ||
    !/^[\x21-\x7e]+$/.test(email) ||
    !EMAIL.test(email)
  ) {
    return { ok: false, error: "email" };
  }
  const message = text(fields.message).replace(/\r\n?/g, "\n").trim();
  if (
    message.length < LIMITS.messageMin ||
    message.length > LIMITS.messageMax
  ) {
    return { ok: false, error: "message" };
  }
  return { ok: true, value: { name, email, message, lang: langOf(fields) } };
}

function langOf(fields) {
  return LANGS.includes(fields.lang) ? fields.lang : "en";
}

/**
 * Clé d'un visiteur pour les plafonds : son adresse IPv4, ou le réseau /64 de
 * son adresse IPv6 (un abonné dispose de tout un /64 : compter par adresse
 * entière ne le limiterait pas).
 */
export function visitorKey(ip) {
  const address = String(ip).replace(/^::ffff:(?=\d+\.\d+\.\d+\.\d+$)/i, "");
  if (!address.includes(":")) return address;
  const [head, tail = ""] = address.split("::");
  const left = head ? head.split(":") : [];
  const right = tail ? tail.split(":") : [];
  const groups = address.includes("::")
    ? [
        ...left,
        ...Array(Math.max(0, 8 - left.length - right.length)).fill("0"),
        ...right,
      ]
    : left;
  const network = groups
    .slice(0, 4)
    .map((group) => group.toLowerCase().replace(/^0+(?=.)/, ""));
  return `${network.join(":")}::/64`;
}

/**
 * Limite les envois. `allow(ip)` rend `false` quand un plafond est atteint :
 * par visiteur et par heure, par visiteur et par jour, pour tout le site par
 * jour. `allow.refund(ip)` rend un envoi qui a échoué ; `allow.purge()`
 * oublie les visiteurs sans envoi depuis un jour.
 */
export function createLimiter(
  { perIp, perIpWindow, perIpDay = Infinity, perDay },
  now = Date.now,
) {
  const DAY = 24 * 60 * 60 * 1000;
  const byVisitor = new Map();
  let day = "";
  let count = 0;
  const allow = (ip) => {
    const t = now();
    const today = new Date(t).toISOString().slice(0, 10);
    if (today !== day) {
      day = today;
      count = 0;
    }
    const key = visitorKey(ip);
    const times = (byVisitor.get(key) ?? []).filter((at) => t - at < DAY);
    const lastHour = times.filter((at) => t - at < perIpWindow).length;
    if (lastHour >= perIp || times.length >= perIpDay || count >= perDay) {
      if (times.length) byVisitor.set(key, times);
      else byVisitor.delete(key);
      return false;
    }
    times.push(t);
    byVisitor.set(key, times);
    count += 1;
    return true;
  };
  allow.refund = (ip) => {
    const key = visitorKey(ip);
    const times = byVisitor.get(key);
    if (!times?.length) return;
    times.pop();
    if (!times.length) byVisitor.delete(key);
    count = Math.max(0, count - 1);
  };
  allow.purge = () => {
    const t = now();
    for (const [key, times] of byVisitor) {
      if (!times.some((at) => t - at < DAY)) byVisitor.delete(key);
    }
    return byVisitor.size;
  };
  return allow;
}

// ---------------------------------------------------------------------------
// Courriel : message et envoi SMTP

/** En-tête hors ASCII ou long : mots encodés de la RFC 2047, repliés. */
export function encodeHeader(text) {
  if (/^[\x20-\x7e]*$/.test(text) && text.length <= 60) return text;
  const words = [];
  let chunk = "";
  for (const char of text) {
    if (Buffer.byteLength(chunk + char) > 42) {
      words.push(chunk);
      chunk = "";
    }
    chunk += char;
  }
  if (chunk) words.push(chunk);
  return words
    .map((word) => `=?UTF-8?B?${Buffer.from(word).toString("base64")}?=`)
    .join("\r\n ");
}

/** Le message au format d'Internet (RFC 5322), corps en base64. */
export function buildMessage({
  from,
  to,
  replyTo,
  subject,
  text,
  date = new Date(),
  id,
}) {
  const messageId = id ?? `${randomBytes(12).toString("hex")}@chatonlm.com`;
  const body = Buffer.from(text.replace(/\r?\n/g, "\r\n")).toString("base64");
  return [
    `From: ${from}`,
    `To: <${to}>`,
    ...(replyTo ? [`Reply-To: <${replyTo}>`] : []),
    `Subject: ${encodeHeader(subject)}`,
    `Date: ${date.toUTCString().replace("GMT", "+0000")}`,
    `Message-ID: <${messageId}>`,
    "MIME-Version: 1.0",
    "Content-Type: text/plain; charset=utf-8",
    "Content-Transfer-Encoding: base64",
    "",
    ...(body.match(/.{1,76}/g) ?? []),
  ].join("\r\n");
}

function openSocket({ host, port, secure = true, connectTimeout = 8000 }) {
  return new Promise((resolve, reject) => {
    const socket = secure
      ? tlsConnect({ host, port, servername: host })
      : netConnect({ host, port });
    const timer = setTimeout(() => {
      socket.destroy();
      reject(new Error("SMTP : délai de connexion dépassé"));
    }, connectTimeout);
    socket.once("error", (error) => {
      clearTimeout(timer);
      reject(error);
    });
    socket.once(secure ? "secureConnect" : "connect", () => {
      clearTimeout(timer);
      resolve(socket);
    });
  });
}

/** Lit les réponses du serveur SMTP, une par appel (dernière ligne de chacune). */
function replyReader(socket, timeout) {
  let buffer = "";
  const replies = [];
  let waiter = null;
  let failure = null;
  const settle = () => {
    if (!waiter || (!replies.length && !failure)) return;
    const { resolve, reject, timer } = waiter;
    waiter = null;
    clearTimeout(timer);
    if (replies.length) resolve(replies.shift());
    else reject(failure);
  };
  const fail = (error) => {
    failure ??= error;
    settle();
  };
  socket.setEncoding("utf8");
  socket.on("data", (chunk) => {
    buffer += chunk;
    for (
      let end = buffer.indexOf("\n");
      end !== -1;
      end = buffer.indexOf("\n")
    ) {
      const line = buffer.slice(0, end).replace(/\r$/, "");
      buffer = buffer.slice(end + 1);
      // « 250-… » annonce une suite ; « 250 … » termine la réponse.
      if (/^\d{3}(?: |$)/.test(line)) replies.push(line);
    }
    settle();
  });
  socket.on("error", fail);
  socket.on("close", () =>
    fail(new Error("SMTP : connexion fermée par le serveur")),
  );
  return () =>
    new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        waiter = null;
        reject(new Error("SMTP : délai de réponse dépassé"));
      }, timeout);
      waiter = { resolve, reject, timer };
      settle();
    });
}

/**
 * Envoie `mail` par le serveur `smtp` (`host`, `port`, `user`, `pass` ;
 * `secure: false` pour un serveur d'essai sans TLS). Lève une erreur si une
 * étape est refusée ; le mot de passe n'apparaît jamais dans l'erreur.
 */
export async function sendMail(mail, smtp, open = openSocket) {
  const socket = await open(smtp);
  // Échéance globale, sous le délai de nginx (30 s) : au-delà, le visiteur
  // verrait une erreur alors que le message partirait encore, et le renverrait.
  const deadline = setTimeout(
    () => socket.destroy(new Error("SMTP : envoi trop long")),
    smtp.deadline ?? 20_000,
  );
  const reply = replyReader(socket, smtp.timeout ?? 15_000);
  const expect = async (code, step) => {
    const line = await reply();
    if (!line.startsWith(String(code))) {
      throw new Error(`SMTP : étape ${step} refusée (${line.slice(0, 120)})`);
    }
  };
  const command = async (line, code, step) => {
    socket.write(`${line}\r\n`);
    await expect(code, step);
  };
  try {
    await expect(220, "accueil");
    await command(`EHLO ${smtp.name ?? "chatonlm.com"}`, 250, "EHLO");
    const login = Buffer.from(`\0${smtp.user}\0${smtp.pass}`).toString(
      "base64",
    );
    await command(`AUTH PLAIN ${login}`, 235, "AUTH");
    await command(`MAIL FROM:<${smtp.user}>`, 250, "MAIL FROM");
    await command(`RCPT TO:<${mail.to}>`, 250, "RCPT TO");
    await command("DATA", 354, "DATA");
    await command(`${buildMessage(mail)}\r\n.`, 250, "message");
    socket.write("QUIT\r\n");
  } finally {
    clearTimeout(deadline);
    socket.destroy();
  }
}

// ---------------------------------------------------------------------------
// Dernière version publiée

/** Les trois fichiers à télécharger d'une Release GitHub, ou `published: false`. */
export function pickAssets(release, repo) {
  const prefix = `https://github.com/${repo}/releases/download/`.toLowerCase();
  const find = (suffix) => {
    const asset = (release?.assets ?? []).find(
      (a) =>
        typeof a.name === "string" &&
        a.name.endsWith(suffix) &&
        String(a.browser_download_url).toLowerCase().startsWith(prefix),
    );
    return asset
      ? { name: asset.name, url: asset.browser_download_url, size: asset.size }
      : null;
  };
  const files = {
    windows: find("-setup.exe"),
    appimage: find(".AppImage"),
    deb: find(".deb"),
  };
  if (!Object.values(files).some(Boolean)) return { published: false };
  return {
    published: true,
    version: String(release.tag_name ?? "").replace(/^v/, ""),
    date: release.published_at ?? null,
    files,
  };
}

async function fetchLatestRelease(repo) {
  const response = await fetch(
    `https://api.github.com/repos/${repo}/releases/latest`,
    {
      headers: {
        Accept: "application/vnd.github+json",
        "User-Agent": "chatonlm-site",
      },
      signal: AbortSignal.timeout(8000),
    },
  );
  // 404 : pas encore de version publiée (ou dépôt encore privé).
  if (response.status === 404) return null;
  if (!response.ok) throw new Error(`GitHub : réponse ${response.status}`);
  return response.json();
}

/**
 * Dernière version, gardée `ttl` ms : GitHub n'est interrogé que par ce service,
 * jamais par le navigateur du visiteur. Si GitHub ne répond pas, la dernière
 * réponse connue reste servie ; sans réponse connue, le résultat porte
 * `unknown: true` (on ne sait pas s'il existe une version) et la question est
 * reposée après `retry` ms.
 */
export function createReleaseSource(
  { repo, ttl = 10 * 60 * 1000, retry = 30 * 1000 },
  fetchRelease = fetchLatestRelease,
  now = Date.now,
) {
  let cached = null;
  let at = -Infinity;
  let wait = ttl;
  let pending = null;
  return () => {
    if (now() - at < wait) return Promise.resolve(cached);
    pending ??= fetchRelease(repo)
      .then((release) => {
        cached = pickAssets(release, repo);
        wait = ttl;
      })
      .catch(() => {
        cached ??= { published: false, unknown: true };
        wait = cached.unknown ? retry : ttl;
      })
      .then(() => {
        at = now();
        pending = null;
        return cached;
      });
    return pending;
  };
}

// ---------------------------------------------------------------------------
// Serveur HTTP

function json(res, status, body, headers = {}) {
  res.writeHead(status, {
    "Content-Type": "application/json; charset=utf-8",
    "Cache-Control": "no-store",
    ...headers,
  });
  res.end(JSON.stringify(body));
}

function redirect(res, status, location) {
  res.writeHead(status, { Location: location, "Cache-Control": "no-store" });
  res.end();
}

function readBody(req, limit) {
  return new Promise((resolve, reject) => {
    let chunks = [];
    let size = 0;
    req.on("data", (chunk) => {
      size += chunk.length;
      // Trop long : la suite est lue sans être gardée, pour que la réponse 413
      // arrive au client (nginx plafonne déjà le corps en amont).
      if (size > limit) chunks = null;
      else chunks?.push(chunk);
    });
    req.on("end", () => {
      if (chunks) resolve(Buffer.concat(chunks).toString("utf8"));
      else reject(Object.assign(new Error("corps trop long"), { status: 413 }));
    });
    req.on("error", reject);
  });
}

function parseFields(type, body) {
  if (type.startsWith("application/json")) {
    const parsed = JSON.parse(body);
    if (!parsed || typeof parsed !== "object" || Array.isArray(parsed))
      throw new Error("format");
    return parsed;
  }
  return Object.fromEntries(new URLSearchParams(body));
}

/**
 * Le serveur du site. `deps` remplace, pour les essais, l'envoi du courriel
 * (`send`), le limiteur (`allow`), la source des versions (`release`) et le
 * journal (`log`).
 */
export function createService(config, deps = {}) {
  // Sans mot de passe d'envoi, le formulaire est en pause : le service répond
  // qu'il ne peut pas envoyer (la page donne alors l'adresse de contact), sans
  // tenter de connexion ; versions et téléchargements fonctionnent.
  const send =
    deps.send ??
    (config.smtp.user && config.smtp.pass
      ? (mail) => sendMail(mail, config.smtp)
      : null);
  const allow = deps.allow ?? createLimiter(LIMITS);
  const release = deps.release ?? createReleaseSource(config);
  const log =
    deps.log ??
    ((entry) =>
      console.log(JSON.stringify({ t: new Date().toISOString(), ...entry })));

  /** L'envoi vient-il d'une page du site ? (en-tête Origin, sinon Referer) */
  function fromSite(req) {
    const origin = req.headers.origin;
    if (origin) return config.origins.includes(origin);
    const referer = String(req.headers.referer ?? "");
    return config.origins.some(
      (allowed) => referer === allowed || referer.startsWith(`${allowed}/`),
    );
  }

  async function contact(req, res) {
    const type = String(req.headers["content-type"] ?? "").toLowerCase();
    const isJson = type.startsWith("application/json");
    if (!isJson && !type.startsWith("application/x-www-form-urlencoded")) {
      return json(res, 415, { ok: false, error: "format" });
    }
    // Sans JavaScript, le navigateur envoie le formulaire lui-même : on le
    // renvoie vers une page du site plutôt que de lui montrer du JSON.
    const wantsJson =
      isJson || String(req.headers.accept ?? "").includes("application/json");
    let fields = {};
    const finish = (status, error) => {
      log({ ev: "contact", result: error ?? "sent" });
      if (wantsJson)
        return json(res, status, error ? { ok: false, error } : { ok: true });
      return redirect(
        res,
        303,
        `/${langOf(fields)}/contact/${error ? "error" : "sent"}/`,
      );
    };

    if (!fromSite(req)) return finish(403, "origin");
    try {
      fields = parseFields(type, await readBody(req, LIMITS.body));
    } catch (error) {
      return finish(error.status ?? 400, "format");
    }
    const checked = validateContact(fields);
    if (!checked.ok) return finish(422, checked.error);
    // Champ piège, invisible : seul un robot le remplit. On répond comme si
    // le message était parti, sans rien envoyer. Son nom ne ressemble à aucun
    // champ qu'un gestionnaire d'identité remplirait de lui-même.
    if (typeof fields[TRAP] === "string" && fields[TRAP].trim()) {
      log({ ev: "contact", result: "honeypot" });
      if (wantsJson) return json(res, 200, { ok: true });
      return redirect(res, 303, `/${checked.value.lang}/contact/sent/`);
    }
    if (isJson && !(Number(fields.elapsed) >= LIMITS.minElapsed))
      return finish(422, "too_fast");
    if (!send) return finish(503, "send");
    const ip = String(
      req.headers["x-real-ip"] ?? req.socket.remoteAddress ?? "",
    );
    if (!allow(ip)) return finish(429, "rate");

    const { name, email, message, lang } = checked.value;
    try {
      await send({
        from: `"ChatonLM (site)" <${config.smtp.user}>`,
        to: config.to,
        replyTo: email,
        subject: `[ChatonLM] Message de ${name}`,
        text: `De : ${name} <${email}>\nLangue du site : ${lang}\n\n${message}\n`,
      });
    } catch (error) {
      // Un envoi raté ne compte pas : sinon une panne de la messagerie fermerait
      // le formulaire pour la journée.
      allow.refund?.(ip);
      log({
        ev: "smtp",
        message: String(error?.message ?? error).slice(0, 200),
      });
      return finish(502, "send");
    }
    return finish(200);
  }

  async function route(req, res) {
    const { pathname } = new URL(req.url ?? "/", "http://localhost");
    const read = req.method === "GET" || req.method === "HEAD";
    if (pathname === "/api/contact") {
      if (req.method !== "POST")
        return json(
          res,
          405,
          { ok: false, error: "method" },
          { Allow: "POST" },
        );
      return contact(req, res);
    }
    if (pathname === "/api/health" && read) return json(res, 200, { ok: true });
    if (pathname === "/api/release" && read) {
      const { unknown, ...latest } = await release();
      return json(res, 200, latest, {
        "Cache-Control": unknown ? "no-store" : "public, max-age=300",
      });
    }
    const download = /^\/download\/([a-z]+)\/?$/.exec(pathname);
    if (download && read && DOWNLOADS.includes(download[1])) {
      const latest = await release();
      const file = latest.published ? latest.files[download[1]] : null;
      if (file) return redirect(res, 302, file.url);
      // GitHub muet depuis le démarrage : sa page des versions, qui saura
      // répondre. Pas encore de version : la page du site qui l'annonce, dans
      // la langue du navigateur.
      if (latest.unknown) {
        return redirect(
          res,
          302,
          `https://github.com/${config.repo}/releases/latest`,
        );
      }
      const accept = String(req.headers["accept-language"] ?? "").toLowerCase();
      const lang = LANGS.find((code) => accept.startsWith(code)) ?? "en";
      return redirect(res, 302, `/${lang}/soon/`);
    }
    return json(res, 404, { ok: false, error: "not_found" });
  }

  const purge = setInterval(() => allow.purge?.(), 10 * 60 * 1000);
  purge.unref();
  const server = createServer((req, res) => {
    route(req, res).catch((error) => {
      log({
        ev: "error",
        message: String(error?.message ?? error).slice(0, 200),
      });
      if (!res.headersSent) json(res, 500, { ok: false, error: "server" });
      else res.end();
    });
  });
  server.on("close", () => clearInterval(purge));
  return server;
}

/** Réglages lus dans l'environnement (voir chatonlm-site.env.example). */
export function configFromEnv(env = process.env) {
  const user = (env.SMTP_USER ?? "").trim();
  return {
    host: env.HOST ?? "127.0.0.1",
    port: Number(env.PORT ?? 8787),
    repo: env.GITHUB_REPO ?? "Elwynn18/chatonlm",
    origins: (env.SITE_ORIGINS ?? "https://chatonlm.com")
      .split(",")
      .map((origin) => origin.trim())
      .filter(Boolean),
    to: (env.CONTACT_TO ?? "").trim() || user,
    smtp: {
      host: env.SMTP_HOST ?? "smtp.gmail.com",
      port: Number(env.SMTP_PORT ?? 465),
      user,
      // Google affiche le mot de passe d'application par groupes de quatre lettres.
      pass: (env.SMTP_PASS ?? "").replace(/\s+/g, ""),
      secure: env.SMTP_SECURE !== "0",
    },
  };
}

/** Ce fichier est-il le programme lancé (y compris par un lien symbolique) ? */
function isMain() {
  try {
    return (
      import.meta.url === pathToFileURL(realpathSync(process.argv[1])).href
    );
  } catch {
    return false;
  }
}

if (isMain()) {
  const config = configFromEnv();
  if (!config.smtp.user || !config.smtp.pass) {
    console.error(
      "SMTP_USER ou SMTP_PASS absent : formulaire en pause (il renvoie à l'adresse de contact), versions et téléchargements actifs. Voir chatonlm-site.env.example.",
    );
  }
  createService(config).listen(config.port, config.host, () => {
    console.log(
      `Service du site ChatonLM à l'écoute sur http://${config.host}:${config.port}`,
    );
  });
}
