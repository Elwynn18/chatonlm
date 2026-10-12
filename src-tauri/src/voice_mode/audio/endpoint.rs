//! Fin de tour : logique pure sur la probabilité de parole (Silero) de
//! chaque trame de 32 ms, après annulation d'écho. Sans ONNX ni fil.
//!
//! - Mains libres, gate `Listen` : début à p ≥ 0,5 pendant 3 trames
//!   consécutives (~96 ms), fin quand p < 0,35 pendant le silence réglé
//!   (entre les deux, la parole continue) ; parole de moins de 250 ms
//!   abandonnée (`Discarded`).
//! - Gate `Respond` (mains libres, interruption permise) : p ≥ 0,7 pendant
//!   8 trames (256 ms) → `BargeIn`, la gate passe en `Listen` et la parole
//!   en cours devient le tour suivant.
//! - Push-to-talk : seuls `hold(true)` et `hold(false)` bornent le tour ;
//!   relâché sans parole entendue (jamais 3 trames de suite à p ≥ 0,5) →
//!   `Discarded`.
//! - Tout tour garde 300 ms de son avant le début de la parole (pré-roll) ;
//!   en mains libres, le silence de fin est ramené à 300 ms au plus. Tour
//!   plafonné à 2 min (fin forcée).

use std::collections::VecDeque;

use super::{Gate, ListenConfig, ListenEvent, RATE, TurnMode};

/// Trame de la détection : 512 échantillons à 16 kHz (32 ms).
pub const FRAME: usize = 512;

/// Début de parole : p ≥ `START` pendant `START_FRAMES` trames de suite.
const START: f32 = 0.5;
const START_FRAMES: usize = 3;
/// La parole continue tant que p ≥ `KEEP` ; en dessous, c'est du silence.
const KEEP: f32 = 0.35;
/// Interruption : p ≥ `BARGE` pendant au moins 250 ms.
const BARGE: f32 = 0.7;
const BARGE_FRAMES: usize = samples_of(250).div_ceil(FRAME);

/// Son gardé avant le début de la parole.
const PREROLL: usize = samples_of(300);
/// Silence gardé après la parole (mains libres).
const POSTROLL: usize = samples_of(300);
/// Parole plus courte : abandonnée.
const MIN_SPEECH: usize = samples_of(250);
/// Tour plus long : fin forcée.
const MAX_TURN: usize = samples_of(120_000);
/// Son retenu en attente d'un début : le pré-roll et les trames qui
/// confirment le début (ou l'interruption).
const HISTORY: usize = PREROLL + FRAME * max(START_FRAMES, BARGE_FRAMES);

const fn samples_of(milliseconds: usize) -> usize {
    milliseconds * RATE as usize / 1000
}

const fn max(a: usize, b: usize) -> usize {
    if a > b { a } else { b }
}

/// Tour en cours.
#[derive(Debug)]
struct Turn {
    samples: Vec<f32>,
    /// Échantillons du pré-roll en tête de `samples`.
    preroll: usize,
    /// Échantillons depuis le début de la parole.
    length: usize,
    /// Fin de la dernière trame parlée (p ≥ `KEEP`), depuis le début.
    voiced: usize,
    /// Trames calmes de suite.
    quiet: usize,
    /// Trames de suite à p ≥ `START`.
    run: usize,
    /// Un début de parole a été entendu (push-to-talk).
    heard: bool,
}

impl Turn {
    /// Tour dont la parole a commencé `frames` trames plus tôt (les
    /// dernières de `history`), pré-roll compris.
    fn starting(history: &VecDeque<f32>, frames: usize) -> Self {
        let length = frames * FRAME;
        let kept = (PREROLL + length).min(history.len());
        let mut samples = Vec::with_capacity(kept + 30 * RATE as usize);
        samples.extend(history.range(history.len() - kept..));
        Self {
            preroll: kept.saturating_sub(length),
            samples,
            length,
            voiced: length,
            quiet: 0,
            run: frames,
            heard: frames >= START_FRAMES,
        }
    }

    fn extend(&mut self, frame: &[f32], p: f32) {
        self.samples.extend_from_slice(frame);
        self.length += frame.len();
        if p >= KEEP {
            self.voiced = self.length;
            self.quiet = 0;
        } else {
            self.quiet += 1;
        }
        if p >= START {
            self.run += 1;
            self.heard |= self.run >= START_FRAMES;
        } else {
            self.run = 0;
        }
    }

    /// Fin en mains libres : la parole, le pré-roll et 300 ms de silence au
    /// plus ; abandonné si la parole est trop courte.
    fn finish(mut self) -> ListenEvent {
        if self.voiced < MIN_SPEECH {
            return ListenEvent::Discarded;
        }
        let tail = (self.length - self.voiced).min(POSTROLL);
        self.samples.truncate(self.preroll + self.voiced + tail);
        ListenEvent::Utterance(self.samples)
    }

    /// Fin en push-to-talk : tout le son tenu, s'il contient de la parole.
    fn release(self) -> ListenEvent {
        if self.heard {
            ListenEvent::Utterance(self.samples)
        } else {
            ListenEvent::Discarded
        }
    }
}

#[derive(Debug)]
enum State {
    /// En attente : `run` trames de suite au-dessus du seuil de début (ou
    /// d'interruption).
    Waiting { run: usize },
    /// Parole en cours (mains libres).
    Speech(Turn),
    /// Bouton maintenu (push-to-talk).
    Held(Turn),
}

/// Fin de tour d'une écoute.
pub struct Endpoint {
    config: ListenConfig,
    gate: Gate,
    state: State,
    /// Derniers échantillons, pour le pré-roll.
    history: VecDeque<f32>,
    /// Passages en `Listen`, ouverture comprise.
    listens: u64,
}

impl Endpoint {
    /// Gate initiale : `Listen`.
    pub fn new(config: ListenConfig) -> Self {
        Self {
            config,
            gate: Gate::Listen,
            state: State::Waiting { run: 0 },
            history: VecDeque::with_capacity(HISTORY + FRAME),
            listens: 1,
        }
    }

    #[cfg(test)]
    pub fn gate(&self) -> Gate {
        self.gate
    }

    /// Passages en `Listen` depuis l'ouverture (1 à l'ouverture) : le faux
    /// micro des essais e2e s'y règle.
    pub fn listens(&self) -> u64 {
        self.listens
    }

    /// Trame de `FRAME` échantillons nettoyés et sa probabilité de parole.
    pub fn push(&mut self, frame: &[f32], p: f32, emit: &mut dyn FnMut(ListenEvent)) {
        debug_assert_eq!(frame.len(), FRAME);
        self.history.extend(frame);
        let excess = self.history.len().saturating_sub(HISTORY);
        self.history.drain(..excess);
        let hands_free = self.config.mode == TurnMode::HandsFree;
        let silence = self.silence_frames();
        match &mut self.state {
            State::Waiting { run } => match self.gate {
                Gate::Listen if hands_free => {
                    *run = if p >= START { *run + 1 } else { 0 };
                    if *run >= START_FRAMES {
                        self.state = State::Speech(Turn::starting(&self.history, *run));
                        emit(ListenEvent::SpeechStarted);
                    }
                }
                Gate::Respond if hands_free && self.config.barge_in => {
                    *run = if p >= BARGE { *run + 1 } else { 0 };
                    if *run >= BARGE_FRAMES {
                        self.state = State::Speech(Turn::starting(&self.history, *run));
                        self.gate = Gate::Listen;
                        self.listens += 1;
                        emit(ListenEvent::BargeIn);
                    }
                }
                _ => *run = 0,
            },
            State::Speech(turn) => {
                turn.extend(frame, p);
                if turn.quiet >= silence || turn.length >= MAX_TURN {
                    self.end(emit);
                }
            }
            State::Held(turn) => {
                turn.extend(frame, p);
                if turn.length >= MAX_TURN {
                    self.end(emit);
                }
            }
        }
    }

    /// Termine le tour en cours (événement de fin), retour à l'attente.
    fn end(&mut self, emit: &mut dyn FnMut(ListenEvent)) {
        match std::mem::replace(&mut self.state, State::Waiting { run: 0 }) {
            State::Speech(turn) => emit(turn.finish()),
            State::Held(turn) => emit(turn.release()),
            State::Waiting { .. } => {}
        }
    }

    /// Trames calmes qui terminent un tour (silence réglé, arrondi au-dessus).
    fn silence_frames(&self) -> usize {
        let samples = self.config.silence.as_millis() as usize * RATE as usize / 1000;
        samples.div_ceil(FRAME).max(1)
    }

    /// Changement de gate ; un tour en cours est abandonné sans événement.
    /// Sans effet si la gate ne change pas.
    pub fn set_gate(&mut self, gate: Gate) {
        if gate == self.gate {
            return;
        }
        self.gate = gate;
        if gate == Gate::Listen {
            self.listens += 1;
        }
        self.state = State::Waiting { run: 0 };
    }

    /// « J'ai fini » : fin du tour si une parole est en cours (mains
    /// libres).
    pub fn end_turn(&mut self, emit: &mut dyn FnMut(ListenEvent)) {
        if matches!(self.state, State::Speech(_)) {
            self.end(emit);
        }
    }

    /// Push-to-talk (ignoré en mains libres) : l'appui commence un tour
    /// (gate passée en `Listen`), le relâchement le termine.
    pub fn hold(&mut self, pressed: bool, emit: &mut dyn FnMut(ListenEvent)) {
        if self.config.mode != TurnMode::PushToTalk {
            return;
        }
        match (pressed, &self.state) {
            (true, State::Held(_)) | (false, State::Waiting { .. } | State::Speech(_)) => {}
            (true, _) => {
                self.set_gate(Gate::Listen);
                self.state = State::Held(Turn::starting(&self.history, 0));
                emit(ListenEvent::SpeechStarted);
            }
            (false, State::Held(_)) => self.end(emit),
        }
    }

    /// Son entendu après la dernière trame (moins d'une trame), ajouté au
    /// tour en cours avant qu'il ne se termine sur demande ; faux sans tour.
    pub fn extend(&mut self, samples: &[f32]) -> bool {
        match &mut self.state {
            State::Speech(turn) | State::Held(turn) => {
                turn.samples.extend_from_slice(samples);
                turn.length += samples.len();
                true
            }
            State::Waiting { .. } => false,
        }
    }

    /// Réglages changés : un changement de mode abandonne le tour en cours ;
    /// le silence et l'interruption valent aussitôt.
    pub fn configure(&mut self, config: ListenConfig) {
        if config.mode != self.config.mode {
            self.state = State::Waiting { run: 0 };
        }
        self.config = config;
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    fn config(mode: TurnMode, silence_ms: u64, barge_in: bool) -> ListenConfig {
        ListenConfig {
            mode,
            silence: Duration::from_millis(silence_ms),
            barge_in,
        }
    }

    fn hands_free() -> ListenConfig {
        config(TurnMode::HandsFree, 1_000, true)
    }

    /// Fin de tour pilotée par des probabilités ; chaque trame porte son
    /// numéro (valeur de ses échantillons) pour suivre le son rendu.
    struct Bench {
        endpoint: Endpoint,
        events: Vec<ListenEvent>,
        frames: usize,
    }

    impl Bench {
        fn new(config: ListenConfig) -> Self {
            Self {
                endpoint: Endpoint::new(config),
                events: Vec::new(),
                frames: 0,
            }
        }

        /// Trames de probabilité `p`.
        fn feed(&mut self, p: f32, count: usize) -> &mut Self {
            for _ in 0..count {
                self.frames += 1;
                let frame = vec![self.frames as f32; FRAME];
                let events = &mut self.events;
                self.endpoint
                    .push(&frame, p, &mut |event| events.push(event));
            }
            self
        }

        fn hold(&mut self, pressed: bool) -> &mut Self {
            let events = &mut self.events;
            self.endpoint.hold(pressed, &mut |event| events.push(event));
            self
        }

        fn end_turn(&mut self) -> &mut Self {
            let events = &mut self.events;
            self.endpoint.end_turn(&mut |event| events.push(event));
            self
        }

        fn take(&mut self) -> Vec<ListenEvent> {
            std::mem::take(&mut self.events)
        }

        /// Événements sans le son ; le son de l'unique `Utterance` en
        /// numéros de trames (premier, dernier) et en échantillons.
        fn summary(&mut self) -> (Vec<&'static str>, Option<(f32, f32, usize)>) {
            let mut audio = None;
            let names = self
                .take()
                .into_iter()
                .map(|event| match event {
                    ListenEvent::SpeechStarted => "started",
                    ListenEvent::BargeIn => "barge-in",
                    ListenEvent::Discarded => "discarded",
                    ListenEvent::Utterance(samples) => {
                        audio = Some((samples[0], *samples.last().unwrap(), samples.len()));
                        "utterance"
                    }
                    ListenEvent::Levels { .. } => "levels",
                    ListenEvent::Failed(_) => "failed",
                })
                .collect();
            (names, audio)
        }
    }

    #[test]
    fn silence_and_doubtful_frames_start_nothing() {
        let mut bench = Bench::new(hands_free());
        bench.feed(0.0, 100).feed(0.49, 50);
        // Deux trames franches, puis une sous le seuil : pas de début.
        bench.feed(0.9, 2).feed(0.4, 1).feed(0.9, 2).feed(0.0, 40);
        assert!(bench.take().is_empty());
    }

    #[test]
    fn speech_starts_after_three_frames_at_one_half() {
        let mut bench = Bench::new(hands_free());
        bench.feed(0.0, 20).feed(0.5, 2);
        assert!(bench.take().is_empty());
        bench.feed(0.5, 1);
        assert_eq!(bench.take(), [ListenEvent::SpeechStarted]);
    }

    /// Fin quand p < 0,35 pendant le silence réglé (1 s = 32 trames) ; entre
    /// 0,35 et 0,5, la parole continue ; une trame parlée remet le compte à
    /// zéro.
    #[test]
    fn the_turn_ends_after_the_configured_silence() {
        let mut bench = Bench::new(hands_free());
        bench.feed(0.0, 20).feed(0.9, 20);
        bench
            .feed(0.4, 60)
            .feed(0.34, 31)
            .feed(0.36, 1)
            .feed(0.0, 31);
        assert_eq!(bench.summary().0, ["started"]);
        bench.feed(0.0, 1);
        assert_eq!(bench.summary().0, ["utterance"]);
        bench.feed(0.0, 100);
        assert!(bench.take().is_empty());
    }

    #[test]
    fn each_pause_setting_has_its_silence() {
        for (silence, frames) in [(600, 19), (1_000, 32), (1_600, 50)] {
            let mut bench = Bench::new(config(TurnMode::HandsFree, silence, true));
            bench.feed(0.9, 20).feed(0.1, frames - 1);
            assert_eq!(bench.summary().0, ["started"], "{silence} ms");
            bench.feed(0.1, 1);
            assert_eq!(bench.summary().0, ["utterance"], "{silence} ms");
        }
    }

    /// Le son rendu commence 300 ms (9,4 trames) avant la première trame
    /// parlée et finit 300 ms après la dernière.
    #[test]
    fn the_utterance_keeps_300_ms_before_and_after_the_speech() {
        let mut bench = Bench::new(hands_free());
        // Trames 1 à 20 : silence ; 21 à 40 : parole ; puis silence.
        bench.feed(0.0, 20).feed(0.9, 20).feed(0.0, 32);
        let (events, audio) = bench.summary();
        assert_eq!(events, ["started", "utterance"]);
        let (first, last, length) = audio.unwrap();
        // 4 800 échantillons avant la trame 21 : fin de la trame 11
        // (4 800 = 9 × 512 + 192).
        assert_eq!(first, 11.0);
        assert_eq!(length, PREROLL + 20 * FRAME + POSTROLL);
        // 300 ms après la trame 40 : dans la trame 50.
        assert_eq!(last, 50.0);
    }

    /// Au tout début de l'écoute, le pré-roll est ce qui a été entendu.
    #[test]
    fn an_early_start_keeps_what_there_is() {
        let mut bench = Bench::new(hands_free());
        bench.feed(0.0, 2).feed(0.9, 10).feed(0.0, 32);
        let (_, audio) = bench.summary();
        let (first, _, length) = audio.unwrap();
        assert_eq!(first, 1.0);
        assert_eq!(length, 12 * FRAME + POSTROLL);
    }

    /// Moins de 250 ms de parole (7 trames, 224 ms) : abandonnée ; 8 trames
    /// (256 ms) : gardée.
    #[test]
    fn speech_shorter_than_250_ms_is_discarded() {
        let mut bench = Bench::new(hands_free());
        bench.feed(0.9, 7).feed(0.0, 32);
        assert_eq!(bench.summary().0, ["started", "discarded"]);
        bench.feed(0.9, 8).feed(0.0, 32);
        assert_eq!(bench.summary().0, ["started", "utterance"]);
        // Les trames entre 0,35 et 0,5 comptent comme parole.
        bench.feed(0.9, 3).feed(0.4, 5).feed(0.0, 32);
        assert_eq!(bench.summary().0, ["started", "utterance"]);
    }

    #[test]
    fn a_two_minute_turn_is_cut() {
        let mut bench = Bench::new(hands_free());
        let frames = MAX_TURN.div_ceil(FRAME);
        bench.feed(0.9, 3).feed(0.9, frames - 4);
        assert_eq!(bench.summary().0, ["started"]);
        bench.feed(0.9, 1);
        let (events, audio) = bench.summary();
        assert_eq!(events, ["utterance"]);
        assert!(audio.unwrap().2 >= MAX_TURN);
        // La parole qui continue fait un nouveau tour.
        bench.feed(0.9, 3);
        assert_eq!(bench.summary().0, ["started"]);
    }

    #[test]
    fn end_turn_finishes_speech_at_once() {
        let mut bench = Bench::new(hands_free());
        bench.end_turn().feed(0.9, 2).end_turn();
        assert!(bench.take().is_empty());
        bench.feed(0.9, 20).end_turn();
        let (events, audio) = bench.summary();
        assert_eq!(events, ["started", "utterance"]);
        assert_eq!(audio.unwrap().1, 22.0);
        // Trop court : abandonné.
        bench.feed(0.9, 4).end_turn();
        assert_eq!(bench.summary().0, ["started", "discarded"]);
        // Sans effet en push-to-talk.
        let mut held = Bench::new(config(TurnMode::PushToTalk, 1_000, true));
        held.hold(true).feed(0.9, 20).end_turn();
        assert_eq!(held.summary().0, ["started"]);
    }

    /// Push-to-talk : le bouton borne le tour, la détection ne le termine
    /// pas ; le pré-roll est gardé.
    #[test]
    fn push_to_talk_follows_the_button() {
        let mut bench = Bench::new(config(TurnMode::PushToTalk, 600, true));
        // Sans appui, la parole ne commence rien.
        bench.feed(0.9, 30).feed(0.0, 40);
        assert!(bench.take().is_empty());
        bench.hold(true).hold(true);
        assert_eq!(bench.summary().0, ["started"]);
        bench.feed(0.0, 5).feed(0.9, 10).feed(0.0, 100);
        assert!(bench.take().is_empty());
        bench.hold(false).hold(false);
        let (events, audio) = bench.summary();
        assert_eq!(events, ["utterance"]);
        let (first, last, length) = audio.unwrap();
        // 300 ms avant l'appui (après la trame 70), tout jusqu'au relâché.
        assert_eq!((first, last), (61.0, 185.0));
        assert_eq!(length, PREROLL + 115 * FRAME);
    }

    #[test]
    fn a_release_without_speech_is_discarded() {
        let mut bench = Bench::new(config(TurnMode::PushToTalk, 1_000, true));
        bench
            .hold(true)
            .feed(0.0, 30)
            .feed(0.9, 2)
            .feed(0.1, 1)
            .feed(0.9, 2);
        bench.hold(false);
        assert_eq!(bench.summary().0, ["started", "discarded"]);
        bench.hold(true).feed(0.9, 3).hold(false);
        assert_eq!(bench.summary().0, ["started", "utterance"]);
    }

    #[test]
    fn a_held_turn_is_cut_at_two_minutes() {
        let mut bench = Bench::new(config(TurnMode::PushToTalk, 1_000, true));
        bench.hold(true).feed(0.9, MAX_TURN.div_ceil(FRAME));
        assert_eq!(bench.summary().0, ["started", "utterance"]);
        // Relâché ensuite : rien de plus.
        bench.feed(0.9, 10).hold(false);
        assert!(bench.take().is_empty());
    }

    #[test]
    fn hold_is_ignored_hands_free() {
        let mut bench = Bench::new(hands_free());
        bench.hold(true).feed(0.0, 10).hold(false);
        assert!(bench.take().is_empty());
    }

    /// Appui pendant la réponse : la gate repasse en `Listen`.
    #[test]
    fn a_press_while_responding_listens() {
        let mut bench = Bench::new(config(TurnMode::PushToTalk, 1_000, true));
        bench.endpoint.set_gate(Gate::Respond);
        bench.hold(true);
        assert_eq!(bench.endpoint.gate(), Gate::Listen);
        bench.feed(0.9, 10).hold(false);
        assert_eq!(bench.summary().0, ["started", "utterance"]);
    }

    /// Pendant la réponse, seule une parole franche (p ≥ 0,7 pendant
    /// 256 ms) interrompt ; elle devient le tour suivant, pré-roll compris.
    #[test]
    fn strong_speech_barges_in_and_becomes_the_next_turn() {
        let mut bench = Bench::new(hands_free());
        bench.endpoint.set_gate(Gate::Respond);
        bench.feed(0.0, 20).feed(0.69, 20).feed(0.9, 7).feed(0.5, 1);
        bench.feed(0.9, 7);
        assert!(bench.take().is_empty());
        assert_eq!(bench.endpoint.gate(), Gate::Respond);
        // Trames 49 à 56 : 8 trames franches.
        bench.feed(0.9, 1);
        assert_eq!(bench.summary().0, ["barge-in"]);
        assert_eq!(bench.endpoint.gate(), Gate::Listen);
        bench.feed(0.9, 10).feed(0.0, 32);
        let (events, audio) = bench.summary();
        assert_eq!(events, ["utterance"]);
        let (first, _, length) = audio.unwrap();
        // 300 ms avant la trame 49 : fin de la trame 39.
        assert_eq!(first, 39.0);
        assert_eq!(length, PREROLL + 18 * FRAME + POSTROLL);
    }

    #[test]
    fn no_barge_in_when_disabled_or_push_to_talk() {
        for config in [
            config(TurnMode::HandsFree, 1_000, false),
            config(TurnMode::PushToTalk, 1_000, true),
        ] {
            let mut bench = Bench::new(config);
            bench.endpoint.set_gate(Gate::Respond);
            bench.feed(1.0, 100);
            assert!(bench.take().is_empty(), "{config:?}");
            assert_eq!(bench.endpoint.gate(), Gate::Respond);
        }
    }

    /// `Ignore` : rien, même une parole franche ; de retour en `Listen`, la
    /// parole est détectée de nouveau (comptes remis à zéro).
    #[test]
    fn ignore_emits_nothing() {
        let mut bench = Bench::new(hands_free());
        bench.endpoint.set_gate(Gate::Ignore);
        bench.feed(1.0, 200);
        assert!(bench.take().is_empty());
        bench.endpoint.set_gate(Gate::Listen);
        bench.feed(1.0, 2);
        assert!(bench.take().is_empty());
        bench.feed(1.0, 1);
        assert_eq!(bench.summary().0, ["started"]);
    }

    /// Un tour en cours est abandonné sans événement si la gate change ;
    /// la même gate ne change rien.
    #[test]
    fn gate_changes_drop_the_turn_but_not_repeats() {
        let mut bench = Bench::new(hands_free());
        bench.feed(0.9, 10);
        bench.endpoint.set_gate(Gate::Listen);
        bench.feed(0.0, 32);
        assert_eq!(bench.summary().0, ["started", "utterance"]);
        bench.feed(0.9, 10);
        bench.endpoint.set_gate(Gate::Ignore);
        bench.endpoint.set_gate(Gate::Listen);
        bench.feed(0.0, 40);
        assert_eq!(bench.summary().0, ["started"]);
    }

    #[test]
    fn listens_count_every_return_to_listen() {
        let mut bench = Bench::new(hands_free());
        assert_eq!(bench.endpoint.listens(), 1);
        bench.endpoint.set_gate(Gate::Listen);
        bench.endpoint.set_gate(Gate::Ignore);
        bench.endpoint.set_gate(Gate::Respond);
        assert_eq!(bench.endpoint.listens(), 1);
        bench.feed(0.9, 8);
        assert_eq!(bench.endpoint.listens(), 2);
        bench.endpoint.set_gate(Gate::Ignore);
        bench.endpoint.set_gate(Gate::Listen);
        assert_eq!(bench.endpoint.listens(), 3);
    }

    #[test]
    fn settings_apply_during_a_session() {
        let mut bench = Bench::new(hands_free());
        bench.feed(0.9, 10).feed(0.0, 19);
        bench
            .endpoint
            .configure(config(TurnMode::HandsFree, 600, true));
        bench.feed(0.0, 1);
        assert_eq!(bench.summary().0, ["started", "utterance"]);
        // Changement de mode : tour abandonné.
        bench.feed(0.9, 10);
        bench
            .endpoint
            .configure(config(TurnMode::PushToTalk, 600, true));
        bench.feed(0.0, 40).feed(0.9, 10);
        assert_eq!(bench.summary().0, ["started"]);
        // Interruption coupée en cours de séance.
        let mut bench = Bench::new(hands_free());
        bench.endpoint.set_gate(Gate::Respond);
        bench
            .endpoint
            .configure(config(TurnMode::HandsFree, 1_000, false));
        bench.feed(1.0, 20);
        assert!(bench.take().is_empty());
    }
}
