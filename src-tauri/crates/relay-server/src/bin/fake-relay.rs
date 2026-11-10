//! `fake-relay` : relais d'essai pour les e2e de l'app (fonctionnalité
//! `fake`, jamais dans le binaire déployé). Le vrai relais (`server.rs`),
//! preuve de travail minuscule, avec de faux moteurs de recherche (Brave et
//! Mojeek) sur la boucle locale : aucun appel réseau, aucune vraie clé.
//!
//! ```text
//! fake-relay --dir <dossier> [--daily <n>] [--paused] [--budget-exhausted]
//!            [--min-version <x.y.z>] [--providers-down]
//! ```
//!
//! Écrit `FAKE_RELAY_URL=http://127.0.0.1:<port>` sur la sortie standard une
//! fois prêt, puis sert jusqu'à la fermeture de son entrée standard (ou un
//! signal d'arrêt).

use relay_proto::api::Provider;
use relay_server::fake::{Behavior, TestRelay};
use std::io::Write;
use std::path::PathBuf;

struct Args {
    dir: PathBuf,
    daily: Option<u32>,
    paused: bool,
    budget_exhausted: bool,
    min_version: Option<String>,
    providers_down: bool,
}

fn parse() -> Option<Args> {
    let mut args = std::env::args().skip(1);
    let mut parsed = Args {
        dir: PathBuf::new(),
        daily: None,
        paused: false,
        budget_exhausted: false,
        min_version: None,
        providers_down: false,
    };
    let mut dir = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--dir" => dir = Some(PathBuf::from(args.next()?)),
            "--daily" => parsed.daily = Some(args.next()?.parse().ok()?),
            "--paused" => parsed.paused = true,
            "--budget-exhausted" => parsed.budget_exhausted = true,
            "--min-version" => parsed.min_version = Some(args.next()?),
            "--providers-down" => parsed.providers_down = true,
            _ => return None,
        }
    }
    parsed.dir = dir?;
    Some(parsed)
}

#[tokio::main]
async fn main() -> std::process::ExitCode {
    let Some(args) = parse() else {
        eprintln!(
            "usage: fake-relay --dir <dir> [--daily <n>] [--paused] [--budget-exhausted] \
             [--min-version <x.y.z>] [--providers-down]"
        );
        return std::process::ExitCode::from(2);
    };
    if std::fs::create_dir_all(&args.dir).is_err() {
        eprintln!("fake-relay: cannot create {}", args.dir.display());
        return std::process::ExitCode::from(1);
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or_default();
    let relay = TestRelay::start(&args.dir.join("relay.sqlite"), now, |config| {
        if let Some(daily) = args.daily {
            config.quota.daily_per_key = daily;
            config.quota.new_key_daily = daily;
        }
        config.paused = args.paused;
        if args.budget_exhausted {
            config.budget.monthly_limit_eur = 0.0;
        }
        config.min_app_version = args.min_version.clone();
    })
    .await;
    if args.providers_down {
        relay.fake.set(Provider::Brave, Behavior::Status(500));
        relay.fake.set(Provider::Mojeek, Behavior::Status(500));
    }
    println!("FAKE_RELAY_URL={}", relay.url);
    let _ = std::io::stdout().flush();
    // Arrêt : entrée standard fermée (processus parent parti) ou signal.
    let stdin_closed = tokio::task::spawn_blocking(|| {
        let mut sink = Vec::new();
        let _ = std::io::Read::read_to_end(&mut std::io::stdin(), &mut sink);
    });
    tokio::select! {
        _ = stdin_closed => {}
        _ = tokio::signal::ctrl_c() => {}
    }
    relay.stop().await;
    std::process::ExitCode::SUCCESS
}
