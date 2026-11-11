//! Outils de la recherche web : recherche par le crate `web-search` contre
//! de faux moteurs (crate `fake-services`, lancés dans le processus), lecture
//! des pages sur un faux site local, garde contre les adresses non publiques.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use fake_services::search::{BAD_KEY, Behavior};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::fetch::{FetchError, is_public};
use super::*;

/// Attente maximale d'un appel.
const WAIT: Duration = Duration::from_secs(10);

/// Arrêt jamais demandé (émetteur lâché : l'attente ne finit pas).
fn idle() -> watch::Receiver<bool> {
    watch::channel(false).1
}

fn error(text: &str) -> Executed {
    Executed::Error(text.into())
}

/// Faux moteurs et outils qui s'y adressent.
async fn bench() -> FakeWeb {
    fake_web().await
}

/// Choisit un moteur personnel et sa clé (ou adresse).
async fn choose(bench: &FakeWeb, engine: crate::store::web_search::Engine, secret: &str) {
    let secret = secret.to_owned();
    bench
        .store
        .call(move |c| crate::store::web_search::choose(c, engine, Some(&secret)))
        .await
        .unwrap();
}

async fn run(
    web: &WebTools,
    policy: &Policy,
    name: &str,
    arguments: Value,
    allowed: &mut AllowedUrls,
) -> Executed {
    let mut cancel = idle();
    let running = execute_with(
        web,
        policy,
        name,
        &arguments,
        allowed,
        &mut cancel,
        FETCH_TIMEOUT,
    );
    tokio::time::timeout(WAIT, running)
        .await
        .unwrap_or_else(|_| panic!("{name} still running after {WAIT:?}"))
}

/// Réponse du faux site.
#[derive(Clone)]
struct Reply {
    status: u16,
    headers: Vec<(&'static str, String)>,
    body: Vec<u8>,
    delay: Duration,
    /// Corps en morceaux (`transfer-encoding: chunked`), sans
    /// `content-length` : seule la lecture peut en borner la taille.
    chunked: bool,
}

impl Reply {
    fn html(body: &str) -> Reply {
        Reply {
            status: 200,
            headers: vec![("content-type", "text/html; charset=utf-8".into())],
            body: body.as_bytes().to_vec(),
            delay: Duration::ZERO,
            chunked: false,
        }
    }

    fn redirect(location: &str) -> Reply {
        Reply {
            status: 302,
            headers: vec![("location", location.into())],
            body: Vec::new(),
            delay: Duration::ZERO,
            chunked: false,
        }
    }
}

/// Faux site HTTP/1.1 sur la boucle locale : une réponse par chemin.
struct Site {
    addr: SocketAddr,
    hits: Arc<AtomicUsize>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for Site {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl Site {
    async fn start(route: impl Fn(&str) -> Reply + Send + Sync + 'static) -> Site {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let hits = Arc::new(AtomicUsize::new(0));
        let route = Arc::new(route);
        let task = tokio::spawn({
            let hits = hits.clone();
            async move {
                while let Ok((mut socket, _)) = listener.accept().await {
                    let route = route.clone();
                    let hits = hits.clone();
                    tokio::spawn(async move {
                        let mut request = Vec::new();
                        let mut buffer = [0u8; 4096];
                        while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                            match socket.read(&mut buffer).await {
                                Ok(0) | Err(_) => return,
                                Ok(n) => request.extend_from_slice(&buffer[..n]),
                            }
                        }
                        hits.fetch_add(1, Ordering::SeqCst);
                        let text = String::from_utf8_lossy(&request);
                        let path = text.split_whitespace().nth(1).unwrap_or("/").to_owned();
                        let reply = route(&path);
                        tokio::time::sleep(reply.delay).await;
                        let length = if reply.chunked {
                            "transfer-encoding: chunked".to_owned()
                        } else {
                            format!("content-length: {}", reply.body.len())
                        };
                        let mut head = format!(
                            "HTTP/1.1 {} X\r\n{length}\r\nconnection: close\r\n",
                            reply.status,
                        );
                        for (name, value) in &reply.headers {
                            head.push_str(&format!("{name}: {value}\r\n"));
                        }
                        head.push_str("\r\n");
                        let _ = socket.write_all(head.as_bytes()).await;
                        if reply.chunked {
                            for piece in reply.body.chunks(64 * 1024) {
                                let mut chunk = format!("{:x}\r\n", piece.len()).into_bytes();
                                chunk.extend_from_slice(piece);
                                chunk.extend_from_slice(b"\r\n");
                                if socket.write_all(&chunk).await.is_err() {
                                    return;
                                }
                            }
                            let _ = socket.write_all(b"0\r\n\r\n").await;
                        } else {
                            let _ = socket.write_all(&reply.body).await;
                        }
                    });
                }
            }
        });
        Site { addr, hits, task }
    }

    fn url(&self, path: &str) -> String {
        format!("http://site.test:{}{path}", self.addr.port())
    }

    /// Règles : `site.test` est ce faux site, et lui seul est permis.
    fn policy(&self) -> Policy {
        Policy::testing(
            &[("site.test", IpAddr::V4(Ipv4Addr::LOCALHOST))],
            &[self.addr],
        )
    }
}

#[test]
fn the_tools_are_sent_like_the_official_app() {
    let tools = definitions();
    let names: Vec<&str> = tools
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["function"]["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, [WEB_SEARCH, WEB_FETCH]);
    // ollama.com ne fait plus la recherche : sa mention est retirée.
    assert_eq!(
        tools[0]["function"]["description"],
        "Search the web for real-time information."
    );
    assert_eq!(
        tools[0]["function"]["parameters"]["required"],
        json!(["query"])
    );
    assert_eq!(
        tools[1]["function"],
        json!({
            "name": "web_fetch",
            "description": "Crawl and extract text content from web pages",
            "parameters": {
                "type": "object",
                "required": ["url"],
                "properties": {
                    "url": { "type": "string", "description": "URL to crawl and extract content from" },
                },
            },
        })
    );
}

#[test]
fn urls_written_by_the_user_are_allowed_as_written() {
    // Les deux cas de url_policy_test.go.
    let allowed = AllowedUrls::from_text("summarize https://attacker.example/x");
    assert!(!allowed.allows("https://attacker.example/x!!!!"));
    assert!(allowed.allows("https://attacker.example/x"));
    let allowed = AllowedUrls::from_text("summarize `https://example.com/privacy`");
    assert!(allowed.allows("https://example.com/privacy"));

    let allowed = AllowedUrls::from_text(
        "See https://example.com/a. Then (https://example.com/b), \"https://example.com/c\" \
         and <http://example.com/d>\nhttps://example.com/e?q=1#top!\n\
         https://example.com/f\u{a0}g HTTPS://EXAMPLE.COM/H ftp://example.com/i",
    );
    for url in [
        "https://example.com/a",
        "https://example.com/b",
        "https://example.com/c",
        "http://example.com/d",
        "https://example.com/e?q=1#top",
        // Espace insécable : pas un `\s` de Go, gardée dans l'adresse.
        "https://example.com/f\u{a0}g",
    ] {
        assert!(allowed.allows(url), "{url}");
    }
    for url in [
        "https://example.com/a.",
        "https://example.com/b)",
        " https://example.com/a",
        "https://example.com/a ",
        "https://example.com/f",
        "HTTPS://EXAMPLE.COM/H",
        "ftp://example.com/i",
        "https://example.com/other",
        "",
        "https://",
    ] {
        assert!(!allowed.allows(url), "{url:?}");
    }

    let mut allowed = AllowedUrls::default();
    allowed.add("https://example.com/next).");
    allowed.add("javascript:alert(1)");
    allowed.add("   ");
    assert!(allowed.allows("https://example.com/next"));
    assert_eq!(allowed.urls.len(), 1);
}

#[tokio::test]
async fn arguments_are_checked_before_any_request() {
    let bench = bench().await;
    let policy = Policy::public();
    let mut allowed = AllowedUrls::from_text("https://example.com/page");
    for (name, arguments, expected) in [
        (WEB_SEARCH, json!({}), "query parameter is required"),
        (
            WEB_SEARCH,
            json!({ "max_results": 3 }),
            "query parameter is required",
        ),
        (
            WEB_SEARCH,
            json!({ "query": " \n" }),
            "query must be a non-empty string",
        ),
        (
            WEB_SEARCH,
            json!({ "query": 3 }),
            "query must be a non-empty string",
        ),
        (
            WEB_SEARCH,
            json!({ "query": "x ".repeat(60) }),
            "the search query is empty or too long",
        ),
        (WEB_FETCH, json!({}), "url parameter is required"),
        (
            WEB_FETCH,
            json!({ "url": ["https://example.com/page"] }),
            "url must be a non-empty string",
        ),
        (
            WEB_FETCH,
            json!({ "url": "https://example.com/page." }),
            "web fetch is only allowed for URLs provided by the user",
        ),
        (
            "browser.search",
            json!({ "query": "x" }),
            "unknown tool: browser.search",
        ),
    ] {
        assert_eq!(
            run(&bench.web, &policy, name, arguments.clone(), &mut allowed).await,
            error(expected),
            "{name} {arguments}"
        );
    }
    assert!(bench.engines.requests().is_empty());
}

#[tokio::test]
async fn a_search_keeps_titles_and_links_only_and_gives_the_excerpts_to_the_model() {
    let bench = bench().await;
    let policy = Policy::public();
    let mut allowed = AllowedUrls::default();
    let found = run(
        &bench.web,
        &policy,
        WEB_SEARCH,
        json!({ "query": "chaton roux", "max_results": 3 }),
        &mut allowed,
    )
    .await;
    let Executed::Result(stored) = found else {
        panic!("{found:?}");
    };
    // Enregistré : titres, liens et moteur ; aucun extrait.
    let value: Value = serde_json::from_str(&stored).unwrap();
    assert_eq!(value["provider"], "duckduckgo");
    let results = value["results"].as_array().unwrap();
    assert_eq!(results.len(), 3);
    for result in results {
        let keys: Vec<&String> = result.as_object().unwrap().keys().collect();
        assert_eq!(keys, ["title", "url"]);
    }
    assert!(!stored.contains("Extrait"), "{stored}");
    // Au modèle : les extraits, au format d'Ollama.
    let model: Value = serde_json::from_str(allowed.model_text(&stored)).unwrap();
    assert_eq!(model["results"][0]["url"], "https://duckduckgo.example/1");
    assert_eq!(model["results"][0]["content"], "Extrait 1 (duckduckgo)");
    assert_eq!(
        model["results"][0]["title"],
        "DuckDuckGo result 1 for chaton roux"
    );
    assert!(model.get("provider").is_none());
    // Un autre résultat est donné tel quel.
    assert_eq!(allowed.model_text("{\"x\":1}"), "{\"x\":1}");
    // Historique du tour suivant : la recherche de cet envoi retrouve ses
    // extraits ; une lecture de page, ou une recherche d'un autre envoi
    // (inconnue ici), reste telle qu'enregistrée.
    use crate::inference::{Content, Message};
    let old_search = "{\"results\":[],\"provider\":\"brave\"}";
    let mut messages = vec![
        Message::user("chaton roux"),
        Message::tool("c1", WEB_SEARCH, stored.clone()),
        Message::tool("c2", WEB_FETCH, stored.clone()),
        Message::tool("c3", WEB_SEARCH, old_search),
    ];
    give_excerpts(&mut messages, &allowed);
    let texts: Vec<String> = messages
        .iter()
        .map(|message| match &message.content {
            Content::Text(text) => text.clone(),
            Content::Parts(_) => panic!("parts"),
        })
        .collect();
    assert_eq!(texts[0], "chaton roux");
    assert_eq!(texts[1], allowed.model_text(&stored));
    assert_ne!(texts[1], stored);
    assert_eq!(texts[2], stored);
    assert_eq!(texts[3], old_search);
    // Les adresses des résultats peuvent être lues.
    assert!(allowed.allows("https://duckduckgo.example/1"));
    assert!(allowed.allows("https://duckduckgo.example/3"));

    // `max_results` : nombre > 0 tronqué, sinon 5, au plus 10 ; langue de la
    // requête seulement si ses mots la disent (Brave dit le nombre demandé).
    choose(&bench, crate::store::web_search::Engine::Brave, "BSA-key").await;
    for (given, sent) in [
        (json!(2.7), "2"),
        (json!(0), "5"),
        (json!(-3), "5"),
        (json!("3"), "5"),
        (json!(50), "10"),
    ] {
        let arguments = json!({ "query": "chaton", "max_results": given });
        run(&bench.web, &policy, WEB_SEARCH, arguments, &mut allowed).await;
        let last = bench.engines.requests().pop().unwrap();
        assert_eq!((last.engine, last.count.as_str()), ("brave", sent));
        assert_eq!(last.lang, "");
    }
    let french = json!({ "query": "les chats et les chiens de la ville" });
    let Executed::Result(stored) = run(&bench.web, &policy, WEB_SEARCH, french, &mut allowed).await
    else {
        panic!("a result was expected");
    };
    let last = bench.engines.requests().pop().unwrap();
    assert_eq!(last.lang, "fr");
    // Le moteur qui a répondu est gardé (mention « Powered by Brave »).
    assert_eq!(
        serde_json::from_str::<Value>(&stored).unwrap()["provider"],
        "brave"
    );
}

/// Recherche impossible à la première recherche de l'envoi : arrêt, avec
/// son code (blocage de DuckDuckGo ou clé refusée : `webSearchRefused` ;
/// pas de réseau, moteurs en panne : `webSearchUnavailable`) ; ensuite, une
/// erreur au modèle.
#[tokio::test]
async fn failed_searches_stop_the_send_with_their_code() {
    use crate::store::web_search::Engine;
    let policy = Policy::public();
    let search = || json!({ "query": "chaton" });
    let stop = |executed: &Executed| match executed {
        Executed::Stop(kind, message) => Some((*kind, message.clone())),
        _ => None,
    };

    // DuckDuckGo bloque : refus, et le conseil d'un moteur personnel.
    let bench = bench().await;
    bench.engines.set("duckduckgo", Behavior::Blocked);
    let mut allowed = AllowedUrls::default();
    let refused = run(&bench.web, &policy, WEB_SEARCH, search(), &mut allowed).await;
    let (kind, message) = stop(&refused).expect("a stop");
    assert_eq!(kind, ChatErrorKind::WebSearchRefused);
    assert!(message.contains("Settings › Web search"), "{message}");

    // Clé personnelle refusée, DuckDuckGo en panne : le message parle de la
    // clé.
    let bench = self::bench().await;
    choose(&bench, Engine::Mojeek, BAD_KEY).await;
    bench.engines.set("duckduckgo", Behavior::Status(500));
    let mut allowed = AllowedUrls::default();
    let refused = run(&bench.web, &policy, WEB_SEARCH, search(), &mut allowed).await;
    let (kind, message) = stop(&refused).expect("a stop");
    assert_eq!(kind, ChatErrorKind::WebSearchRefused);
    assert!(message.contains("Mojeek"), "{message}");
    // Clé refusée mais DuckDuckGo répond : la recherche aboutit.
    bench.engines.set("duckduckgo", Behavior::Ok);
    assert!(matches!(
        run(&bench.web, &policy, WEB_SEARCH, search(), &mut allowed).await,
        Executed::Result(_)
    ));

    // Moteurs injoignables : arrêt à la première recherche de l'envoi…
    let closed = closed_web_tools();
    let mut allowed = AllowedUrls::default();
    let refused = run(&closed, &policy, WEB_SEARCH, search(), &mut allowed).await;
    assert_eq!(
        stop(&refused).map(|(kind, _)| kind),
        Some(ChatErrorKind::WebSearchUnavailable)
    );
    // … une erreur au modèle après une recherche réussie.
    allowed.searched = true;
    let Executed::Error(text) = run(&closed, &policy, WEB_SEARCH, search(), &mut allowed).await
    else {
        panic!("an error for the model was expected");
    };
    assert_eq!(text, "web search failed: duckduckgo offline");
}

#[tokio::test]
async fn a_stop_interrupts_the_search() {
    let bench = bench().await;
    bench
        .engines
        .set("duckduckgo", Behavior::Slow(Duration::from_secs(5)));
    let (stop, mut cancel) = watch::channel(false);
    let web = bench.web.clone();
    let task = tokio::spawn(async move {
        let mut allowed = AllowedUrls::default();
        execute(
            &web,
            WEB_SEARCH,
            &json!({ "query": "slow" }),
            &mut allowed,
            &mut cancel,
        )
        .await
    });
    let polling = async {
        while bench.engines.requests().is_empty() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    };
    tokio::time::timeout(WAIT, polling).await.unwrap();
    let asked = Instant::now();
    stop.send(true).unwrap();
    let done = tokio::time::timeout(Duration::from_secs(1), task)
        .await
        .expect("stopped within 1 s")
        .unwrap();
    assert_eq!(done, Executed::Cancelled);
    assert!(asked.elapsed() < Duration::from_secs(1));
}

#[tokio::test]
async fn a_page_is_read_by_the_app_and_allows_its_links() {
    let site = Site::start(|path| match path {
        "/page" => Reply::html(
            "<!doctype html><html><head><title>Le &amp; chaton</title>\
             <style>body{color:red}</style><script>alert('x')</script></head>\
             <body><h1>Titre</h1><p>Un  texte&nbsp;roux.<!-- caché --></p>\
             <a href=\"/next#part\">suite</a> <a href='https://example.org/x'>x</a> \
             <a href=\"javascript:alert(1)\">j</a></body></html>",
        ),
        "/next" => Reply {
            headers: vec![("content-type", "text/plain".into())],
            ..Reply::html("plain text")
        },
        _ => Reply {
            status: 404,
            ..Reply::html("")
        },
    })
    .await;
    let policy = site.policy();
    let page = site.url("/page");
    let next = site.url("/next");
    let web = closed_web_tools();
    let mut allowed = AllowedUrls::from_text(&format!("Read {page}."));
    let refused = run(
        &web,
        &policy,
        WEB_FETCH,
        json!({ "url": next }),
        &mut allowed,
    )
    .await;
    assert_eq!(
        refused,
        error("web fetch is only allowed for URLs provided by the user")
    );
    let read = run(
        &web,
        &policy,
        WEB_FETCH,
        json!({ "url": page }),
        &mut allowed,
    )
    .await;
    let Executed::Result(text) = read else {
        panic!("{read:?}");
    };
    let value: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(value["title"], "Le & chaton");
    assert_eq!(value["content"], "Titre\n\nUn texte roux.\nsuite x j");
    assert_eq!(value["links"], json!([next, "https://example.org/x"]));
    let read = run(
        &web,
        &policy,
        WEB_FETCH,
        json!({ "url": next }),
        &mut allowed,
    )
    .await;
    assert_eq!(
        read,
        Executed::Result(r#"{"title":"","content":"plain text","links":[]}"#.into())
    );
    // Statut d'erreur : au modèle.
    allowed.add(&site.url("/missing"));
    let missing = run(
        &web,
        &policy,
        WEB_FETCH,
        json!({ "url": site.url("/missing") }),
        &mut allowed,
    )
    .await;
    assert_eq!(
        missing,
        error("failed to fetch the page: the server answered with status 404")
    );
}

#[test]
fn only_public_addresses_are_public() {
    for private in [
        "0.0.0.0",
        "0.1.2.3",
        "10.0.0.1",
        "10.255.255.255",
        "100.64.0.1",
        "100.127.255.254",
        "127.0.0.1",
        "127.8.9.10",
        "169.254.169.254",
        "172.16.0.1",
        "172.31.255.255",
        "192.0.0.8",
        "192.0.2.1",
        "192.168.1.1",
        "198.18.0.1",
        "198.19.255.255",
        "198.51.100.7",
        "203.0.113.9",
        "224.0.0.1",
        "239.255.255.250",
        "240.0.0.1",
        "255.255.255.255",
        "::",
        "::1",
        "::ffff:10.0.0.1",
        "::ffff:127.0.0.1",
        "::ffff:169.254.169.254",
        "::10.0.0.1",
        "64:ff9b::a00:1",
        "64:ff9b:1::808:808",
        "::ffff:0:808:808",
        "2002:c0a8:101::1",
        "fc00::1",
        "fd12:3456::1",
        "fe80::1",
        "febf::1",
        "fec0::1",
        "ff02::1",
        "2001:db8::1",
        "2001::1",
        "100::1",
    ] {
        assert!(!is_public(private.parse().unwrap()), "{private}");
    }
    for public in [
        "1.1.1.1",
        "8.8.8.8",
        "100.63.255.255",
        "100.128.0.1",
        "172.15.255.255",
        "172.32.0.1",
        "192.169.0.1",
        "223.255.255.255",
        "2606:4700:4700::1111",
        "2a00:1450:4007:80c::200e",
        "::ffff:8.8.8.8",
        "64:ff9b::808:808",
        "2002:808:808::1",
    ] {
        assert!(is_public(public.parse().unwrap()), "{public}");
    }
}

#[tokio::test]
async fn the_guard_refuses_private_addresses_names_and_redirects() {
    let other = Site::start(|_| Reply::html("secret")).await;
    let other_addr = other.addr;
    let site = Site::start(move |path| match path {
        "/to-private" => Reply::redirect(&format!("http://127.0.0.1:{}/", other_addr.port())),
        "/to-mapped" => Reply::redirect("http://[::ffff:10.0.0.1]/"),
        "/to-localhost" => Reply::redirect(&format!("http://localhost:{}/", other_addr.port())),
        "/loop" => Reply::redirect("/loop"),
        "/ok" => Reply::html("<p>fine</p>"),
        "/to-ok" => Reply::redirect("/ok"),
        "/binary" => Reply {
            headers: vec![("content-type", "application/octet-stream".into())],
            ..Reply::html("\u{0}\u{1}")
        },
        "/huge" => Reply::html(&"a".repeat(fetch::MAX_BYTES + 1)),
        "/huge-chunked" => Reply {
            chunked: true,
            ..Reply::html(&"a".repeat(fetch::MAX_BYTES + 1))
        },
        "/chunked" => Reply {
            chunked: true,
            ..Reply::html("<p>in pieces</p>")
        },
        "/slow" => Reply {
            delay: Duration::from_secs(3),
            ..Reply::html("late")
        },
        _ => Reply::html(""),
    })
    .await;
    let policy = site.policy();
    let read = |url: String| {
        let policy = policy.clone();
        async move { fetch::fetch(&policy, &url).await }
    };
    // Une redirection permise, revérifiée, est suivie.
    assert_eq!(read(site.url("/to-ok")).await.unwrap().text, "fine");
    // Une page en flux sous la borne est lue.
    assert_eq!(read(site.url("/chunked")).await.unwrap().text, "in pieces");
    for (path, expected) in [
        ("/to-private", FetchError::NotPublic),
        ("/to-mapped", FetchError::NotPublic),
        // Nom qui résout vers la boucle locale.
        ("/to-localhost", FetchError::NotPublic),
        ("/loop", FetchError::Redirects),
        (
            "/binary",
            FetchError::ContentType("application/octet-stream".into()),
        ),
        ("/huge", FetchError::TooLarge),
        // En flux, sans taille annoncée : bornée à la lecture.
        ("/huge-chunked", FetchError::TooLarge),
    ] {
        assert_eq!(read(site.url(path)).await, Err(expected), "{path}");
    }
    // Rien n'est arrivé à l'autre site.
    assert_eq!(other.hits.load(Ordering::SeqCst), 0);
    // Adresses refusées avant toute connexion.
    for (url, expected) in [
        (
            format!("http://127.0.0.1:{}/", other_addr.port()),
            FetchError::NotPublic,
        ),
        (
            "http://[::ffff:10.0.0.1]/".to_owned(),
            FetchError::NotPublic,
        ),
        ("http://[fd00::1]/".to_owned(), FetchError::NotPublic),
        (
            "http://169.254.169.254/latest".to_owned(),
            FetchError::NotPublic,
        ),
        ("http://localhost/".to_owned(), FetchError::NotPublic),
        ("http://0.0.0.0:80/".to_owned(), FetchError::NotPublic),
        ("ftp://example.org/x".to_owned(), FetchError::Scheme),
        (
            "http://user:pw@example.org/".to_owned(),
            FetchError::Address,
        ),
        ("file:///etc/passwd".to_owned(), FetchError::Scheme),
    ] {
        assert_eq!(read(url.clone()).await, Err(expected), "{url}");
    }
    assert_eq!(other.hits.load(Ordering::SeqCst), 0);
    // Réponse DNS mêlée : une adresse permise d'abord (le faux site), puis
    // une privée ; une seule adresse non publique suffit à refuser, avant
    // toute connexion (aucune requête n'atteint le faux site).
    let hits = site.hits.load(Ordering::SeqCst);
    let mixed = Policy::testing(
        &[
            ("mixed.test", IpAddr::V4(Ipv4Addr::LOCALHOST)),
            ("mixed.test", "10.0.0.1".parse().unwrap()),
        ],
        &[site.addr],
    );
    assert_eq!(
        fetch::fetch(
            &mixed,
            &format!("http://mixed.test:{}/ok", site.addr.port())
        )
        .await,
        Err(FetchError::NotPublic)
    );
    assert_eq!(site.hits.load(Ordering::SeqCst), hits);
    // Sans les règles d'essai, le faux site lui-même est refusé.
    assert_eq!(
        fetch::fetch(&Policy::public(), &format!("http://{}/ok", site.addr)).await,
        Err(FetchError::NotPublic)
    );

    // Durée bornée, arrêt pendant la lecture.
    let web = closed_web_tools();
    let mut allowed = AllowedUrls::from_text(&site.url("/slow"));
    let mut cancel = idle();
    let started = Instant::now();
    let late = execute_with(
        &web,
        &policy,
        WEB_FETCH,
        &json!({ "url": site.url("/slow") }),
        &mut allowed,
        &mut cancel,
        Duration::from_millis(200),
    )
    .await;
    assert_eq!(late, error("failed to fetch the page: timed out"));
    assert!(started.elapsed() < Duration::from_secs(2));
    let (stop, mut cancel) = watch::channel(false);
    let task = {
        let (web, policy, url) = (web.clone(), policy.clone(), site.url("/slow"));
        tokio::spawn(async move {
            let mut allowed = AllowedUrls::from_text(&url);
            execute_with(
                &web,
                &policy,
                WEB_FETCH,
                &json!({ "url": url }),
                &mut allowed,
                &mut cancel,
                FETCH_TIMEOUT,
            )
            .await
        })
    };
    tokio::time::sleep(Duration::from_millis(100)).await;
    stop.send(true).unwrap();
    let done = tokio::time::timeout(Duration::from_secs(1), task)
        .await
        .expect("stopped within 1 s")
        .unwrap();
    assert_eq!(done, Executed::Cancelled);
}

#[test]
fn pages_become_text() {
    let base = url::Url::parse("https://example.org/dir/page").unwrap();
    let page = page::html(
        "<TITLE>A\n  title</TITLE><BODY><P>One<BR>two</P>3 < 5 and 7 > 2\
         <SCRIPT type=\"x\">var a = '</p>';</SCRIPT><div data-x=\"a>b\">&#233;&#x20AC;&bogus; &lt;b&gt;</div>\
         <table><tr><td>a</td><td>b</td></tr></table><a href=../up>up</a> <a href=\"mailto:x@y\">m</a>",
        &base,
    );
    assert_eq!(page.title, "A title");
    assert_eq!(
        page.text,
        "One\ntwo\n3 < 5 and 7 > 2\né€&bogus; <b>\n\na b\n\nup m"
    );
    assert_eq!(page.links, ["https://example.org/up"]);
    // Liens bornés, texte borné.
    let many: String = (0..150)
        .map(|n| format!("<a href=\"/{n}\">{n}</a>"))
        .collect();
    assert_eq!(page::html(&many, &base).links.len(), page::MAX_LINKS);
    let long = page::plain(&"word ".repeat(20_000));
    assert_eq!(long.text.chars().count(), page::MAX_TEXT);
    // Rien de lisible : un texte vide, jamais une erreur.
    assert_eq!(page::html("<", &base).text, "<");
    // Éléments sautés, toutes casses : jusqu'à leur balise fermante.
    assert_eq!(
        page::html("a<SVG><p>x</p></Svg>b<style>c</STYLE>d<script>never", &base).text,
        "abd"
    );
    assert_eq!(page::html("<!-- open", &base).text, "");
    assert_eq!(
        page::decode("a &amp b &#0; &#x110000;"),
        "a &amp b &#0; &#x110000;"
    );
}

/// Page hostile : 4 Mio d'éléments sautés (`<svg></svg>`). L'extraction
/// est linéaire (elle ne recopie pas le reste du document à chaque
/// élément) et rend vite la main.
#[test]
fn a_page_of_skipped_elements_is_read_quickly() {
    let base = url::Url::parse("https://example.org/").unwrap();
    let hostile = "<svg></svg>".repeat(380_000);
    let started = Instant::now();
    let page = page::html(&hostile, &base);
    assert_eq!(page.text, "");
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "{:?}",
        started.elapsed()
    );
}

/// Page hostile servie au `web_fetch` : lue sous le délai de l'outil (le
/// calcul se fait hors du fil async) ; et une extraction interrompue
/// (délai ou arrêt de l'envoi : drapeau levé à l'abandon) rend la main à
/// la balise suivante.
#[tokio::test]
async fn a_hostile_page_is_read_under_the_tool_delay_and_can_be_stopped() {
    let hostile = "<svg></svg>".repeat(380_000);
    let site = Site::start(move |_| Reply::html(&hostile)).await;
    let policy = site.policy();
    let web = closed_web_tools();
    let url = site.url("/hostile");
    let mut allowed = AllowedUrls::from_text(&url);
    let mut cancel = idle();
    let started = Instant::now();
    let read = execute_with(
        &web,
        &policy,
        WEB_FETCH,
        &json!({ "url": url }),
        &mut allowed,
        &mut cancel,
        Duration::from_secs(3),
    )
    .await;
    assert!(matches!(read, Executed::Result(_)), "{read:?}");
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "{:?}",
        started.elapsed()
    );
    // Arrêt levé : plus aucune balise lue (ni lien, ni élément sauté).
    let base = url::Url::parse("https://example.org/").unwrap();
    let links = "<a href=\"/x\">x</a>".repeat(10);
    assert_eq!(page::html(&links, &base).links.len(), 1);
    let stop = std::sync::atomic::AtomicBool::new(true);
    assert!(page::html_until(&links, &base, &stop).links.is_empty());
}
