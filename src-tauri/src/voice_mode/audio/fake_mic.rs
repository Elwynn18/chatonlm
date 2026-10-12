//! Faux micro des essais e2e (`CHATONLM_E2E_VOICE_MIC`) : un souffle très
//! faible (-70 dBFS) au rythme réel, et des WAV joués dans l'ordre d'un
//! script JSON `[{"on": "listening" | "speaking", "wav": "<chemin>",
//! "delayMs": 300}, …]`, ou `none` (pas de micro). Chaque WAV part
//! `delayMs` après son signal : l'écoute passe en `Listen` (`listening` ;
//! l'ouverture compte), ou la sortie joue un son non nul (`speaking`) ;
//! le signal est attendu une fois le WAV précédent fini. À la fin de chaque
//! WAV : `chatonlm-voice-e2e: wav <n> ended` sur stderr (n : rang dans le
//! script, à partir de 0), pour mesurer la latence.
//!
//! Le script est partagé par les écoutes successives d'un même lancement
//! (micro coupé puis rouvert…) : il reprend où il en était ; un WAV coupé
//! par la fermeture de l'écoute est perdu.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use serde::Deserialize;

use super::RATE;
use super::listener::{Cues, Microphone};
use super::noise::Noise;
use crate::dictation::audio::{could_not_open, no_microphone, read_wav, resample};

/// Variable du build e2e.
pub const ENV: &str = "CHATONLM_E2E_VOICE_MIC";

/// Souffle de fond : RMS à -70 dBFS (uniforme d'amplitude RMS × √3).
const HISS: f32 = 3.162e-4 * 1.732;

/// Signal d'un WAV.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Trigger {
    Listening,
    Speaking,
}

/// Entrée du script.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Cue {
    pub on: Trigger,
    pub wav: PathBuf,
    #[serde(default)]
    pub delay_ms: u64,
}

/// Script lu dans la variable : `None` = pas de micro.
pub fn parse(value: &str) -> Result<Option<Vec<Cue>>, String> {
    if value.trim() == "none" {
        return Ok(None);
    }
    serde_json::from_str(value)
        .map(Some)
        .map_err(|error| could_not_open(&format!("{ENV}: {error}")))
}

/// Script et position, partagés par les écoutes.
#[derive(Debug)]
pub struct Script {
    cues: Vec<Cue>,
    next: usize,
}

impl Script {
    pub fn new(cues: Vec<Cue>) -> Arc<Mutex<Self>> {
        Arc::new(Mutex::new(Self { cues, next: 0 }))
    }
}

/// Ouvre le faux micro d'après la valeur de la variable.
pub fn open(
    value: &str,
    script: &Mutex<Option<Arc<Mutex<Script>>>>,
) -> Result<Box<dyn Microphone>, String> {
    let mut shared = script.lock();
    let script = match &*shared {
        Some(script) => script.clone(),
        None => {
            let Some(cues) = parse(value)? else {
                return Err(no_microphone());
            };
            for cue in &cues {
                if !cue.wav.is_file() {
                    return Err(could_not_open(&cue.wav.display()));
                }
            }
            shared.insert(Script::new(cues)).clone()
        }
    };
    Ok(Box::new(FakeMicrophone::new(script, Instant::now())))
}

/// WAV en cours.
struct Playing {
    samples: Vec<f32>,
    position: usize,
    index: usize,
}

pub struct FakeMicrophone {
    script: Arc<Mutex<Script>>,
    start: Instant,
    produced: u64,
    noise: Noise,
    playing: Option<Playing>,
    /// Passages en `Listen` vus à la fin du WAV précédent (0 : aucun, la
    /// gate initiale compte).
    armed_at: u64,
    /// Départ du prochain WAV, une fois son signal reçu.
    starts_at: Option<Instant>,
    /// WAV finis (essais).
    #[cfg(test)]
    ended: Vec<usize>,
}

impl FakeMicrophone {
    fn new(script: Arc<Mutex<Script>>, start: Instant) -> Self {
        Self {
            script,
            start,
            produced: 0,
            noise: Noise::new(70),
            playing: None,
            armed_at: 0,
            starts_at: None,
            #[cfg(test)]
            ended: Vec::new(),
        }
    }

    /// Ce qui est dû à `now` (rythme réel).
    fn read_at(&mut self, now: Instant, out: &mut Vec<f32>, cues: &Cues) -> Result<(), String> {
        if self.playing.is_none() {
            self.watch(now, cues)?;
        }
        let due = (now.duration_since(self.start).as_secs_f64() * f64::from(RATE)) as u64;
        for _ in self.produced..due {
            let mut sample = self.noise.next() * HISS;
            if let Some(playing) = &mut self.playing {
                sample += playing.samples[playing.position];
                playing.position += 1;
                if playing.position == playing.samples.len() {
                    eprintln!("chatonlm-voice-e2e: wav {} ended", playing.index);
                    #[cfg(test)]
                    self.ended.push(playing.index);
                    self.playing = None;
                    self.armed_at = cues.listens;
                }
            }
            out.push(sample);
        }
        self.produced = self.produced.max(due);
        Ok(())
    }

    /// Attend le signal du WAV suivant, puis son délai.
    fn watch(&mut self, now: Instant, cues: &Cues) -> Result<(), String> {
        let mut script = self.script.lock();
        let Some(cue) = script.cues.get(script.next).cloned() else {
            return Ok(());
        };
        if self.starts_at.is_none() {
            let fired = match cue.on {
                Trigger::Listening => cues.listens > self.armed_at,
                Trigger::Speaking => cues.output_sound,
            };
            if fired {
                self.starts_at = Some(now + Duration::from_millis(cue.delay_ms));
            }
        }
        if self.starts_at.is_some_and(|at| now >= at) {
            self.starts_at = None;
            let index = script.next;
            script.next += 1;
            let samples = load(&cue.wav)?;
            if !samples.is_empty() {
                self.playing = Some(Playing {
                    samples,
                    position: 0,
                    index,
                });
            }
        }
        Ok(())
    }
}

/// WAV à 16 kHz mono.
fn load(path: &Path) -> Result<Vec<f32>, String> {
    let captured = read_wav(path)?;
    Ok(resample(&captured.samples, captured.rate, RATE))
}

impl Microphone for FakeMicrophone {
    fn rate(&self) -> u32 {
        RATE
    }

    fn read(&mut self, out: &mut Vec<f32>, cues: &Cues) -> Result<(), String> {
        self.read_at(Instant::now(), out, cues)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wav(dir: &Path, name: &str, samples: &[f32]) -> PathBuf {
        let path = dir.join(name);
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: RATE,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        };
        let mut writer = hound::WavWriter::create(&path, spec).unwrap();
        for sample in samples {
            writer.write_sample(*sample).unwrap();
        }
        writer.finalize().unwrap();
        path
    }

    fn cues(listens: u64, output_sound: bool) -> Cues {
        Cues {
            listens,
            output_sound,
        }
    }

    #[test]
    fn the_script_is_read_from_json_or_none() {
        assert_eq!(parse("none"), Ok(None));
        let script = parse(
            r#"[{"on":"listening","wav":"/a.wav","delayMs":300},{"on":"speaking","wav":"/b.wav"}]"#,
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            script,
            [
                Cue {
                    on: Trigger::Listening,
                    wav: "/a.wav".into(),
                    delay_ms: 300
                },
                Cue {
                    on: Trigger::Speaking,
                    wav: "/b.wav".into(),
                    delay_ms: 0
                }
            ]
        );
        assert!(parse(r#"[{"on":"later","wav":"/a.wav"}]"#).is_err());
        let cached = Mutex::new(None);
        assert!(open("none", &cached).is_err());
        assert!(open(r#"[{"on":"listening","wav":"/missing.wav"}]"#, &cached).is_err());
    }

    /// Souffle à -70 dBFS au rythme réel ; WAV au signal, après le délai,
    /// puis le suivant au signal suivant.
    #[test]
    fn wavs_play_on_their_cues_in_real_time() {
        let dir = tempfile::tempdir().unwrap();
        let first = wav(dir.path(), "first.wav", &[0.5; 1_600]);
        let second = wav(dir.path(), "second.wav", &[-0.5; 800]);
        let script = Script::new(vec![
            Cue {
                on: Trigger::Listening,
                wav: first,
                delay_ms: 100,
            },
            Cue {
                on: Trigger::Speaking,
                wav: second,
                delay_ms: 0,
            },
        ]);
        let start = Instant::now();
        let mut mic = FakeMicrophone::new(script.clone(), start);
        let at = |ms: u64| start + Duration::from_millis(ms);
        let mut out = Vec::new();
        // Gate initiale `Listen` : signal du premier WAV, joué 100 ms après.
        mic.read_at(at(50), &mut out, &cues(1, false)).unwrap();
        assert_eq!(out.len(), 800);
        let rms = (out.iter().map(|s| s * s).sum::<f32>() / out.len() as f32).sqrt();
        let dbfs = 20.0 * rms.log10();
        assert!((-71.5..-68.5).contains(&dbfs), "{dbfs}");
        mic.read_at(at(149), &mut out, &cues(1, false)).unwrap();
        assert!(out.iter().all(|s| s.abs() < 0.01));
        mic.read_at(at(150), &mut out, &cues(1, false)).unwrap();
        mic.read_at(at(250), &mut out, &cues(1, false)).unwrap();
        assert_eq!(out.len(), 4_000);
        // Parti avec la lecture de 150 ms (échantillon 2 384).
        assert!(out[2_384..3_984].iter().all(|s| (s - 0.5).abs() < 0.01));
        assert!(out[3_984..].iter().all(|s| s.abs() < 0.01));
        assert_eq!(mic.ended, [0]);
        // Le second attend que la sortie joue.
        mic.read_at(at(400), &mut out, &cues(2, false)).unwrap();
        assert!(out[4_000..].iter().all(|s| s.abs() < 0.01));
        mic.read_at(at(410), &mut out, &cues(2, true)).unwrap();
        mic.read_at(at(500), &mut out, &cues(2, false)).unwrap();
        assert_eq!(mic.ended, [0, 1]);
        assert_eq!(out.len(), 8_000);
        assert!(out[6_400..7_200].iter().all(|s| (s + 0.5).abs() < 0.01));
        // Fin du script : le souffle seul.
        mic.read_at(at(1_000), &mut out, &cues(3, true)).unwrap();
        assert!(out[8_000..].iter().all(|s| s.abs() < 0.01));
    }

    /// `listening` après un WAV : seulement au passage suivant en `Listen`.
    /// Une nouvelle écoute reprend le script là où il en est.
    #[test]
    fn a_listening_cue_waits_for_the_next_turn() {
        let dir = tempfile::tempdir().unwrap();
        let path = wav(dir.path(), "word.wav", &[0.5; 160]);
        let cue = Cue {
            on: Trigger::Listening,
            wav: path,
            delay_ms: 0,
        };
        let script = Script::new(vec![cue.clone(), cue.clone(), cue]);
        let start = Instant::now();
        let at = |ms: u64| start + Duration::from_millis(ms);
        let mut mic = FakeMicrophone::new(script.clone(), start);
        let mut out = Vec::new();
        mic.read_at(at(10), &mut out, &cues(1, false)).unwrap();
        mic.read_at(at(30), &mut out, &cues(1, false)).unwrap();
        assert_eq!(mic.ended, [0]);
        mic.read_at(at(100), &mut out, &cues(1, false)).unwrap();
        mic.read_at(at(200), &mut out, &cues(1, false)).unwrap();
        assert_eq!(mic.ended, [0]);
        mic.read_at(at(210), &mut out, &cues(2, false)).unwrap();
        mic.read_at(at(300), &mut out, &cues(2, false)).unwrap();
        assert_eq!(mic.ended, [0, 1]);
        // Écoute rouverte : l'ouverture compte comme un passage en Listen.
        let mut reopened = FakeMicrophone::new(script, start);
        reopened.read_at(at(10), &mut out, &cues(1, false)).unwrap();
        reopened.read_at(at(50), &mut out, &cues(1, false)).unwrap();
        assert_eq!(reopened.ended, [2]);
    }
}
