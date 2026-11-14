//! Lance le faux Hugging Face, pour essayer l'app réelle sans réseau :
//!   fake-services [hf|hf-status:<code>|hf-ratelimit|hf-html|hf-silent] [port]
//! ou le faux GitHub des outils de la Transcription (module `github`) :
//!   fake-services github [port]
//! (binaires des archives : fichiers donnés par `FAKE_GITHUB_YTDLP_BIN` et
//! `FAKE_GITHUB_DENO_BIN`, sinon scripts `sh` qui répondent à `--version`).
//! Affiche l'adresse, puis tourne jusqu'à SIGINT ou SIGTERM. Donner
//! l'adresse à l'app par `CHATONLM_HF_URL` et `CHATONLM_HF_STATUS_URL` (voir
//! le module `hf` : dépôts de `fixtures/hf/repos.json`, requêtes reçues sur
//! `GET /fake/requests`). Le faux `sd-server` est le binaire
//! `fake-sd-server` ; le faux moteur de texte, le crate `fake-llama`.

use std::net::SocketAddr;
use std::time::Duration;

use fake_services::Terminate;
use fake_services::github::{Binaries, FakeGithub};
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
    if first.as_deref() == Some("github") {
        let port = port_argument(args.next());
        let read = |name: &str| {
            std::env::var_os(name).map(|path| std::fs::read(path).expect("binaire illisible"))
        };
        let binaries = Binaries {
            yt_dlp: read("FAKE_GITHUB_YTDLP_BIN"),
            deno: read("FAKE_GITHUB_DENO_BIN"),
        };
        let server = FakeGithub::start_with(SocketAddr::from(([127, 0, 0, 1], port)), binaries)
            .await
            .expect("cannot start the fake GitHub server");
        println!("{}", server.url());
        until_stopped().await;
        server.stop().await;
        return;
    }
    let Some(mode) = HfMode::parse(first.as_deref().unwrap_or("hf")) else {
        eprintln!(
            "mode inconnu : {} (hf, hf-status:<code>, hf-ratelimit, hf-html, hf-silent, github)",
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
