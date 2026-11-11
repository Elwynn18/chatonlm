//! Faux moteurs de recherche web pour les e2e (`fake_services::search`) :
//!   fake-search [port]
//! Affiche `FAKE_SEARCH_URL=<adresse>`, puis tourne jusqu'à la fermeture de
//! son entrée standard, SIGINT ou SIGTERM. Donner à l'app
//! `CHATONLM_DUCKDUCKGO_URL=<adresse>/ddg`, `CHATONLM_BRAVE_URL=<adresse>/brave`,
//! `CHATONLM_MOJEEK_URL=<adresse>/mojeek` ; instance SearXNG : `<adresse>/searxng`.

use std::net::SocketAddr;

use fake_services::Terminate;
use fake_services::search::FakeSearch;
use tokio::io::AsyncReadExt;

#[tokio::main]
async fn main() {
    let port = match std::env::args().nth(1).map(|p| p.parse::<u16>()) {
        None => 0,
        Some(Ok(port)) => port,
        Some(Err(_)) => {
            eprintln!("port invalide");
            std::process::exit(2);
        }
    };
    let server = FakeSearch::start_at(SocketAddr::from(([127, 0, 0, 1], port)))
        .await
        .expect("cannot start the fake search engines");
    println!("FAKE_SEARCH_URL={}", server.url());
    let mut terminate = Terminate::listen();
    // Entrée standard fermée (le test qui l'a lancé est mort) : arrêt.
    let stdin_closed = async {
        let mut stdin = tokio::io::stdin();
        let mut buffer = [0u8; 64];
        while stdin.read(&mut buffer).await.is_ok_and(|read| read > 0) {}
    };
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        _ = terminate.recv() => {}
        () = stdin_closed => {}
    }
    server.stop().await;
}
