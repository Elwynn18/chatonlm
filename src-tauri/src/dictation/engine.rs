//! Moteur de transcription : Parakeet (ONNX Runtime) ou Whisper
//! (whisper.cpp), par transcribe-rs. Un seul modèle chargé à la fois :
//! chargé dès le début de l'enregistrement (en parallèle), gardé 5 min après
//! la dernière dictée puis libéré, remplacé si le réglage change.
//!
//! Une longue dictée est transcrite par morceaux de 40 s environ, coupés sur
//! le bloc de 30 ms le plus calme près de chaque frontière : Parakeet échoue
//! au-delà d'environ 400 s (encodage de position limité à 5 000 trames) et,
//! bien avant, perd des phrases entières (mesuré sur 472 s de français :
//! morceaux de 40 s, 1 426 mots sur ~1 432 ; de 50 à 60 s, 11 à 13 % de
//! perdus ; un seul bloc de 75 s, 31 mots sur ~220). L'abandon prend effet
//! entre deux morceaux.

use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::time::Duration;

use parking_lot::Mutex;

use super::audio::{BLOCK, RATE};
use super::models::{CatalogModel, SpeechEngine};
use crate::i18n::t;

/// Modèle gardé en mémoire après sa dernière utilisation.
pub const IDLE: Duration = Duration::from_secs(5 * 60);

/// Refus de charger un modèle pendant la fermeture de l'app (jamais montré :
/// la dictée est déjà abandonnée).
pub const CLOSING: &str = "ChatonLM is closing";

/// Transcription abandonnée entre deux morceaux (jamais montré : la dictée
/// est déjà abandonnée).
pub const CANCELLED: &str = "Transcription cancelled";

/// Échec de la transcription ; `error` : détail technique.
pub(crate) fn transcription_failed(error: &dyn std::fmt::Display) -> String {
    t!("dictation.transcriptionFailed", error = error)
}

/// Dictée transcrite d'un seul morceau jusqu'à cette durée.
const MAX_CHUNK: usize = 45 * RATE as usize;

/// Longueur visée des morceaux d'une longue dictée.
const CHUNK_TARGET: usize = 40 * RATE as usize;

/// Une coupure se cherche à 4 s au plus de sa place idéale (morceaux de
/// 48 s au plus).
const CUT_SEARCH: usize = 4 * RATE as usize;

/// Fils de calcul de Whisper au plus (au-delà, plus lent : mémoire).
const MAX_THREADS: usize = 8;

/// Fichiers d'un modèle sur le disque.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelSpec {
    /// Identifiant du catalogue.
    pub id: String,
    pub engine: SpeechEngine,
    /// Dossier du modèle (Parakeet : plusieurs fichiers).
    pub dir: PathBuf,
    /// Premier fichier (Whisper : le seul).
    pub file: PathBuf,
}

impl ModelSpec {
    /// `root` : dossier des modèles de transcription.
    pub fn new(model: &CatalogModel, root: &Path) -> Self {
        let dir = root.join(model.id);
        Self {
            id: model.id.to_owned(),
            engine: model.engine,
            file: dir.join(model.files[0].path),
            dir,
        }
    }
}

/// Réglages d'une transcription.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Request {
    /// Code de langue ; `None` : détection (Whisper), ignoré par Parakeet.
    pub language: Option<String>,
    /// Dictionnaire personnalisé (consigne initiale de Whisper).
    pub glossary: Vec<String>,
    /// Noms du projet d'une dictée de l'éditeur (pont VS Code), hors du
    /// dictionnaire : la moitié de la consigne au moins leur revient.
    pub project: Vec<String>,
}

/// Longueur maximale de la consigne initiale de Whisper (caractères) :
/// whisper.cpp n'en garde que la fin, environ 224 jetons (la moitié de son
/// contexte de texte) ; des noms de code font 2 à 4 caractères par jeton.
pub const MAX_PROMPT_CHARS: usize = 500;

/// Début de la consigne initiale.
const PROMPT_START: &str = "Glossary: ";

impl Request {
    /// Consigne initiale de Whisper : « Glossary: A, B, C. », bornée à
    /// [`MAX_PROMPT_CHARS`] (whisper.cpp couperait le début), premiers mots
    /// de chaque liste gardés. Avec des noms du projet (dictée de l'éditeur),
    /// le dictionnaire de l'utilisateur en prend la moitié au plus, puis les
    /// noms (les plus proches du fichier ouvert d'abord), puis la suite du
    /// dictionnaire dans ce qui reste : un long dictionnaire ne les chasse
    /// jamais.
    pub fn initial_prompt(&self) -> Option<String> {
        let budget = MAX_PROMPT_CHARS - PROMPT_START.len() - 1;
        let mut words = Words::default();
        let mut dictionary = 0;
        if self.project.is_empty() {
            words.take(&self.glossary, &mut dictionary, budget);
        } else {
            words.take(&self.glossary, &mut dictionary, budget / 2);
            words.take(&self.project, &mut 0, budget);
            words.take(&self.glossary, &mut dictionary, budget);
        }
        (!words.list.is_empty()).then(|| format!("{PROMPT_START}{}.", words.list.join(", ")))
    }
}

/// Mots de la consigne, et leur longueur avec les séparateurs (sans le début
/// ni le point final).
#[derive(Default)]
struct Words<'a> {
    list: Vec<&'a str>,
    used: usize,
}

impl<'a> Words<'a> {
    /// Prend les mots de `from` à partir de `next` tant que la longueur reste
    /// dans `limit` ; s'arrête au premier qui ne tient pas (`next` le
    /// désigne alors : il peut être repris avec une limite plus large).
    fn take(&mut self, from: &'a [String], next: &mut usize, limit: usize) {
        while let Some(word) = from.get(*next) {
            let cost = word.chars().count() + if self.list.is_empty() { 0 } else { 2 };
            if self.used + cost > limit {
                break;
            }
            self.used += cost;
            self.list.push(word);
            *next += 1;
        }
    }
}

/// Modèle chargé.
pub trait Recognizer: Send {
    fn transcribe(&mut self, samples: &[f32], request: &Request) -> Result<String, String>;
}

/// Chargement d'un modèle (bloquant).
pub type Loader = dyn Fn(&ModelSpec) -> Result<Box<dyn Recognizer>, String> + Send + Sync;

/// Moteur partagé (clonable).
#[derive(Clone)]
pub struct Engine {
    shared: Arc<Shared>,
}

struct Shared {
    loader: Box<Loader>,
    idle: Duration,
    slot: Mutex<Option<Loaded>>,
    /// Dictées en cours : le modèle n'est pas libéré pendant ce temps.
    holds: AtomicUsize,
    /// Change à chaque fin de dictée : seul le dernier délai libère.
    generation: AtomicU64,
    /// Fermeture de l'app : plus aucun chargement (voir [`Engine::shutdown`]).
    closed: AtomicBool,
}

struct Loaded {
    id: String,
    recognizer: Box<dyn Recognizer>,
}

impl Default for Engine {
    fn default() -> Self {
        Self::new()
    }
}

impl Engine {
    /// Moteur de transcribe-rs, modèle libéré après [`IDLE`].
    pub fn new() -> Self {
        Self::with_loader(Box::new(load), IDLE)
    }

    pub fn with_loader(loader: Box<Loader>, idle: Duration) -> Self {
        Self {
            shared: Arc::new(Shared {
                loader,
                idle,
                slot: Mutex::new(None),
                holds: AtomicUsize::new(0),
                generation: AtomicU64::new(0),
                closed: AtomicBool::new(false),
            }),
        }
    }

    /// Garde le modèle chargé tant que le résultat vit (une dictée) ; le délai
    /// de libération part à sa fin.
    pub fn hold(&self) -> Hold {
        self.shared.holds.fetch_add(1, Ordering::SeqCst);
        Hold(self.shared.clone())
    }

    /// Charge le modèle en arrière-plan s'il ne l'est pas (début de
    /// l'enregistrement). Un échec est signalé par [`Engine::transcribe`],
    /// qui réessaie.
    pub fn preload(&self, spec: ModelSpec) {
        if self.shared.closed.load(Ordering::SeqCst) {
            return;
        }
        let shared = self.shared.clone();
        tauri::async_runtime::spawn_blocking(move || {
            let mut slot = shared.slot.lock();
            if let Err(error) = shared.ensure(&mut slot, &spec) {
                eprintln!("chatonlm: speech model {}: {error}", spec.id);
            }
        });
    }

    /// Transcrit (bloquant), après avoir chargé le modèle si besoin (ou
    /// attendu la fin de son chargement anticipé), morceau par morceau (voir
    /// [`chunks`]), textes joints par une espace. `cancel` levé, ou l'app qui
    /// ferme : arrêt avant le morceau suivant ([`CANCELLED`]).
    pub fn transcribe(
        &self,
        spec: &ModelSpec,
        samples: &[f32],
        request: &Request,
        cancel: &AtomicBool,
    ) -> Result<String, String> {
        let mut slot = self.shared.slot.lock();
        let loaded = self.shared.ensure(&mut slot, spec)?;
        let mut texts = Vec::new();
        for range in chunks(samples) {
            if cancel.load(Ordering::SeqCst) || self.shared.closed.load(Ordering::SeqCst) {
                return Err(CANCELLED.into());
            }
            let text = loaded.recognizer.transcribe(&samples[range], request)?;
            let text = text.trim();
            if !text.is_empty() {
                texts.push(text.to_owned());
            }
        }
        Ok(texts.join(" "))
    }

    /// Identifiant du modèle chargé (attend une transcription en cours).
    #[cfg(test)]
    pub fn loaded(&self) -> Option<String> {
        self.shared
            .slot
            .lock()
            .as_ref()
            .map(|loaded| loaded.id.clone())
    }

    /// Libère le modèle chargé, sauf s'il s'agit de `keep` (bloquant : attend
    /// une transcription en cours).
    pub fn release_except(&self, keep: Option<&str>) {
        let mut slot = self.shared.slot.lock();
        if slot
            .as_ref()
            .is_some_and(|loaded| Some(loaded.id.as_str()) != keep)
        {
            *slot = None;
        }
    }

    /// Libère le modèle `id` s'il est chargé (bloquant).
    pub fn release_model(&self, id: &str) {
        let mut slot = self.shared.slot.lock();
        if slot.as_ref().is_some_and(|loaded| loaded.id == id) {
            *slot = None;
        }
    }

    /// Fermeture de l'app (bloquant) : plus aucun chargement ne commence ;
    /// attend la fin d'un chargement ou d'une transcription en cours,
    /// `limit` au plus, puis libère le modèle. Sinon, les destructeurs
    /// statiques de whisper.cpp et de GGML tourneraient à la sortie pendant
    /// que le fil de chargement s'en sert encore (SIGSEGV dans
    /// `whisper_init_state`). Faux si le délai est dépassé.
    pub fn shutdown(&self, limit: Duration) -> bool {
        self.shared.closed.store(true, Ordering::SeqCst);
        match self.shared.slot.try_lock_for(limit) {
            Some(mut slot) => {
                *slot = None;
                true
            }
            None => false,
        }
    }
}

impl Shared {
    fn ensure<'a>(
        &self,
        slot: &'a mut Option<Loaded>,
        spec: &ModelSpec,
    ) -> Result<&'a mut Loaded, String> {
        // Vérifié sous le verrou : un chargement anticipé qui l'obtient après
        // `shutdown` ne commence pas.
        if self.closed.load(Ordering::SeqCst) {
            return Err(CLOSING.into());
        }
        if slot.as_ref().is_some_and(|loaded| loaded.id != spec.id) {
            // Autre modèle réglé : l'ancien libère sa mémoire d'abord.
            *slot = None;
        }
        if slot.is_none() {
            *slot = Some(Loaded {
                id: spec.id.clone(),
                recognizer: (self.loader)(spec)?,
            });
        }
        Ok(slot.as_mut().expect("model just loaded"))
    }

    /// Libère le modèle après `idle`, sauf si une dictée a commencé ou fini
    /// entre-temps.
    fn schedule_release(self: &Arc<Self>) {
        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        let shared = self.clone();
        tauri::async_runtime::spawn(async move {
            loop {
                tokio::time::sleep(shared.idle).await;
                let current = || {
                    shared.generation.load(Ordering::SeqCst) == generation
                        && shared.holds.load(Ordering::SeqCst) == 0
                };
                if !current() {
                    return;
                }
                // Chargement ou transcription en cours : nouvel essai plus tard.
                if let Some(mut slot) = shared.slot.try_lock() {
                    if current() {
                        *slot = None;
                    }
                    return;
                }
            }
        });
    }
}

/// Morceaux d'une dictée (16 kHz) : tout d'un bloc jusqu'à 45 s ; au-delà,
/// des morceaux de 40 s environ, chaque coupure placée au milieu du bloc de
/// 30 ms le plus calme à 4 s au plus de sa place idéale (48 s au plus par
/// morceau).
pub fn chunks(samples: &[f32]) -> Vec<Range<usize>> {
    let length = samples.len();
    if length <= MAX_CHUNK {
        return std::iter::once(0..length).collect();
    }
    let count = length.div_ceil(CHUNK_TARGET);
    let mut cuts = vec![0];
    for index in 1..count {
        let ideal = index * length / count;
        cuts.push(quietest(
            samples,
            ideal.saturating_sub(CUT_SEARCH),
            (ideal + CUT_SEARCH).min(length),
        ));
    }
    cuts.push(length);
    cuts.windows(2).map(|pair| pair[0]..pair[1]).collect()
}

/// Milieu du bloc de 30 ms le plus calme de `samples[start..end]`.
fn quietest(samples: &[f32], start: usize, end: usize) -> usize {
    let mut best = (f32::INFINITY, (start + end) / 2);
    let mut block = start;
    while block + BLOCK <= end {
        let energy: f32 = samples[block..block + BLOCK].iter().map(|s| s * s).sum();
        if energy < best.0 {
            best = (energy, block + BLOCK / 2);
        }
        block += BLOCK;
    }
    best.1
}

/// Dictée en cours (voir [`Engine::hold`]).
pub struct Hold(Arc<Shared>);

impl Drop for Hold {
    fn drop(&mut self) {
        if self.0.holds.fetch_sub(1, Ordering::SeqCst) == 1 {
            self.0.schedule_release();
        }
    }
}

/// Charge un modèle de transcribe-rs.
fn load(spec: &ModelSpec) -> Result<Box<dyn Recognizer>, String> {
    load_on(spec, super::engine_support)
}

/// Charge un modèle si `support` accepte son moteur sur cette machine ;
/// sinon le refus, sans rien appeler de whisper.cpp (pas même ses journaux :
/// instruction illégale sans AVX2...) ni d'`ort` (arrêt de l'app sans
/// ONNX Runtime).
fn load_on(
    spec: &ModelSpec,
    support: impl Fn(SpeechEngine) -> Result<(), String>,
) -> Result<Box<dyn Recognizer>, String> {
    support(spec.engine)?;
    let failed = |error: &dyn std::fmt::Display| t!("dictation.loadFailed", error = error);
    match spec.engine {
        SpeechEngine::Parakeet => {
            use transcribe_rs::onnx::Quantization;
            use transcribe_rs::onnx::parakeet::ParakeetModel;
            let model = ParakeetModel::load(&spec.dir, &Quantization::Int8)
                .map_err(|error| failed(&error))?;
            Ok(Box::new(Parakeet(model)))
        }
        SpeechEngine::Whisper => {
            use transcribe_rs::whisper_cpp::{WhisperEngine, WhisperLoadParams};
            // Journaux de whisper.cpp et GGML écartés (sinon sur stderr).
            whisper_rs::install_logging_hooks();
            let with_gpu = |use_gpu| {
                WhisperEngine::load_with_params(
                    &spec.file,
                    WhisperLoadParams {
                        use_gpu,
                        ..WhisperLoadParams::default()
                    },
                )
            };
            // Sans GPU utilisable (pilote, mémoire), le processeur prend le relais.
            let model = with_gpu(true)
                .or_else(|_| with_gpu(false))
                .map_err(|error| failed(&error))?;
            Ok(Box::new(Whisper(model)))
        }
    }
}

struct Parakeet(transcribe_rs::onnx::parakeet::ParakeetModel);

impl Recognizer for Parakeet {
    fn transcribe(&mut self, samples: &[f32], _request: &Request) -> Result<String, String> {
        use transcribe_rs::{SpeechModel, TranscribeOptions};
        // Langue détectée par le modèle ; silence de tête ajouté par
        // transcribe-rs (250 ms, comme le modèle l'attend).
        self.0
            .transcribe(samples, &TranscribeOptions::default())
            .map(|result| clean_transcript(&result.text))
            .map_err(|error| transcription_failed(&error))
    }
}

struct Whisper(transcribe_rs::whisper_cpp::WhisperEngine);

impl Recognizer for Whisper {
    fn transcribe(&mut self, samples: &[f32], request: &Request) -> Result<String, String> {
        use transcribe_rs::whisper_cpp::WhisperInferenceParams;
        let threads = std::thread::available_parallelism()
            .map_or(4, usize::from)
            .min(MAX_THREADS);
        let params = WhisperInferenceParams {
            language: request.language.clone(),
            n_threads: threads as i32,
            initial_prompt: request.initial_prompt(),
            ..WhisperInferenceParams::default()
        };
        self.0
            .transcribe_with(samples, &params)
            .map(|result| clean_transcript(&result.text))
            .map_err(|error| transcription_failed(&error))
    }
}

/// Texte d'une transcription : sans les annotations de Whisper
/// (« [BLANK_AUDIO] », « [Music] », « (silence) ») quand il n'y a rien
/// d'autre, espaces retirés aux bords.
pub fn clean_transcript(text: &str) -> String {
    let text = text.replace("[BLANK_AUDIO]", " ");
    let text = text.trim();
    if only_annotations(text) {
        return String::new();
    }
    text.to_owned()
}

fn only_annotations(text: &str) -> bool {
    let mut rest = text.trim();
    while !rest.is_empty() {
        let close = match rest.chars().next() {
            Some('[') => ']',
            Some('(') => ')',
            _ => return false,
        };
        match rest.find(close) {
            Some(end) => rest = rest[end + 1..].trim_start(),
            None => return false,
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicUsize;

    use super::*;

    /// Modèle qui rend « <id>: <n échantillons> » et compte ses chargements.
    struct Echo(String);

    impl Recognizer for Echo {
        fn transcribe(&mut self, samples: &[f32], request: &Request) -> Result<String, String> {
            Ok(format!(
                "{}: {} {:?}",
                self.0,
                samples.len(),
                request.language
            ))
        }
    }

    /// Jamais levé.
    static GO: AtomicBool = AtomicBool::new(false);

    fn engine(idle: Duration) -> (Engine, Arc<AtomicUsize>) {
        let loads = Arc::new(AtomicUsize::new(0));
        let counter = loads.clone();
        let engine = Engine::with_loader(
            Box::new(move |spec: &ModelSpec| {
                counter.fetch_add(1, Ordering::SeqCst);
                if spec.id == "broken" {
                    return Err("Could not load the speech model: bad file".into());
                }
                Ok(Box::new(Echo(spec.id.clone())) as Box<dyn Recognizer>)
            }),
            idle,
        );
        (engine, loads)
    }

    fn spec(id: &str) -> ModelSpec {
        ModelSpec {
            id: id.into(),
            engine: SpeechEngine::Whisper,
            dir: PathBuf::from("/nowhere").join(id),
            file: PathBuf::from("/nowhere").join(id).join("model.bin"),
        }
    }

    async fn wait_until(condition: impl Fn() -> bool) {
        for _ in 0..200 {
            if condition() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("condition not reached");
    }

    #[tokio::test]
    async fn the_preloaded_model_serves_the_transcription() {
        let (engine, loads) = engine(IDLE);
        let _hold = engine.hold();
        engine.preload(spec("whisper-tiny"));
        let probe = engine.clone();
        wait_until(|| probe.loaded().is_some()).await;
        let request = Request {
            language: Some("fr".into()),
            ..Request::default()
        };
        let text = engine
            .transcribe(&spec("whisper-tiny"), &[0.0; 3], &request, &GO)
            .unwrap();
        assert_eq!(text, "whisper-tiny: 3 Some(\"fr\")");
        assert_eq!(loads.load(Ordering::SeqCst), 1);
        // Autre modèle réglé : remplacé.
        engine
            .transcribe(&spec("whisper-base"), &[0.0], &request, &GO)
            .unwrap();
        assert_eq!(engine.loaded().as_deref(), Some("whisper-base"));
        assert_eq!(loads.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn a_failed_load_is_reported_and_retried() {
        let (engine, loads) = engine(IDLE);
        let error = engine
            .transcribe(&spec("broken"), &[0.0], &Request::default(), &GO)
            .unwrap_err();
        assert_eq!(error, "Could not load the speech model: bad file");
        assert!(engine.loaded().is_none());
        let _ = engine.transcribe(&spec("broken"), &[0.0], &Request::default(), &GO);
        assert_eq!(loads.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn the_model_is_released_once_idle_after_the_last_dictation() {
        let (engine, _) = engine(Duration::from_millis(100));
        let hold = engine.hold();
        engine
            .transcribe(&spec("whisper-tiny"), &[0.0], &Request::default(), &GO)
            .unwrap();
        // Dictée en cours : gardé au-delà du délai.
        tokio::time::sleep(Duration::from_millis(250)).await;
        assert!(engine.loaded().is_some());
        drop(hold);
        // Une nouvelle dictée qui commence aussitôt le garde aussi.
        let again = engine.hold();
        tokio::time::sleep(Duration::from_millis(250)).await;
        assert!(engine.loaded().is_some());
        drop(again);
        let probe = engine.clone();
        wait_until(|| probe.loaded().is_none()).await;
    }

    #[tokio::test]
    async fn release_keeps_or_frees_the_model() {
        let (engine, _) = engine(IDLE);
        engine
            .transcribe(&spec("whisper-tiny"), &[0.0], &Request::default(), &GO)
            .unwrap();
        engine.release_except(Some("whisper-tiny"));
        assert!(engine.loaded().is_some());
        engine.release_model("whisper-base");
        assert!(engine.loaded().is_some());
        engine.release_model("whisper-tiny");
        assert!(engine.loaded().is_none());
        engine
            .transcribe(&spec("whisper-tiny"), &[0.0], &Request::default(), &GO)
            .unwrap();
        engine.release_except(None);
        assert!(engine.loaded().is_none());
    }

    /// Modèle dont le chargement prend `delay` ; compte les chargements
    /// commencés et finis.
    fn slow_engine(delay: Duration) -> (Engine, Arc<AtomicUsize>, Arc<AtomicUsize>) {
        let started = Arc::new(AtomicUsize::new(0));
        let finished = Arc::new(AtomicUsize::new(0));
        let (begin, end) = (started.clone(), finished.clone());
        let engine = Engine::with_loader(
            Box::new(move |spec: &ModelSpec| {
                begin.fetch_add(1, Ordering::SeqCst);
                std::thread::sleep(delay);
                end.fetch_add(1, Ordering::SeqCst);
                Ok(Box::new(Echo(spec.id.clone())) as Box<dyn Recognizer>)
            }),
            IDLE,
        );
        (engine, started, finished)
    }

    #[tokio::test]
    async fn shutdown_waits_for_a_load_in_progress_then_refuses_new_ones() {
        let (engine, started, finished) = slow_engine(Duration::from_millis(300));
        engine.preload(spec("whisper-tiny"));
        let probe = started.clone();
        wait_until(move || probe.load(Ordering::SeqCst) == 1).await;
        let closing = engine.clone();
        let released =
            tokio::task::spawn_blocking(move || closing.shutdown(Duration::from_secs(10)))
                .await
                .unwrap();
        assert!(released);
        // Le chargement a fini avant le retour : la sortie peut suivre.
        assert_eq!(finished.load(Ordering::SeqCst), 1);
        assert!(engine.loaded().is_none());
        // Plus rien ne se charge.
        let refused = engine.transcribe(&spec("whisper-tiny"), &[0.0], &Request::default(), &GO);
        assert_eq!(refused.unwrap_err(), CLOSING);
        engine.preload(spec("whisper-base"));
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(started.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn shutdown_gives_up_after_its_limit() {
        let (engine, started, _) = slow_engine(Duration::from_secs(2));
        engine.preload(spec("whisper-tiny"));
        let probe = started.clone();
        wait_until(move || probe.load(Ordering::SeqCst) == 1).await;
        let begun = std::time::Instant::now();
        let closing = engine.clone();
        let released =
            tokio::task::spawn_blocking(move || closing.shutdown(Duration::from_millis(200)))
                .await
                .unwrap();
        assert!(!released);
        assert!(begun.elapsed() < Duration::from_secs(1));
    }

    /// Bruit pseudo-aléatoire de `seconds` à -20 dBFS environ.
    fn noise(seconds: usize) -> Vec<f32> {
        let mut state = 12_345u32;
        (0..seconds * RATE as usize)
            .map(|_| {
                state = state.wrapping_mul(1_103_515_245).wrapping_add(12_345);
                ((state >> 16) as f32 / 32_768.0 - 1.0) * 0.17
            })
            .collect()
    }

    #[test]
    fn long_dictations_are_cut_in_quiet_places() {
        let short = chunks(&vec![0.0; 45 * 16_000]);
        assert_eq!((short.len(), short[0].clone()), (1, 0..45 * 16_000));
        let mut samples = noise(200);
        // Silence de 0,2 s à 37 s (coupure idéale : 40 s).
        let gap = 37 * 16_000..37 * 16_000 + 3_200;
        samples[gap.clone()].fill(0.0);
        let parts = chunks(&samples);
        assert_eq!(parts.len(), 5);
        assert_eq!(parts[0].start, 0);
        assert_eq!(parts[4].end, samples.len());
        for pair in parts.windows(2) {
            assert_eq!(pair[0].end, pair[1].start);
        }
        assert!(
            parts
                .iter()
                .all(|part| part.len() <= CHUNK_TARGET + 2 * CUT_SEARCH)
        );
        assert!(gap.contains(&parts[0].end), "{parts:?}");
    }

    #[tokio::test]
    async fn chunk_texts_are_joined_and_cancel_stops_between_chunks() {
        let (engine, _) = engine(IDLE);
        let samples = noise(200);
        let text = engine
            .transcribe(&spec("whisper-tiny"), &samples, &Request::default(), &GO)
            .unwrap();
        let parts: Vec<String> = chunks(&samples)
            .iter()
            .map(|part| format!("whisper-tiny: {} None", part.len()))
            .collect();
        assert_eq!(text, parts.join(" "));

        // Modèle qui attend le feu vert au premier morceau.
        let calls = Arc::new(AtomicUsize::new(0));
        let (go, gate) = std::sync::mpsc::channel::<()>();
        let gate = Arc::new(Mutex::new(Some(gate)));
        let (counter, waiting) = (calls.clone(), gate.clone());
        struct Gated(
            Arc<AtomicUsize>,
            Arc<Mutex<Option<std::sync::mpsc::Receiver<()>>>>,
        );
        impl Recognizer for Gated {
            fn transcribe(&mut self, _: &[f32], _: &Request) -> Result<String, String> {
                self.0.fetch_add(1, Ordering::SeqCst);
                if let Some(gate) = self.1.lock().take() {
                    let _ = gate.recv_timeout(Duration::from_secs(10));
                }
                Ok("part".into())
            }
        }
        let gated = Engine::with_loader(
            Box::new(move |_: &ModelSpec| {
                Ok(Box::new(Gated(counter.clone(), waiting.clone())) as Box<dyn Recognizer>)
            }),
            IDLE,
        );
        let cancel = Arc::new(AtomicBool::new(false));
        let flag = cancel.clone();
        let running = tokio::task::spawn_blocking(move || {
            gated.transcribe(&spec("whisper-tiny"), &samples, &Request::default(), &flag)
        });
        let probe = calls.clone();
        wait_until(move || probe.load(Ordering::SeqCst) == 1).await;
        cancel.store(true, Ordering::SeqCst);
        go.send(()).unwrap();
        assert_eq!(running.await.unwrap().unwrap_err(), CANCELLED);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    /// Processeur sans AVX2 : Whisper refusé avant whisper.cpp (un fichier
    /// absent donnerait sinon l'échec de chargement de whisper.cpp) ;
    /// Parakeet n'est pas concerné.
    #[test]
    fn whisper_is_refused_without_its_instructions() {
        let dir = tempfile::tempdir().unwrap();
        let whisper = ModelSpec {
            id: "whisper-tiny".into(),
            engine: SpeechEngine::Whisper,
            dir: dir.path().into(),
            file: dir.path().join("missing.bin"),
        };
        let unsupported = super::super::cpu::unsupported(&["AVX2", "FMA"]);
        let refused = load_on(&whisper, |_| Err(unsupported.clone()))
            .err()
            .unwrap();
        assert_eq!(refused, unsupported);
        // Accepté : whisper.cpp est appelé (et échoue ici).
        let failed = load_on(&whisper, |_| Ok(())).err().unwrap();
        assert_ne!(failed, refused);
        assert!(!failed.contains("AVX2"), "{failed}");
    }

    /// Sans ONNX Runtime : Parakeet refusé avant `ort`, qui s'arrêterait
    /// sur une bibliothèque introuvable (un dossier sans modèle donnerait
    /// sinon l'échec de chargement de transcribe-rs).
    #[test]
    fn parakeet_is_refused_without_onnx_runtime() {
        let dir = tempfile::tempdir().unwrap();
        let parakeet = ModelSpec {
            id: "parakeet-tdt-0.6b-v3".into(),
            engine: SpeechEngine::Parakeet,
            dir: dir.path().into(),
            file: dir.path().join("encoder-model.int8.onnx"),
        };
        let seen = std::sync::Mutex::new(Vec::new());
        let refused = load_on(&parakeet, |engine| {
            seen.lock().unwrap().push(engine);
            Err("no runtime".into())
        })
        .err()
        .unwrap();
        assert_eq!(refused, "no runtime");
        assert_eq!(*seen.lock().unwrap(), [SpeechEngine::Parakeet]);
    }

    #[test]
    fn glossary_becomes_the_initial_prompt() {
        let request = Request {
            language: None,
            glossary: vec!["ChatonLM".into(), "Ollama".into()],
            project: Vec::new(),
        };
        assert_eq!(
            request.initial_prompt().as_deref(),
            Some("Glossary: ChatonLM, Ollama.")
        );
        assert_eq!(Request::default().initial_prompt(), None);
    }

    /// Glossaire trop long (dictionnaire et noms d'un projet) : les premiers
    /// mots gardés, la consigne bornée (whisper.cpp en couperait le début).
    #[test]
    fn a_long_glossary_keeps_its_first_words() {
        let glossary: Vec<String> = (0..400).map(|i| format!("projectName{i}")).collect();
        let request = Request {
            language: None,
            glossary,
            project: Vec::new(),
        };
        let prompt = request.initial_prompt().unwrap();
        assert!(prompt.chars().count() <= MAX_PROMPT_CHARS, "{prompt}");
        assert!(prompt.starts_with("Glossary: projectName0, projectName1, "));
        assert!(prompt.ends_with('.'));
        assert!(!prompt.contains("projectName399"));
        // Un seul mot trop long pour la consigne : rien.
        let huge = Request {
            language: None,
            glossary: vec!["x".repeat(MAX_PROMPT_CHARS)],
            project: Vec::new(),
        };
        assert_eq!(huge.initial_prompt(), None);
    }

    /// Dictée de l'éditeur : un dictionnaire qui remplirait à lui seul la
    /// consigne n'en chasse pas les noms du projet (la moitié leur revient) ;
    /// peu de noms, le dictionnaire reprend la place laissée.
    #[test]
    fn the_project_names_keep_their_share_of_the_prompt() {
        let dictionary: Vec<String> = (0..60).map(|i| format!("userWord{i:02}")).collect();
        let alone = Request {
            language: None,
            glossary: dictionary.clone(),
            project: Vec::new(),
        };
        let filled = alone.initial_prompt().unwrap();
        assert!(filled.chars().count() > MAX_PROMPT_CHARS - 12, "{filled}");

        let project: Vec<String> = (0..60).map(|i| format!("projectName{i:02}")).collect();
        let request = Request {
            language: None,
            glossary: dictionary.clone(),
            project,
        };
        let prompt = request.initial_prompt().unwrap();
        assert!(prompt.chars().count() <= MAX_PROMPT_CHARS, "{prompt}");
        assert!(prompt.starts_with("Glossary: userWord00, "), "{prompt}");
        assert!(prompt.ends_with('.'), "{prompt}");
        let names = prompt.matches("projectName").count();
        let words = prompt.matches("userWord").count();
        assert!(
            names >= 15 && words >= 15,
            "{names} names, {words} words: {prompt}"
        );
        assert!(prompt.contains("projectName00"), "{prompt}");

        // Deux noms seulement : le dictionnaire reprend le reste.
        let few = Request {
            language: None,
            glossary: dictionary,
            project: vec!["GpuChief".into(), "use_dictation".into()],
        };
        let prompt = few.initial_prompt().unwrap();
        assert!(prompt.contains("GpuChief, use_dictation"), "{prompt}");
        assert!(prompt.chars().count() > MAX_PROMPT_CHARS - 40, "{prompt}");
        assert!(prompt.chars().count() <= MAX_PROMPT_CHARS, "{prompt}");
    }

    #[test]
    fn whisper_annotations_alone_are_no_text() {
        assert_eq!(clean_transcript(" [BLANK_AUDIO] "), "");
        assert_eq!(clean_transcript("[Music] (silence)"), "");
        assert_eq!(clean_transcript(" Hello [BLANK_AUDIO] "), "Hello");
        assert_eq!(
            clean_transcript("(Bonjour) tout le monde"),
            "(Bonjour) tout le monde"
        );
    }

    /// Contrats : vrais modèles et échantillons (voir `test_data`).
    mod contract {
        use super::*;
        use crate::dictation::test_data::{model, reference, sample};

        /// Part des mots de la référence retrouvés dans le texte.
        fn recall(text: &str, reference: &str) -> f64 {
            let found = words(text);
            let expected = words(reference);
            let hits = expected.iter().filter(|word| found.contains(word)).count();
            hits as f64 / expected.len().max(1) as f64
        }

        fn words(text: &str) -> Vec<String> {
            text.to_lowercase()
                .split(|c: char| !c.is_alphanumeric())
                .filter(|word| !word.is_empty())
                .map(str::to_owned)
                .collect()
        }

        const JFK: &str = "And so my fellow Americans, ask not what your country can do for you, ask what you can do for your country.";

        fn whisper_tiny() -> ModelSpec {
            ModelSpec {
                id: "whisper-tiny".into(),
                engine: SpeechEngine::Whisper,
                dir: model("whisper"),
                file: model("whisper/ggml-tiny.bin"),
            }
        }

        fn parakeet() -> ModelSpec {
            let dir = model("parakeet-tdt-0.6b-v3-int8");
            ModelSpec {
                id: "parakeet-tdt-0.6b-v3".into(),
                engine: SpeechEngine::Parakeet,
                file: dir.join("encoder-model.int8.onnx"),
                dir,
            }
        }

        fn run(spec: &ModelSpec, language: Option<&str>) -> (String, String) {
            let jfk = sample("jfk.wav");
            let fr0 = sample("fr0.wav");
            let mut recognizer = load(spec).unwrap();
            let english = Request {
                language: language.map(|_| "en".to_owned()),
                ..Request::default()
            };
            let french = Request {
                language: language.map(str::to_owned),
                glossary: vec!["Salomon".into()],
                project: Vec::new(),
            };
            (
                recognizer.transcribe(&jfk, &english).unwrap(),
                recognizer.transcribe(&fr0, &french).unwrap(),
            )
        }

        #[test]
        #[ignore = "needs the speech models and samples of .tools"]
        fn contract_whisper_tiny_transcribes_english_and_french() {
            let (english, french) = run(&whisper_tiny(), Some("fr"));
            eprintln!("whisper tiny: {english} | {french}");
            assert!(recall(&english, JFK) > 0.9, "{english}");
            assert!(recall(&french, &reference("fr0.txt")) > 0.5, "{french}");
        }

        #[test]
        #[ignore = "needs the speech models and samples of .tools"]
        fn contract_parakeet_transcribes_english_and_french() {
            let (english, french) = run(&parakeet(), None);
            eprintln!("parakeet: {english} | {french}");
            assert!(recall(&english, JFK) > 0.9, "{english}");
            assert!(recall(&french, &reference("fr0.txt")) > 0.8, "{french}");
            // Ponctuée, avec majuscules.
            assert!(
                english.contains(',') && english.starts_with('A'),
                "{english}"
            );
        }

        /// Sept minutes de dictée : Parakeet ne passe pas 400 s d'un seul
        /// morceau (encodage de position limité à 5 000 trames).
        #[test]
        #[ignore = "needs the speech models and samples of .tools"]
        fn contract_parakeet_transcribes_seven_minutes() {
            let once = sample("fr-all.wav");
            let mut long = Vec::new();
            while long.len() < 7 * 60 * 16_000 {
                long.extend_from_slice(&once);
            }
            let rounds = long.len() / once.len();
            let expected: String = ["fr0.txt", "fr1.txt", "fr2.txt", "fr3.txt"]
                .map(reference)
                .join(" ");
            let engine = Engine::new();
            let started = std::time::Instant::now();
            let text = engine
                .transcribe(&parakeet(), &long, &Request::default(), &GO)
                .unwrap();
            eprintln!(
                "parakeet, {} s in {:?}: {} words",
                long.len() / 16_000,
                started.elapsed(),
                words(&text).len()
            );
            assert!(recall(&text, &expected) > 0.8);
            // Rien de perdu aux coupures.
            assert!(
                words(&text).len() as f64 > 0.9 * (rounds * words(&expected).len()) as f64,
                "{text}"
            );
        }
    }
}
