//! Essais du client contre les faux moteurs (`fake_services::search`) : ordre
//! moteur personnel → DuckDuckGo, blocage et pause, rythme, annulation,
//! langue transmise, clés refusées, SearXNG sans JSON.

use std::time::Duration;

use fake_services::search::{BAD_KEY, Behavior, FakeSearch};
use tokio_util::sync::CancellationToken;

use super::*;

fn client(fake: &FakeSearch, gap: Duration) -> Client {
    Client::new(Options {
        user_agent: "ChatonLM/test".into(),
        endpoints: Endpoints {
            duckduckgo: fake.base("duckduckgo"),
            brave: fake.base("brave"),
            mojeek: fake.base("mojeek"),
        },
        duckduckgo_gap: gap,
        duckduckgo_pause: Duration::from_secs(60),
        timeout: Duration::from_secs(5),
    })
}

fn query(text: &str) -> Query<'_> {
    Query::new(text, Some(2), Some("fr")).unwrap()
}

fn engines(fake: &FakeSearch) -> Vec<&'static str> {
    fake.requests().iter().map(|r| r.engine).collect()
}

#[test]
fn queries_are_checked() {
    assert!(Query::new("  ", None, None).is_none());
    assert!(Query::new("a\u{0}b", None, None).is_none());
    assert!(Query::new(&"a".repeat(MAX_QUERY_CHARS + 1), None, None).is_none());
    assert!(Query::new(&"a ".repeat(MAX_QUERY_WORDS + 1), None, None).is_none());
    let ok = Query::new(" chat ", None, Some("fr")).unwrap();
    assert_eq!(
        (ok.text, ok.max_results, ok.lang),
        ("chat", DEFAULT_RESULTS, Some("fr"))
    );
    assert_eq!(
        Query::new("x", Some(50), None).unwrap().max_results,
        MAX_RESULTS
    );
    assert_eq!(Query::new("x", Some(0), None).unwrap().max_results, 1);
    assert_eq!(Query::new("x", None, Some("FR\"")).unwrap().lang, None);
}

#[tokio::test]
async fn duckduckgo_answers_by_default() {
    let fake = FakeSearch::start().await.unwrap();
    let client = client(&fake, Duration::ZERO);
    let found = client
        .search(None, &query("chaton roux"), &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(found.provider, Provider::DuckDuckGo);
    assert_eq!(found.personal_failed, None);
    // Bornées à 2, titre et extrait nettoyés (balise <b> du faux retirée).
    assert_eq!(found.results.len(), 2);
    assert_eq!(
        found.results[0].title,
        "DuckDuckGo result 1 for chaton roux"
    );
    assert_eq!(found.results[0].url, "https://duckduckgo.example/1");
    assert_eq!(found.results[0].content, "Extrait 1 (duckduckgo)");
    let requests = fake.requests();
    assert_eq!(requests[0].query, "chaton roux");
    assert_eq!(requests[0].lang, "fr-fr");
    fake.stop().await;
}

#[tokio::test]
async fn the_personal_engine_comes_first() {
    let fake = FakeSearch::start().await.unwrap();
    let client = client(&fake, Duration::ZERO);
    let searxng = fake.base("searxng");
    for (personal, provider, lang) in [
        (Personal::Brave { key: "k".into() }, Provider::Brave, "fr"),
        (Personal::Mojeek { key: "k".into() }, Provider::Mojeek, "FR"),
        (
            Personal::Searxng {
                url: format!("{searxng}/"),
            },
            Provider::Searxng,
            "fr",
        ),
    ] {
        let found = client
            .search(Some(&personal), &query("chat"), &CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(found.provider, provider);
        assert_eq!(
            found.results[1].content,
            format!("Extrait 2 ({})", provider.as_str())
        );
        assert_eq!(fake.requests().last().unwrap().lang, lang);
    }
    // DuckDuckGo n'a jamais été demandé.
    assert_eq!(engines(&fake), ["brave", "mojeek", "searxng"]);
    fake.stop().await;
}

#[tokio::test]
async fn duckduckgo_takes_over_when_the_personal_engine_fails() {
    let fake = FakeSearch::start().await.unwrap();
    let client = client(&fake, Duration::ZERO);
    let cancel = CancellationToken::new();
    let bad = Personal::Brave {
        key: BAD_KEY.into(),
    };
    let found = client
        .search(Some(&bad), &query("chat"), &cancel)
        .await
        .unwrap();
    assert_eq!(found.provider, Provider::DuckDuckGo);
    assert_eq!(
        found.personal_failed,
        Some((Provider::Brave, EngineError::Auth))
    );
    assert_eq!(engines(&fake), ["brave", "duckduckgo"]);

    fake.set("mojeek", Behavior::Status(503));
    let mojeek = Personal::Mojeek { key: "k".into() };
    let found = client
        .search(Some(&mojeek), &query("chat"), &cancel)
        .await
        .unwrap();
    assert_eq!(
        found.personal_failed,
        Some((Provider::Mojeek, EngineError::Unavailable))
    );

    // Les deux échouent : l'échec de chacun est rendu.
    fake.set("duckduckgo", Behavior::Status(500));
    let error = client.search(Some(&bad), &query("chat"), &cancel).await;
    assert_eq!(
        error,
        Err(SearchError::Failed {
            personal: Some((Provider::Brave, EngineError::Auth)),
            duckduckgo: EngineError::Unavailable,
        })
    );
    fake.stop().await;
}

#[tokio::test]
async fn searxng_without_json_and_bad_addresses() {
    let fake = FakeSearch::start().await.unwrap();
    let client = client(&fake, Duration::ZERO);
    let cancel = CancellationToken::new();
    let searxng = Personal::Searxng {
        url: fake.base("searxng"),
    };
    fake.set("searxng", Behavior::Html);
    assert_eq!(
        client.ask(&searxng, &query("chat"), &cancel).await,
        Err(Some(EngineError::JsonDisabled))
    );
    fake.set("searxng", Behavior::Status(403));
    assert_eq!(
        client.ask(&searxng, &query("chat"), &cancel).await,
        Err(Some(EngineError::JsonDisabled))
    );
    let nowhere = Personal::Searxng {
        url: "searx.example".into(),
    };
    assert_eq!(
        client.ask(&nowhere, &query("chat"), &cancel).await,
        Err(Some(EngineError::BadAddress))
    );
    // Instance fermée : hors ligne.
    let closed = Personal::Searxng {
        url: "http://127.0.0.1:9".into(),
    };
    assert_eq!(
        client.ask(&closed, &query("chat"), &cancel).await,
        Err(Some(EngineError::Offline))
    );
    fake.stop().await;
}

/// Blocage de DuckDuckGo : erreur distincte, puis pause (aucune nouvelle
/// requête pendant ce temps).
#[tokio::test]
async fn a_block_is_distinct_and_pauses_duckduckgo() {
    let fake = FakeSearch::start().await.unwrap();
    let client = client(&fake, Duration::ZERO);
    let cancel = CancellationToken::new();
    fake.set("duckduckgo", Behavior::Blocked);
    let blocked = Err(SearchError::Failed {
        personal: None,
        duckduckgo: EngineError::Blocked,
    });
    assert_eq!(client.search(None, &query("chat"), &cancel).await, blocked);
    fake.set("duckduckgo", Behavior::Ok);
    assert_eq!(client.search(None, &query("chat"), &cancel).await, blocked);
    assert_eq!(engines(&fake), ["duckduckgo"]);
    // Un 429 aussi est un blocage.
    let other = FakeSearch::start().await.unwrap();
    other.set("duckduckgo", Behavior::Status(429));
    let fresh = self::client(&other, Duration::ZERO);
    assert_eq!(
        fresh.search(None, &query("chat"), &cancel).await,
        Err(SearchError::Failed {
            personal: None,
            duckduckgo: EngineError::Blocked,
        })
    );
    other.stop().await;
    fake.stop().await;
}

/// Deux recherches de suite vers DuckDuckGo : la seconde attend
/// l'intervalle minimal.
#[tokio::test]
async fn duckduckgo_requests_are_spaced() {
    let fake = FakeSearch::start().await.unwrap();
    let gap = Duration::from_millis(400);
    let client = client(&fake, gap);
    let cancel = CancellationToken::new();
    client.search(None, &query("un"), &cancel).await.unwrap();
    let started = std::time::Instant::now();
    client.search(None, &query("deux"), &cancel).await.unwrap();
    assert!(
        started.elapsed() >= gap - Duration::from_millis(50),
        "{:?}",
        started.elapsed()
    );
    fake.stop().await;
}

/// Rythme seul, en temps simulé : intervalle, file des appels concurrents,
/// pause après un blocage, annulation pendant l'attente.
#[tokio::test(start_paused = true)]
async fn the_pacer_keeps_its_rhythm() {
    let pacer = Pacer::new(Duration::from_secs(2), Duration::from_secs(60));
    let cancel = CancellationToken::new();
    let start = Instant::now();
    pacer.turn(&cancel).await.unwrap();
    assert_eq!(start.elapsed(), Duration::ZERO);
    pacer.turn(&cancel).await.unwrap();
    assert_eq!(start.elapsed(), Duration::from_secs(2));
    tokio::time::advance(Duration::from_secs(5)).await;
    let before = Instant::now();
    pacer.turn(&cancel).await.unwrap();
    assert_eq!(before.elapsed(), Duration::ZERO);

    let stopped = CancellationToken::new();
    let waiting = pacer.turn(&stopped);
    stopped.cancel();
    assert_eq!(waiting.await, Err(None));

    pacer.blocked().await;
    tokio::time::advance(Duration::from_secs(30)).await;
    assert_eq!(pacer.turn(&cancel).await, Err(Some(EngineError::Blocked)));
    tokio::time::advance(Duration::from_secs(31)).await;
    assert_eq!(pacer.turn(&cancel).await, Ok(()));
}

#[tokio::test]
async fn cancelling_stops_a_slow_search() {
    let fake = FakeSearch::start().await.unwrap();
    fake.set("brave", Behavior::Slow(Duration::from_secs(30)));
    let client = client(&fake, Duration::ZERO);
    let cancel = CancellationToken::new();
    let personal = Personal::Brave { key: "k".into() };
    let chat = query("chat");
    let searching = client.search(Some(&personal), &chat, &cancel);
    let stopper = async {
        tokio::time::sleep(Duration::from_millis(200)).await;
        cancel.cancel();
    };
    let (result, ()) = tokio::join!(searching, stopper);
    assert_eq!(result, Err(SearchError::Cancelled));
    // Pas de secours vers DuckDuckGo après une annulation.
    assert_eq!(engines(&fake), ["brave"]);
    fake.stop().await;
}

#[test]
fn the_key_never_shows_in_debug_output() {
    let personal = Personal::Brave {
        key: "BSA-very-secret".into(),
    };
    assert!(!format!("{personal:?}").contains("secret"));
}
