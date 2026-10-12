//! Tests des téléchargements du ModelHub, contre le faux serveur (modèles
//! `hf.co/chatonlm/...` et scénarios `chatonlm-pull*`, registre de Hugging Face
//! de l'espace `chatonlm`).

use std::sync::Arc;
use std::time::Duration;

use fake_ollama::{FakeOllama, Mode};
use parking_lot::Mutex;

use super::*;
use crate::hub::errors::english::{
    CONNECTION_LOST, LOCATION_CHANGING, NOT_FOUND_OR_PRIVATE, NOT_GGUF, OFFLINE, SERVER_CHANGED,
    SHARDED, TIMED_OUT, UNAVAILABLE_VERSION, UNREACHABLE,
};
use crate::hub::installed;

use HubDownloadState::{Cancelled, Done, Downloading, Failed, Finishing, Queued, Starting};

/// Attente maximale d'un état, sur le faux serveur.
const WAIT: Duration = Duration::from_secs(5);

/// Liste émise, datée.
type Emitted = (Instant, Vec<HubDownload>);

/// Listes émises.
#[derive(Clone, Default)]
struct Recorder {
    events: Arc<Mutex<Vec<Emitted>>>,
}

impl Recorder {
    fn events(&self) -> Vec<Emitted> {
        self.events.lock().clone()
    }

    /// États successifs (distincts) de `model` dans les listes émises.
    fn states(&self, model: &str) -> Vec<HubDownloadState> {
        let mut states: Vec<HubDownloadState> = Vec::new();
        for (_, list) in self.events() {
            if let Some(download) = list.iter().find(|download| download.model == model)
                && states.last() != Some(&download.state)
            {
                states.push(download.state);
            }
        }
        states
    }

    /// Émissions où `model` figure, dans l'ordre.
    fn of(&self, model: &str) -> Vec<HubDownload> {
        self.events()
            .into_iter()
            .filter_map(|(_, list)| list.into_iter().find(|download| download.model == model))
            .collect()
    }
}

fn hub(options: Options) -> (HubDownloads, Recorder) {
    let recorder = Recorder::default();
    let sink = recorder.events.clone();
    let downloads = HubDownloads::with_options(
        move |list| sink.lock().push((Instant::now(), list.to_vec())),
        options,
    );
    (downloads, recorder)
}

/// Réglages par défaut, Hugging Face à `hf_url`.
fn at(hf_url: &str) -> Options {
    Options {
        hf_url: Some(hf_url.to_owned()),
        ..Options::default()
    }
}

fn ask(model: &str) -> HubDownloadRequest {
    HubDownloadRequest {
        model: model.to_owned(),
        repo: None,
        label: None,
    }
}

fn entry(downloads: &HubDownloads, model: &str) -> HubDownload {
    downloads
        .list()
        .into_iter()
        .find(|download| download.model == model)
        .unwrap_or_else(|| panic!("{model} is not listed"))
}

async fn wait_until(what: &str, limit: Duration, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + limit;
    while !condition() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// Attend la fin de `model` et la renvoie.
async fn finished(downloads: &HubDownloads, model: &str) -> HubDownload {
    finished_within(downloads, model, WAIT).await
}

async fn finished_within(downloads: &HubDownloads, model: &str, limit: Duration) -> HubDownload {
    let what = format!("the end of {model}");
    wait_until(&what, limit, || !active(entry(downloads, model).state)).await;
    entry(downloads, model)
}

fn sorted(mut names: Vec<String>) -> Vec<String> {
    names.sort();
    names
}

/// Adresse où rien n'écoute.
async fn closed_port() -> String {
    let fake = FakeOllama::start(Mode::ollama()).await.unwrap();
    let url = fake.url();
    fake.stop().await;
    url
}

const VISION: &str = "hf.co/chatonlm/Eye-vision-GGUF:Q4_K_M";

#[tokio::test]
async fn a_download_follows_every_layer_then_finishes() {
    let fake = FakeOllama::start(Mode::ollama()).await.unwrap();
    let (downloads, recorder) = hub(at(&fake.url()));
    downloads.download(&fake.url(), ask(VISION)).unwrap();
    let done = finished(&downloads, VISION).await;
    let whole = fake_ollama::hf_total(VISION);
    assert_eq!(done.state, Done, "{done:?}");
    assert_eq!((done.completed, done.total), (whole, whole));
    assert_eq!(done.error, None);
    assert_eq!(done.bytes_per_second, None);
    assert!(done.finished_at.unwrap() >= done.started_at);
    // Dépôt et version déduits du nom.
    assert_eq!(done.repo.as_deref(), Some("chatonlm/Eye-vision-GGUF"));
    assert_eq!(done.label.as_deref(), Some("Q4_K_M"));
    // Place libre : démarré tout de suite, sans passer par la file.
    assert_eq!(
        recorder.states(VISION),
        [Starting, Downloading, Finishing, Done]
    );
    // Taille du manifeste dès le démarrage ; progression qui ne recule pas,
    // projecteur compris.
    let mut previous = 0;
    let mut projector = false;
    for download in recorder.of(VISION) {
        if download.state == Downloading {
            assert_eq!(download.total, whole);
            assert!(download.completed >= previous, "{download:?}");
            previous = download.completed;
            projector |= download.completed > fake_ollama::HF_MODEL_LAYER;
        }
    }
    assert!(projector, "{:?}", recorder.of(VISION));
    assert_eq!(fake.pulled_names(), [VISION]);
    assert_eq!(
        fake.manifest_requests(),
        ["GET /v2/chatonlm/Eye-vision-GGUF/manifests/Q4_K_M"]
    );
    // Installé sous ce nom.
    let models = installed::list(&fake.url()).await.unwrap();
    assert_eq!(models[0].model, VISION);
    assert_eq!(models[0].size, whole);
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_eq!(fake.disconnects(), 0);
}

/// Sans manifeste (Hugging Face injoignable, autre nom) : pull quand même,
/// progression de la plus grosse couche comme l'app officielle ; elle ne
/// recule jamais (une somme des seules couches vues reculerait à l'arrivée
/// du projecteur).
#[tokio::test]
async fn without_a_manifest_the_largest_layer_is_shown() {
    let fake = FakeOllama::start(Mode::ollama()).await.unwrap();
    let (downloads, recorder) = hub(at(&closed_port().await));
    downloads.download(&fake.url(), ask(VISION)).unwrap();
    let done = finished(&downloads, VISION).await;
    let layer = fake_ollama::HF_MODEL_LAYER;
    assert_eq!(done.state, Done, "{done:?}");
    assert_eq!((done.completed, done.total), (layer, layer));
    let shown: Vec<(u64, u64)> = recorder
        .of(VISION)
        .iter()
        .filter(|download| download.state == Downloading)
        .map(|download| (download.total, download.completed))
        .collect();
    assert!(!shown.is_empty());
    assert!(shown.iter().all(|&(total, _)| total == layer), "{shown:?}");
    assert!(
        shown.windows(2).all(|pair| pair[0].1 <= pair[1].1),
        "{shown:?}"
    );

    // Nom d'Ollama : jamais de manifeste demandé à Hugging Face.
    let (downloads, _) = hub(at(&fake.url()));
    downloads
        .download(&fake.url(), ask("chatonlm-pull"))
        .unwrap();
    let done = finished(&downloads, "chatonlm-pull").await;
    let total = fake_ollama::PULL_TOTAL;
    assert_eq!(done.state, Done, "{done:?}");
    assert_eq!((done.completed, done.total), (total, total));
    assert_eq!(done.repo, None);
    assert!(fake.manifest_requests().is_empty());
}

#[tokio::test]
async fn two_downloads_at_a_time_in_request_order() {
    let fake = FakeOllama::start(Mode::ollama()).await.unwrap();
    let (downloads, _) = hub(at(&fake.url()));
    let [a, b, c, d] =
        ["A", "B", "C", "D"].map(|name| format!("hf.co/chatonlm/{name}-stall-GGUF:Q4_K_M"));
    for model in [&a, &b, &c, &d] {
        downloads.download(&fake.url(), ask(model)).unwrap();
    }
    wait_until("two downloads", WAIT, || {
        entry(&downloads, &a).state == Downloading && entry(&downloads, &b).state == Downloading
    })
    .await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    let states: Vec<_> = downloads
        .list()
        .iter()
        .map(|d| (d.model.clone(), d.state))
        .collect();
    assert_eq!(
        states,
        [
            (a.clone(), Downloading),
            (b.clone(), Downloading),
            (c.clone(), Queued),
            (d.clone(), Queued)
        ]
    );
    // Les deux premiers partent ensemble : leur ordre d'arrivée varie.
    assert_eq!(sorted(fake.pulled_names()), [a.clone(), b.clone()]);

    // Arrêt en cours : tout de suite arrêté, la requête est fermée, et le
    // premier de la file démarre.
    downloads.cancel(&a);
    let cancelled = entry(&downloads, &a);
    assert_eq!(cancelled.state, Cancelled);
    assert!(cancelled.finished_at.is_some());
    wait_until("the third download", WAIT, || {
        fake.pulled_names().len() == 3
    })
    .await;
    assert_eq!(fake.pulled_names()[2], c);
    assert_eq!(entry(&downloads, &d).state, Queued);
    wait_until("the closed request", WAIT, || fake.disconnects() == 1).await;
    // Arrêt en file : sans requête.
    downloads.cancel(&d);
    assert_eq!(entry(&downloads, &d).state, Cancelled);
    downloads.cancel(&b);
    downloads.cancel(&c);
    wait_until("every closed request", WAIT, || fake.disconnects() == 3).await;
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_eq!(
        sorted(fake.pulled_names()),
        [a.clone(), b.clone(), c.clone()]
    );
    // La fin des tâches arrêtées ne change plus rien.
    assert!(
        downloads
            .list()
            .iter()
            .all(|download| download.state == Cancelled)
    );
    assert_eq!(fake.disconnects(), 3);
}

/// La cible change (Ollama intégré ou de la machine, adresse) : les
/// téléchargements vers l'ancienne s'arrêtent, en cours (requête fermée) ou
/// en file, avec un message ; ceux vers la nouvelle continuent.
#[tokio::test]
async fn a_new_target_stops_the_downloads_to_the_old_one() {
    let old = FakeOllama::start(Mode::ollama()).await.unwrap();
    let new = FakeOllama::start(Mode::ollama()).await.unwrap();
    let (downloads, _) = hub(Options {
        max_running: 1,
        ..at(&old.url())
    });
    let [running, queued, kept] =
        ["A", "B", "C"].map(|name| format!("hf.co/chatonlm/{name}-stall-GGUF:Q4_K_M"));
    downloads.download(&old.url(), ask(&running)).unwrap();
    downloads.download(&old.url(), ask(&queued)).unwrap();
    wait_until("the first download", WAIT, || {
        entry(&downloads, &running).state == Downloading
    })
    .await;
    downloads.retarget(&new.url());
    for model in [&running, &queued] {
        let stopped = entry(&downloads, model);
        assert_eq!(stopped.state, Failed, "{model}");
        assert_eq!(stopped.error.as_deref(), Some(SERVER_CHANGED));
    }
    wait_until("the closed request", WAIT, || old.disconnects() == 1).await;
    assert_eq!(old.pulled_names(), std::slice::from_ref(&running));
    assert!(!downloads.any_active());

    downloads.download(&new.url(), ask(&kept)).unwrap();
    wait_until("the new download", WAIT, || {
        entry(&downloads, &kept).state == Downloading
    })
    .await;
    downloads.retarget(&new.url());
    assert_eq!(entry(&downloads, &kept).state, Downloading);
    downloads.cancel(&kept);
}

/// Settings > Model location attend tant qu'un téléchargement est en file ou
/// en cours, pas après sa fin ni son arrêt.
#[tokio::test]
async fn activity_lasts_while_a_download_is_queued_or_running() {
    let fake = FakeOllama::start(Mode::ollama()).await.unwrap();
    let (downloads, _) = hub(at(&fake.url()));
    assert!(!downloads.any_active());
    let slow = "hf.co/chatonlm/Turtle-slow-GGUF:Q4_K_M";
    downloads.download(&fake.url(), ask(slow)).unwrap();
    assert!(downloads.any_active());
    downloads.cancel(slow);
    assert_eq!(entry(&downloads, slow).state, Cancelled);
    assert!(!downloads.any_active());
    downloads.download(&fake.url(), ask(VISION)).unwrap();
    assert!(downloads.any_active());
    assert_eq!(finished(&downloads, VISION).await.state, Done);
    assert!(!downloads.any_active());
}

/// Verrou propre à ce test (celui de l'app est une variable globale des
/// commandes).
static CHANGING: crate::ollama::models_dir::OperationLock =
    crate::ollama::models_dir::OperationLock::new();

fn changing() -> bool {
    CHANGING.is_held()
}

/// Pendant un changement de Settings > Model location, un nouveau
/// téléchargement est refusé (rien en file, aucune requête) ; il repart une
/// fois le changement fini.
#[tokio::test]
async fn no_download_starts_while_the_model_location_changes() {
    let fake = FakeOllama::start(Mode::ollama()).await.unwrap();
    let (downloads, recorder) = hub(Options {
        location_changing: changing,
        ..at(&fake.url())
    });
    let guard = CHANGING.try_acquire().unwrap();
    let refused = downloads.download(&fake.url(), ask(VISION));
    let Err(AppError::Invalid(message)) = refused else {
        panic!("expected a refusal: {refused:?}");
    };
    assert_eq!(message, LOCATION_CHANGING);
    assert!(downloads.list().is_empty());
    assert!(!downloads.any_active());
    assert!(recorder.events().is_empty());
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(fake.manifest_requests().is_empty());
    assert!(fake.pulled_names().is_empty());
    drop(guard);
    downloads.download(&fake.url(), ask(VISION)).unwrap();
    assert_eq!(finished(&downloads, VISION).await.state, Done);
}

#[tokio::test]
async fn ollama_errors_are_readable() {
    let fake = FakeOllama::start(Mode::ollama()).await.unwrap();
    // Manifeste injoignable : Ollama dit lui-même ce qui ne va pas.
    let (downloads, _) = hub(at(&closed_port().await));
    let cases = [
        ("hf.co/chatonlm/Big-sharded-GGUF:Q8_0", SHARDED),
        (
            "hf.co/chatonlm/Gone-missing-GGUF:Q4_K_M",
            NOT_FOUND_OR_PRIVATE,
        ),
        (
            "hf.co/chatonlm/Lab-restricted-GGUF:Q4_K_M",
            NOT_FOUND_OR_PRIVATE,
        ),
        ("hf.co/chatonlm/Tiny-GGUF:badtag", UNAVAILABLE_VERSION),
        ("hf.co/chatonlm/Tiny-GGUF:badquant", UNAVAILABLE_VERSION),
        ("hf.co/chatonlm/Weights-notgguf:Q4_K_M", NOT_GGUF),
        // Fin du flux sans `success`.
        ("hf.co/chatonlm/Tiny-cut-GGUF:Q4_K_M", CONNECTION_LOST),
        ("chatonlm-pull-cut", CONNECTION_LOST),
        // Registre injoignable pour Ollama : texte officiel.
        ("chatonlm-pull-offline", OFFLINE),
        (
            "chatonlm-pull-error",
            "pull model manifest: file does not exist",
        ),
        // Refus HTTP d'Ollama : son message.
        ("chatonlm-pull-refused", "invalid model name"),
    ];
    for (model, _) in cases {
        downloads.download(&fake.url(), ask(model)).unwrap();
    }
    downloads
        .download(&fake.url(), ask("chatonlm-pull-reset"))
        .unwrap();
    for (model, error) in cases {
        let failed = finished(&downloads, model).await;
        assert_eq!(failed.state, Failed, "{model}");
        assert_eq!(failed.error.as_deref(), Some(error), "{model}");
        assert!(failed.finished_at.is_some(), "{model}");
        assert_eq!(failed.bytes_per_second, None, "{model}");
    }
    // Connexion coupée en plein flux.
    let reset = finished(&downloads, "chatonlm-pull-reset").await;
    assert_eq!(reset.state, Failed);
    let message = reset.error.unwrap();
    assert!(message.starts_with("Connection lost: "), "{message}");
    assert_eq!(fake.pull_requests().len(), cases.len() + 1);
}

#[tokio::test]
async fn refused_manifests_never_reach_ollama() {
    let fake = FakeOllama::start(Mode::ollama()).await.unwrap();
    let (downloads, recorder) = hub(at(&fake.url()));
    let cases = [
        ("hf.co/chatonlm/Big-sharded-GGUF:Q8_0", SHARDED),
        (
            "hf.co/chatonlm/Gone-missing-GGUF:Q4_K_M",
            NOT_FOUND_OR_PRIVATE,
        ),
        (
            "hf.co/chatonlm/Lab-restricted-GGUF:Q4_K_M",
            NOT_FOUND_OR_PRIVATE,
        ),
        ("hf.co/chatonlm/Tiny-GGUF:badtag", UNAVAILABLE_VERSION),
        ("hf.co/chatonlm/Tiny-GGUF:badquant", UNAVAILABLE_VERSION),
        ("hf.co/chatonlm/Weights-notgguf:Q4_K_M", NOT_GGUF),
    ];
    for (model, _) in cases {
        downloads.download(&fake.url(), ask(model)).unwrap();
    }
    for (model, error) in cases {
        let failed = finished(&downloads, model).await;
        assert_eq!(failed.error.as_deref(), Some(error), "{model}");
        assert_eq!(recorder.states(model).last(), Some(&Failed), "{model}");
        assert!(!recorder.states(model).contains(&Downloading), "{model}");
    }
    assert!(fake.pull_requests().is_empty());
    assert_eq!(fake.manifest_requests().len(), cases.len());
    // Réponse inattendue du registre (page HTML) : pull quand même.
    let html = "hf.co/chatonlm/Down-html-GGUF:Q4_K_M";
    downloads.download(&fake.url(), ask(html)).unwrap();
    assert_eq!(finished(&downloads, html).await.state, Done);
    assert_eq!(fake.pulled_names(), [html]);
}

#[tokio::test]
async fn an_unreachable_ollama() {
    let closed = closed_port().await;
    let (downloads, _) = hub(at(&closed));
    for model in ["hf.co/chatonlm/Tiny-GGUF:Q4_K_M", "qwen3:0.6b"] {
        downloads.download(&closed, ask(model)).unwrap();
        let failed = finished(&downloads, model).await;
        assert_eq!(failed.state, Failed, "{model}");
        assert_eq!(failed.error.as_deref(), Some(UNREACHABLE), "{model}");
    }
}

#[tokio::test]
async fn silence_ends_a_download() {
    let fake = FakeOllama::start(Mode::ollama()).await.unwrap();
    let idle = Duration::from_millis(300);
    let (downloads, recorder) = hub(Options {
        idle_timeout: idle,
        ..at(&fake.url())
    });
    // Trois lignes puis seulement des lignes vides (toutes les 100 ms) : elles
    // ne comptent pas.
    let stall = "hf.co/chatonlm/Tiny-stall-GGUF:Q4_K_M";
    downloads.download(&fake.url(), ask(stall)).unwrap();
    wait_until("the download", WAIT, || {
        entry(&downloads, stall).state == Downloading
    })
    .await;
    let since = Instant::now();
    let failed = finished(&downloads, stall).await;
    assert!(since.elapsed() >= idle - Duration::from_millis(100));
    assert_eq!(failed.state, Failed);
    assert_eq!(failed.error.as_deref(), Some(TIMED_OUT));
    // Pas même d'en-têtes.
    downloads
        .download(&fake.url(), ask("chatonlm-pull-wait"))
        .unwrap();
    let failed = finished(&downloads, "chatonlm-pull-wait").await;
    assert_eq!(failed.error.as_deref(), Some(TIMED_OUT));
    // Requêtes fermées.
    wait_until("the closed requests", WAIT, || fake.disconnects() == 2).await;
    assert_eq!(recorder.states(stall), [Starting, Downloading, Failed]);
}

/// Écart voulu : pas de délai d'inactivité pendant `Finishing` (Ollama
/// vérifie l'empreinte de fichiers de plusieurs Go sans rien envoyer).
#[tokio::test]
async fn a_long_checksum_is_not_silence() {
    let fake = FakeOllama::start(Mode::ollama()).await.unwrap();
    let idle = Duration::from_millis(300);
    assert!(fake_ollama::HF_CHECKSUM_PAUSE > 2 * idle);
    let (downloads, recorder) = hub(Options {
        idle_timeout: idle,
        ..at(&fake.url())
    });
    let model = "hf.co/chatonlm/Huge-checksum-GGUF:Q4_K_M";
    downloads.download(&fake.url(), ask(model)).unwrap();
    let done = finished(&downloads, model).await;
    assert_eq!(done.state, Done, "{done:?}");
    assert_eq!(done.error, None);
    assert_eq!(
        recorder.states(model),
        [Starting, Downloading, Finishing, Done]
    );
}

#[tokio::test]
async fn finished_downloads_are_replaced_or_dismissed() {
    let fake = FakeOllama::start(Mode::ollama()).await.unwrap();
    let (downloads, recorder) = hub(at(&closed_port().await));
    let cut = "hf.co/chatonlm/Tiny-cut-GGUF:Q4_K_M";
    downloads.download(&fake.url(), ask(cut)).unwrap();
    let first = finished(&downloads, cut).await;
    assert_eq!(first.state, Failed);
    downloads
        .download(&fake.url(), ask("chatonlm-pull-small"))
        .unwrap();
    finished(&downloads, "chatonlm-pull-small").await;

    // Nouvelle demande : l'échec est remplacé, en fin de liste.
    let request = HubDownloadRequest {
        model: format!(" {cut} "),
        repo: Some("chatonlm/Tiny-cut-GGUF".into()),
        label: Some("Q4_K_M".into()),
    };
    downloads.download(&fake.url(), request).unwrap();
    let names: Vec<_> = downloads.list().into_iter().map(|d| d.model).collect();
    assert_eq!(names, ["chatonlm-pull-small", cut]);
    let again = entry(&downloads, cut);
    assert!(active(again.state), "{again:?}");
    assert_eq!((again.error, again.finished_at), (None, None));
    // Déjà en cours (casse différente) : rien de plus.
    downloads
        .download(&fake.url(), ask("HF.CO/chatonlm/tiny-cut-gguf:q4_k_m"))
        .unwrap();
    assert_eq!(downloads.list().len(), 2);
    // Un actif ne se retire pas.
    downloads.dismiss(cut);
    assert_eq!(downloads.list().len(), 2);
    assert_eq!(finished(&downloads, cut).await.state, Failed);
    assert_eq!(fake.pulled_names(), [cut, "chatonlm-pull-small", cut]);

    // Fini : retiré (sans la casse) ; inconnu : rien, sans événement.
    downloads.dismiss("hf.co/chatonlm/TINY-cut-GGUF:Q4_K_M");
    let names: Vec<_> = downloads.list().into_iter().map(|d| d.model).collect();
    assert_eq!(names, ["chatonlm-pull-small"]);
    let emitted = recorder.events().len();
    downloads.dismiss("unknown");
    downloads.cancel("unknown");
    downloads.cancel("chatonlm-pull-small");
    assert_eq!(recorder.events().len(), emitted);
    assert_eq!(entry(&downloads, "chatonlm-pull-small").state, Done);

    // Noms qu'Ollama refuserait, ou `hf.co` sans étiquette.
    for model in [
        "",
        "  ",
        "hf.co/chatonlm/Tiny-GGUF",
        "hf.co/bart.owski/x:y",
        "hf.co/chatonlm/Tiny-GGUF:-tag",
        "gemma 4",
        "../gemma4",
    ] {
        assert!(
            matches!(
                downloads.download(&fake.url(), ask(model)),
                Err(AppError::Invalid(_))
            ),
            "{model:?}"
        );
    }
    assert_eq!(downloads.list().len(), 1);
}

#[tokio::test]
async fn progress_events_are_limited() {
    let fake = FakeOllama::start(Mode::ollama()).await.unwrap();
    let interval = Duration::from_millis(250);
    let (downloads, recorder) = hub(at(&fake.url()));
    // Une vingtaine de lignes en 0,7 s environ.
    downloads.download(&fake.url(), ask(VISION)).unwrap();
    finished(&downloads, VISION).await;
    let events = recorder.events();
    let mut progress = 0;
    for pair in events.windows(2) {
        let ((before, previous), (at, list)) = (&pair[0], &pair[1]);
        let states = |list: &[HubDownload]| list.iter().map(|d| d.state).collect::<Vec<_>>();
        if states(previous) == states(list) {
            progress += 1;
            let gap = at.duration_since(*before);
            assert!(gap >= interval - Duration::from_millis(20), "{gap:?}");
        }
    }
    assert!(progress >= 1, "{events:?}");
    let (_, last) = events.last().unwrap();
    assert_eq!(last[0].state, Done);

    // La progression retenue part à l'échéance, même si plus rien n'arrive :
    // trois lignes (0, 10 %, 20 %) en 60 ms, puis le silence.
    let stall = "hf.co/chatonlm/Tiny-stall-GGUF:Q4_K_M";
    downloads.download(&fake.url(), ask(stall)).unwrap();
    let third = fake_ollama::HF_MODEL_LAYER * 2 / 10;
    wait_until("the held progress", WAIT, || {
        recorder
            .of(stall)
            .last()
            .is_some_and(|d| d.completed == third)
    })
    .await;
    downloads.cancel(stall);
}

#[tokio::test]
async fn leaving_closes_running_downloads() {
    let fake = FakeOllama::start(Mode::ollama()).await.unwrap();
    let (downloads, _) = hub(at(&fake.url()));
    let [a, b, c] = ["A", "B", "C"].map(|name| format!("hf.co/chatonlm/{name}-stall-GGUF:Q4_K_M"));
    for model in [&a, &b, &c] {
        downloads.download(&fake.url(), ask(model)).unwrap();
    }
    wait_until("two downloads", WAIT, || fake.pulled_names().len() == 2).await;
    let asked = Instant::now();
    downloads.stop_all(Duration::from_secs(2)).await;
    assert!(asked.elapsed() < Duration::from_secs(1));
    wait_until("the closed requests", WAIT, || fake.disconnects() == 2).await;
    // Plus rien ne démarre.
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(sorted(fake.pulled_names()), [a, b]);
    assert_eq!(entry(&downloads, &c).state, Queued);
    downloads
        .download(&fake.url(), ask("chatonlm-pull"))
        .unwrap();
    assert_eq!(downloads.list().len(), 3);
}

#[test]
fn the_rate_is_smoothed_over_three_seconds() {
    let start = Instant::now();
    let second = Duration::from_secs(1);
    let mut rate = Rate::default();
    // Reprise : le premier relevé compte déjà des octets.
    assert_eq!(rate.add(start, 5_000), None);
    assert_eq!(rate.add(start + Duration::from_millis(250), 6_000), None);
    assert_eq!(rate.add(start + second, 9_000), Some(4_000));
    // 1 Mo/s régulier : 1 Mo/s.
    let at = |seconds: u32| start + second * seconds;
    let completed = |seconds: u64| 9_000 + (seconds - 1) * 1_000_000;
    for seconds in 2..=10 {
        rate.add(at(seconds), completed(u64::from(seconds)));
    }
    assert_eq!(rate.samples.front().unwrap().0, at(7));
    assert_eq!(rate.add(at(11), completed(11)), Some(1_000_000));
    // Arrêt net : le débit baisse sur la fenêtre, sans tomber à zéro d'un coup.
    assert_eq!(rate.add(at(12), completed(11)), Some(666_667));
    assert_eq!(rate.add(at(15), completed(11)), Some(0));
}

/// Instance Ollama de test (`scripts/test-ollama.sh`).
const TEST_INSTANCE: &str = "http://127.0.0.1:11435";

/// Contrat : téléchargement depuis la fiche d'un modèle déjà présent sur
/// l'instance de test (rien de plus n'est tiré : seul le manifeste est relu),
/// au total du manifeste de Hugging Face ; puis une version absente du
/// dépôt, refusée avec un texte lisible sans rien télécharger, que le
/// manifeste soit lu par ChatonLM ou par Ollama.
#[tokio::test]
#[ignore = "instance Ollama de test (scripts/test-ollama.sh start) et Hugging Face : accès au réseau"]
async fn contract_hub_download_from_hugging_face() {
    let hf = names::DEFAULT_HF_URL;
    let model = "hf.co/bartowski/SmolLM2-135M-Instruct-GGUF:Q4_K_M";
    let name = parse_hf_name(model).unwrap();
    let Manifest::Found { total } = manifest::fetch(hf, &name, Duration::from_secs(15)).await
    else {
        panic!("no manifest for {model}");
    };
    assert_eq!(total, 105_455_694);
    let (downloads, recorder) = hub(at(hf));
    downloads.download(TEST_INSTANCE, ask(model)).unwrap();
    let done = finished_within(&downloads, model, Duration::from_secs(120)).await;
    assert_eq!(done.state, Done, "{done:?}");
    assert_eq!((done.completed, done.total), (total, total));
    for download in recorder.of(model) {
        assert!(download.completed <= download.total, "{download:?}");
        if download.state != Starting {
            assert_eq!(download.total, total, "{download:?}");
        }
    }

    let missing = "hf.co/bartowski/SmolLM2-135M-Instruct-GGUF:bf16";
    let closed = closed_port().await;
    for hf_url in [hf, closed.as_str()] {
        let (downloads, recorder) = hub(at(hf_url));
        downloads.download(TEST_INSTANCE, ask(missing)).unwrap();
        let failed = finished_within(&downloads, missing, Duration::from_secs(60)).await;
        assert_eq!(failed.state, Failed, "{hf_url}: {failed:?}");
        assert_eq!(
            failed.error.as_deref(),
            Some(UNAVAILABLE_VERSION),
            "{hf_url}"
        );
        assert!(!recorder.states(missing).contains(&Downloading), "{hf_url}");
        assert_eq!(failed.completed, 0, "{hf_url}");
    }
    let models = installed::list(TEST_INSTANCE).await.unwrap();
    assert!(!models.iter().any(|m| m.model.eq_ignore_ascii_case(missing)));
    let smol = models.iter().find(|m| m.model == model).expect(model);
    assert_eq!(smol.quantization_level, "Q4_K_M");
    assert_eq!(smol.size, total);
    assert_eq!(
        smol.repo.as_deref(),
        Some("bartowski/SmolLM2-135M-Instruct-GGUF")
    );
}
