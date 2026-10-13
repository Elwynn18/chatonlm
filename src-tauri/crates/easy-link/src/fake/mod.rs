//! Faux serveurs Telegram et Discord (fonctionnalité `fake`) : essais du
//! crate, essais de l'app, et e2e par le binaire `fake-messengers`.
//!
//! Les deux se pilotent de la même façon, en Rust comme par HTTP :
//! - `POST /fake/message` avec `{"user": "42", "name": "Ana", "text": "…"}`
//!   fait arriver un message privé de ce compte ; champs en option :
//!   `"image": true` (joint une petite image PNG), `"voice": true` (joint le
//!   message vocal `fixtures/voice-speech.ogg` : son voisé synthétique en trois
//!   salves séparées de silences, que la garde « y a-t-il de la parole ? » de
//!   l'app laisse passer, ce qu'un ton continu ne fait pas), `"private": false`
//!   (message d'un groupe ou d'un serveur, que le bot doit ignorer),
//!   `"bot": true` (auteur qui est un bot), `"age": 600` (envoyé il y a tant
//!   de secondes) ;
//! - `GET /fake/sent` rend, dans l'ordre, ce que le bot a fait :
//!   `[{"kind": "send" | "edit" | "typing", "channel": "…", "id": "…",
//!   "text": "…"}]` (`id` et `text` absents pour `typing` ; `text` tel que
//!   reçu, et pour Telegram `"parseMode"`, pour Discord `"allowedMentions"`
//!   et `"flags"`).
//!
//! Un seul jeton est accepté ([`telegram::TOKEN`], [`discord::TOKEN`]) ; tout
//! autre reçoit le refus de la vraie messagerie.

pub mod discord;
pub mod telegram;
