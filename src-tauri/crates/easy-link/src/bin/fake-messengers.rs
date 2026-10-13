//! Faux serveurs Telegram et Discord pour les e2e : `fake-messengers
//! <telegram|discord> [port]` écrit son adresse sur la sortie standard, puis
//! sert jusqu'à SIGINT ou SIGTERM (comme `fake-ollama`). L'adresse affichée se
//! donne telle quelle à `CHATONLM_TELEGRAM_URL` ou `CHATONLM_DISCORD_URL` ;
//! le pilotage (`/fake/message`, `/fake/sent`) est à la racine du même hôte.

use std::net::SocketAddr;

use easy_link::fake::discord::FakeDiscord;
use easy_link::fake::telegram::FakeTelegram;

/// Attend la demande d'arrêt (SIGINT ou SIGTERM ; Ctrl+C sous Windows).
async fn until_stopped() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let mut terminate = signal(SignalKind::terminate()).expect("SIGTERM");
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = terminate.recv() => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

fn usage() -> ! {
    eprintln!("usage : fake-messengers <telegram|discord> [port]");
    std::process::exit(2);
}

#[tokio::main]
async fn main() {
    let mut args = std::env::args().skip(1);
    let kind = args.next().unwrap_or_default();
    let port = match args.next() {
        None => 0,
        Some(port) => port.parse::<u16>().unwrap_or_else(|_| usage()),
    };
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    match kind.as_str() {
        "telegram" => {
            let server = FakeTelegram::start_on(addr)
                .await
                .expect("cannot start the fake Telegram server");
            println!("{}", server.url());
            until_stopped().await;
            server.stop().await;
        }
        "discord" => {
            let server = FakeDiscord::start_on(addr)
                .await
                .expect("cannot start the fake Discord server");
            println!("{}", server.url());
            until_stopped().await;
            server.stop().await;
        }
        _ => usage(),
    }
}
