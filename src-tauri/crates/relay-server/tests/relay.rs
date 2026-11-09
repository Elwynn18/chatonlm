//! Relais réel (HTTP sur la boucle locale) contre de faux fournisseurs :
//! inscription, signatures, rejeu, horloge, quotas, budget, secours,
//! journaux et base sans secret ni requête.

use relay_proto::api::{PATH_CHALLENGE, PATH_REGISTER, PATH_SEARCH, PATH_STATUS, Provider};
use relay_proto::pow::{self, Challenge};
use relay_proto::sign::Identity;
use relay_proto::time::{next_day_start, next_month_start};
use relay_server::config::Tiers;
use relay_server::fake::{BRAVE_TEST_KEY, Behavior, MOJEEK_TEST_KEY, TestRelay, UNICODE_TITLES};
use serde_json::{Value, json};
use std::sync::atomic::AtomicBool;

/// 8 nov. 2026, 12 h UTC.
const T: u64 = 1_794_139_200;
const UA: &str = "ChatonLM/0.10.0";

struct Client {
    http: reqwest::Client,
    url: String,
    ip: Option<String>,
}

struct Reply {
    status: u16,
    body: Value,
}

impl Reply {
    fn code(&self) -> &str {
        self.body["error"]["code"].as_str().unwrap_or_default()
    }
}

impl Client {
    fn new(relay: &TestRelay) -> Self {
        relay_server::install_crypto_provider();
        Self {
            http: reqwest::Client::builder().no_proxy().build().unwrap(),
            url: relay.url.clone(),
            ip: None,
        }
    }

    fn from_ip(relay: &TestRelay, ip: &str) -> Self {
        Self {
            ip: Some(ip.into()),
            ..Self::new(relay)
        }
    }

    async fn send(
        &self,
        method: &str,
        path: &str,
        headers: &[(&str, String)],
        body: Vec<u8>,
    ) -> Reply {
        let mut request = match method {
            "GET" => self.http.get(format!("{}{path}", self.url)),
            _ => self
                .http
                .post(format!("{}{path}", self.url))
                .header("content-type", "application/json")
                .body(body),
        };
        if !headers.iter().any(|(name, _)| *name == "user-agent") {
            request = request.header("user-agent", UA);
        }
        if let Some(ip) = &self.ip {
            request = request.header("x-real-ip", ip);
        }
        for (name, value) in headers {
            request = request.header(*name, value);
        }
        let response = request.send().await.unwrap();
        let status = response.status().as_u16();
        let bytes = response.bytes().await.unwrap();
        Reply {
            status,
            body: serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        }
    }

    async fn signed(
        &self,
        id: &Identity,
        method: &str,
        path: &str,
        body: Value,
        time: u64,
    ) -> Reply {
        let body = if method == "GET" {
            Vec::new()
        } else {
            serde_json::to_vec(&body).unwrap()
        };
        let headers = id.sign_request(method, path, time, &body);
        let pairs: Vec<(&str, String)> = headers
            .pairs()
            .iter()
            .map(|(n, v)| (*n, v.to_string()))
            .collect();
        self.send(method, path, &pairs, body).await
    }

    async fn challenge(&self) -> Reply {
        self.send("POST", PATH_CHALLENGE, &[], b"{}".to_vec()).await
    }

    /// Inscription complète : défi, preuve, inscription signée (défi refusé :
    /// sa réponse).
    async fn register(&self, id: &Identity, time: u64) -> Reply {
        let challenge = self.challenge().await;
        if challenge.status != 200 {
            return challenge;
        }
        let token = challenge.body["challenge"].as_str().unwrap().to_owned();
        let parsed = Challenge::parse(&token).unwrap();
        let nonces =
            pow::solve(&parsed, &id.public_key(), &AtomicBool::new(false), |_| {}).unwrap();
        self.signed(
            id,
            "POST",
            PATH_REGISTER,
            json!({ "challenge": token, "nonces": nonces }),
            time,
        )
        .await
    }

    async fn search(&self, id: &Identity, query: &str, time: u64) -> Reply {
        self.signed(
            id,
            "POST",
            PATH_SEARCH,
            json!({ "query": query, "lang": "fr", "country": "FR" }),
            time,
        )
        .await
    }
}

fn db() -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("relay.sqlite");
    (dir, path)
}

#[tokio::test]
async fn health_answers() {
    let (_dir, path) = db();
    let relay = TestRelay::start(&path, T, |_| {}).await;
    let reply = Client::new(&relay)
        .send("GET", "/health", &[], Vec::new())
        .await;
    assert_eq!(
        (reply.status, reply.body["status"].as_str()),
        (200, Some("ok"))
    );
}

#[tokio::test]
async fn register_then_search_through_brave() {
    let (_dir, path) = db();
    let relay = TestRelay::start(&path, T, |_| {}).await;
    let client = Client::new(&relay);
    let id = Identity::generate();
    let reply = client.register(&id, T).await;
    assert_eq!(reply.status, 200, "{:?}", reply.body);
    assert_eq!(reply.body["quota"]["remainingToday"], 50);
    // Inscrire de nouveau : sans effet, sans erreur.
    assert_eq!(client.register(&id, T).await.status, 200);

    let reply = client.search(&id, "chaton roux", T).await;
    assert_eq!(reply.status, 200, "{:?}", reply.body);
    assert_eq!(reply.body["provider"], "brave");
    assert_eq!(reply.body["quota"]["remainingToday"], 49);
    assert_eq!(reply.body["quota"]["resetsAt"], next_day_start(T));
    let results = reply.body["results"].as_array().unwrap();
    assert_eq!(results.len(), 5);
    // Balises et entités retirées, adresse `javascript:` et doublon écartés.
    assert_eq!(results[0]["title"], "Résultat 1 pour chaton roux");
    assert_eq!(results[0]["content"], "Extrait 1 & chaton roux");
    let urls: Vec<_> = results.iter().map(|r| r["url"].as_str().unwrap()).collect();
    assert_eq!(
        urls,
        [
            "https://example.org/1",
            "https://example.org/2",
            "https://example.org/3",
            "https://example.org/4",
            "https://example.org/5"
        ]
    );
    // Ce que Brave a reçu : la clé dans l'en-tête, la requête, la langue.
    let sent = &relay.fake.requests()[0];
    assert_eq!(sent.provider, Provider::Brave);
    assert_eq!(sent.token.as_deref(), Some(BRAVE_TEST_KEY));
    for (name, value) in [
        ("q", "chaton roux"),
        ("count", "5"),
        ("search_lang", "fr"),
        ("country", "FR"),
        ("text_decorations", "false"),
        ("safesearch", "moderate"),
        ("result_filter", "web"),
    ] {
        assert_eq!(
            sent.params.get(name).map(String::as_str),
            Some(value),
            "{name}"
        );
    }
    assert!(!sent.params.contains_key("extra_snippets"));

    let status = client.signed(&id, "GET", PATH_STATUS, Value::Null, T).await;
    assert_eq!(status.status, 200);
    assert_eq!(status.body["service"], "ok");
    assert_eq!(status.body["quota"]["remainingToday"], 49);
    assert_eq!(status.body["serverTime"], T);
}

#[tokio::test]
async fn bad_proofs_and_challenges_are_refused() {
    let (_dir, path) = db();
    let relay = TestRelay::start(&path, T, |_| {}).await;
    let client = Client::new(&relay);
    let id = Identity::generate();
    let token = client.challenge().await.body["challenge"]
        .as_str()
        .unwrap()
        .to_owned();
    let parsed = Challenge::parse(&token).unwrap();
    let nonces = pow::solve(&parsed, &id.public_key(), &AtomicBool::new(false), |_| {}).unwrap();

    // Preuve calculée pour une autre clé.
    let other = Identity::generate();
    let theirs = pow::solve(
        &parsed,
        &other.public_key(),
        &AtomicBool::new(false),
        |_| {},
    )
    .unwrap();
    // Preuve d'une autre clé, qui ne vaut pas pour celle-ci (avec 1 bit exigé,
    // elle vaudrait aussi une fois sur quatre : essai sinon instable).
    if !pow::verify(&parsed, &id.public_key(), &theirs) {
        let reply = client
            .signed(
                &id,
                "POST",
                PATH_REGISTER,
                json!({ "challenge": token, "nonces": theirs }),
                T,
            )
            .await;
        assert_eq!((reply.status, reply.code()), (400, "pow_invalid"));
    }
    // Trop peu de nonces.
    let reply = client
        .signed(
            &id,
            "POST",
            PATH_REGISTER,
            json!({ "challenge": token, "nonces": [nonces[0]] }),
            T,
        )
        .await;
    assert_eq!((reply.status, reply.code()), (400, "pow_invalid"));
    // Défi adouci (0 bit exigé) : sceau faux.
    let mut bytes = relay_proto::b64::decode(&token).unwrap();
    bytes[37] = 0;
    let softened = relay_proto::b64::encode(&bytes);
    let reply = client
        .signed(
            &id,
            "POST",
            PATH_REGISTER,
            json!({ "challenge": softened, "nonces": nonces }),
            T,
        )
        .await;
    assert_eq!((reply.status, reply.code()), (400, "challenge_invalid"));
    // Défi périmé (10 min par défaut).
    relay.set_time(T + 601);
    let reply = client
        .signed(
            &id,
            "POST",
            PATH_REGISTER,
            json!({ "challenge": token, "nonces": nonces }),
            T + 601,
        )
        .await;
    assert_eq!((reply.status, reply.code()), (400, "challenge_invalid"));
    // Encore valable juste avant.
    relay.set_time(T + 600);
    let reply = client
        .signed(
            &id,
            "POST",
            PATH_REGISTER,
            json!({ "challenge": token, "nonces": nonces }),
            T + 600,
        )
        .await;
    assert_eq!(reply.status, 200, "{:?}", reply.body);
}

#[tokio::test]
async fn signatures_replays_and_clocks() {
    let (_dir, path) = db();
    let relay = TestRelay::start(&path, T, |_| {}).await;
    let client = Client::new(&relay);
    let id = Identity::generate();
    // Pas inscrite.
    let reply = client.search(&id, "x", T).await;
    assert_eq!((reply.status, reply.code()), (401, "unknown_key"));
    assert_eq!(client.register(&id, T).await.status, 200);

    // Rejeu : la même requête signée, deux fois.
    let body = serde_json::to_vec(&json!({ "query": "rejeu" })).unwrap();
    let headers = id.sign_request("POST", PATH_SEARCH, T, &body);
    let pairs: Vec<(&str, String)> = headers
        .pairs()
        .iter()
        .map(|(n, v)| (*n, v.to_string()))
        .collect();
    assert_eq!(
        client
            .send("POST", PATH_SEARCH, &pairs, body.clone())
            .await
            .status,
        200
    );
    let reply = client.send("POST", PATH_SEARCH, &pairs, body.clone()).await;
    assert_eq!((reply.status, reply.code()), (401, "replay"));
    assert_eq!(relay.fake.count(Provider::Brave), 1);

    // Corps changé sous la même signature.
    let other = serde_json::to_vec(&json!({ "query": "autre" })).unwrap();
    let headers = id.sign_request("POST", PATH_SEARCH, T, &body);
    let pairs: Vec<(&str, String)> = headers
        .pairs()
        .iter()
        .map(|(n, v)| (*n, v.to_string()))
        .collect();
    let reply = client.send("POST", PATH_SEARCH, &pairs, other).await;
    assert_eq!((reply.status, reply.code()), (401, "bad_signature"));
    // Sans signature.
    let reply = client.send("POST", PATH_SEARCH, &[], body).await;
    assert_eq!((reply.status, reply.code()), (401, "bad_signature"));

    // Horloge de l'app en retard d'une heure : le relais donne son heure.
    let reply = client.search(&id, "x", T - 3_600).await;
    assert_eq!((reply.status, reply.code()), (401, "clock_skew"));
    assert_eq!(reply.body["error"]["serverTime"], T);
    // En avance de 4 min : accepté.
    assert_eq!(client.search(&id, "x", T + 240).await.status, 200);
}

#[tokio::test]
async fn daily_quota_per_installation() {
    let (_dir, path) = db();
    let relay = TestRelay::start(&path, T, |config| {
        config.quota.daily_per_key = 3;
        config.quota.new_key_daily = 3;
    })
    .await;
    let client = Client::new(&relay);
    let id = Identity::generate();
    client.register(&id, T).await;
    // Un échec de tous les fournisseurs ne compte pas.
    relay.fake.set(Provider::Brave, Behavior::Status(503));
    relay.fake.set(Provider::Mojeek, Behavior::Status(503));
    let reply = client.search(&id, "x", T).await;
    assert_eq!((reply.status, reply.code()), (502, "provider_unavailable"));
    relay.fake.set(Provider::Brave, Behavior::Ok);
    for remaining in [2, 1, 0] {
        let reply = client.search(&id, "x", T).await;
        assert_eq!(reply.body["quota"]["remainingToday"], remaining);
    }
    let reply = client.search(&id, "x", T).await;
    assert_eq!((reply.status, reply.code()), (429, "quota_exceeded"));
    assert_eq!(reply.body["error"]["retryAt"], next_day_start(T));
    // Une autre installation a son propre quota.
    let other = Identity::generate();
    client.register(&other, T).await;
    assert_eq!(client.search(&other, "x", T).await.status, 200);
    // Le lendemain.
    let tomorrow = next_day_start(T) + 10;
    relay.set_time(tomorrow);
    assert_eq!(client.search(&id, "x", tomorrow).await.status, 200);
}

#[tokio::test]
async fn new_keys_can_get_a_smaller_first_day() {
    let (_dir, path) = db();
    let relay = TestRelay::start(&path, T, |config| config.quota.new_key_daily = 1).await;
    let client = Client::new(&relay);
    let id = Identity::generate();
    assert_eq!(client.register(&id, T).await.body["quota"]["dailyLimit"], 1);
    assert_eq!(client.search(&id, "x", T).await.status, 200);
    assert_eq!(client.search(&id, "x", T).await.code(), "quota_exceeded");
    let tomorrow = next_day_start(T) + 10;
    relay.set_time(tomorrow);
    assert_eq!(
        client.search(&id, "x", tomorrow).await.body["quota"]["dailyLimit"],
        50
    );
}

#[tokio::test]
async fn mojeek_takes_over_when_brave_fails() {
    let (_dir, path) = db();
    let relay = TestRelay::start(&path, T, |_| {}).await;
    let client = Client::new(&relay);
    let id = Identity::generate();
    client.register(&id, T).await;
    for failure in [
        Behavior::Status(500),
        Behavior::Status(429),
        Behavior::Status(401),
        Behavior::Garbage,
    ] {
        relay.fake.set(Provider::Brave, failure);
        let reply = client.search(&id, "secours", T).await;
        assert_eq!(reply.status, 200, "{failure:?}: {:?}", reply.body);
        assert_eq!(reply.body["provider"], "mojeek", "{failure:?}");
        assert_eq!(reply.body["results"][0]["url"], "https://example.org/1");
    }
    let sent = relay
        .fake
        .requests()
        .into_iter()
        .find(|r| r.provider == Provider::Mojeek)
        .unwrap();
    assert_eq!(sent.token.as_deref(), Some(MOJEEK_TEST_KEY));
    for (name, value) in [
        ("q", "secours"),
        ("fmt", "json"),
        ("t", "5"),
        ("lb", "FR"),
        ("lbb", "100"),
        ("rb", "FR"),
        ("safe", "1"),
    ] {
        assert_eq!(
            sent.params.get(name).map(String::as_str),
            Some(value),
            "{name}"
        );
    }
    // Mojeek à sa limite du jour et Brave en panne : rien.
    relay.fake.set(Provider::Mojeek, Behavior::DailyLimit);
    let reply = client.search(&id, "secours", T).await;
    assert_eq!((reply.status, reply.code()), (502, "provider_unavailable"));
}

#[tokio::test]
async fn slow_providers_time_out() {
    let (_dir, path) = db();
    let relay = TestRelay::start(&path, T, |_| {}).await;
    let client = Client::new(&relay);
    let id = Identity::generate();
    client.register(&id, T).await;
    relay.fake.set(
        Provider::Brave,
        Behavior::Slow(std::time::Duration::from_secs(5)),
    );
    let started = std::time::Instant::now();
    let reply = client.search(&id, "lent", T).await;
    assert_eq!(reply.body["provider"], "mojeek");
    assert!(started.elapsed() < std::time::Duration::from_secs(4));
    // M5 : la requête est partie chez Brave, qui a pu la facturer : comptée
    // (0,005 €), en plus de Mojeek (0,0025 €).
    assert_eq!(relay.relay.spent_this_month(), 7_500);
}

/// M5 : une connexion impossible n'est pas facturée : rendue au budget.
#[tokio::test]
async fn an_unreachable_provider_is_refunded() {
    let (_dir, path) = db();
    let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", closed.local_addr().unwrap());
    drop(closed);
    let relay = TestRelay::start(&path, T, |config| config.providers.brave.base_url = url).await;
    let client = Client::new(&relay);
    let id = Identity::generate();
    client.register(&id, T).await;
    let reply = client.search(&id, "x", T).await;
    assert_eq!(reply.body["provider"], "mojeek", "{:?}", reply.body);
    assert_eq!(relay.relay.spent_this_month(), 2_500);
}

#[tokio::test]
async fn a_failing_provider_is_avoided_for_a_while() {
    let (_dir, path) = db();
    let relay =
        TestRelay::start(&path, T, |config| config.providers.brave.cooldown_secs = 30).await;
    let client = Client::new(&relay);
    let id = Identity::generate();
    client.register(&id, T).await;
    relay.fake.set(Provider::Brave, Behavior::Status(503));
    client.search(&id, "x", T).await;
    relay.fake.set(Provider::Brave, Behavior::Ok);
    assert_eq!(client.search(&id, "x", T).await.body["provider"], "mojeek");
    assert_eq!(relay.fake.count(Provider::Brave), 1);
    relay.set_time(T + 31);
    assert_eq!(
        client.search(&id, "x", T + 31).await.body["provider"],
        "brave"
    );
}

#[tokio::test]
async fn monthly_budget_cap_survives_restarts() {
    let (_dir, path) = db();
    // 0,012 € : deux recherches Brave (0,005 €) ; la troisième (0,010 + 0,005)
    // ne tient plus, ni chez Mojeek (0,0025 → 0,0125 > 0,012).
    let cap = |config: &mut relay_server::config::Config| {
        config.budget.monthly_limit_eur = 0.012;
        // Sans enveloppe du jour : c'est le plafond du mois qu'on essaie.
        config.budget.daily_share = 31.0;
    };
    let relay = TestRelay::start(&path, T, cap).await;
    let client = Client::new(&relay);
    let id = Identity::generate();
    client.register(&id, T).await;
    assert_eq!(client.search(&id, "x", T).await.status, 200);
    assert_eq!(client.search(&id, "x", T).await.status, 200);
    let reply = client.search(&id, "x", T).await;
    assert_eq!((reply.status, reply.code()), (503, "budget_exhausted"));
    assert_eq!(reply.body["error"]["retryAt"], next_month_start(T));
    assert_eq!(relay.fake.count(Provider::Brave), 2);
    assert_eq!(relay.fake.count(Provider::Mojeek), 0);
    let status = client.signed(&id, "GET", PATH_STATUS, Value::Null, T).await;
    assert_eq!(status.body["service"], "budget_exhausted");
    // Le quota de l'installation n'a pas été entamé par le refus.
    assert_eq!(status.body["quota"]["remainingToday"], 48);
    assert!(
        relay
            .log_lines()
            .iter()
            .any(|line| line.contains("monthly cap reached"))
    );
    assert!(
        relay
            .log_lines()
            .iter()
            .any(|line| line.contains("80% of the monthly cap"))
    );

    // Redémarrage : toujours épuisé.
    let fake = relay.stop().await;
    let relay = TestRelay::start_with(&path, T + 60, fake, cap).await;
    let client = Client::new(&relay);
    let reply = client.search(&id, "x", T + 60).await;
    assert_eq!(reply.code(), "budget_exhausted");
    // Mois suivant : la recherche reprend.
    let next = next_month_start(T) + 60;
    relay.set_time(next);
    assert_eq!(client.search(&id, "x", next).await.status, 200);
}

#[tokio::test]
async fn failed_calls_are_refunded_to_the_budget() {
    let (_dir, path) = db();
    let cap = |config: &mut relay_server::config::Config| {
        config.budget.monthly_limit_eur = 0.006;
        config.budget.daily_share = 31.0;
    };
    let relay = TestRelay::start(&path, T, cap).await;
    let client = Client::new(&relay);
    let id = Identity::generate();
    client.register(&id, T).await;
    relay.fake.set(Provider::Brave, Behavior::Status(500));
    relay.fake.set(Provider::Mojeek, Behavior::Status(500));
    for _ in 0..5 {
        assert_eq!(
            client.search(&id, "x", T).await.code(),
            "provider_unavailable"
        );
    }
    relay.fake.set(Provider::Brave, Behavior::Ok);
    assert_eq!(client.search(&id, "x", T).await.body["provider"], "brave");
}

#[tokio::test]
async fn switches_pause_outdated_and_banned() {
    let (_dir, path) = db();
    let banned = Identity::generate();
    let banned_key = banned.public_key().to_b64();
    let relay = TestRelay::start(&path, T, |config| {
        config.min_app_version = Some("0.10.0".into());
        config.banned_keys = vec![banned_key];
    })
    .await;
    let client = Client::new(&relay);
    let id = Identity::generate();
    client.register(&id, T).await;
    // Version trop ancienne, ou inconnue.
    for agent in ["ChatonLM/0.9.5", "curl/8.0"] {
        let reply = client
            .send(
                "POST",
                PATH_CHALLENGE,
                &[("user-agent", agent.into())],
                b"{}".to_vec(),
            )
            .await;
        assert_eq!((reply.status, reply.code()), (426, "outdated"), "{agent}");
    }
    // Clé bannie.
    let reply = client.register(&banned, T).await;
    assert_eq!((reply.status, reply.code()), (403, "banned"));
    drop(relay);

    let (_dir2, path2) = db();
    let relay = TestRelay::start(&path2, T, |config| config.paused = true).await;
    let client = Client::new(&relay);
    client.register(&id, T).await;
    let reply = client.search(&id, "x", T).await;
    assert_eq!((reply.status, reply.code()), (503, "paused"));
    assert_eq!(
        client
            .signed(&id, "GET", PATH_STATUS, Value::Null, T)
            .await
            .body["service"],
        "paused"
    );
    assert_eq!(relay.fake.requests().len(), 0);
}

#[tokio::test]
async fn limits_per_network() {
    let (_dir, path) = db();
    let relay = TestRelay::start(&path, T, |config| {
        config.quota.per_ip_daily_registrations = Tiers([2, 100, 100]);
        config.quota.per_ip_daily_searches = Tiers([3, 100, 100]);
    })
    .await;
    let ids: Vec<Identity> = (0..4).map(|_| Identity::generate()).collect();
    assert_eq!(
        Client::from_ip(&relay, "203.0.113.1")
            .register(&ids[0], T)
            .await
            .status,
        200
    );
    assert_eq!(
        Client::from_ip(&relay, "203.0.113.2")
            .register(&ids[1], T)
            .await
            .status,
        200
    );
    // Troisième inscription du même /24 : refusée jusqu'à minuit.
    let reply = Client::from_ip(&relay, "203.0.113.3")
        .register(&ids[2], T)
        .await;
    assert_eq!((reply.status, reply.code()), (429, "rate_limited"));
    assert_eq!(reply.body["error"]["retryAt"], next_day_start(T));
    // Autre réseau : libre.
    assert_eq!(
        Client::from_ip(&relay, "198.51.100.1")
            .register(&ids[2], T)
            .await
            .status,
        200
    );
    // Recherches du réseau : 3 par jour, toutes clés confondues.
    let net = Client::from_ip(&relay, "203.0.113.9");
    for id in [&ids[0], &ids[1], &ids[2]] {
        assert_eq!(net.search(id, "x", T).await.status, 200);
    }
    let reply = net.search(&ids[0], "x", T).await;
    assert_eq!((reply.status, reply.code()), (429, "rate_limited"));
    // Le refus n'a pas entamé le quota de la clé.
    let other = Client::from_ip(&relay, "192.0.2.1");
    assert_eq!(
        other.search(&ids[0], "x", T).await.body["quota"]["remainingToday"],
        48
    );
}

#[tokio::test]
async fn forwarded_address_is_ignored_unless_trusted() {
    let (_dir, path) = db();
    let relay = TestRelay::start(&path, T, |config| {
        config.trust_proxy = false;
        config.quota.per_ip_daily_challenges = Tiers([2, 100, 100]);
    })
    .await;
    // Toutes viennent de 127.0.0.1, quel que soit X-Real-IP.
    assert_eq!(
        Client::from_ip(&relay, "203.0.113.1")
            .challenge()
            .await
            .status,
        200
    );
    assert_eq!(
        Client::from_ip(&relay, "198.51.100.1")
            .challenge()
            .await
            .status,
        200
    );
    assert_eq!(
        Client::from_ip(&relay, "192.0.2.1")
            .challenge()
            .await
            .code(),
        "rate_limited"
    );
}

#[tokio::test]
async fn bad_bodies_are_refused() {
    let (_dir, path) = db();
    let relay = TestRelay::start(&path, T, |_| {}).await;
    let client = Client::new(&relay);
    let id = Identity::generate();
    client.register(&id, T).await;
    for body in [
        json!({ "query": "" }),
        json!({ "query": "a".repeat(401) }),
        json!({ "q": "x" }),
        json!({ "query": "x", "lang": "FR" }),
    ] {
        let reply = client
            .signed(&id, "POST", PATH_SEARCH, body.clone(), T)
            .await;
        assert_eq!((reply.status, reply.code()), (400, "bad_request"), "{body}");
    }
    let huge = json!({ "query": "x".repeat(10_000) });
    let reply = client.signed(&id, "POST", PATH_SEARCH, huge, T).await;
    assert_eq!((reply.status, reply.code()), (400, "bad_request"));
    assert_eq!(relay.fake.requests().len(), 0);
}

#[tokio::test]
async fn unused_keys_are_forgotten() {
    let (_dir, path) = db();
    let relay = TestRelay::start(&path, T, |_| {}).await;
    let client = Client::new(&relay);
    let (kept, dropped) = (Identity::generate(), Identity::generate());
    client.register(&kept, T).await;
    client.register(&dropped, T).await;
    let later = T + 60 * 86_400;
    relay.set_time(later);
    assert_eq!(client.search(&kept, "x", later).await.status, 200);
    // Laisse la mise à jour du dernier usage s'écrire.
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let fake = relay.stop().await;
    let restart = T + 100 * 86_400;
    let relay = TestRelay::start_with(&path, restart, fake, |_| {}).await;
    let client = Client::new(&relay);
    assert_eq!(client.search(&kept, "x", restart).await.status, 200);
    assert_eq!(
        client.search(&dropped, "x", restart).await.code(),
        "unknown_key"
    );
}

/// Ni clé de fournisseur, ni requête, ni adresse IP, ni clé d'installation
/// dans le journal ; ni requête ni IP dans la base.
#[tokio::test]
async fn secrets_and_queries_never_leak() {
    let (dir, path) = db();
    let relay = TestRelay::start(&path, T, |_| {}).await;
    let client = Client::from_ip(&relay, "203.0.113.77");
    let id = Identity::generate();
    client.register(&id, T).await;
    let query = "requete-tres-privee-31415";
    // Brave refuse en recopiant la clé dans son corps, Mojeek est à sa limite
    // puis renvoie du charabia, puis tout va bien.
    relay.fake.set(Provider::Brave, Behavior::Status(401));
    relay.fake.set(Provider::Mojeek, Behavior::DailyLimit);
    assert_eq!(
        client.search(&id, query, T).await.code(),
        "provider_unavailable"
    );
    relay.fake.set(Provider::Mojeek, Behavior::Status(500));
    client.search(&id, query, T).await;
    relay.fake.set(Provider::Brave, Behavior::Ok);
    assert_eq!(client.search(&id, query, T).await.status, 200);
    let lines = relay.log_lines();
    assert!(
        lines
            .iter()
            .any(|line| line.contains("provider brave: key refused")),
        "{lines:?}"
    );
    let pk = id.public_key().to_b64();
    for line in &lines {
        for secret in [
            BRAVE_TEST_KEY,
            MOJEEK_TEST_KEY,
            query,
            "203.0.113",
            pk.as_str(),
        ] {
            assert!(!line.contains(secret), "{secret} in {line:?}");
        }
    }
    drop(relay);
    for entry in std::fs::read_dir(dir.path()).unwrap() {
        let bytes = std::fs::read(entry.unwrap().path()).unwrap();
        let text = String::from_utf8_lossy(&bytes);
        assert!(!text.contains(query) && !text.contains("203.0.113"));
    }
}

/// B1 : un titre où `&` précède du texte non ASCII ne coupe plus la
/// connexion (la recherche aboutit, titres intacts).
#[tokio::test]
async fn non_ascii_titles_with_ampersands() {
    let (_dir, path) = db();
    let relay = TestRelay::start(&path, T, |_| {}).await;
    let client = Client::new(&relay);
    let id = Identity::generate();
    client.register(&id, T).await;
    relay.fake.set(Provider::Brave, Behavior::Unicode);
    let reply = client.search(&id, "x", T).await;
    assert_eq!(reply.status, 200, "{:?}", reply.body);
    let titles: Vec<_> = reply.body["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["title"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(titles, UNICODE_TITLES);
    assert_eq!(
        reply.body["results"][0]["content"],
        "Tom & Jérôme à Paris – R&D à l'été"
    );
}

/// I1 : les /56 d'un même /48 partagent un plafond, puis le /32.
#[tokio::test]
async fn wider_networks_cap_registrations() {
    let (_dir, path) = db();
    let relay = TestRelay::start(&path, T, |config| {
        config.quota.per_ip_daily_registrations = Tiers([3, 10, 50]);
        config.quota.per_ip_daily_challenges = Tiers([100, 1_000, 1_000]);
    })
    .await;
    let mut accepted = 0;
    for n in 1..=12u32 {
        let client = Client::from_ip(&relay, &format!("2001:db8:0:{n:x}00::1"));
        let reply = client.register(&Identity::generate(), T).await;
        if reply.status == 200 {
            accepted += 1;
        } else {
            assert_eq!((reply.status, reply.code()), (429, "rate_limited"));
            assert_eq!(reply.body["error"]["retryAt"], next_day_start(T));
        }
    }
    assert_eq!(accepted, 10);
    // Un autre /48 du même /32 : encore permis.
    let other = Client::from_ip(&relay, "2001:db8:1::1");
    assert_eq!(other.register(&Identity::generate(), T).await.status, 200);
}

/// I1 : enveloppe du jour. Un jour ne dépense pas plus de 3 jours moyens ;
/// un redémarrage ne la rouvre pas ; elle se rouvre à minuit.
#[tokio::test]
async fn the_daily_envelope_stops_a_wave_for_the_day_only() {
    let (_dir, path) = db();
    // 0,30 € en novembre : 0,03 € par jour, soit 6 recherches Brave.
    let cap = |config: &mut relay_server::config::Config| config.budget.monthly_limit_eur = 0.3;
    let relay = TestRelay::start(&path, T, cap).await;
    let client = Client::new(&relay);
    let id = Identity::generate();
    client.register(&id, T).await;
    for _ in 0..6 {
        assert_eq!(client.search(&id, "x", T).await.status, 200);
    }
    let reply = client.search(&id, "x", T).await;
    assert_eq!((reply.status, reply.code()), (503, "budget_exhausted"));
    assert_eq!(reply.body["error"]["retryAt"], next_day_start(T));
    assert!(
        relay
            .log_lines()
            .iter()
            .any(|line| line.contains("daily envelope reached"))
    );
    let status = client.signed(&id, "GET", PATH_STATUS, Value::Null, T).await;
    assert_eq!(status.body["service"], "budget_exhausted");
    // Redémarrage : la dépense du jour est relue.
    let fake = relay.stop().await;
    let relay = TestRelay::start_with(&path, T + 60, fake, cap).await;
    let client = Client::new(&relay);
    assert_eq!(
        client.search(&id, "x", T + 60).await.code(),
        "budget_exhausted"
    );
    // Le lendemain.
    let tomorrow = next_day_start(T) + 5;
    relay.set_time(tomorrow);
    assert_eq!(client.search(&id, "x", tomorrow).await.status, 200);
}

/// I2 : la cadence par clé passe avant la table anti-rejeu (une requête
/// refusée n'y laisse pas son nonce) ; pleine, la table ne refuse pas les
/// autres clés.
#[tokio::test]
async fn the_replay_table_cannot_be_filled_by_one_key() {
    let (_dir, path) = db();
    let relay = TestRelay::start(&path, T, |config| config.quota.per_key_per_minute = 1).await;
    let id = Identity::generate();
    Client::new(&relay).register(&id, T).await;
    for n in 0..50u32 {
        let client = Client::from_ip(&relay, &format!("198.51.{}.{}", n / 200, n % 200 + 1));
        client.signed(&id, "GET", PATH_STATUS, Value::Null, T).await;
    }
    assert_eq!(relay.relay.replay_len(), 1);
    drop(relay);

    let (_dir, path) = db();
    let relay = TestRelay::start(&path, T, |config| config.tables.replay = 4).await;
    let (busy, other) = (Identity::generate(), Identity::generate());
    let client = Client::new(&relay);
    client.register(&busy, T).await;
    client.register(&other, T).await;
    for _ in 0..10 {
        let reply = client
            .signed(&busy, "GET", PATH_STATUS, Value::Null, T)
            .await;
        assert_eq!(reply.status, 200, "{:?}", reply.body);
    }
    assert_eq!(relay.relay.replay_len(), 4);
    let reply = client
        .signed(&other, "GET", PATH_STATUS, Value::Null, T)
        .await;
    assert_eq!(reply.status, 200, "{:?}", reply.body);
}

/// I3 : des défis venus de réseaux sans nombre ne coupent ni la recherche ni
/// l'inscription des autres : seuls les défis attendent le lendemain ; les
/// compteurs par réseau oublient les moins récents au lieu de refuser.
#[tokio::test]
async fn full_network_tables_only_refuse_challenges() {
    let (_dir, path) = db();
    let relay = TestRelay::start(&path, T, |config| {
        // L'inscription du début prend 2 places (/24 et /16 de 127.0.0.1).
        config.tables.challenges = 3;
        config.tables.network_daily = 4;
        config.tables.network_minute = 4;
    })
    .await;
    let id = Identity::generate();
    assert_eq!(Client::new(&relay).register(&id, T).await.status, 200);
    for n in 0..30u32 {
        let client = Client::from_ip(&relay, &format!("2001:db8:{n:x}::1"));
        client.challenge().await;
        client.search(&id, "x", T).await;
    }
    let (minute, daily, challenges) = relay.relay.network_tables();
    assert!(minute <= 4 && daily <= 4 && challenges <= 3);
    // Un utilisateur d'un réseau neuf cherche toujours.
    let user = Client::from_ip(&relay, "192.0.2.10");
    let reply = user.search(&id, "x", T).await;
    assert_eq!(reply.status, 200, "{:?}", reply.body);
    // Seul son défi attend.
    let reply = user.challenge().await;
    assert_eq!((reply.status, reply.code()), (429, "rate_limited"));
    assert_eq!(reply.body["error"]["retryAt"], next_day_start(T));
}

/// M2 : plus d'inscription possible depuis ce réseau aujourd'hui → le défi
/// est refusé, avant que l'app calcule sa preuve pour rien.
#[tokio::test]
async fn no_challenge_once_the_network_cannot_register() {
    let (_dir, path) = db();
    let relay = TestRelay::start(&path, T, |config| {
        config.quota.per_ip_daily_registrations = Tiers([1, 100, 100]);
    })
    .await;
    let client = Client::from_ip(&relay, "203.0.113.5");
    assert_eq!(client.register(&Identity::generate(), T).await.status, 200);
    let reply = Client::from_ip(&relay, "203.0.113.6").challenge().await;
    assert_eq!((reply.status, reply.code()), (429, "rate_limited"));
    assert_eq!(reply.body["error"]["retryAt"], next_day_start(T));
    // Un autre réseau en a toujours.
    assert_eq!(
        Client::from_ip(&relay, "198.51.100.5")
            .challenge()
            .await
            .status,
        200
    );
}

/// M9 : vérifications de preuve toutes prises → `rate_limited` tout de
/// suite (pas de file sans fin), et l'unité d'inscription est rendue.
#[tokio::test]
async fn busy_proof_checks_answer_at_once() {
    let (_dir, path) = db();
    let relay = TestRelay::start(&path, T, |config| {
        config.quota.per_ip_daily_registrations = Tiers([1, 100, 100]);
    })
    .await;
    let client = Client::new(&relay);
    let id = Identity::generate();
    let slots = relay.relay.occupy_pow_slots();
    assert!(!slots.is_empty());
    let reply = tokio::time::timeout(std::time::Duration::from_secs(5), client.register(&id, T))
        .await
        .expect("aucune attente");
    assert_eq!((reply.status, reply.code()), (429, "rate_limited"));
    assert_eq!(reply.body["error"]["retryAt"], T + 10);
    drop(slots);
    assert_eq!(client.register(&id, T).await.status, 200);
}

/// M5 : Brave reçoit ses propres codes de langue (`ja` → `jp`) ; une langue
/// qu'il ne connaît pas n'est pas envoyée (sinon 422, et Mojeek à chaque fois).
#[tokio::test]
async fn brave_receives_its_own_language_codes() {
    let (_dir, path) = db();
    let relay = TestRelay::start(&path, T, |_| {}).await;
    let client = Client::new(&relay);
    let id = Identity::generate();
    client.register(&id, T).await;
    for (lang, sent) in [("ja", Some("jp")), ("pt", Some("pt-br")), ("xx", None)] {
        let reply = client
            .signed(
                &id,
                "POST",
                PATH_SEARCH,
                json!({ "query": "x", "lang": lang }),
                T,
            )
            .await;
        assert_eq!(reply.body["provider"], "brave", "{lang}");
        let request = relay.fake.requests().pop().unwrap();
        assert_eq!(
            request.params.get("search_lang").map(String::as_str),
            sent,
            "{lang}"
        );
    }
}
