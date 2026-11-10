//! Lance le faux Hugging Face, pour essayer l'app réelle sans réseau :
//!   fake-services [hf|hf-status:<code>|hf-ratelimit|hf-html|hf-silent] [port]
//! Affiche l'adresse, puis tourne jusqu'à SIGINT ou SIGTERM. Donner
//! l'adresse à l'app par `CHATONLM_HF_URL` et `CHATONLM_HF_STATUS_URL` (voir
//! le module `hf` : dépôts de `fixtures/hf/repos.json`, requêtes reçues sur
//! `GET /fake/requests`). Le faux `sd-server` est le binaire
//! `fake-sd-server` ; le faux moteur de texte, le crate `fake-llama`.

use std::net::SocketAddr;
use std::time::Duration;

use fake_services::Terminate;
use fake_services::hf::{FakeHf, HfMode};

/// Attend SIGINT ou SIGTERM (sous Windows : Ctrl+C ou Ctrl+Pause).
async fn until_stopped() {
    let mut terminate = Terminate::listen();
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        _ = terminate.recv() => {}
    }
}

fn port_argument(argument: Option<String>) -> u16 {
    match argument.map(|p| p.parse()) {
        None => 0,
        Some(Ok(port)) => port,
        Some(Err(_)) => {
            eprintln!("port invalide");
            std::process::exit(2);
        }
    }
}

#[tokio::main]
async fn main() {
    let mut args = std::env::args().skip(1);
    let first = args.next();
    let Some(mode) = HfMode::parse(first.as_deref().unwrap_or("hf")) else {
        eprintln!(
            "mode inconnu : {} (hf, hf-status:<code>, hf-ratelimit, hf-html, hf-silent)",
            first.unwrap_or_default()
        );
        std::process::exit(2);
    };
    let port = port_argument(args.next());
    let server = FakeHf::start_with(
        SocketAddr::from(([127, 0, 0, 1], port)),
        mode,
        Duration::ZERO,
    )
    .await
    .expect("cannot start the fake Hugging Face server");
    println!("{}", server.url());
    until_stopped().await;
    server.stop().await;
}
