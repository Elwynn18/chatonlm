//! Essais du choix de la cible : vérification de la machine simulée, Ollama
//! intégré joué par le faux `ollama serve` (voir `builtin::tests`).

use std::sync::atomic::AtomicUsize;
use std::time::Instant;

use fake_ollama::{FakeOllama, Mode};
use tokio::sync::Notify;

use super::*;
use crate::hub::downloads::Options;
use crate::hub::types::{HubDownloadRequest, HubDownloadState};
use crate::ollama::builtin::tests::{fake_config, reaches, temp_dir};
use crate::ollama::builtin::{Config, Snapshot};
use crate::store::settings::SettingsPatch;

/// Vérification simulée de la machine : état imposé, appels comptés ;
/// retenue jusqu'au feu vert si une barrière est posée.
#[derive(Clone)]
struct Machine {
    state: Arc<Mutex<ServerState>>,
    calls: Arc<AtomicUsize>,
    gate: Arc<Mutex<Option<Arc<Notify>>>>,
}

impl Machine {
    fn new(state: ServerState) -> Machine {
        Machine {
            state: Arc::new(Mutex::new(state)),
            calls: Arc::new(AtomicUsize::new(0)),
            gate: Arc::new(Mutex::new(None)),
        }
    }

    /// Les vérifications suivantes attendent `notify_one` sur la barrière.
    fn hold(&self) -> Arc<Notify> {
        let gate = Arc::new(Notify::new());
        *self.gate.lock() = Some(gate.clone());
        gate
    }

    fn set(&self, state: ServerState) {
        *self.state.lock() = state;
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

fn canned(url: String, state: ServerState) -> ServerStatus {
    ServerStatus {
        url,
        state,
        version: (state == ServerState::Running).then(|| "0.35.1".into()),
        alternative_url: None,
        host_env: None,
        http_status: None,
        detail: None,
        service: None,
        builtin: false,
        builtin_available: false,
        explicit_address: false,
        log_path: None,
    }
}

/// Cible dont la machine est simulée ; changements de cible notés.
fn targeting(builtin: Builtin, machine: &Machine) -> (Targeting, Arc<Mutex<Vec<OllamaTarget>>>) {
    let changes = Arc::new(Mutex::new(Vec::new()));
    let noted = changes.clone();
    let machine = machine.clone();
    let targeting = Targeting::with_check(
        builtin,
        move |target| noted.lock().push(target.clone()),
        move |url| {
            machine.calls.fetch_add(1, Ordering::SeqCst);
            let state = *machine.state.lock();
            let gate = machine.gate.lock().clone();
            Box::pin(async move {
                if let Some(gate) = gate {
                    gate.notified().await;
                }
                canned(url, state)
            })
        },
    )
    .without_ollama_host();
    (targeting, changes)
}

/// Travaux arrêtés avant un arrêt de l'intégré : état de l'intégré à chaque
/// appel.
fn record_stops(targeting: Targeting) -> (Targeting, Arc<Mutex<Vec<BuiltinState>>>) {
    let stops = Arc::new(Mutex::new(Vec::new()));
    let (seen, builtin) = (stops.clone(), targeting.builtin().clone());
    let targeting = targeting.before_builtin_stop(move || {
        seen.lock().push(builtin.snapshot().state);
    });
    (targeting, stops)
}

/// Attend que `condition` soit vraie, 10 s au plus.
async fn eventually(mut condition: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if condition() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    condition()
}

fn store_with_address(address: &str) -> Store {
    let store = Store::open_in_memory().unwrap();
    let patch = SettingsPatch {
        server_url: Some(address.into()),
        ..Default::default()
    };
    store
        .blocking(|c| settings::update(c, patch, None).map(drop))
        .unwrap();
    store
}

async fn running(builtin: &Builtin) -> Snapshot {
    builtin.wait_ready(Duration::from_secs(20)).await
}

#[test]
fn choice_of_the_target() {
    use OllamaSource::{Auto, Builtin as Chosen};
    use TargetKind::{Builtin, Machine};
    // (source, adresse explicite, intégré choisi pour la session, livré)
    let cases = [
        ((Chosen, false, false, true), Builtin),
        ((Chosen, true, false, true), Builtin),
        ((Auto, true, true, true), Machine),
        ((Auto, false, true, true), Builtin),
        ((Auto, false, false, true), Machine),
        ((Chosen, false, true, false), Machine),
        ((Auto, false, true, false), Machine),
    ];
    for ((source, explicit, session, available), expected) in cases {
        assert_eq!(
            choose(source, explicit, session, available),
            expected,
            "{source:?} {explicit} {session} {available}"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn nothing_installed_starts_the_builtin_for_the_whole_session() {
    let dir = temp_dir();
    let builtin = Builtin::new(fake_config(dir.path(), &[("FAKE_OLLAMA_DELAY_MS", "500")]));
    let machine = Machine::new(ServerState::NotInstalled);
    let (targeting, changes) = targeting(builtin.clone(), &machine);
    let store = Store::open_in_memory().unwrap();

    let status = targeting.status(&store).await.unwrap();
    assert!(status.builtin, "{status:?}");
    assert_eq!(status.state, ServerState::BuiltinStarting);
    assert!(status.builtin_available && !status.explicit_address);
    assert_eq!(status.log_path, Some(targeting.log_path()));
    let url = builtin.snapshot().url.unwrap();
    assert_eq!(status.url, url);

    // Une requête attend la fin du démarrage, vers l'intégré.
    let settings = targeting.request_settings(&store).await.unwrap();
    assert_eq!(settings.effective_server_url, url);
    assert_eq!(builtin.snapshot().state, BuiltinState::Running);
    let status = targeting.status(&store).await.unwrap();
    assert_eq!(status.state, ServerState::Running);
    assert_eq!(status.version.as_deref(), Some("0.40.0"));

    // L'Ollama de la machine apparaît : l'intégré reste la cible, sans même
    // revérifier la machine.
    machine.set(ServerState::Running);
    let calls = machine.calls();
    let status = targeting.status(&store).await.unwrap();
    assert!(status.builtin);
    assert_eq!(machine.calls(), calls);
    assert!(changes.lock().is_empty(), "{:?}", changes.lock());
    builtin.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn installed_but_stopped_offers_the_builtin_without_starting_it() {
    let dir = temp_dir();
    let builtin = Builtin::new(fake_config(dir.path(), &[]));
    for state in [
        ServerState::ServiceStopped,
        ServerState::NoService,
        ServerState::ServiceFailed,
    ] {
        let machine = Machine::new(state);
        let (targeting, _) = targeting(builtin.clone(), &machine);
        let store = Store::open_in_memory().unwrap();
        let status = targeting.status(&store).await.unwrap();
        assert_eq!(status.state, state);
        assert!(!status.builtin && status.builtin_available && !status.explicit_address);
        assert_eq!(status.url, address::DEFAULT_SERVER_URL);
        let settings = targeting.request_settings(&store).await.unwrap();
        assert_eq!(settings.effective_server_url, address::DEFAULT_SERVER_URL);
    }
    assert_eq!(builtin.snapshot().state, BuiltinState::Stopped);
}

#[tokio::test(flavor = "multi_thread")]
async fn an_explicit_address_is_never_replaced() {
    let dir = temp_dir();
    let builtin = Builtin::new(fake_config(dir.path(), &[]));
    let machine = Machine::new(ServerState::NotInstalled);
    let (targeting, _) = targeting(builtin.clone(), &machine);
    let store = store_with_address("127.0.0.1:9");
    let status = targeting.status(&store).await.unwrap();
    assert_eq!(status.state, ServerState::NotInstalled);
    assert!(!status.builtin && status.builtin_available && status.explicit_address);
    assert_eq!(status.url, "http://127.0.0.1:9");
    assert_eq!(builtin.snapshot().state, BuiltinState::Stopped);
}

#[tokio::test(flavor = "multi_thread")]
async fn without_a_builtin_ollama_nothing_changes() {
    let dir = temp_dir();
    let builtin = Builtin::new(Config::new(None, dir.path()));
    let machine = Machine::new(ServerState::NotInstalled);
    let (targeting, _) = targeting(builtin, &machine);
    let store = Store::open_in_memory().unwrap();
    store
        .blocking(|c| builtin_ollama::set_source(c, OllamaSource::Builtin))
        .unwrap();
    let status = targeting.status(&store).await.unwrap();
    assert_eq!(status.state, ServerState::NotInstalled);
    assert!(!status.builtin && !status.builtin_available);
    let Err(AppError::Invalid(message)) = targeting.set_source(&store, OllamaSource::Builtin).await
    else {
        panic!("built-in Ollama chosen without one");
    };
    assert!(message.contains("built-in Ollama"), "{message}");
}

#[tokio::test(flavor = "multi_thread")]
async fn the_builtin_source_is_remembered_and_reversible() {
    let dir = temp_dir();
    let builtin = Builtin::new(fake_config(dir.path(), &[]));
    let machine = Machine::new(ServerState::ServiceStopped);
    let (targeting, changes) = targeting(builtin.clone(), &machine);
    let store = Store::open_in_memory().unwrap();
    assert!(!targeting.status(&store).await.unwrap().builtin);

    // « Use the built-in Ollama instead ».
    let runtime = targeting
        .set_source(&store, OllamaSource::Builtin)
        .await
        .unwrap();
    assert_eq!(runtime.source, OllamaSource::Builtin);
    assert_eq!(runtime.kind, TargetKind::Builtin);
    assert!(runtime.local && runtime.builtin_available && runtime.builtin_models_dir_is_default);
    let url = builtin.snapshot().url.unwrap();
    assert_eq!(runtime.url, url);
    assert_eq!(
        *changes.lock(),
        [OllamaTarget {
            url: url.clone(),
            kind: TargetKind::Builtin
        }]
    );
    let saved = store.blocking(|c| builtin_ollama::get(c)).unwrap();
    assert_eq!(saved.source, OllamaSource::Builtin);
    assert_eq!(running(&builtin).await.state, BuiltinState::Running);
    // Gardé même si la machine répond.
    machine.set(ServerState::Running);
    assert!(targeting.status(&store).await.unwrap().builtin);

    // Retour à l'Ollama de la machine : l'intégré s'arrête.
    let runtime = targeting
        .set_source(&store, OllamaSource::Auto)
        .await
        .unwrap();
    assert_eq!(runtime.kind, TargetKind::Machine);
    assert_eq!(runtime.url, address::DEFAULT_SERVER_URL);
    assert_eq!(builtin.snapshot().state, BuiltinState::Stopped);
    assert_eq!(
        changes.lock().last(),
        Some(&OllamaTarget {
            url: address::DEFAULT_SERVER_URL.into(),
            kind: TargetKind::Machine
        })
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn back_to_auto_keeps_the_builtin_when_nothing_is_installed() {
    let dir = temp_dir();
    let builtin = Builtin::new(fake_config(dir.path(), &[]));
    let machine = Machine::new(ServerState::NotInstalled);
    let (targeting, _) = targeting(builtin.clone(), &machine);
    let store = Store::open_in_memory().unwrap();
    targeting
        .set_source(&store, OllamaSource::Builtin)
        .await
        .unwrap();
    let pid = running(&builtin).await.pid;
    let runtime = targeting
        .set_source(&store, OllamaSource::Auto)
        .await
        .unwrap();
    assert_eq!(runtime.kind, TargetKind::Builtin);
    // Même processus : ni arrêt ni relance.
    assert_eq!(builtin.snapshot().pid, pid);
    builtin.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_failed_builtin_is_restarted_only_on_request() {
    let dir = temp_dir();
    let crashes = dir.path().join("crashes");
    std::fs::write(&crashes, "100").unwrap();
    let crashes_env = crashes.to_string_lossy().into_owned();
    let builtin = Builtin::new(fake_config(
        dir.path(),
        &[("FAKE_OLLAMA_CRASHES", crashes_env.as_str())],
    ));
    let machine = Machine::new(ServerState::Running);
    let (targeting, _) = targeting(builtin.clone(), &machine);
    let store = Store::open_in_memory().unwrap();
    store
        .blocking(|c| builtin_ollama::set_source(c, OllamaSource::Builtin))
        .unwrap();
    let status = targeting.status(&store).await.unwrap();
    assert_eq!(status.state, ServerState::BuiltinStarting);
    assert!(
        reaches(&builtin, Duration::from_secs(20), |snapshot| {
            snapshot.state == BuiltinState::Failed
        })
        .await
    );
    let left = std::fs::read_to_string(&crashes).unwrap();
    // L'écran revérifie : l'échec reste affiché, rien n'est relancé.
    for _ in 0..3 {
        let status = targeting.status(&store).await.unwrap();
        assert_eq!(status.state, ServerState::BuiltinFailed);
        assert!(
            status
                .detail
                .as_deref()
                .is_some_and(|detail| detail.contains("exit code 1"))
        );
        assert_eq!(status.log_path, Some(targeting.log_path()));
    }
    assert_eq!(std::fs::read_to_string(&crashes).unwrap(), left);
    // Une requête échoue aussitôt, clairement, sans viser l'ancien port.
    let started = Instant::now();
    let Err(AppError::Ollama(message)) = targeting.request_settings(&store).await else {
        panic!("a request went to a failed built-in Ollama");
    };
    assert!(
        message.contains("built-in Ollama isn't running") && message.contains("exit code 1"),
        "{message}"
    );
    assert!(started.elapsed() < Duration::from_secs(1));
    assert_eq!(std::fs::read_to_string(&crashes).unwrap(), left);

    // Retry.
    std::fs::write(&crashes, "0").unwrap();
    targeting.restart_builtin(&store).await.unwrap();
    assert_eq!(running(&builtin).await.state, BuiltinState::Running);
    assert_eq!(
        targeting.status(&store).await.unwrap().state,
        ServerState::Running
    );
    builtin.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_runtime_tells_which_ollama_serves() {
    let dir = temp_dir();
    let builtin = Builtin::new(fake_config(dir.path(), &[]));
    let models = builtin.config().default_models_dir();
    // Dossier vide : zéro modèle, pas « inconnu ».
    let (targeting_empty, _) = targeting(builtin.clone(), &Machine::new(ServerState::Running));
    let runtime = targeting_empty
        .runtime(&Store::open_in_memory().unwrap())
        .await
        .unwrap();
    assert_eq!(
        runtime.builtin_models,
        Some(ModelsSummary { count: 0, bytes: 0 })
    );
    assert_eq!(runtime.machine_url, address::DEFAULT_SERVER_URL);
    assert!(runtime.machine_local && !runtime.builtin_models_dir_missing);
    let manifest = models.join("manifests/registry.ollama.ai/library/qwen3/0.6b");
    std::fs::create_dir_all(manifest.parent().unwrap()).unwrap();
    std::fs::write(&manifest, "{}").unwrap();
    std::fs::create_dir_all(models.join("blobs")).unwrap();
    std::fs::write(models.join("blobs/sha256-a"), [0u8; 64]).unwrap();
    let server = FakeOllama::start(Mode::ollama()).await.unwrap();
    let machine = Machine::new(ServerState::Running);
    let (targeting, _) = targeting(builtin.clone(), &machine);
    let store = store_with_address(&server.url());
    let runtime = targeting.runtime(&store).await.unwrap();
    assert_eq!(runtime.kind, TargetKind::Machine);
    assert_eq!(runtime.url, server.url());
    assert_eq!(runtime.served_version.as_deref(), Some("0.35.1"));
    assert!(runtime.local && runtime.explicit_address);
    assert_eq!(runtime.machine_url, server.url());
    assert!(runtime.machine_local);
    assert_eq!(runtime.builtin_state, BuiltinState::Stopped);
    // Modèles restés dans le dossier de l'intégré.
    assert_eq!(
        runtime.builtin_models,
        Some(ModelsSummary {
            count: 1,
            bytes: 64
        })
    );
    assert_eq!(runtime.builtin_models_dir, models.to_string_lossy());
    assert_eq!(runtime.log_path, targeting.log_path());
}

/// rust-4 : source `auto` à l'adresse par défaut, rien d'installé ; une
/// requête qui arrive avant la première vérification de l'écran l'attend (la
/// prend) au lieu de partir vers l'adresse par défaut.
#[tokio::test(flavor = "multi_thread")]
async fn an_early_request_waits_for_the_first_decision() {
    let dir = temp_dir();
    let builtin = Builtin::new(fake_config(dir.path(), &[]));
    let machine = Machine::new(ServerState::NotInstalled);
    let (targeting, _) = targeting(builtin.clone(), &machine);
    let store = Store::open_in_memory().unwrap();
    let settings = targeting.request_settings(&store).await.unwrap();
    let snapshot = builtin.snapshot();
    assert_eq!(snapshot.state, BuiltinState::Running);
    assert_eq!(Some(settings.effective_server_url), snapshot.url);
    // Une seule vérification, gardée pour la session.
    targeting.request_settings(&store).await.unwrap();
    assert_eq!(machine.calls(), 1);
    builtin.stop().await;
}

/// rust-9 : une requête a noté l'Ollama de la machine, puis la première
/// vérification ne trouve rien d'installé : un seul changement, vers
/// l'intégré.
#[tokio::test(flavor = "multi_thread")]
async fn a_noted_machine_gives_way_to_the_builtin_once() {
    let dir = temp_dir();
    let builtin = Builtin::new(fake_config(dir.path(), &[]));
    let machine = Machine::new(ServerState::NotInstalled);
    let (targeting, changes) = targeting(builtin.clone(), &machine);
    let store = Store::open_in_memory().unwrap();
    let (_, noted) = targeting.settings(&store).await.unwrap();
    assert_eq!(noted.kind, TargetKind::Machine);
    let status = targeting.status(&store).await.unwrap();
    assert!(status.builtin);
    assert_eq!(
        *changes.lock(),
        [OllamaTarget {
            url: builtin.snapshot().url.unwrap(),
            kind: TargetKind::Builtin
        }]
    );
    targeting.status(&store).await.unwrap();
    assert_eq!(changes.lock().len(), 1);
    builtin.stop().await;
}

/// rust-9 : le rappel de l'app ([`announce`]) arrête les téléchargements du
/// ModelHub lancés vers l'ancienne cible, puis prévient la page.
#[tokio::test(flavor = "multi_thread")]
async fn a_target_change_closes_the_downloads_to_the_old_one() {
    let dir = temp_dir();
    let old = FakeOllama::start(Mode::ollama()).await.unwrap();
    let downloads = Arc::new(HubDownloads::with_options(
        |_| {},
        Options {
            hf_url: Some(old.url()),
            ..Options::default()
        },
    ));
    let events = Arc::new(Mutex::new(Vec::new()));
    let builtin = Builtin::new(fake_config(dir.path(), &[]));
    let (to_downloads, to_page) = (downloads.clone(), events.clone());
    let targeting = Targeting::with_check(
        builtin.clone(),
        move |target| {
            announce(target, Some(&to_downloads), |target| {
                to_page.lock().push(target.clone());
            });
        },
        |url| Box::pin(async move { canned(url, ServerState::Running) }),
    )
    .without_ollama_host();
    let store = store_with_address(&old.url());
    targeting.settings(&store).await.unwrap();
    let model = "hf.co/chatonlm/A-stall-GGUF:Q4_K_M".to_owned();
    downloads
        .download(
            &old.url(),
            HubDownloadRequest {
                model: model.clone(),
                repo: None,
                label: None,
            },
        )
        .unwrap();
    let state = || {
        downloads
            .list()
            .into_iter()
            .find(|download| download.model == model)
            .map(|download| (download.state, download.error))
    };
    assert!(
        eventually(|| state().is_some_and(|(state, _)| state == HubDownloadState::Downloading))
            .await
    );
    targeting
        .set_source(&store, OllamaSource::Builtin)
        .await
        .unwrap();
    let (state, error) = state().unwrap();
    assert_eq!(state, HubDownloadState::Failed);
    assert_eq!(
        error.as_deref(),
        Some(crate::hub::errors::english::SERVER_CHANGED)
    );
    assert!(eventually(|| old.disconnects() == 1).await);
    assert_eq!(
        *events.lock(),
        [OllamaTarget {
            url: builtin.snapshot().url.unwrap(),
            kind: TargetKind::Builtin
        }]
    );
    builtin.stop().await;
}

/// rust-3 : une vérification de la machine lancée avant « Use the built-in
/// Ollama » finit après : sa décision, périmée, n'est pas notée (ni
/// événement vers la machine, ni arrêt de l'intégré).
#[tokio::test(flavor = "multi_thread")]
async fn a_decision_taken_with_old_settings_is_dropped() {
    let dir = temp_dir();
    let builtin = Builtin::new(fake_config(dir.path(), &[]));
    let machine = Machine::new(ServerState::NotResponding);
    let (targeting, changes) = targeting(builtin.clone(), &machine);
    let targeting = Arc::new(targeting);
    let store = store_with_address("127.0.0.1:9");
    targeting.status(&store).await.unwrap();
    let gate = machine.hold();
    let checking = tokio::spawn({
        let (targeting, store) = (targeting.clone(), store.clone());
        async move { targeting.status(&store).await.unwrap() }
    });
    assert!(eventually(|| machine.calls() == 2).await);
    *machine.gate.lock() = None;
    targeting
        .set_source(&store, OllamaSource::Builtin)
        .await
        .unwrap();
    let url = builtin.snapshot().url.unwrap();
    gate.notify_one();
    let status = checking.await.unwrap();
    // Refaite avec les nouveaux réglages.
    assert!(status.builtin, "{status:?}");
    assert_eq!(
        *changes.lock(),
        [OllamaTarget {
            url,
            kind: TargetKind::Builtin
        }]
    );
    assert_eq!(running(&builtin).await.state, BuiltinState::Running);
    builtin.stop().await;
}

/// rust-1 : retour à l'Ollama de la machine : les travaux qui parlent à
/// l'intégré (chat, cours, correction) s'arrêtent avant lui.
#[tokio::test(flavor = "multi_thread")]
async fn leaving_the_builtin_stops_its_work_first() {
    let dir = temp_dir();
    let builtin = Builtin::new(fake_config(dir.path(), &[]));
    let machine = Machine::new(ServerState::Running);
    let (targeting, _) = targeting(builtin.clone(), &machine);
    let (targeting, stops) = record_stops(targeting);
    let store = Store::open_in_memory().unwrap();
    targeting
        .set_source(&store, OllamaSource::Builtin)
        .await
        .unwrap();
    assert_eq!(running(&builtin).await.state, BuiltinState::Running);
    assert!(stops.lock().is_empty());
    targeting
        .set_source(&store, OllamaSource::Auto)
        .await
        .unwrap();
    assert_eq!(builtin.snapshot().state, BuiltinState::Stopped);
    let stops = stops.lock().clone();
    assert!(!stops.is_empty());
    assert!(
        stops.iter().all(|state| *state == BuiltinState::Running),
        "{stops:?}"
    );
}

/// rust-6 : intégré choisi pour la session, puis adresse réglée dans
/// Settings : il ne sert plus, ses travaux puis lui s'arrêtent.
#[tokio::test(flavor = "multi_thread")]
async fn an_explicit_address_stops_the_session_builtin() {
    let dir = temp_dir();
    let builtin = Builtin::new(fake_config(dir.path(), &[]));
    let machine = Machine::new(ServerState::NotInstalled);
    let (targeting, changes) = targeting(builtin.clone(), &machine);
    let (targeting, stops) = record_stops(targeting);
    let store = Store::open_in_memory().unwrap();
    assert!(targeting.status(&store).await.unwrap().builtin);
    assert_eq!(running(&builtin).await.state, BuiltinState::Running);
    let server = FakeOllama::start(Mode::ollama()).await.unwrap();
    let patch = SettingsPatch {
        server_url: Some(server.url()),
        ..Default::default()
    };
    store
        .blocking(|c| settings::update(c, patch, None).map(drop))
        .unwrap();
    targeting.settings_changed();
    let (settings, _) = targeting.settings(&store).await.unwrap();
    assert_eq!(settings.effective_server_url, server.url());
    assert!(eventually(|| builtin.snapshot().state == BuiltinState::Stopped).await);
    assert_eq!(*stops.lock(), [BuiltinState::Running]);
    assert_eq!(
        changes.lock().last(),
        Some(&OllamaTarget {
            url: server.url(),
            kind: TargetKind::Machine
        })
    );
}

/// rust-1 : pendant un changement du dossier des modèles (intégré suspendu
/// et arrêté), une requête attend la fin, puis va à l'intégré relancé ;
/// jamais à l'ancienne adresse.
#[tokio::test(flavor = "multi_thread")]
async fn requests_wait_while_the_builtin_is_paused() {
    let dir = temp_dir();
    let builtin = Builtin::new(fake_config(dir.path(), &[]));
    let machine = Machine::new(ServerState::Running);
    let (targeting, _) = targeting(builtin.clone(), &machine);
    let targeting = Arc::new(targeting);
    let store = Store::open_in_memory().unwrap();
    store
        .blocking(|c| builtin_ollama::set_source(c, OllamaSource::Builtin))
        .unwrap();
    targeting.request_settings(&store).await.unwrap();
    let first = builtin.snapshot().pid;
    let paused = builtin.pause();
    builtin.stop().await;
    let request = tokio::spawn({
        let (targeting, store) = (targeting.clone(), store.clone());
        async move { targeting.request_settings(&store).await }
    });
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(!request.is_finished());
    assert_eq!(builtin.snapshot().state, BuiltinState::Stopped);
    drop(paused);
    let settings = tokio::time::timeout(Duration::from_secs(20), request)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let snapshot = builtin.snapshot();
    assert_eq!(snapshot.state, BuiltinState::Running);
    assert_ne!(snapshot.pid, first);
    assert_eq!(Some(settings.effective_server_url), snapshot.url);
    builtin.stop().await;
}

/// rust-5, sec-2 : après `shutdown` (installation d'une mise à jour), ni la
/// vérification de l'écran, ni Settings, ni une requête ne relancent
/// l'intégré ; la requête échoue clairement. Relancé après la reprise.
#[tokio::test(flavor = "multi_thread")]
async fn after_shutdown_nothing_restarts_the_builtin() {
    let dir = temp_dir();
    let builtin = Builtin::new(fake_config(dir.path(), &[]));
    let machine = Machine::new(ServerState::Running);
    let (targeting, _) = targeting(builtin.clone(), &machine);
    let store = Store::open_in_memory().unwrap();
    store
        .blocking(|c| builtin_ollama::set_source(c, OllamaSource::Builtin))
        .unwrap();
    targeting.request_settings(&store).await.unwrap();
    builtin.shutdown().await;
    for _ in 0..2 {
        targeting.status(&store).await.unwrap();
        targeting.runtime(&store).await.unwrap();
        let Err(AppError::Ollama(message)) = targeting.request_settings(&store).await else {
            panic!("request after shutdown");
        };
        assert!(message.contains("isn't running"), "{message}");
    }
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(builtin.snapshot().state, BuiltinState::Stopped);
    assert!(!builtin.config().pid_file().exists());
    builtin.reopen();
    targeting.request_settings(&store).await.unwrap();
    assert_eq!(builtin.snapshot().state, BuiltinState::Running);
    builtin.stop().await;
}

/// sec-10 : dossier des modèles choisi mais absent (disque démonté) : comme
/// l'officiel, l'intégré prend le dossier par défaut, sans recréer l'autre ;
/// Settings le sait.
#[tokio::test(flavor = "multi_thread")]
async fn a_missing_models_folder_falls_back_to_the_default() {
    let dir = temp_dir();
    let builtin = Builtin::new(fake_config(dir.path(), &[]));
    let machine = Machine::new(ServerState::Running);
    let (targeting, _) = targeting(builtin.clone(), &machine);
    let store = Store::open_in_memory().unwrap();
    let unplugged = dir.path().join("unplugged/models");
    let chosen = unplugged.to_string_lossy().into_owned();
    store
        .blocking(move |c| {
            builtin_ollama::set_source(c, OllamaSource::Builtin)?;
            builtin_ollama::set_models_dir(c, Some(&chosen))
        })
        .unwrap();
    let runtime = targeting.runtime(&store).await.unwrap();
    let default = builtin.config().default_models_dir();
    assert!(runtime.builtin_models_dir_missing);
    assert!(runtime.builtin_models_dir_is_default);
    assert_eq!(runtime.builtin_models_dir, default.to_string_lossy());
    targeting.request_settings(&store).await.unwrap();
    assert_eq!(
        builtin.snapshot().models_dir.as_deref(),
        Some(default.as_path())
    );
    assert!(!dir.path().join("unplugged").exists());
    builtin.stop().await;

    // Le disque revient : son dossier sert de nouveau.
    std::fs::create_dir_all(&unplugged).unwrap();
    let runtime = targeting.runtime(&store).await.unwrap();
    assert!(!runtime.builtin_models_dir_missing && !runtime.builtin_models_dir_is_default);
    assert_eq!(runtime.builtin_models_dir, unplugged.to_string_lossy());
    builtin.stop().await;
}

/// ui-6 : l'adresse que reprendrait la source `auto` (ici `OLLAMA_HOST`,
/// distante), pour le bouton qui quitte l'intégré.
#[tokio::test(flavor = "multi_thread")]
async fn the_runtime_gives_the_address_left_to_auto() {
    let dir = temp_dir();
    let builtin = Builtin::new(fake_config(dir.path(), &[]));
    let (targeting, _) = targeting(builtin.clone(), &Machine::new(ServerState::Running));
    let targeting = targeting.with_ollama_host(|| Some("http://192.168.1.2:11434".into()));
    let store = Store::open_in_memory().unwrap();
    store
        .blocking(|c| builtin_ollama::set_source(c, OllamaSource::Builtin))
        .unwrap();
    let runtime = targeting.runtime(&store).await.unwrap();
    assert_eq!(runtime.kind, TargetKind::Builtin);
    assert!(runtime.local && runtime.explicit_address);
    assert_eq!(runtime.machine_url, "http://192.168.1.2:11434");
    assert!(!runtime.machine_local);
    builtin.stop().await;
}
