//! Lance le faux serveur Ollama, pour essayer l'app réelle sans modèle :
//!   fake-ollama [ollama|no-models|not-ollama|silent|close|huge|status:<code>] [port]
//!   fake-ollama serve
//! Affiche l'adresse, puis tourne jusqu'à SIGINT ou SIGTERM. En mode `ollama`,
//! le scénario de `/api/chat` dépend du nom du modèle (voir `chat` dans
//! lib.rs : `chatonlm-slow`, `chatonlm-stall`, `chatonlm-longthink`,
//! `chatonlm-markdown`...) ; `/api/show` les dit installés (capacité
//! `completion`, plus `vision` pour `chatonlm-vision`, qui accepte les images
//! jointes) et décrit aussi les modèles de scénario du raisonnement
//! (`chatonlm-levels`, `chatonlm-never`... : voir `thinking_scenario`) ;
//! `/api/pull` joue les scénarios `chatonlm-pull*` (voir `pull`) ; seuls
//! `chatonlm-pull`, `chatonlm-pull-slow` et `chatonlm-pull-small`, une fois
//! entièrement tirés, sont montrés en tête de `/api/tags` (oubliés à l'arrêt
//! du serveur) ; les autres (`-stall`, `-error`, `-offline`...) échouent ou ne
//! finissent jamais. `/api/experimental/model-recommendations` donne la liste par défaut
//! d'Ollama (gemma4, qwen3.5 et des modèles cloud), même en mode `no-models`.
//! `/api/pull` joue aussi les modèles de Hugging Face `hf.co/chatonlm/<dépôt>:<étiquette>`
//! (voir `hf_pull`) : téléchargement complet (projecteur en plus si le nom
//! contient `vision`, lent avec `slow`), ou selon un mot du nom `sharded`,
//! `missing`, `restricted`, `badtag`, `badquant`, `notgguf` (erreurs exactes
//! d'Ollama), `stall` (puis silence), `cut` (fin sans `success`) ; une fois
//! tiré, le modèle figure dans `/api/tags` sous le nom demandé.
//! `DELETE /api/delete` supprime un modèle tiré ou enregistré (200), 404 sinon.
//! Modèles en mémoire (aucun au départ) : `GET /api/ps` au format d'Ollama
//! (1 Gio chacun, 25 % sur le CPU pour un nom qui contient `split`, comme
//! `chatonlm-split`) ; `/api/chat` charge le modèle qui répond, `POST
//! /api/generate` sans prompt le charge (`options.num_ctx`, sinon 4096,
//! ramené au maximum du modèle ; `keep_alive`, sinon 5 min) ou le décharge
//! (`keep_alive: 0`) ; `chatonlm-slowload` met 1,5 s à se charger,
//! `chatonlm-longload` 6 s, `chatonlm-loadfail` échoue (500, mémoire
//! insuffisante), `chatonlm-polish-remote` (distant) n'est jamais en mémoire ;
//! un modèle supprimé reste en mémoire et ne se décharge plus (404, comme
//! Ollama) ; demandes lues sur `GET /fake/loads` (absentes de
//! `/fake/requests`).
//! Le serveur sert aussi de registre (`CHATONLM_REGISTRY_URL` de l'app) :
//! `HEAD /v2/library/<nom>/manifests/<étiquette>` répond 200 avec
//! `ollama-content-digest` et `ollama-push-time` pour `chatonlm-*`, `gemma4`,
//! `qwen3.5` et `llama3.2`, 404 sinon (voir `manifest`) ; l'espace `chatonlm`
//! imite le registre de Hugging Face (`CHATONLM_HF_URL` de l'app pointé sur ce
//! serveur : manifestes et refus des mêmes noms `hf.co/chatonlm/...`).
//! `POST /fake/publish {"model": "<nom>"}` y publie une nouvelle version d'un
//! modèle : celui qui est installé devient périmé (autre empreinte, date de
//! publication récente) jusqu'à son prochain téléchargement. Ce que le serveur a reçu se
//! lit sur `GET /fake/requests` (chat), `GET /fake/pulls`, `GET /fake/deletes`,
//! `GET /fake/web` (recherche web), `GET /fake/loads`, `GET
//! /fake/disconnects` et `GET /fake/manifests` (requêtes reçues par le
//! registre).
//!
//! Compte et recherche web (le serveur joue aussi ollama.com) : compte délié au
//! départ, relié ou délié par `POST /fake/account {"signedIn": bool}` ;
//! `POST /api/me` (200 et l'utilisateur « ada », ou 401 avec `signin_url`
//! `http://<Host>/connect?...`), `POST /api/signout`, `GET /api/status`
//! (cloud coupé par `POST /fake/cloud {"disabled": bool}`), `GET
//! /fake/avatar.png` ; `POST /api/experimental/web_search` et `web_fetch`
//! (403 cloud coupé, 401 « Unauthorized » délié, résultats déterministes ;
//! `slow` dans la
//! requête ou l'adresse : 30 s d'attente ; `huge` : résultat de 5 Mio). Les
//! modèles `chatonlm-search`, `chatonlm-fetch` et `chatonlm-loop` (capacité
//! `tools`) appellent ces outils (voir `tool_reply` dans lib.rs).
//!
//! Faux Hugging Face (`CHATONLM_HF_URL` et `CHATONLM_HF_STATUS_URL` de l'app),
//! serveur à part : `fake-ollama hf|hf-status:<code>|hf-ratelimit|hf-html|hf-silent
//! [port]` (voir le module `hf` : dépôts de `fixtures/hf/repos.json`, requêtes
//! reçues sur `GET /fake/requests`).
//!
//! `fake-ollama serve` : lancé comme `ollama serve` par l'Ollama intégré de
//! l'app (`CHATONLM_OLLAMA_BIN`), adresse lue dans `OLLAMA_HOST` (voir le
//! module `serve`).

use std::net::SocketAddr;
use std::time::Duration;

use fake_ollama::hf::{FakeHf, HfMode};
use fake_ollama::{FakeOllama, Mode};

/// Attend SIGINT ou SIGTERM (sous Windows : Ctrl+C ou Ctrl+Pause).
async fn until_stopped() {
    let mut terminate = fake_ollama::serve::Terminate::listen();
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
    if fake_ollama::serve::is_runner() || first.as_deref() == Some("serve") {
        std::process::exit(fake_ollama::serve::run().await);
    }
    if let Some(mode) = first.as_deref().and_then(HfMode::parse) {
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
        return;
    }
    let mode = match first.as_deref() {
        None | Some("ollama") => Mode::ollama(),
        Some("no-models") => Mode::no_models(),
        Some("not-ollama") => Mode::NotOllama,
        Some("silent") => Mode::Silent,
        Some("close") => Mode::Close,
        Some("huge") => Mode::Huge,
        Some(other) => match other.strip_prefix("status:").map(str::parse) {
            Some(Ok(code)) => Mode::Status(code),
            _ => {
                eprintln!(
                    "mode inconnu : {other} (ollama, no-models, not-ollama, silent, close, huge, status:<code>, hf, hf-status:<code>, hf-ratelimit, hf-html, hf-silent)"
                );
                std::process::exit(2);
            }
        },
    };
    let port = port_argument(args.next());

    let server = FakeOllama::start_on(SocketAddr::from(([127, 0, 0, 1], port)), mode)
        .await
        .expect("cannot start the fake Ollama server");
    println!("{}", server.url());
    until_stopped().await;
    server.stop().await;
}
