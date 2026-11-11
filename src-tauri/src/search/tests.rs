//! Recherche web de l'app : moteur personnel lu en base, codes et textes des
//! échecs, essai d'un moteur (bouton « Test »), clé de l'ancien relais
//! effacée ; contrat contre le vrai DuckDuckGo (`#[ignore]`, réseau requis).

use fake_services::search::{BAD_KEY, Behavior, FakeSearch};
use web_search::{EngineError as E, Provider as P};

use super::*;

fn failed(personal: Option<(P, E)>, duckduckgo: E) -> SearchError {
    SearchError::Failed {
        personal,
        duckduckgo,
    }
}

fn on(fake: &FakeSearch) -> (WebSearch, Store) {
    let store = Store::open_in_memory().unwrap();
    let search = WebSearch::with_endpoints(store.clone(), crate::chat::tools::fake_endpoints(fake));
    (search, store)
}

async fn choose(store: &Store, engine: Engine, secret: &str) {
    let secret = secret.to_owned();
    store
        .call(move |c| saved::choose(c, engine, Some(&secret)))
        .await
        .unwrap();
}

/// Ce que l'utilisateur peut corriger passe d'abord ; puis le blocage de
/// DuckDuckGo (avec le conseil d'un moteur personnel s'il n'en a pas) ;
/// puis l'absence de réseau.
#[test]
fn each_failure_has_its_code_and_its_text() {
    use ChatErrorKind as K;
    let lang = Lang::En;
    let cases = [
        (
            failed(None, E::Blocked),
            K::WebSearchRefused,
            t!("webSearch.blocked" in lang),
        ),
        (
            failed(Some((P::Brave, E::Offline)), E::Blocked),
            K::WebSearchRefused,
            t!("webSearch.blockedWithPersonal" in lang, engine = "Brave Search"),
        ),
        (
            failed(Some((P::Brave, E::Auth)), E::Blocked),
            K::WebSearchRefused,
            t!("webSearch.keyRefused" in lang, engine = "Brave Search"),
        ),
        (
            failed(Some((P::Mojeek, E::Exhausted)), E::Offline),
            K::WebSearchRefused,
            t!("webSearch.exhausted" in lang, engine = "Mojeek"),
        ),
        (
            failed(Some((P::Searxng, E::JsonDisabled)), E::Unavailable),
            K::WebSearchRefused,
            t!("webSearch.jsonDisabled" in lang),
        ),
        (
            failed(Some((P::Searxng, E::BadAddress)), E::Timeout),
            K::WebSearchRefused,
            t!("webSearch.badAddress" in lang),
        ),
        (
            failed(None, E::Offline),
            K::WebSearchUnavailable,
            t!("webSearch.offline" in lang),
        ),
        (
            failed(Some((P::Searxng, E::Offline)), E::Offline),
            K::WebSearchUnavailable,
            t!("webSearch.offline" in lang),
        ),
        (
            failed(Some((P::Brave, E::Unavailable)), E::Offline),
            K::WebSearchUnavailable,
            t!("webSearch.unavailable" in lang),
        ),
        (
            failed(None, E::Malformed),
            K::WebSearchUnavailable,
            t!("webSearch.unavailable" in lang),
        ),
        (
            SearchError::InvalidQuery,
            K::WebSearchUnavailable,
            t!("webSearch.invalidQuery" in lang),
        ),
    ];
    let mut texts = std::collections::HashSet::new();
    for (error, kind, text) in cases {
        assert_eq!(super::kind(&error), Some(kind), "{error:?}");
        assert_eq!(message_in(&error, lang), text, "{error:?}");
        assert!(
            !text.is_empty() && !text.starts_with("webSearch."),
            "{text}"
        );
        texts.insert(text);
    }
    // Chaque texte est le sien (aucune clé ne retombe sur une autre).
    assert_eq!(texts.len(), 9);
    assert_eq!(super::kind(&SearchError::Cancelled), None);
    // Le conseil d'un moteur personnel n'est donné qu'à qui n'en a pas.
    assert!(t!("webSearch.blocked" in lang).contains("Settings › Web search"));
    // Code donné au modèle : sans texte, ni clé.
    assert_eq!(
        code(&failed(Some((P::Brave, E::Auth)), E::Blocked)),
        "brave keyRefused, duckduckgo blocked"
    );
}

/// Le moteur personnel est relu en base à chaque recherche.
#[tokio::test]
async fn the_personal_engine_is_read_at_each_search() {
    let fake = FakeSearch::start().await.unwrap();
    let (search, store) = on(&fake);
    let never = CancellationToken::new();
    let found = search.search("chat", None, None, &never).await.unwrap();
    assert_eq!(found.provider, P::DuckDuckGo);
    choose(&store, Engine::Searxng, &fake.base("searxng")).await;
    let found = search
        .search("chat", None, Some("fr"), &never)
        .await
        .unwrap();
    assert_eq!(found.provider, P::Searxng);
    assert_eq!(fake.requests().last().unwrap().lang, "fr");
    // Clé refusée : DuckDuckGo en secours, l'échec gardé.
    choose(&store, Engine::Brave, BAD_KEY).await;
    let found = search.search("chat", None, None, &never).await.unwrap();
    assert_eq!(found.provider, P::DuckDuckGo);
    assert_eq!(found.personal_failed, Some((P::Brave, E::Auth)));
    assert_eq!(
        search.search(" ", None, None, &never).await,
        Err(SearchError::InvalidQuery)
    );
    fake.stop().await;
}

/// Essai d'un moteur seul : réussi, sans clé, clé refusée, JSON coupé,
/// DuckDuckGo bloqué ; jamais de secours vers DuckDuckGo.
#[tokio::test]
async fn an_engine_is_checked_alone() {
    let fake = FakeSearch::start().await.unwrap();
    let (search, store) = on(&fake);
    let never = CancellationToken::new();
    assert_eq!(
        search.check(Engine::Brave, &never).await,
        Checked::NotConfigured
    );
    choose(&store, Engine::Brave, BAD_KEY).await;
    assert_eq!(
        search.check(Engine::Brave, &never).await,
        Checked::Failed(E::Auth)
    );
    choose(&store, Engine::Brave, "BSA-good").await;
    assert_eq!(search.check(Engine::Brave, &never).await, Checked::Ok);
    // Le moteur essayé n'a pas besoin d'être celui qui est choisi.
    choose(&store, Engine::Searxng, &fake.base("searxng")).await;
    choose(&store, Engine::DuckDuckGo, "").await;
    fake.set("searxng", Behavior::Html);
    assert_eq!(
        search.check(Engine::Searxng, &never).await,
        Checked::Failed(E::JsonDisabled)
    );
    assert_eq!(search.check(Engine::DuckDuckGo, &never).await, Checked::Ok);
    fake.set("duckduckgo", Behavior::Blocked);
    assert_eq!(
        search.check(Engine::DuckDuckGo, &never).await,
        Checked::Failed(E::Blocked)
    );
    let engines: Vec<_> = fake.requests().iter().map(|r| r.engine).collect();
    assert_eq!(
        engines,
        ["brave", "brave", "searxng", "duckduckgo", "duckduckgo"]
    );
    // Textes : le nom du moteur, et le texte des réglages pour une clé.
    assert!(check_message(Engine::Brave, Checked::Ok).contains("Brave Search"));
    assert_eq!(
        check_message(Engine::Mojeek, Checked::Failed(E::Auth)),
        t!("webSearch.keyRefused", engine = "Mojeek")
    );
    assert_eq!(
        check_message(Engine::Brave, Checked::NotConfigured),
        t!("webSearch.check.notConfigured")
    );
    fake.stop().await;
}

/// Dans les essais Rust, les moteurs sont fermés par défaut.
#[tokio::test]
async fn tests_never_reach_a_real_engine_by_default() {
    let search = WebSearch::new(Store::open_in_memory().unwrap());
    let error = search
        .search("chat", None, None, &CancellationToken::new())
        .await;
    assert_eq!(error, Err(failed(None, E::Offline)));
}

#[test]
fn the_old_relay_identity_is_removed() {
    let dir = tempfile::tempdir().unwrap();
    let relay = dir.path().join(RELAY_DIR);
    std::fs::create_dir_all(&relay).unwrap();
    std::fs::write(relay.join(RELAY_IDENTITY), b"{\"version\":1}").unwrap();
    remove_relay_identity(dir.path());
    assert!(!relay.exists());
    // Rien à effacer : rien ne se passe ; un dossier qui a autre chose reste.
    remove_relay_identity(dir.path());
    std::fs::create_dir_all(&relay).unwrap();
    std::fs::write(relay.join("other"), b"x").unwrap();
    remove_relay_identity(dir.path());
    assert!(relay.join("other").exists());
}

/// Contrat avec le vrai DuckDuckGo (réseau requis, lancé par
/// `check.sh --full`) : une recherche sans clé donne des résultats lisibles,
/// quels qu'ils soient. Un échec « blocked » veut dire que DuckDuckGo bloque
/// cette machine pour le moment, pas que la page a changé.
#[tokio::test]
#[ignore = "réseau : vrai DuckDuckGo"]
async fn duckduckgo_contract_answers_without_a_key() {
    let search = WebSearch::with_endpoints(
        Store::open_in_memory().unwrap(),
        web_search::Endpoints::default(),
    );
    let found = search
        .search(
            "Rust programming language",
            Some(5),
            Some("en"),
            &CancellationToken::new(),
        )
        .await
        .unwrap_or_else(|error| panic!("DuckDuckGo: {}", code(&error)));
    assert_eq!(found.provider, P::DuckDuckGo);
    assert!(!found.results.is_empty());
    assert!(found.results.len() <= 5);
    for result in &found.results {
        assert!(result.url.starts_with("http"), "{result:?}");
        assert!(!result.url.contains("duckduckgo.com/"), "{result:?}");
        assert!(!result.title.is_empty(), "{result:?}");
    }
    // La plupart des résultats ont un extrait.
    let with_excerpt = found
        .results
        .iter()
        .filter(|r| !r.content.is_empty())
        .count();
    assert!(
        with_excerpt * 2 >= found.results.len(),
        "{:?}",
        found.results
    );
}
