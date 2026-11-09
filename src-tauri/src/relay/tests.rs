//! Client de l'app contre le vrai relais (crate `relay-server`) lancé dans le
//! processus, avec de faux fournisseurs : inscription, recherche, réinscription,
//! horloge, erreurs codées et traduites, clé gardée.

use super::*;
use relay_proto::api::Provider;
use relay_server::fake::{Behavior, TestRelay};

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

fn client(relay: &TestRelay, dir: &std::path::Path) -> RelayClient {
    RelayClient::with_url(relay.url.clone(), dir.to_path_buf())
}

#[tokio::test]
async fn registers_then_searches_and_keeps_its_key() {
    let dir = tempfile::tempdir().unwrap();
    // Un seul défi par réseau et par jour : un second client qui reprend la clé
    // ne doit pas se réinscrire.
    let relay = TestRelay::start(&dir.path().join("relay.sqlite"), now(), |config| {
        config.quota.per_ip_daily_challenges = relay_server::config::Tiers([1, 1, 1]);
    })
    .await;
    let data = dir.path().join("data");
    let first = client(&relay, &data);
    let cancel = CancellationToken::new();
    let reply = first
        .search("chaton roux", Some(3), Some("fr"), &cancel)
        .await
        .unwrap();
    assert_eq!(reply.provider, Provider::Brave);
    assert_eq!(reply.results.len(), 3);
    assert_eq!(reply.results[0].url, "https://example.org/1");
    assert_eq!(reply.quota.remaining_today, 49);
    let sent = &relay.fake.requests()[0];
    assert_eq!(
        sent.params.get("search_lang").map(String::as_str),
        Some("fr")
    );

    let path = data.join(identity::DIR).join(identity::FILE);
    let saved: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(saved["registered"][0], relay.url.as_str());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    let second = client(&relay, &data);
    let status = second.status(&cancel).await.unwrap();
    assert_eq!(status.quota.remaining_today, 49);
    assert_eq!(
        second.identity.load().unwrap().key.public_key(),
        first.identity.load().unwrap().key.public_key()
    );
}

#[tokio::test]
async fn registers_again_when_the_relay_forgot_the_key() {
    let dir = tempfile::tempdir().unwrap();
    let relay = TestRelay::start(&dir.path().join("relay.sqlite"), now(), |_| {}).await;
    let data = dir.path().join("data");
    // Clé qui se croit inscrite auprès de ce relais, qui ne la connaît pas.
    let store = identity::Store::new(data.join(identity::DIR).join(identity::FILE));
    store.mark_registered(&relay.url).unwrap();
    let client = client(&relay, &data);
    let reply = client
        .search("x", None, None, &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(reply.results.len(), 5);
}

#[tokio::test]
async fn adjusts_to_a_skewed_clock() {
    let dir = tempfile::tempdir().unwrap();
    // Le relais a une heure d'avance sur cette machine. Son horloge d'essai est figée : le
    // décalage adopté vaut son heure moins l'heure locale du moment, qui avance pendant la preuve
    // de travail (plusieurs secondes sous charge) ; il est donc encadré, pas comparé à 3 600.
    let before = now();
    let server = before + 3_600;
    let relay = TestRelay::start(&dir.path().join("relay.sqlite"), server, |_| {}).await;
    let client = client(&relay, &dir.path().join("data"));
    client
        .search("x", None, None, &CancellationToken::new())
        .await
        .unwrap();
    let after = now();
    let offset = client.offset.load(Ordering::Relaxed);
    let lowest = i64::try_from(server - after).unwrap() - 1;
    assert!(
        (lowest..=3_601).contains(&offset),
        "{offset} ∉ [{lowest}, 3601]"
    );
    assert!(
        lowest > 3_000,
        "essai trop lent pour être probant : {lowest}"
    );
}

#[tokio::test]
async fn relay_errors_are_typed() {
    let dir = tempfile::tempdir().unwrap();
    let cancel = CancellationToken::new();
    let relay = TestRelay::start(&dir.path().join("relay.sqlite"), now(), |config| {
        config.quota.daily_per_key = 1;
        config.quota.new_key_daily = 1;
    })
    .await;
    let c = client(&relay, &dir.path().join("data"));
    relay.fake.set(Provider::Brave, Behavior::Status(500));
    relay.fake.set(Provider::Mojeek, Behavior::Status(500));
    assert_eq!(
        c.search("x", None, None, &cancel).await.unwrap_err(),
        RelayError::ProviderUnavailable
    );
    relay.fake.set(Provider::Brave, Behavior::Ok);
    c.search("x", None, None, &cancel).await.unwrap();
    let error = c.search("x", None, None, &cancel).await.unwrap_err();
    assert!(
        matches!(error, RelayError::QuotaExceeded { retry_at: Some(_) }),
        "{error:?}"
    );
    // Requête refusée avant tout envoi.
    let requests = relay.fake.requests().len();
    assert_eq!(
        c.search("  ", None, None, &cancel).await.unwrap_err(),
        RelayError::InvalidQuery
    );
    assert_eq!(
        c.search("x", None, Some("FRA"), &cancel).await.unwrap_err(),
        RelayError::InvalidQuery
    );
    assert_eq!(relay.fake.requests().len(), requests);
    drop(relay);

    let paused = TestRelay::start(&dir.path().join("paused.sqlite"), now(), |config| {
        config.paused = true
    })
    .await;
    let c = client(&paused, &dir.path().join("data"));
    assert_eq!(
        c.search("x", None, None, &cancel).await.unwrap_err(),
        RelayError::Paused
    );
    drop(paused);

    let budget = TestRelay::start(&dir.path().join("budget.sqlite"), now(), |config| {
        config.budget.monthly_limit_eur = 0.0
    })
    .await;
    let c = client(&budget, &dir.path().join("data"));
    let error = c.search("x", None, None, &cancel).await.unwrap_err();
    assert!(
        matches!(error, RelayError::BudgetExhausted { retry_at: Some(_) }),
        "{error:?}"
    );
    drop(budget);

    let old = TestRelay::start(&dir.path().join("old.sqlite"), now(), |config| {
        config.min_app_version = Some("99.0.0".into());
    })
    .await;
    let c = client(&old, &dir.path().join("data"));
    assert_eq!(
        c.search("x", None, None, &cancel).await.unwrap_err(),
        RelayError::Outdated
    );
    drop(old);

    // Rien n'écoute.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let closed = format!("http://{}", listener.local_addr().unwrap());
    drop(listener);
    let c = RelayClient::with_url(closed, dir.path().join("data"));
    assert_eq!(
        c.search("x", None, None, &cancel).await.unwrap_err(),
        RelayError::Offline
    );
}

#[tokio::test]
async fn registration_can_be_cancelled() {
    let dir = tempfile::tempdir().unwrap();
    // La plus longue preuve permise (≈ 16 s optimisée, bien plus ici) :
    // l'annulation doit l'arrêter aussitôt.
    let relay = TestRelay::start(&dir.path().join("relay.sqlite"), now(), |config| {
        config.pow.memory_kib = 32 * 1024;
        config.pow.bits = relay_proto::pow::MAX_BITS;
        config.pow.count = 1;
    })
    .await;
    let c = Arc::new(client(&relay, &dir.path().join("data")));
    let cancel = CancellationToken::new();
    let started = std::time::Instant::now();
    let search = tokio::spawn({
        let c = c.clone();
        let cancel = cancel.clone();
        async move { c.search("x", None, None, &cancel).await }
    });
    tokio::time::sleep(Duration::from_millis(300)).await;
    cancel.cancel();
    assert_eq!(search.await.unwrap().unwrap_err(), RelayError::Cancelled);
    assert!(started.elapsed() < Duration::from_secs(5));
    assert!(relay.fake.requests().is_empty());
}

#[tokio::test]
async fn a_malformed_key_file_is_replaced() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(identity::DIR).join(identity::FILE);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, b"{ pas du json").unwrap();
    let store = identity::Store::new(path.clone());
    let key = store.load().unwrap().key.public_key();
    let again = identity::Store::new(path).load().unwrap().key.public_key();
    assert_eq!(key, again);
}

#[test]
fn every_error_has_a_text_in_every_language() {
    let errors = [
        RelayError::Offline,
        RelayError::QuotaExceeded { retry_at: None },
        RelayError::RateLimited { retry_at: None },
        RelayError::BudgetExhausted { retry_at: None },
        RelayError::Paused,
        RelayError::Outdated,
        RelayError::Banned,
        RelayError::ProviderUnavailable,
        RelayError::Registration,
        RelayError::Rejected(ErrorCode::Replay),
        RelayError::InvalidQuery,
        RelayError::Malformed,
        RelayError::Storage("disk full".into()),
        RelayError::Cancelled,
    ];
    for lang in Lang::ALL {
        for error in &errors {
            let text = error.message_in(lang);
            assert!(
                !text.is_empty() && !text.starts_with("relay."),
                "{lang:?} {error:?}: {text}"
            );
        }
    }
    // M10 : ni l'erreur du système ni un chemin dans le texte montré.
    for lang in Lang::ALL {
        let text =
            RelayError::Storage("/home/alice/.local/share/x: disk full".into()).message_in(lang);
        assert!(
            !text.contains("disk full") && !text.contains("alice"),
            "{text}"
        );
    }
    // Enveloppe du jour (reprise demain) ou plafond du mois (reprise le 1er).
    let tomorrow = relay_proto::time::next_day_start(now());
    let month = relay_proto::time::next_month_start(now());
    for lang in Lang::ALL {
        let today = RelayError::BudgetExhausted {
            retry_at: Some(tomorrow),
        }
        .message_in(lang);
        let monthly = RelayError::BudgetExhausted {
            retry_at: Some(month),
        }
        .message_in(lang);
        assert_eq!(today == monthly, tomorrow == month, "{lang:?}");
    }
    assert_eq!(
        RelayError::BudgetExhausted { retry_at: None }.message_in(Lang::En),
        RelayError::BudgetExhausted {
            retry_at: Some(month)
        }
        .message_in(Lang::En)
    );
}

/// M3 : un 429 qui n'est pas du JSON (page de nginx, `limit_req`) reste un
/// refus de cadence, pas une réponse illisible.
#[tokio::test]
async fn a_plain_429_is_rate_limited() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let dir = tempfile::tempdir().unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            let mut buffer = [0u8; 4096];
            let _ = socket.read(&mut buffer).await;
            let body = "<html><body>429 Too Many Requests</body></html>";
            let reply = format!(
                "HTTP/1.1 429 Too Many Requests\r\ncontent-type: text/html\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = socket.write_all(reply.as_bytes()).await;
        }
    });
    let c = RelayClient::with_url(url, dir.path().join("data"));
    assert_eq!(
        c.search("x", None, None, &CancellationToken::new())
            .await
            .unwrap_err(),
        RelayError::RateLimited { retry_at: None }
    );
}

/// M2 : inscription refusée (réseau à son plafond du jour) → aucun nouvel
/// essai, ni défi ni preuve, avant l'heure de reprise donnée par le relais.
#[tokio::test]
async fn a_refused_registration_waits_for_its_retry_time() {
    let dir = tempfile::tempdir().unwrap();
    let relay = TestRelay::start(&dir.path().join("relay.sqlite"), now(), |config| {
        config.quota.per_ip_daily_registrations = relay_server::config::Tiers([1, 100, 100]);
    })
    .await;
    let cancel = CancellationToken::new();
    client(&relay, &dir.path().join("first"))
        .search("x", None, None, &cancel)
        .await
        .unwrap();
    let second = client(&relay, &dir.path().join("second"));
    let error = second.search("x", None, None, &cancel).await.unwrap_err();
    let tomorrow = relay_proto::time::next_day_start(now());
    assert_eq!(
        error,
        RelayError::RateLimited {
            retry_at: Some(tomorrow)
        }
    );
    // Relais arrêté (plus rien n'écoute) : le client ne l'appelle même pas,
    // sinon ce serait « injoignable ».
    let _providers = relay.stop().await;
    assert_eq!(
        second.search("x", None, None, &cancel).await.unwrap_err(),
        RelayError::RateLimited {
            retry_at: Some(tomorrow)
        }
    );
}

#[test]
fn relay_address() {
    // Sans variable : l'adresse officielle (la variable n'est lue que dans les
    // builds d'essai).
    if std::env::var_os("CHATONLM_RELAY_URL").is_none() {
        assert_eq!(relay_url(), DEFAULT_URL);
    }
}
