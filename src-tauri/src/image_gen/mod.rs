//! Moteur d'images (propre à ChatonLM) : `sd-server` de stable-diffusion.cpp,
//! lancé et piloté par l'app, comme l'Ollama intégré (choix et mesures :
//! `.tools/research/82-image-generation-research.md`).
//!
//! - Binaire : `<ressources>/sd-server/sd-server` (`.exe` sous Windows) ;
//!   builds de débogage et d'essai : `CHATONLM_SD_SERVER_BIN` le remplace
//!   (faux `fake-sd-server` du crate fake-ollama). Journal :
//!   `<données>/logs/sd-server.log` (le précédent en `.1`).
//! - Un serveur charge UN modèle (ses fichiers sont des arguments) : il est
//!   lancé à la première image, gardé tant que le même modèle sert, relancé
//!   pour un autre, arrêté après `Timing::idle_stop` sans image (la mémoire,
//!   RAM et VRAM, revient alors au système et à Ollama).
//! - Une image à la fois : les demandes suivantes attendent leur tour.
//!   Chaque demande donne tous ses réglages (le serveur accepte sans rien
//!   dire des valeurs absentes ou fausses et prend alors les siennes) ; taille,
//!   étapes et guidage sont vérifiés ici avant tout lancement ; méthode
//!   d'échantillonnage et planificateur viennent du catalogue des modèles et
//!   passent tels quels (un nom inconnu est ignoré par le serveur).
//! - Sous-processus isolé comme l'intégré (`ollama::builtin::process` :
//!   groupe et PDEATHSIG sous Linux, Job Object sous Windows) : rien ne
//!   survit à ChatonLM.
//! - Arrêt d'une image : abandonner (lâcher) le futur de [`ImageGen::generate`].
//!   Le serveur ne sait pas interrompre un calcul (409) : une image commencée
//!   est arrêtée en arrêtant le processus, relancé à la demande suivante.
//! - Mémoire de la carte : le serveur place lui-même poids et calculs selon
//!   la VRAM libre au moment de générer (« auto-fit ») ; rien n'est déchargé
//!   d'Ollama ici.

// Branché à la conversation à une étape suivante (outil `generate_image`,
// bouton « Image ») : seuls les essais s'en servent pour l'instant.
#![allow(dead_code, reason = "branché à la conversation ensuite")]

mod api;
mod server;

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::Duration;

use rand::RngExt;
use tokio::sync::watch;

use crate::i18n::t;
use crate::ollama::builtin::process::{Spawner, is_executable};
use server::{Running, Stops};

/// Côtés permis d'une image, en pixels (pas de 16 : exigence des modèles
/// FLUX et Z-Image).
pub const MIN_SIDE: u32 = 64;
pub const MAX_SIDE: u32 = 2048;
pub const SIDE_STEP: u32 = 16;
/// Étapes d'échantillonnage permises.
pub const MAX_STEPS: u32 = 150;
/// Longueur maximale d'une description (caractères).
pub const MAX_PROMPT: usize = 8_000;

/// Rôle d'un fichier du modèle : l'option de `sd-server` qui le reçoit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Component {
    /// Modèle complet en un fichier (SD 1.x, SDXL...).
    Model,
    /// Modèle de diffusion seul (FLUX, Z-Image...).
    DiffusionModel,
    Vae,
    /// Encodeur de texte qui est un modèle de langage (Qwen3...).
    Llm,
    ClipL,
    ClipG,
    T5xxl,
}

impl Component {
    fn option(self) -> &'static str {
        match self {
            Component::Model => "--model",
            Component::DiffusionModel => "--diffusion-model",
            Component::Vae => "--vae",
            Component::Llm => "--llm",
            Component::ClipL => "--clip_l",
            Component::ClipG => "--clip_g",
            Component::T5xxl => "--t5xxl",
        }
    }
}

/// Ce que le serveur charge : fichiers du modèle et options de lancement
/// (`--vae-tiling`, `-t 4`...). Deux demandes au même modèle partagent le
/// serveur ; un autre modèle le relance.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelSpec {
    pub files: Vec<(Component, PathBuf)>,
    pub options: Vec<String>,
}

impl ModelSpec {
    /// Arguments de `sd-server` pour ce modèle.
    fn args(&self) -> Vec<OsString> {
        let mut args = Vec::new();
        for (component, path) in &self.files {
            args.push(component.option().into());
            args.push(path.as_os_str().to_owned());
        }
        args.extend(self.options.iter().map(OsString::from));
        args
    }
}

/// Réglages d'échantillonnage (propres au modèle : 4 étapes et CFG 1 pour
/// FLUX.2 klein, 8 et 1 pour Z-Image-Turbo...).
#[derive(Clone, Debug, PartialEq)]
pub struct Sampling {
    pub steps: u32,
    pub cfg_scale: f32,
    pub sample_method: Option<String>,
    pub scheduler: Option<String>,
}

/// Une image à produire.
#[derive(Clone, Debug)]
pub struct Request {
    pub prompt: String,
    pub negative_prompt: String,
    pub width: u32,
    pub height: u32,
    /// `None` : tirée au hasard (et rendue avec l'image).
    pub seed: Option<u32>,
    pub sampling: Sampling,
    /// Aperçus pendant le calcul (projection rapide des latents).
    pub preview: bool,
}

/// Image produite (PNG).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Image {
    pub png: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub seed: u32,
}

/// Avancement d'une demande.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Progress {
    /// Le serveur démarre (modèle pas encore chargé).
    Starting,
    /// Demande reçue, en attente d'une autre image.
    Queued,
    /// Calcul en cours : chargement du modèle (pas encore d'étape), puis
    /// étape `step` sur `total` et son aperçu (PNG) s'il est demandé.
    Generating {
        step: Option<(u32, u32)>,
        preview: Option<Vec<u8>>,
    },
}

/// Échec d'une demande ; messages dans la langue du moment.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ImageGenError {
    #[error("{}", t!("imageGen.unavailable"))]
    Unavailable,
    /// Fermeture de l'app (ou mise à jour) : plus aucune image.
    #[error("{}", t!("imageGen.closed"))]
    Closed,
    #[error("{}", t!("imageGen.missingFile", path = _0.display().to_string()))]
    MissingFile(PathBuf),
    /// Demande refusée avant tout lancement (message déjà produit).
    #[error("{0}")]
    Invalid(String),
    /// Le serveur n'a pas pu charger le modèle ou démarrer.
    #[error("{}", t!("imageGen.startFailed", reason = _0))]
    StartFailed(String),
    #[error("{}", t!("imageGen.notReady", seconds = _0))]
    NotReady(u64),
    /// Le serveur s'est arrêté pendant une image.
    #[error("{}", t!("imageGen.crashed", status = _0))]
    Crashed(String),
    /// Le serveur a refusé ou raté l'image (raison donnée par lui).
    #[error("{}", t!("imageGen.failed", reason = _0))]
    Failed(String),
    /// Le serveur ne répond plus, ou par autre chose que son API.
    #[error("{}", t!("imageGen.lost", error = _0))]
    Lost(String),
}

/// Délais (raccourcis par les essais).
#[derive(Clone, Debug)]
pub struct Timing {
    /// Attente de la première réponse du serveur (le modèle est chargé
    /// plus tard, à la première image).
    pub ready_timeout: Duration,
    pub ready_poll: Duration,
    /// Intervalle de suivi d'une image.
    pub poll: Duration,
    /// Serveur vivant qui ne répond plus : abandon après ce délai.
    pub lost_after: Duration,
    /// Arrêt du serveur après ce délai sans image.
    pub idle_stop: Duration,
    pub stop_grace: Duration,
    pub kill_wait: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            ready_timeout: Duration::from_secs(60),
            ready_poll: Duration::from_millis(200),
            poll: Duration::from_millis(250),
            lost_after: Duration::from_secs(20),
            idle_stop: Duration::from_secs(300),
            stop_grace: Duration::from_secs(2),
            kill_wait: Duration::from_secs(2),
        }
    }
}

/// Réglages du sous-processus.
#[derive(Clone, Debug)]
pub struct Config {
    /// Binaire ; `None` : moteur indisponible.
    pub bin: Option<PathBuf>,
    /// Arguments placés avant ceux du serveur (essais : relance du binaire
    /// d'essai sur le faux).
    pub prefix_args: Vec<OsString>,
    /// Essais : les arguments du serveur passent par la variable
    /// `fake_ollama::sdcpp::ARGS_ENV` (la ligne de commande est à libtest).
    pub args_in_env: Option<String>,
    /// `<données>/logs/sd-server.log`.
    pub log: PathBuf,
    /// Variables ajoutées en dernier (essais, pilote Vulkan masqué...).
    pub extra_env: Vec<(OsString, OsString)>,
    pub timing: Timing,
}

impl Config {
    /// Réglages de l'app : `data_dir` = dossier des données.
    pub fn new(bin: Option<PathBuf>, data_dir: &Path) -> Config {
        Config {
            bin,
            prefix_args: Vec::new(),
            args_in_env: None,
            log: data_dir.join("logs").join("sd-server.log"),
            extra_env: Vec::new(),
            timing: Timing::default(),
        }
    }
}

/// Binaire du moteur sous `resource_dir` ; builds de débogage et d'essai :
/// `CHATONLM_SD_SERVER_BIN` (chemin absolu ; vide : aucun) le remplace.
pub fn locate(resource_dir: Option<&Path>) -> Option<PathBuf> {
    #[cfg(any(debug_assertions, feature = "e2e"))]
    if let Some(path) = std::env::var_os("CHATONLM_SD_SERVER_BIN") {
        let path = PathBuf::from(path);
        return (path.is_absolute() && is_executable(&path)).then_some(path);
    }
    let bin = resource_dir?
        .join("sd-server")
        .join(format!("sd-server{}", std::env::consts::EXE_SUFFIX));
    is_executable(&bin).then_some(bin)
}

/// Moteur d'images, partagé par l'app.
#[derive(Clone)]
pub struct ImageGen {
    inner: Arc<Inner>,
}

struct Inner {
    config: Config,
    /// Fil qui crée les processus (PDEATHSIG suit ce fil).
    spawner: parking_lot::Mutex<Option<Spawner>>,
    /// Serveur lancé ; le verrou est tenu pendant toute une image (une à la
    /// fois, les suivantes attendent dans l'ordre).
    server: tokio::sync::Mutex<Option<Running>>,
    /// Numéro du processus du serveur lancé (0 : aucun), lisible pendant une
    /// image (jauge de la VRAM).
    pid: Arc<AtomicU32>,
    /// Arrêts de serveurs en cours : attendus avant d'en lancer un autre.
    stops: Stops,
    /// Numéro de la dernière activité : un arrêt pour inactivité n'a lieu que
    /// s'il n'a pas changé entre-temps.
    activity: AtomicU64,
    /// Passé à `true` par [`ImageGen::shutdown`] : plus aucune image, celle en
    /// cours s'arrête.
    closed: watch::Sender<bool>,
}

impl ImageGen {
    pub fn new(config: Config) -> ImageGen {
        ImageGen {
            inner: Arc::new(Inner {
                config,
                spawner: parking_lot::Mutex::new(None),
                server: tokio::sync::Mutex::new(None),
                pid: Arc::new(AtomicU32::new(0)),
                stops: Stops::default(),
                activity: AtomicU64::new(0),
                closed: watch::channel(false).0,
            }),
        }
    }

    /// Un binaire est là.
    pub fn available(&self) -> bool {
        self.inner.config.bin.as_deref().is_some_and(is_executable)
    }

    /// Numéro du processus du serveur lancé (jauge de la VRAM), même pendant
    /// une image.
    pub fn server_pid(&self) -> Option<u32> {
        Some(self.inner.pid.load(Ordering::SeqCst)).filter(|pid| *pid != 0)
    }

    /// Attente bornée des arrêts en cours.
    fn stop_limit(&self) -> Duration {
        let timing = &self.inner.config.timing;
        timing.stop_grace + timing.kill_wait + Duration::from_secs(1)
    }

    /// Produit une image avec le modèle `spec`. `progress` reçoit
    /// l'avancement. Lâcher le futur arrête l'image (et le serveur s'il
    /// calculait).
    pub async fn generate(
        &self,
        spec: &ModelSpec,
        request: &Request,
        progress: &mut (dyn FnMut(Progress) + Send),
    ) -> Result<Image, ImageGenError> {
        validate(request)?;
        if !self.available() {
            return Err(ImageGenError::Unavailable);
        }
        let mut closed = self.inner.closed.subscribe();
        if *closed.borrow_and_update() {
            return Err(ImageGenError::Closed);
        }
        // Quelle que soit la fin (même futur lâché), l'arrêt pour inactivité
        // est reprogrammé.
        let _idle = IdleStop(self);
        self.touch();
        let mut slot = match self.inner.server.try_lock() {
            Ok(slot) => slot,
            Err(_) => {
                progress(Progress::Queued);
                tokio::select! {
                    biased;
                    _ = closed.wait_for(|closed| *closed) => return Err(ImageGenError::Closed),
                    slot = self.inner.server.lock() => slot,
                }
            }
        };
        if *closed.borrow() {
            return Err(ImageGenError::Closed);
        }
        self.touch();
        let outcome = {
            // Arrêt du serveur si le futur est lâché pendant l'image (le
            // serveur ne sait pas l'interrompre).
            let mut flight = InFlight {
                slot: &mut slot,
                armed: false,
            };
            let outcome = tokio::select! {
                biased;
                _ = closed.wait_for(|closed| *closed) => Err(ImageGenError::Closed),
                outcome = self.run(&mut flight, spec, request, progress) => outcome,
            };
            // Fin normale : l'arrêt éventuel est fait ci-dessous, attendu.
            flight.armed = false;
            outcome
        };
        if matches!(
            outcome,
            Err(ImageGenError::Crashed(_) | ImageGenError::Lost(_) | ImageGenError::Closed)
        ) && let Some(running) = slot.take()
        {
            running.stop(&self.inner.config.timing).await;
        }
        outcome
    }

    async fn run(
        &self,
        flight: &mut InFlight<'_>,
        spec: &ModelSpec,
        request: &Request,
        progress: &mut (dyn FnMut(Progress) + Send),
    ) -> Result<Image, ImageGenError> {
        let timing = &self.inner.config.timing;
        let seed = request
            .seed
            .unwrap_or_else(|| rand::rng().random_range(0..=i32::MAX as u32));
        let mut started = false;
        let id = loop {
            // Autre modèle, ou serveur arrêté entre-temps : relancé.
            let reuse = flight
                .slot
                .as_mut()
                .is_some_and(|running| running.spec() == spec && running.alive());
            if !reuse {
                if let Some(previous) = flight.slot.take() {
                    previous.stop(timing).await;
                }
                progress(Progress::Starting);
                // Un serveur lâché (image arrêtée) meurt peut-être encore : sa
                // mémoire n'est pas partagée avec le suivant.
                self.inner.stops.settled(self.stop_limit()).await;
                let running = server::start(
                    &self.inner.config,
                    &self.inner.spawner,
                    spec,
                    &self.inner.pid,
                    &self.inner.stops,
                )
                .await?;
                *flight.slot = Some(running);
                started = true;
            }
            let Some(running) = flight.slot.as_mut() else {
                return Err(ImageGenError::Lost(t!("imageGen.detail.noServer")));
            };
            // Armé dès l'envoi : une demande reçue puis abandonnée occuperait
            // le serveur pour rien.
            flight.armed = true;
            let client = running.client();
            match api::submit(&client, running.url(), &api::body(request, seed)).await {
                Ok(id) => break id,
                Err(error) => {
                    // Serveur gardé mort entre deux images, sa fin pas encore
                    // signalée (il semblait vivant) : relancé une fois.
                    if !started && died_within(running, DEATH_NOTICE).await {
                        flight.armed = false;
                        flight.slot.take();
                        continue;
                    }
                    return Err(if running.alive() {
                        ImageGenError::Lost(error)
                    } else {
                        ImageGenError::Crashed(running.exit_description())
                    });
                }
            }
        };
        let Some(running) = flight.slot.as_mut() else {
            return Err(ImageGenError::Lost(t!("imageGen.detail.noServer")));
        };
        let client = running.client();
        // Dernier avancement annoncé (état, passe et étape) : chaque
        // changement l'est une fois.
        let mut announced: Option<(bool, Option<(u32, u32)>)> = None;
        let mut failing_since: Option<tokio::time::Instant> = None;
        loop {
            if !running.alive() {
                return Err(ImageGenError::Crashed(running.exit_description()));
            }
            match api::job(&client, running.url(), &id).await {
                Ok(job) => {
                    failing_since = None;
                    match job.state {
                        api::JobState::Queued => {
                            if announced != Some((false, None)) {
                                announced = Some((false, None));
                                progress(Progress::Queued);
                            }
                        }
                        api::JobState::Generating { preview } => {
                            let key = Some((true, preview.as_ref().map(|f| (f.pass, f.step))));
                            if key != announced {
                                announced = key;
                                progress(Progress::Generating {
                                    step: preview.as_ref().map(|frame| (frame.step, frame.total)),
                                    preview: preview.map(|frame| frame.png),
                                });
                            }
                        }
                        api::JobState::Completed { png } => {
                            flight.armed = false;
                            let (width, height) = api::png_size(&png).ok_or_else(|| {
                                ImageGenError::Failed(t!("imageGen.detail.notPng"))
                            })?;
                            return Ok(Image {
                                png,
                                width,
                                height,
                                seed,
                            });
                        }
                        api::JobState::Failed { message } => {
                            flight.armed = false;
                            return Err(ImageGenError::Failed(message));
                        }
                    }
                }
                Err(error) => {
                    let since = *failing_since.get_or_insert_with(tokio::time::Instant::now);
                    if since.elapsed() >= timing.lost_after {
                        return Err(if running.alive() {
                            ImageGenError::Lost(error)
                        } else {
                            ImageGenError::Crashed(running.exit_description())
                        });
                    }
                }
            }
            tokio::time::sleep(timing.poll).await;
        }
    }

    /// Nouvelle activité : annule un arrêt pour inactivité prévu.
    fn touch(&self) -> u64 {
        self.inner.activity.fetch_add(1, Ordering::SeqCst) + 1
    }

    /// Arrête le serveur après `idle_stop` sans nouvelle demande.
    fn schedule_idle_stop(&self) {
        let mark = self.touch();
        let inner = self.inner.clone();
        crate::ollama::builtin::spawn(async move {
            tokio::time::sleep(inner.config.timing.idle_stop).await;
            if inner.activity.load(Ordering::SeqCst) != mark {
                return;
            }
            let mut slot = inner.server.lock().await;
            if inner.activity.load(Ordering::SeqCst) != mark {
                return;
            }
            if let Some(running) = slot.take() {
                running.stop(&inner.config.timing).await;
            }
        });
    }

    /// Fermeture (ou installation d'une mise à jour) : l'image en cours
    /// s'arrête, le serveur aussi ; plus rien ne démarre ensuite.
    pub async fn shutdown(&self) {
        self.inner.closed.send_replace(true);
        let limit = self.stop_limit();
        let stopping = async {
            let mut slot = self.inner.server.lock().await;
            if let Some(running) = slot.take() {
                running.stop(&self.inner.config.timing).await;
            }
            drop(slot);
            self.inner.stops.settled(limit).await;
        };
        if tokio::time::timeout(limit, stopping).await.is_err() {
            // i18n-ignore : journal.
            eprintln!("chatonlm: the image engine didn't stop in time; leaving it");
        }
    }
}

/// Délai laissé à la fin d'un serveur pour être signalée (un processus à
/// plusieurs fils semble encore vivant un instant après sa mort).
const DEATH_NOTICE: Duration = Duration::from_secs(1);

/// Le serveur s'avère mort dans `limit`.
async fn died_within(running: &mut Running, limit: Duration) -> bool {
    let deadline = tokio::time::Instant::now() + limit;
    loop {
        if !running.alive() {
            return true;
        }
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// Reprogramme l'arrêt pour inactivité à la fin d'une demande, quelle
/// qu'elle soit.
struct IdleStop<'a>(&'a ImageGen);

impl Drop for IdleStop<'_> {
    fn drop(&mut self) {
        self.0.schedule_idle_stop();
    }
}

/// Image en cours : si son futur est lâché pendant le calcul, le serveur est
/// lâché aussi, donc arrêté (voir `Running`), car il ne sait pas interrompre
/// une image ; relancé à la demande suivante.
struct InFlight<'a> {
    slot: &'a mut Option<Running>,
    armed: bool,
}

impl Drop for InFlight<'_> {
    fn drop(&mut self) {
        if self.armed {
            self.slot.take();
        }
    }
}

/// Demande vérifiée avant tout lancement.
fn validate(request: &Request) -> Result<(), ImageGenError> {
    let prompt = request.prompt.trim();
    if prompt.is_empty() {
        return Err(ImageGenError::Invalid(t!("imageGen.invalid.emptyPrompt")));
    }
    if request.prompt.chars().count() > MAX_PROMPT
        || request.negative_prompt.chars().count() > MAX_PROMPT
    {
        return Err(ImageGenError::Invalid(t!(
            "imageGen.invalid.longPrompt",
            max = MAX_PROMPT
        )));
    }
    let side_ok =
        |side: u32| (MIN_SIDE..=MAX_SIDE).contains(&side) && side.is_multiple_of(SIDE_STEP);
    if !side_ok(request.width) || !side_ok(request.height) {
        return Err(ImageGenError::Invalid(t!(
            "imageGen.invalid.size",
            min = MIN_SIDE,
            max = MAX_SIDE,
            step = SIDE_STEP
        )));
    }
    let sampling = &request.sampling;
    if !(1..=MAX_STEPS).contains(&sampling.steps) {
        return Err(ImageGenError::Invalid(t!(
            "imageGen.invalid.steps",
            max = MAX_STEPS
        )));
    }
    if !(sampling.cfg_scale.is_finite() && (0.0..=30.0).contains(&sampling.cfg_scale)) {
        return Err(ImageGenError::Invalid(t!("imageGen.invalid.cfg")));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
