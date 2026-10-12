// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: Kyutai
// SPDX-FileCopyrightText: 2026 Elwynn18 and ChatonLM contributors
//
// Generation loop translated to Rust from pocket-tts 3.3.0
// (https://github.com/kyutai-labs/pocket-tts), file
// pocket_tts/models/tts_model.py (_generate, _autoregressive_generation,
// _decode_audio_worker), over our ONNX export (scripts/tts/).
// Modified for ChatonLM: early stops are retried once (audio held back until
// the chunk is long enough), stop flag checked at every frame.

//! Synthèse Pocket TTS sur ONNX Runtime (rapport 57, contrat 59 § 6).
//!
//! Trois sessions `ort` (1 fil chacune) créées après
//! `dictation::onnx::ensure()` : conditionneur de texte et pas FlowLM (par
//! langue), décodeur Mimi (commun). Boucle de pocket-tts 3.3.0
//! (`TTSModel._generate`, MIT, Kyutai) : pour chaque morceau, l'état de la
//! voix est recopié, le texte et BOS passent ensemble au premier pas, puis un
//! pas par trame de 80 ms (bruit normal d'écart-type √0,3), fin quand le
//! logit EOS dépasse −4 (pas avant la 6e trame) suivie de 3 à 5 trames,
//! plafond selon le nombre de jetons. Les latents partent au fil de Mimi au
//! fur et à mesure (lot = ce qui attend ; le premier seul), état de Mimi
//! remis à zéro à chaque morceau.
//!
//! Le texte reçu est déjà prétraité ; la préparation propre au modèle et le
//! découpage en morceaux sont dans `prompt`.
//!
//! Garde des arrêts prématurés (propre à ChatonLM) : un morceau qui finit
//! avant la moitié de la durée attendue pour ses jetons est refait une fois.
//! Pour pouvoir le refaire sans rien répéter à l'oreille, le son d'un
//! morceau n'est envoyé qu'une fois ce seuil atteint (le reste suit en
//! flux). Exception : le premier morceau d'une lecture ne retient pas son
//! son plus de `FIRST_SOUND_BUDGET` (premier son rapide même sur un
//! processeur lent) ; déjà entendu, il n'est pas refait.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use ort::session::Session;
use ort::session::builder::GraphOptimizationLevel;
use ort::value::TensorRef;
use rand::{RngExt, SeedableRng};

use super::VoiceLanguage;
use super::prompt::{self, Chunk, PromptConfig};
use super::tokenizer::Tokenizer;
use super::voice::{HEAD_DIM, HEADS, KV_ROW, LAYERS, Voice};
use crate::error::{AppError, AppResult};

/// Débit des échantillons produits.
pub const SAMPLE_RATE: u32 = 24_000;

/// Fichiers lus dans le dossier d'une voix (plus `<voix>.safetensors`).
pub const MODEL_FILES: &[&str] = &[MIMI_FILE, FLOW_FILE, TEXT_FILE, TOKENIZER_FILE];
const MIMI_FILE: &str = "mimi_decoder.onnx";
const FLOW_FILE: &str = "flow_lm_step.int8.onnx";
const TEXT_FILE: &str = "text_conditioner.onnx";
const TOKENIZER_FILE: &str = "tokenizer.json";

/// Dimension d'un latent (une trame).
pub const LATENT: usize = 32;
/// Dimension des plongements du transformer de FlowLM.
const EMBED: usize = 1024;
/// Échantillons par trame (80 ms à 24 kHz).
pub const SAMPLES_PER_FRAME: usize = 1920;
/// Pas du transformer de Mimi par trame.
const MIMI_STEPS_PER_FRAME: i64 = 16;
/// Cache de Mimi : fenêtre de 250 pas, 2 couches, k/v, 8 têtes de 64.
const MIMI_KV_SHAPE: [usize; 5] = [250, 2, 2, 8, 64];
const MIMI_KV_LEN: usize = 250 * 2 * 2 * 8 * 64;
/// Tampons des convolutions de Mimi, bout à bout (manifest.json de l'export).
const MIMI_CONV_LEN: usize = 14_720;

/// Température de l'échantillonnage (`default_temperature`).
const TEMPERATURE: f64 = 0.3;
const EOS_THRESHOLD: f32 = -4.0;
/// Fin ignorée avant cette trame (`_MIN_FRAMES_BEFORE_EOS`).
const MIN_FRAMES_BEFORE_EOS: usize = 6;

/// Garde des arrêts prématurés : durée parlée par jeton, médiane du banc
/// d'écoute (rapport 57 § 8, int8 et PyTorch, 4 textes par langue : fr
/// 0,126 à 0,165 s, en 0,161 à 0,214 s, es 0,183 à 0,209 s) ; un morceau
/// plus court que `EARLY_STOP_RATIO` fois sa durée attendue est refait.
fn seconds_per_token(language: VoiceLanguage) -> f64 {
    match language {
        VoiceLanguage::Fr => 0.14,
        VoiceLanguage::En => 0.18,
        VoiceLanguage::Es => 0.19,
    }
}
const EARLY_STOP_RATIO: f64 = 0.5;

/// Retenue au plus du son du premier morceau d'une lecture (le seuil de la
/// garde est atteint plus tôt sur un processeur rapide : 0,25 à 0,3 s pour
/// un morceau de 50 jetons sur 2 cœurs d'un Ryzen 7 3700X).
const FIRST_SOUND_BUDGET: Duration = Duration::from_millis(300);

/// Trames à atteindre pour qu'un morceau ne soit pas un arrêt prématuré
/// (au plus son plafond).
fn guard_frames(chunk: &Chunk, language: VoiceLanguage) -> usize {
    let expected = chunk.tokens.len() as f64 * seconds_per_token(language);
    ((EARLY_STOP_RATIO * expected * prompt::FRAME_RATE).ceil() as usize).min(chunk.max_frames)
}

/// Sorties d'un pas de FlowLM.
pub struct StepOutput {
    /// Latent dénormalisé, pour Mimi.
    pub mimi_latent: [f32; LATENT],
    pub eos_logit: f32,
    /// Entrée du pas suivant, `[1, EMBED]`.
    pub next_embeds: Vec<f32>,
    /// Nouvelles positions du cache, `[lignes, KV_ROW]`.
    pub new_kv: Vec<f32>,
}

/// Conditionneur et pas de FlowLM (sessions ONNX, ou fausses dans les
/// essais).
pub trait FlowModel: Send {
    /// Plongements du texte `[T, EMBED]` et de BOS `[EMBED]`.
    fn condition(&mut self, tokens: &[i64]) -> Result<(Vec<f32>, Vec<f32>), String>;
    /// Un pas : `inputs` `[rows, EMBED]`, cache `past` `[positions, KV_ROW]`.
    fn step(
        &mut self,
        inputs: &[f32],
        rows: usize,
        past: &[f32],
        positions: usize,
        noise: &[f32; LATENT],
    ) -> Result<StepOutput, String>;
}

/// État de Mimi en flux, à zéro au début de chaque morceau.
pub struct MimiState {
    pub offset: i64,
    pub past_kv: Vec<f32>,
    pub conv: Vec<f32>,
}

impl MimiState {
    pub fn new() -> Self {
        Self {
            offset: 0,
            past_kv: vec![0.0; MIMI_KV_LEN],
            conv: vec![0.0; MIMI_CONV_LEN],
        }
    }

    fn reset(&mut self) {
        self.offset = 0;
        self.past_kv.fill(0.0);
        self.conv.fill(0.0);
    }
}

impl Default for MimiState {
    fn default() -> Self {
        Self::new()
    }
}

/// Décodeur Mimi : `latents` `[N, LATENT]` -> `N × SAMPLES_PER_FRAME`
/// échantillons ; avance `state`.
pub trait MimiModel: Send {
    fn decode(&mut self, latents: &[f32], state: &mut MimiState) -> Result<Vec<f32>, String>;
}

/// Bruit de l'échantillonnage.
pub trait Noise: Send {
    fn sample(&mut self, out: &mut [f32; LATENT]);
}

/// Loi normale d'écart-type √température, par Box-Muller.
struct Gaussian {
    rng: rand::rngs::StdRng,
    std: f64,
}

impl Gaussian {
    fn new() -> Self {
        Self {
            rng: rand::rngs::StdRng::from_rng(&mut rand::rng()),
            std: TEMPERATURE.sqrt(),
        }
    }
}

impl Noise for Gaussian {
    fn sample(&mut self, out: &mut [f32; LATENT]) {
        for pair in out.as_chunks_mut::<2>().0 {
            // u1 dans ]0, 1] : ln(u1) fini.
            let u1 = 1.0 - self.rng.random::<f64>();
            let u2 = self.rng.random::<f64>();
            let radius = (-2.0 * u1.ln()).sqrt() * self.std;
            let angle = std::f64::consts::TAU * u2;
            pair[0] = (radius * angle.cos()) as f32;
            pair[1] = (radius * angle.sin()) as f32;
        }
    }
}

fn ort_error(error: impl std::fmt::Display) -> String {
    error.to_string()
}

/// Session d'un graphe : 1 fil, optimisations complètes (comme le banc).
fn session(path: &Path) -> Result<Session, String> {
    Session::builder()
        .map_err(ort_error)?
        .with_optimization_level(GraphOptimizationLevel::All)
        .map_err(ort_error)?
        .with_intra_threads(1)
        .map_err(ort_error)?
        .with_inter_threads(1)
        .map_err(ort_error)?
        .commit_from_file(path)
        .map_err(|error| format!("{}: {error}", path.display()))
}

struct OnnxFlow {
    text: Session,
    step: Session,
}

impl FlowModel for OnnxFlow {
    fn condition(&mut self, tokens: &[i64]) -> Result<(Vec<f32>, Vec<f32>), String> {
        let ids = TensorRef::from_array_view(([1, tokens.len()], tokens)).map_err(ort_error)?;
        let outputs = self
            .text
            .run(ort::inputs!["token_ids" => ids])
            .map_err(ort_error)?;
        let (_, text) = outputs["text_embeds"]
            .try_extract_tensor::<f32>()
            .map_err(ort_error)?;
        let (_, bos) = outputs["bos_embeds"]
            .try_extract_tensor::<f32>()
            .map_err(ort_error)?;
        if text.len() != tokens.len() * EMBED || bos.len() != EMBED {
            return Err("text conditioner: unexpected output size".into());
        }
        Ok((text.to_vec(), bos.to_vec()))
    }

    fn step(
        &mut self,
        inputs: &[f32],
        rows: usize,
        past: &[f32],
        positions: usize,
        noise: &[f32; LATENT],
    ) -> Result<StepOutput, String> {
        let inputs = TensorRef::from_array_view(([1, rows, EMBED], inputs)).map_err(ort_error)?;
        let past = TensorRef::from_array_view(([positions, LAYERS, 2, HEADS, HEAD_DIM], past))
            .map_err(ort_error)?;
        let noise = TensorRef::from_array_view(([1, LATENT], &noise[..])).map_err(ort_error)?;
        let outputs = self
            .step
            .run(ort::inputs![
                "inputs_embeds" => inputs,
                "past_kv" => past,
                "noise" => noise,
            ])
            .map_err(ort_error)?;
        let extract = |name: &str| {
            outputs[name]
                .try_extract_tensor::<f32>()
                .map(|(_, data)| data)
                .map_err(ort_error)
        };
        let mimi_latent: [f32; LATENT] = extract("mimi_latent")?
            .try_into()
            .map_err(|_| "flow step: bad latent size".to_owned())?;
        let eos_logit = *extract("eos_logit")?
            .first()
            .ok_or("flow step: no end logit")?;
        let next_embeds = extract("next_embeds")?.to_vec();
        let new_kv = extract("new_kv")?.to_vec();
        if next_embeds.len() != EMBED || new_kv.len() != rows * KV_ROW {
            return Err("flow step: unexpected output size".into());
        }
        Ok(StepOutput {
            mimi_latent,
            eos_logit,
            next_embeds,
            new_kv,
        })
    }
}

struct OnnxMimi {
    session: Session,
}

impl MimiModel for OnnxMimi {
    fn decode(&mut self, latents: &[f32], state: &mut MimiState) -> Result<Vec<f32>, String> {
        let frames = latents.len() / LATENT;
        let latent =
            TensorRef::from_array_view(([1, frames, LATENT], latents)).map_err(ort_error)?;
        let offset = [state.offset];
        let offset = TensorRef::from_array_view(((), &offset[..])).map_err(ort_error)?;
        let past =
            TensorRef::from_array_view((MIMI_KV_SHAPE, &state.past_kv[..])).map_err(ort_error)?;
        let conv =
            TensorRef::from_array_view(([MIMI_CONV_LEN], &state.conv[..])).map_err(ort_error)?;
        let outputs = self
            .session
            .run(ort::inputs![
                "latent" => latent,
                "offset" => offset,
                "past_kv" => past,
                "conv_state" => conv,
            ])
            .map_err(ort_error)?;
        let (_, audio) = outputs["audio"]
            .try_extract_tensor::<f32>()
            .map_err(ort_error)?;
        let (_, present) = outputs["present_kv"]
            .try_extract_tensor::<f32>()
            .map_err(ort_error)?;
        let (_, conv) = outputs["new_conv_state"]
            .try_extract_tensor::<f32>()
            .map_err(ort_error)?;
        if audio.len() != frames * SAMPLES_PER_FRAME
            || present.len() != MIMI_KV_LEN
            || conv.len() != MIMI_CONV_LEN
        {
            return Err("mimi: unexpected output size".into());
        }
        let audio = audio.to_vec();
        state.past_kv.copy_from_slice(present);
        state.conv.copy_from_slice(conv);
        state.offset += MIMI_STEPS_PER_FRAME * frames as i64;
        Ok(audio)
    }
}

enum ToMimi {
    Latent([f32; LATENT]),
    /// Fin du morceau : décoder ce qui reste, remettre l'état à zéro.
    End,
}

enum FromMimi {
    Audio(Vec<f32>),
    Failed(String),
    Ended,
}

/// Fil du décodeur Mimi (en parallèle de FlowLM).
struct MimiThread {
    jobs: Option<Sender<ToMimi>>,
    results: Receiver<FromMimi>,
    /// Arrêt demandé : les latents en attente sont jetés sans décodage.
    discard: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl MimiThread {
    fn spawn(mut model: Box<dyn MimiModel>) -> Result<Self, String> {
        let (jobs, inbox) = mpsc::channel::<ToMimi>();
        let (outbox, results) = mpsc::channel();
        let discard = Arc::new(AtomicBool::new(false));
        let skip = discard.clone();
        let thread = std::thread::Builder::new()
            .name("chatonlm-tts-mimi".into())
            .spawn(move || {
                let mut state = MimiState::new();
                let mut batch: Vec<f32> = Vec::new();
                let mut failed = false;
                while let Ok(job) = inbox.recv() {
                    let mut end = matches!(job, ToMimi::End);
                    if let ToMimi::Latent(latent) = job {
                        batch.extend_from_slice(&latent);
                        // Tout ce qui attend, en un seul appel.
                        while let Ok(next) = inbox.try_recv() {
                            match next {
                                ToMimi::Latent(latent) => batch.extend_from_slice(&latent),
                                ToMimi::End => {
                                    end = true;
                                    break;
                                }
                            }
                        }
                    }
                    if !batch.is_empty() && !failed && !skip.load(Ordering::Acquire) {
                        let reply = match model.decode(&batch, &mut state) {
                            Ok(audio) => FromMimi::Audio(audio),
                            Err(error) => {
                                failed = true;
                                FromMimi::Failed(error)
                            }
                        };
                        if outbox.send(reply).is_err() {
                            return;
                        }
                    }
                    batch.clear();
                    if end {
                        state.reset();
                        failed = false;
                        if outbox.send(FromMimi::Ended).is_err() {
                            return;
                        }
                    }
                }
            })
            .map_err(|error| error.to_string())?;
        Ok(Self {
            jobs: Some(jobs),
            results,
            discard,
            thread: Some(thread),
        })
    }

    fn send(&self, job: ToMimi) -> Result<(), String> {
        self.jobs
            .as_ref()
            .and_then(|jobs| jobs.send(job).ok())
            .ok_or_else(|| "mimi thread stopped".to_owned())
    }
}

impl Drop for MimiThread {
    fn drop(&mut self) {
        // Canal fermé : le fil sort de sa boucle.
        self.jobs.take();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Son d'un essai de morceau.
struct Attempt {
    /// Trames générées (son : `frames × SAMPLES_PER_FRAME`).
    frames: usize,
    /// Fini par la fin détectée (sinon par le plafond).
    ended: bool,
    /// Arrêt demandé pendant l'essai.
    stopped: bool,
    /// Son retenu tant que le seuil de la garde n'est pas atteint.
    held: Vec<f32>,
    /// Son déjà envoyé (seuil atteint).
    released: bool,
}

/// Où va le son reçu de Mimi pendant un essai.
fn route(attempt: &mut Attempt, audio: &[f32], stop: &AtomicBool, sink: &mut dyn FnMut(&[f32])) {
    if stop.load(Ordering::Acquire) {
        return;
    }
    if attempt.released {
        sink(audio);
    } else {
        attempt.held.extend_from_slice(audio);
    }
}

/// Modèle chargé pour une langue (une voix).
pub struct Synthesizer {
    language: VoiceLanguage,
    tokenizer: Tokenizer,
    config: PromptConfig,
    voice: Voice,
    flow: Box<dyn FlowModel>,
    mimi: MimiThread,
    noise: Box<dyn Noise>,
    /// Cache de FlowLM du morceau en cours, `[positions, KV_ROW]`.
    kv: Vec<f32>,
    first_sound_budget: Duration,
}

impl Synthesizer {
    /// Charge les fichiers de `dir` (dossier de la voix dans `voice-models`).
    /// ONNX Runtime indisponible : `AppError::System` (détail du problème).
    pub fn load(dir: &Path, language: VoiceLanguage) -> AppResult<Self> {
        crate::dictation::onnx::ensure().map_err(|problem| AppError::System(problem.detail))?;
        let model = super::models::for_language(language)
            .ok_or_else(|| AppError::Internal(format!("no voice for {}", language.code())))?;
        let engine = |error: String| AppError::Internal(error);
        if let Some(missing) = MODEL_FILES.iter().find(|name| !dir.join(name).is_file()) {
            return Err(engine(format!("{}: missing", dir.join(missing).display())));
        }
        let tokenizer = Tokenizer::from_file(&dir.join(TOKENIZER_FILE)).map_err(engine)?;
        let voice =
            Voice::load(&dir.join(format!("{}.safetensors", model.voice))).map_err(engine)?;
        let flow = OnnxFlow {
            text: session(&dir.join(TEXT_FILE)).map_err(engine)?,
            step: session(&dir.join(FLOW_FILE)).map_err(engine)?,
        };
        let mimi = OnnxMimi {
            session: session(&dir.join(MIMI_FILE)).map_err(engine)?,
        };
        Self::from_parts(
            language,
            tokenizer,
            voice,
            Box::new(flow),
            Box::new(mimi),
            Box::new(Gaussian::new()),
        )
        .map_err(engine)
    }

    fn from_parts(
        language: VoiceLanguage,
        tokenizer: Tokenizer,
        voice: Voice,
        flow: Box<dyn FlowModel>,
        mimi: Box<dyn MimiModel>,
        noise: Box<dyn Noise>,
    ) -> Result<Self, String> {
        Ok(Self {
            language,
            tokenizer,
            config: PromptConfig::for_language(language),
            voice,
            flow,
            mimi: MimiThread::spawn(mimi)?,
            noise,
            kv: Vec::new(),
            first_sound_budget: FIRST_SOUND_BUDGET,
        })
    }

    /// Synthétise `text` en flux, premier son d'une lecture : voir
    /// [`Synthesizer::speak_with`]. Gardée pour les appels simples (essais,
    /// contrats) ; le service passe par `speak_with`.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn speak(
        &mut self,
        text: &str,
        stop: &AtomicBool,
        sink: &mut dyn FnMut(&[f32]),
    ) -> AppResult<()> {
        self.speak_with(text, true, stop, sink)
    }

    /// Synthétise `text` en flux : `sink` reçoit les échantillons mono à
    /// `SAMPLE_RATE` au fil de la génération ; retour anticipé (Ok) dès que
    /// `stop` est levé (vérifié à chaque trame ; `sink` n'est alors plus
    /// appelé). Garde des arrêts prématurés : un morceau nettement plus
    /// court que prévu est refait une fois (son retenu jusqu'au seuil ; voir
    /// l'en-tête du module). `first_sound` : rien ne joue encore (début
    /// d'une lecture) : le premier morceau ne retient pas son son plus de
    /// `FIRST_SOUND_BUDGET`. Faux pour la suite d'une lecture automatique
    /// (synthétisée pendant que la phrase précédente joue) : garde entière.
    /// Après une erreur, le moteur reste utilisable.
    pub fn speak_with(
        &mut self,
        text: &str,
        first_sound: bool,
        stop: &AtomicBool,
        sink: &mut dyn FnMut(&[f32]),
    ) -> AppResult<()> {
        let started = Instant::now();
        let chunks = prompt::chunks(&self.tokenizer, text, &self.config);
        for (index, chunk) in chunks.iter().enumerate() {
            if stop.load(Ordering::Acquire) {
                return Ok(());
            }
            let guard = guard_frames(chunk, self.language);
            // Premier son d'une lecture : retenu au plus jusqu'à cette
            // échéance.
            let deadline = (first_sound && index == 0).then(|| started + self.first_sound_budget);
            let first = self.attempt(chunk, guard, deadline, stop, sink)?;
            if first.stopped {
                return Ok(());
            }
            if first.released {
                if first.ended && first.frames < guard {
                    // Déjà entendu (échéance du premier son) : pas refait.
                    eprintln!(
                        "chatonlm: read aloud: early stop ({} of {guard} frames) after the first sound",
                        first.frames
                    );
                }
                continue;
            }
            // Fini avant le seuil : arrêt prématuré, un nouvel essai.
            eprintln!(
                "chatonlm: read aloud: early stop ({} of {guard} frames), retrying",
                first.frames
            );
            let second = self.attempt(chunk, guard, deadline, stop, sink)?;
            if second.stopped {
                return Ok(());
            }
            if !second.released {
                let best = if second.frames >= first.frames {
                    second
                } else {
                    first
                };
                sink(&best.held);
            }
        }
        Ok(())
    }

    /// Un essai de `chunk` ; le son part dans `sink` dès `guard` trames, ou
    /// dès `deadline` passée. Quoi qu'il arrive (fin, arrêt, erreur), Mimi
    /// finit le morceau et repart de zéro, et aucun son ne reste en attente :
    /// le moteur sert encore après une erreur.
    fn attempt(
        &mut self,
        chunk: &Chunk,
        guard: usize,
        deadline: Option<Instant>,
        stop: &AtomicBool,
        sink: &mut dyn FnMut(&[f32]),
    ) -> AppResult<Attempt> {
        let engine = |error: String| AppError::Internal(error);
        let mut attempt = Attempt {
            frames: 0,
            ended: false,
            stopped: false,
            held: Vec::new(),
            released: guard == 0,
        };
        let mut failure = self
            .generate(chunk, guard, deadline, stop, sink, &mut attempt)
            .err();
        // Fin du morceau (ou arrêt, ou erreur) : le reste du son, puis Mimi
        // à zéro.
        if attempt.stopped || failure.is_some() {
            self.mimi.discard.store(true, Ordering::Release);
        }
        self.mimi.send(ToMimi::End).map_err(engine)?;
        loop {
            match self.mimi.results.recv_timeout(Duration::from_millis(20)) {
                Ok(FromMimi::Audio(audio)) => {
                    if failure.is_none() {
                        route(&mut attempt, &audio, stop, sink);
                    }
                }
                Ok(FromMimi::Failed(error)) => {
                    failure.get_or_insert(error);
                }
                Ok(FromMimi::Ended) => break,
                Err(RecvTimeoutError::Timeout) => {
                    if stop.load(Ordering::Acquire) {
                        attempt.stopped = true;
                        self.mimi.discard.store(true, Ordering::Release);
                    }
                }
                Err(RecvTimeoutError::Disconnected) => {
                    return Err(engine("mimi thread stopped".into()));
                }
            }
        }
        self.mimi.discard.store(false, Ordering::Release);
        attempt.stopped |= stop.load(Ordering::Acquire);
        match failure {
            Some(error) => Err(engine(error)),
            None => Ok(attempt),
        }
    }

    /// Boucle de FlowLM d'un essai ; les latents partent à Mimi. Une erreur
    /// (de FlowLM, ou rapportée par Mimi) l'arrête.
    fn generate(
        &mut self,
        chunk: &Chunk,
        guard: usize,
        deadline: Option<Instant>,
        stop: &AtomicBool,
        sink: &mut dyn FnMut(&[f32]),
        attempt: &mut Attempt,
    ) -> Result<(), String> {
        let tokens: Vec<i64> = chunk.tokens.iter().map(|&id| i64::from(id)).collect();
        let (mut inputs, bos) = self.flow.condition(&tokens)?;
        inputs.extend_from_slice(&bos);
        let mut rows = tokens.len() + 1;
        // Chaque morceau repart de l'état de la voix.
        self.kv.clear();
        self.kv
            .reserve((self.voice.positions + rows + chunk.max_frames + 1) * KV_ROW);
        self.kv.extend_from_slice(&self.voice.kv);
        let mut positions = self.voice.positions;
        let mut eos_step = None;
        let mut noise = [0.0f32; LATENT];
        for step in 0..chunk.max_frames {
            if stop.load(Ordering::Acquire) {
                attempt.stopped = true;
                break;
            }
            // Son déjà décodé.
            while let Ok(message) = self.mimi.results.try_recv() {
                match message {
                    FromMimi::Audio(audio) => route(attempt, &audio, stop, sink),
                    FromMimi::Failed(error) => return Err(error),
                    FromMimi::Ended => {}
                }
            }
            self.noise.sample(&mut noise);
            let output = self.flow.step(
                &inputs,
                rows,
                &self.kv[..positions * KV_ROW],
                positions,
                &noise,
            )?;
            if output.new_kv.len() != rows * KV_ROW {
                return Err("flow step: unexpected cache size".into());
            }
            self.kv.extend_from_slice(&output.new_kv);
            positions += rows;
            if output.eos_logit > EOS_THRESHOLD
                && eos_step.is_none()
                && step >= MIN_FRAMES_BEFORE_EOS
            {
                eos_step = Some(step);
            }
            if let Some(eos) = eos_step
                && step >= eos + chunk.frames_after_eos
            {
                attempt.ended = true;
                break;
            }
            inputs = output.next_embeds;
            rows = 1;
            self.mimi.send(ToMimi::Latent(output.mimi_latent))?;
            attempt.frames += 1;
            if !attempt.released
                && (attempt.frames >= guard || deadline.is_some_and(|at| Instant::now() >= at))
            {
                attempt.released = true;
                if !stop.load(Ordering::Acquire) {
                    sink(&attempt.held);
                }
                attempt.held = Vec::new();
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
