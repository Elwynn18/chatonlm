//! Essais du moteur : boucle avec de fausses sessions (fin, plafond, arrêt,
//! garde des arrêts prématurés, état par morceau), bruit ; contrats
//! `tts_contract_*` avec les vrais fichiers (`CHATONLM_VOICE_MODELS_DIR`).

use std::collections::VecDeque;
use std::sync::Mutex;

use super::*;
use crate::tts::tokenizer::tests::{fixture, from_fixture};
use crate::tts::voice::tests::synthetic;

/// Un appel de `FakeFlow::step`.
#[derive(Clone, Debug)]
struct Step {
    /// Essai (un par `condition`).
    attempt: usize,
    rows: usize,
    positions: usize,
    first_input: f32,
    noise: f32,
}

#[derive(Default)]
struct FlowLog {
    steps: Vec<Step>,
    attempts: usize,
}

/// FlowLM simulé : la fin arrive au pas choisi pour chaque essai (`None` :
/// jamais) ; le latent porte le numéro global du pas.
struct FakeFlow {
    log: Arc<Mutex<FlowLog>>,
    eos_at: VecDeque<Option<usize>>,
    current: Option<usize>,
    step: usize,
    total: usize,
    /// Durée d'un pas.
    delay: Duration,
    /// Cache de la voix, attendu en tête de chaque cache reçu.
    voice: Vec<f32>,
    /// Pas (numéro global) qui échoue, une fois.
    fail_at: Option<usize>,
}

impl FakeFlow {
    fn new(eos_at: &[Option<usize>]) -> (Self, Arc<Mutex<FlowLog>>) {
        let log = Arc::new(Mutex::new(FlowLog::default()));
        (
            Self {
                log: log.clone(),
                eos_at: eos_at.iter().copied().collect(),
                current: None,
                step: 0,
                total: 0,
                delay: Duration::ZERO,
                voice: Vec::new(),
                fail_at: None,
            },
            log,
        )
    }
}

impl FlowModel for FakeFlow {
    fn condition(&mut self, tokens: &[i64]) -> Result<(Vec<f32>, Vec<f32>), String> {
        self.log.lock().unwrap().attempts += 1;
        self.current = self.eos_at.pop_front().unwrap_or(Some(10));
        self.step = 0;
        Ok((vec![1.0; tokens.len() * EMBED], vec![2.0; EMBED]))
    }

    fn step(
        &mut self,
        inputs: &[f32],
        rows: usize,
        past: &[f32],
        positions: usize,
        noise: &[f32; LATENT],
    ) -> Result<StepOutput, String> {
        assert_eq!(inputs.len(), rows * EMBED);
        assert_eq!(past.len(), positions * KV_ROW);
        // Le cache repart de la voix à chaque morceau, puis grandit des
        // sorties des pas de cet essai (valeur = numéro du pas).
        assert_eq!(&past[..self.voice.len()], &self.voice[..]);
        if self.step > 0 {
            let last = &past[past.len() - KV_ROW..];
            assert!(last.iter().all(|v| *v == (self.step - 1) as f32));
        }
        std::thread::sleep(self.delay);
        if self.fail_at == Some(self.total) {
            self.fail_at = None;
            self.total += 1;
            return Err("flow broke".into());
        }
        let mut log = self.log.lock().unwrap();
        let attempt = log.attempts;
        log.steps.push(Step {
            attempt,
            rows,
            positions,
            first_input: inputs[0],
            noise: noise[0],
        });
        let eos = self.current.is_some_and(|at| self.step >= at);
        let output = StepOutput {
            mimi_latent: [self.total as f32; LATENT],
            eos_logit: if eos { 0.0 } else { -10.0 },
            next_embeds: vec![100.0 + self.step as f32; EMBED],
            new_kv: vec![self.step as f32; rows * KV_ROW],
        };
        self.step += 1;
        self.total += 1;
        Ok(output)
    }
}

/// Mimi simulé : 1920 échantillons valant le numéro du latent ; note
/// l'offset et la taille de chaque lot.
struct FakeMimi {
    calls: Arc<Mutex<Vec<(i64, usize)>>>,
    fail: bool,
}

impl MimiModel for FakeMimi {
    fn decode(&mut self, latents: &[f32], state: &mut MimiState) -> Result<Vec<f32>, String> {
        if self.fail {
            return Err("mimi broke".into());
        }
        let frames = latents.len() / LATENT;
        self.calls.lock().unwrap().push((state.offset, frames));
        state.offset += MIMI_STEPS_PER_FRAME * frames as i64;
        Ok(latents
            .chunks(LATENT)
            .flat_map(|latent| std::iter::repeat_n(latent[0], SAMPLES_PER_FRAME))
            .collect())
    }
}

struct Counter(f32);

impl Noise for Counter {
    fn sample(&mut self, out: &mut [f32; LATENT]) {
        self.0 += 1.0;
        out.fill(self.0);
    }
}

const VOICE_POSITIONS: usize = 3;

struct Rig {
    synthesizer: Synthesizer,
    flow: Arc<Mutex<FlowLog>>,
    mimi: Arc<Mutex<Vec<(i64, usize)>>>,
}

fn rig(eos_at: &[Option<usize>], mimi_fails: bool) -> Rig {
    rig_with(eos_at, mimi_fails, Duration::ZERO)
}

fn rig_with(eos_at: &[Option<usize>], mimi_fails: bool, delay: Duration) -> Rig {
    let tokenizer = from_fixture(&fixture("en"));
    let voice = Voice::parse(&synthetic(4, VOICE_POSITIONS, 0)).unwrap();
    let (mut flow, log) = FakeFlow::new(eos_at);
    flow.delay = delay;
    flow.voice = voice.kv.clone();
    let calls = Arc::new(Mutex::new(Vec::new()));
    let mimi = FakeMimi {
        calls: calls.clone(),
        fail: mimi_fails,
    };
    let synthesizer = Synthesizer::from_parts(
        VoiceLanguage::En,
        tokenizer,
        voice,
        Box::new(flow),
        Box::new(mimi),
        Box::new(Counter(0.0)),
    )
    .unwrap();
    Rig {
        synthesizer,
        flow: log,
        mimi: calls,
    }
}

const SHORT: &str = "Hello! How are you?";
const LONG: &str = "First sentence. Second sentence! Third sentence? Fourth sentence. Fifth sentence, a little longer than the others. Sixth. Seventh sentence to end the paragraph, with a few more words.";

fn chunks_of(text: &str) -> Vec<Chunk> {
    let tokenizer = from_fixture(&fixture("en"));
    prompt::chunks(
        &tokenizer,
        text,
        &PromptConfig::for_language(VoiceLanguage::En),
    )
}

/// Numéros des latents entendus, une fois par trame.
fn frames_heard(audio: &[f32]) -> Vec<f32> {
    assert_eq!(audio.len() % SAMPLES_PER_FRAME, 0);
    audio
        .chunks(SAMPLES_PER_FRAME)
        .map(|frame| {
            assert!(frame.iter().all(|value| *value == frame[0]));
            frame[0]
        })
        .collect()
}

fn speak(rig: &mut Rig, text: &str) -> Vec<f32> {
    let stop = AtomicBool::new(false);
    let mut audio = Vec::new();
    rig.synthesizer
        .speak(text, &stop, &mut |samples| audio.extend_from_slice(samples))
        .unwrap();
    audio
}

#[test]
fn a_chunk_ends_a_few_frames_after_the_end_logit() {
    let chunk = &chunks_of(SHORT)[0];
    assert_eq!(chunks_of(SHORT).len(), 1);
    let mut rig = rig(&[Some(12)], false);
    let audio = speak(&mut rig, SHORT);
    // Fin vue au pas 12, puis `frames_after_eos` trames ; le pas qui
    // constate la fin ne produit pas de son.
    let expected = 12 + chunk.frames_after_eos;
    let heard = frames_heard(&audio);
    assert_eq!(heard, (0..expected).map(|i| i as f32).collect::<Vec<_>>());
    assert_eq!(rig.flow.lock().unwrap().steps.len(), expected + 1);
}

#[test]
fn the_end_is_ignored_before_the_sixth_frame() {
    let chunk = &chunks_of(SHORT)[0];
    let mut rig = rig(&[Some(0)], false);
    let audio = speak(&mut rig, SHORT);
    assert_eq!(
        frames_heard(&audio).len(),
        MIN_FRAMES_BEFORE_EOS + chunk.frames_after_eos
    );
}

#[test]
fn a_chunk_without_end_stops_at_its_cap() {
    let chunk = &chunks_of(SHORT)[0];
    let mut rig = rig(&[None], false);
    let audio = speak(&mut rig, SHORT);
    assert_eq!(frames_heard(&audio).len(), chunk.max_frames);
    assert_eq!(rig.flow.lock().unwrap().steps.len(), chunk.max_frames);
}

/// Premier pas : texte puis BOS, depuis l'état de la voix ; ensuite un
/// latent à la fois, le cache grandissant d'autant ; bruit neuf à chaque
/// pas.
#[test]
fn each_step_extends_the_cache_of_the_voice() {
    let chunks = chunks_of(LONG);
    assert!(chunks.len() >= 2, "{chunks:?}");
    let mut rig = rig(&[Some(60), Some(60), Some(60)], false);
    speak(&mut rig, LONG);
    let log = rig.flow.lock().unwrap();
    assert_eq!(log.attempts, chunks.len());
    for (index, chunk) in chunks.iter().enumerate() {
        let steps: Vec<_> = log
            .steps
            .iter()
            .filter(|s| s.attempt == index + 1)
            .collect();
        let rows = chunk.tokens.len() + 1;
        assert_eq!(steps[0].rows, rows);
        assert_eq!(steps[0].positions, VOICE_POSITIONS);
        assert_eq!(steps[0].first_input, 1.0);
        for (k, pair) in steps.windows(2).enumerate() {
            assert_eq!(pair[1].rows, 1);
            assert_eq!(pair[1].positions, pair[0].positions + pair[0].rows);
            assert_eq!(pair[1].first_input, 100.0 + k as f32);
            assert_ne!(pair[1].noise, pair[0].noise);
        }
    }
    // Mimi repart de zéro à chaque morceau.
    let calls = rig.mimi.lock().unwrap();
    let starts = calls.iter().filter(|(offset, _)| *offset == 0).count();
    assert_eq!(starts, chunks.len(), "{calls:?}");
    for pair in calls.windows(2) {
        if pair[1].0 != 0 {
            assert_eq!(pair[1].0, pair[0].0 + 16 * pair[0].1 as i64);
        }
    }
}

#[test]
fn stopping_ends_the_reading_at_the_next_frame() {
    let mut rig = rig(&[None, None, None], false);
    let stop = AtomicBool::new(false);
    let mut calls_after_stop = 0;
    rig.synthesizer
        .speak(LONG, &stop, &mut |_| {
            if stop.load(Ordering::Acquire) {
                calls_after_stop += 1;
            }
            stop.store(true, Ordering::Release);
        })
        .unwrap();
    assert_eq!(calls_after_stop, 0);
    let log = rig.flow.lock().unwrap();
    // Un seul morceau commencé, arrêté bien avant son plafond.
    assert_eq!(log.attempts, 1);
    assert!(
        log.steps.len() < chunks_of(LONG)[0].max_frames / 2,
        "{}",
        log.steps.len()
    );
}

/// Un morceau fini bien avant sa durée attendue est refait, et seul le
/// second essai s'entend.
#[test]
fn an_early_stop_is_retried_once() {
    let chunk = &chunks_of(LONG)[0];
    let guard = guard_frames(chunk, VoiceLanguage::En);
    assert!(guard > 6 + chunk.frames_after_eos, "{guard}");
    let mut rig = rig(&[Some(6), Some(guard + 5), Some(40), Some(40)], false);
    let audio = speak(&mut rig, LONG);
    let heard = frames_heard(&audio);
    let first_attempt = (6 + chunk.frames_after_eos + 1) as f32;
    // Rien du premier essai (latents 0 à first_attempt - 2).
    assert_eq!(heard[0], first_attempt);
    assert!(heard.iter().all(|frame| *frame >= first_attempt));
    let log = rig.flow.lock().unwrap();
    assert_eq!(log.attempts, chunks_of(LONG).len() + 1);
}

/// Deux arrêts prématurés de suite : le plus long des deux essais, pas de
/// troisième.
#[test]
fn a_second_early_stop_keeps_the_longer_attempt() {
    let chunks = chunks_of(LONG);
    let chunk = &chunks[0];
    let mut eos = vec![Some(9), Some(6)];
    eos.extend(std::iter::repeat_n(Some(40), chunks.len()));
    let mut rig = rig(&eos, false);
    let audio = speak(&mut rig, LONG);
    let heard = frames_heard(&audio);
    let first = 9 + chunk.frames_after_eos;
    assert_eq!(
        heard[..first],
        (0..first).map(|i| i as f32).collect::<Vec<_>>()
    );
    assert!(heard[first] > (first + 6 + chunk.frames_after_eos) as f32);
    assert_eq!(rig.flow.lock().unwrap().attempts, chunks.len() + 1);
}

/// Le son d'un morceau n'est envoyé qu'une fois le seuil de la garde
/// atteint (rien à reprendre si l'essai est refait).
#[test]
fn audio_waits_for_the_guard_threshold() {
    let chunk = &chunks_of(LONG)[0];
    let guard = guard_frames(chunk, VoiceLanguage::En);
    let mut rig = rig(&[Some(guard + 10), Some(40), Some(40)], false);
    let flow = rig.flow.clone();
    let stop = AtomicBool::new(false);
    let mut first_call = None;
    rig.synthesizer
        .speak(LONG, &stop, &mut |_| {
            first_call.get_or_insert_with(|| flow.lock().unwrap().steps.len());
        })
        .unwrap();
    assert!(first_call.unwrap() >= guard, "{first_call:?} < {guard}");
}

/// Premier morceau sur un processeur lent : son envoyé à l'échéance du
/// premier son, avant le seuil de la garde ; déjà entendu, il n'est pas
/// refait même s'il finit trop tôt.
#[test]
fn the_first_sound_is_not_held_beyond_its_budget() {
    let chunks = chunks_of(LONG);
    let guard = guard_frames(&chunks[0], VoiceLanguage::En);
    let mut eos = vec![Some(guard - 8)];
    eos.extend(std::iter::repeat_n(Some(60), chunks.len()));
    let mut rig = rig_with(&eos, false, Duration::from_millis(2));
    rig.synthesizer.first_sound_budget = Duration::from_millis(30);
    let flow = rig.flow.clone();
    let stop = AtomicBool::new(false);
    let mut first_call = None;
    rig.synthesizer
        .speak(LONG, &stop, &mut |_| {
            first_call.get_or_insert_with(|| flow.lock().unwrap().steps.len());
        })
        .unwrap();
    let first_call = first_call.unwrap();
    assert!(first_call < guard - 8, "{first_call} / {guard}");
    assert_eq!(rig.flow.lock().unwrap().attempts, chunks.len());
}

/// Suite d'une lecture (`first_sound` faux) : pas d'échéance, la garde
/// entière s'applique même sur un processeur lent, et un arrêt prématuré
/// est refait.
#[test]
fn the_rest_of_a_reading_keeps_the_whole_guard() {
    let chunks = chunks_of(LONG);
    let guard = guard_frames(&chunks[0], VoiceLanguage::En);
    let mut eos = vec![Some(guard - 8)];
    eos.extend(std::iter::repeat_n(Some(60), chunks.len()));
    let mut rig = rig_with(&eos, false, Duration::from_millis(2));
    rig.synthesizer.first_sound_budget = Duration::from_millis(30);
    let flow = rig.flow.clone();
    let stop = AtomicBool::new(false);
    let mut first_call = None;
    rig.synthesizer
        .speak_with(LONG, false, &stop, &mut |_| {
            first_call.get_or_insert_with(|| flow.lock().unwrap().steps.len());
        })
        .unwrap();
    // Premier essai entier (fini avant le seuil) puis le second jusqu'au
    // seuil, avant le premier son.
    let first_call = first_call.unwrap();
    assert!(first_call >= guard, "{first_call} / {guard}");
    assert_eq!(rig.flow.lock().unwrap().attempts, chunks.len() + 1);
}

/// Erreur de FlowLM au milieu d'un morceau : la lecture suivante, avec le
/// même moteur, n'entend rien de l'ancienne et Mimi repart de zéro.
#[test]
fn an_error_leaves_the_engine_usable() {
    let chunk = &chunks_of(SHORT)[0];
    let tokenizer = from_fixture(&fixture("en"));
    let voice = Voice::parse(&synthetic(4, VOICE_POSITIONS, 0)).unwrap();
    let (mut flow, log) = FakeFlow::new(&[None, Some(12)]);
    flow.voice = voice.kv.clone();
    flow.fail_at = Some(15);
    let calls = Arc::new(Mutex::new(Vec::new()));
    let mimi = FakeMimi {
        calls: calls.clone(),
        fail: false,
    };
    let mut synthesizer = Synthesizer::from_parts(
        VoiceLanguage::En,
        tokenizer,
        voice,
        Box::new(flow),
        Box::new(mimi),
        Box::new(Counter(0.0)),
    )
    .unwrap();
    let stop = AtomicBool::new(false);
    let mut heard = Vec::new();
    assert!(
        synthesizer
            .speak(SHORT, &stop, &mut |s| heard.extend_from_slice(s))
            .is_err()
    );
    let before = calls.lock().unwrap().len();
    let mut audio = Vec::new();
    synthesizer
        .speak(SHORT, &stop, &mut |s| audio.extend_from_slice(s))
        .unwrap();
    // Latents 16 et suivants seulement (le pas 15 a échoué).
    let expected: Vec<f32> = (16..16 + 12 + chunk.frames_after_eos)
        .map(|i| i as f32)
        .collect();
    assert_eq!(frames_heard(&audio), expected);
    assert_eq!(calls.lock().unwrap()[before].0, 0, "Mimi starts from zero");
    assert_eq!(log.lock().unwrap().attempts, 2);
}

#[test]
fn a_mimi_failure_is_an_error() {
    let mut rig = rig(&[Some(10)], true);
    let stop = AtomicBool::new(false);
    let result = rig.synthesizer.speak(SHORT, &stop, &mut |_| {});
    assert!(result.is_err());
    // Le fil de Mimi reste utilisable.
    assert!(rig.synthesizer.mimi.send(ToMimi::End).is_ok());
}

#[test]
fn nothing_to_say_gives_no_audio() {
    let mut rig = rig(&[], false);
    assert!(speak(&mut rig, "  ").is_empty());
    assert_eq!(rig.flow.lock().unwrap().attempts, 0);
}

#[test]
fn the_noise_is_normal_with_the_model_temperature() {
    let mut noise = Gaussian::new();
    let mut values = Vec::new();
    let mut block = [0.0f32; LATENT];
    for _ in 0..2_000 {
        noise.sample(&mut block);
        values.extend(block.iter().map(|v| f64::from(*v)));
    }
    let mean = values.iter().sum::<f64>() / values.len() as f64;
    let variance = values.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / values.len() as f64;
    assert!(mean.abs() < 0.02, "{mean}");
    assert!((variance - TEMPERATURE).abs() < 0.02, "{variance}");
    // Queues de la loi normale (|z| > 3σ rare mais présent).
    let sigma = TEMPERATURE.sqrt();
    let tails = values.iter().filter(|v| v.abs() > 3.0 * sigma).count();
    assert!((5..=500).contains(&tails), "{tails}");
}

/// Seuil de la garde contre les mesures du banc d'écoute (rapport 57 § 8) :
/// (langue, jetons du morceau, durée la plus courte mesurée en s).
#[test]
fn the_guard_sits_well_below_measured_durations() {
    let measured = [
        (VoiceLanguage::Fr, 46, 6.88),
        (VoiceLanguage::Fr, 32, 4.08), // « recette » : 162 jetons, 20,4 s, 5 morceaux
        (VoiceLanguage::En, 34, 7.12),
        (VoiceLanguage::En, 34, 5.46), // « recette » : 136 jetons, 21,84 s, 4 morceaux
        (VoiceLanguage::Es, 44, 8.88),
        (VoiceLanguage::Es, 32, 6.14), // « recette » : 158 jetons, 30,72 s, 5 morceaux
    ];
    for (language, tokens, seconds) in measured {
        let chunk = Chunk {
            text: String::new(),
            tokens: vec![0; tokens],
            frames_after_eos: 3,
            max_frames: prompt::max_frames(tokens),
        };
        let guard = guard_frames(&chunk, language) as f64 / prompt::FRAME_RATE;
        // Pas de faux arrêt : marge d'au moins 35 % sous la mesure.
        assert!(guard < 0.65 * seconds, "{language:?} {tokens}: {guard} s");
        // Un morceau coupé au tiers est rattrapé.
        assert!(guard > seconds / 3.0, "{language:?} {tokens}: {guard} s");
    }
}

/// Contrats : vrais fichiers des voix (`scripts/tts/assemble.py`), dossier
/// donné par `CHATONLM_VOICE_MODELS_DIR` (sautés sans lui).
mod contract {
    use std::time::Instant;

    use super::*;

    fn models() -> Option<std::path::PathBuf> {
        let dir = std::env::var_os("CHATONLM_VOICE_MODELS_DIR").filter(|v| !v.is_empty());
        if dir.is_none() {
            eprintln!("skipped: CHATONLM_VOICE_MODELS_DIR is not set");
        }
        dir.map(std::path::PathBuf::from)
    }

    fn languages() -> [(VoiceLanguage, &'static str); 3] {
        [
            (
                VoiceLanguage::Fr,
                include_str!("../fixtures/reference-fr.json"),
            ),
            (
                VoiceLanguage::En,
                include_str!("../fixtures/reference-en.json"),
            ),
            (
                VoiceLanguage::Es,
                include_str!("../fixtures/reference-es.json"),
            ),
        ]
    }

    fn floats(value: &serde_json::Value) -> Vec<f32> {
        serde_json::from_value(value.clone()).unwrap()
    }

    fn close(actual: &[f32], expected: &[f32], tolerance: f32, what: &str) {
        assert_eq!(actual.len(), expected.len(), "{what}");
        let worst = actual
            .iter()
            .zip(expected)
            .map(|(a, e)| (a - e).abs() / (1.0 + e.abs()))
            .fold(0.0f32, f32::max);
        assert!(worst <= tolerance, "{what}: écart {worst}");
    }

    /// Rejeu pas à pas de la boucle de référence (numpy + ONNX Runtime,
    /// `scripts/tts/fixtures.py reference`, même bruit) : jetons, voix,
    /// conditionneur, premiers pas de FlowLM, premiers blocs de Mimi, fin.
    #[test]
    #[ignore = "needs the voice models (CHATONLM_VOICE_MODELS_DIR)"]
    fn tts_contract_replays_the_python_reference() {
        let Some(root) = models() else { return };
        crate::dictation::onnx::ensure().expect("ONNX Runtime");
        for (language, reference) in languages() {
            let reference: serde_json::Value = serde_json::from_str(reference).unwrap();
            let dir = root.join(format!("pocket-tts-{}", language.code()));
            let tokenizer = Tokenizer::from_file(&dir.join(TOKENIZER_FILE)).unwrap();
            let tokens: Vec<u32> = serde_json::from_value(reference["tokens"].clone()).unwrap();
            let text = reference["text"].as_str().unwrap();
            assert_eq!(tokenizer.encode(text), tokens, "{language:?}");
            let chunks = prompt::chunks(&tokenizer, text, &PromptConfig::for_language(language));
            assert_eq!(chunks.len(), 1);
            assert_eq!(chunks[0].tokens, tokens);
            assert_eq!(
                chunks[0].frames_after_eos as u64,
                reference["frames_after_eos"].as_u64().unwrap()
            );
            assert_eq!(
                chunks[0].max_frames as u64,
                reference["max_gen_len"].as_u64().unwrap()
            );
            let voice_name = reference["voice"].as_str().unwrap();
            let voice = Voice::load(&dir.join(format!("{voice_name}.safetensors"))).unwrap();
            assert_eq!(
                voice.positions as u64,
                reference["voice_positions"].as_u64().unwrap()
            );
            let mut flow = OnnxFlow {
                text: session(&dir.join(TEXT_FILE)).unwrap(),
                step: session(&dir.join(FLOW_FILE)).unwrap(),
            };
            let mut mimi = OnnxMimi {
                session: session(&dir.join(MIMI_FILE)).unwrap(),
            };

            // Conditionneur.
            let ids: Vec<i64> = tokens.iter().map(|&id| i64::from(id)).collect();
            let (text_embeds, bos) = flow.condition(&ids).unwrap();
            let conditioner = &reference["conditioner"];
            for (row, head) in conditioner["text_head"]
                .as_array()
                .unwrap()
                .iter()
                .enumerate()
            {
                close(
                    &text_embeds[row * EMBED..row * EMBED + 8],
                    &floats(head),
                    1e-5,
                    "text_embeds",
                );
            }
            let sums: Vec<f32> = text_embeds
                .chunks(EMBED)
                .map(|row| row.iter().sum())
                .collect();
            close(&sums, &floats(&conditioner["text_sums"]), 1e-4, "text sums");
            close(&bos[..32], &floats(&conditioner["bos_head"]), 1e-5, "bos");

            // FlowLM pas à pas, même bruit.
            let noise: Vec<Vec<f32>> = serde_json::from_value(reference["noise"].clone()).unwrap();
            let mut inputs = text_embeds;
            inputs.extend_from_slice(&bos);
            let mut rows = tokens.len() + 1;
            let mut kv = voice.kv.clone();
            let mut positions = voice.positions;
            let mut latents = Vec::new();
            let mut eos_step = None;
            let frames_after_eos = chunks[0].frames_after_eos;
            let steps = reference["steps"].as_array().unwrap();
            for step in 0..chunks[0].max_frames {
                let noise: [f32; LATENT] = noise[step].clone().try_into().unwrap();
                let output = flow.step(&inputs, rows, &kv, positions, &noise).unwrap();
                if let Some(expected) = steps.get(step) {
                    let what = format!("{language:?} step {step}");
                    close(
                        &output.mimi_latent,
                        &floats(&expected["mimi_latent"]),
                        2e-3,
                        &what,
                    );
                    let eos = expected["eos_logit"].as_f64().unwrap() as f32;
                    assert!(
                        (output.eos_logit - eos).abs() < 2e-2,
                        "{what}: {} / {eos}",
                        output.eos_logit
                    );
                    let last = &output.new_kv[(rows - 1) * KV_ROW..(rows - 1) * KV_ROW + 16];
                    close(last, &floats(&expected["new_kv_head"]), 2e-3, &what);
                    close(
                        &output.next_embeds[..16],
                        &floats(&expected["next_embeds_head"]),
                        2e-3,
                        &what,
                    );
                }
                kv.extend_from_slice(&output.new_kv);
                positions += rows;
                if output.eos_logit > EOS_THRESHOLD
                    && eos_step.is_none()
                    && step >= MIN_FRAMES_BEFORE_EOS
                {
                    eos_step = Some(step);
                }
                if eos_step.is_some_and(|eos| step >= eos + frames_after_eos) {
                    break;
                }
                inputs = output.next_embeds;
                rows = 1;
                latents.extend_from_slice(&output.mimi_latent);
            }
            let frames = latents.len() / LATENT;
            let expected_frames = reference["frames"].as_u64().unwrap() as usize;
            eprintln!(
                "{language:?}: {frames} frames (reference {expected_frames}), end at {eos_step:?} (reference {})",
                reference["eos_step"]
            );
            assert!(frames.abs_diff(expected_frames) <= 2, "{language:?}");

            // Mimi : premiers latents un par un, puis le reste d'un coup.
            let mut state = MimiState::new();
            let mut audio = Vec::new();
            let blocks = reference["mimi_blocks"].as_array().unwrap();
            for (index, block) in blocks.iter().enumerate() {
                let samples = mimi
                    .decode(&latents[index * LATENT..(index + 1) * LATENT], &mut state)
                    .unwrap();
                let what = format!("{language:?} mimi block {index}");
                close(&samples[..32], &floats(&block["head"]), 1e-3, &what);
                let rms =
                    (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt();
                let expected = block["rms"].as_f64().unwrap() as f32;
                assert!(
                    (rms - expected).abs() <= 1e-3 * (1.0 + expected),
                    "{what}: {rms} / {expected}"
                );
                audio.extend(samples);
            }
            audio.extend(
                mimi.decode(&latents[blocks.len() * LATENT..], &mut state)
                    .unwrap(),
            );
            assert_eq!(audio.len(), frames * SAMPLES_PER_FRAME);
            let rms = (audio.iter().map(|s| s * s).sum::<f32>() / audio.len() as f32).sqrt();
            let expected = reference["audio_rms"].as_f64().unwrap() as f32;
            eprintln!("{language:?}: RMS {rms} (reference {expected})");
            assert!((rms - expected).abs() < 0.1 * expected, "{language:?}");
        }
    }

    /// Mémoire résidente maximale du processus (Mo).
    fn peak_rss_mb() -> u64 {
        std::fs::read_to_string("/proc/self/status")
            .ok()
            .and_then(|status| {
                status
                    .lines()
                    .find(|line| line.starts_with("VmHWM:"))
                    .and_then(|line| line.split_whitespace().nth(1)?.parse::<u64>().ok())
            })
            .map_or(0, |kb| kb / 1024)
    }

    const TEXTS: [(VoiceLanguage, &str); 3] = [
        (
            VoiceLanguage::Fr,
            "Bonne question ! La lecture à voix haute fonctionne entièrement sur votre ordinateur. Elle découpe la réponse en phrases, puis les lit les unes après les autres, sans attendre la fin du texte.",
        ),
        (
            VoiceLanguage::En,
            "Good question! Reading aloud runs entirely on your computer. It splits the answer into sentences, then reads them one after another, without waiting for the end of the text.",
        ),
        (
            VoiceLanguage::Es,
            "¡Buena pregunta! La lectura en voz alta funciona por completo en tu ordenador. Divide la respuesta en frases y las lee una tras otra, sin esperar al final del texto.",
        ),
    ];

    /// Une phrase par langue, en vrai : durée et énergie plausibles, vitesse
    /// et premier son rapportés (lancer sous `taskset -c 0,1` pour 2 cœurs).
    #[test]
    #[ignore = "needs the voice models (CHATONLM_VOICE_MODELS_DIR)"]
    fn tts_contract_speaks_each_language() {
        let Some(root) = models() else { return };
        for (language, text) in TEXTS {
            let dir = root.join(format!("pocket-tts-{}", language.code()));
            let started = Instant::now();
            let mut synthesizer = Synthesizer::load(&dir, language).unwrap();
            let loaded = started.elapsed();
            // Préchauffage (premières exécutions d'ORT plus lentes).
            synthesizer
                .speak("Bonjour.", &AtomicBool::new(false), &mut |_| {})
                .unwrap();
            let tokens: usize = prompt::chunks(
                &synthesizer.tokenizer,
                text,
                &PromptConfig::for_language(language),
            )
            .iter()
            .map(|chunk| chunk.tokens.len())
            .sum();
            let stop = AtomicBool::new(false);
            let mut audio = Vec::new();
            let mut first = None;
            let started = Instant::now();
            synthesizer
                .speak(text, &stop, &mut |samples| {
                    first.get_or_insert_with(|| started.elapsed());
                    audio.extend_from_slice(samples);
                })
                .unwrap();
            let wall = started.elapsed().as_secs_f64();
            let seconds = audio.len() as f64 / f64::from(SAMPLE_RATE);
            let rms =
                (audio.iter().map(|s| f64::from(s * s)).sum::<f64>() / audio.len() as f64).sqrt();
            let peak = audio.iter().fold(0.0f32, |m, s| m.max(s.abs()));
            let expected = tokens as f64 * seconds_per_token(language);
            eprintln!(
                "{language:?}: {tokens} tokens, {seconds:.2} s of audio (expected {expected:.2} s) in {wall:.2} s = {:.1}x real time, first sound {:?}, load {loaded:?}, RMS {rms:.3}, peak {peak:.2}, peak RSS {} MB",
                seconds / wall,
                first.unwrap(),
                peak_rss_mb()
            );
            assert!(audio.iter().all(|s| s.is_finite()));
            assert!(
                seconds > 0.6 * expected && seconds < 2.0 * expected + 2.0,
                "{language:?}: {seconds} s"
            );
            assert!((0.02..0.3).contains(&rms), "{language:?}: RMS {rms}");
            assert!(peak < 1.5, "{language:?}: peak {peak}");
            // Plus vite que le temps réel, avec de la marge.
            assert!(seconds / wall > 2.0, "{language:?}: {:.2}x", seconds / wall);
        }
    }

    /// Arrêt en vrai : `speak` rend la main vite.
    #[test]
    #[ignore = "needs the voice models (CHATONLM_VOICE_MODELS_DIR)"]
    fn tts_contract_stops_quickly() {
        let Some(root) = models() else { return };
        let mut synthesizer =
            Synthesizer::load(&root.join("pocket-tts-en"), VoiceLanguage::En).unwrap();
        let stop = AtomicBool::new(false);
        let mut stopped_at = None;
        synthesizer
            .speak(TEXTS[1].1, &stop, &mut |_| {
                if stopped_at.is_none() {
                    stop.store(true, Ordering::Release);
                    stopped_at = Some(Instant::now());
                }
            })
            .unwrap();
        let delay = stopped_at.unwrap().elapsed();
        eprintln!("stop: speak returned {delay:?} after the stop");
        assert!(delay < Duration::from_millis(200), "{delay:?}");
    }
}
