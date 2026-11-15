//! Essais de la synthèse : découpage, budget, nettoyage (purs) ; synthèse
//! complète contre le faux moteur (`FakeBench`, scénarios
//! `chatonlm-synthesis*` de `fake-llama`) ; en contrat, contre le vrai
//! `llama-server` (`RealBench`).

use std::sync::Arc;
use std::time::Duration;

use fake_llama::scenarios::synthesis::{
    DOCUMENT_MARK, PART_MARK, RETRY_MARK, SHORT_LIMIT, STRAY_DOCUMENT, SYNTHESIS_THINKING,
};
use serde_json::Value;

use super::*;
use crate::inference::InferenceError;
use crate::llm::testing::{FakeBench, RealBench, Role};
use crate::store::settings::SettingsPatch;
use crate::store::transcriptions::{NewTranscription, insert, set_language, set_segments};
use crate::transcription::SourceKind;
use crate::transcription::paragraphs::Segment;

const SYNTH: &str = "test/Synth-GGUF:Q4_K_M";
const OTHER: &str = "test/Other-GGUF:Q4_K_M";
const SLOW: &str = "test/Slow-GGUF:Q4_K_M";
const THINKER: &str = "test/Thinker-GGUF:Q4_K_M";
const STRAY: &str = "test/Stray-GGUF:Q4_K_M";
const EMPTY: &str = "test/Empty-GGUF:Q4_K_M";
const CUT: &str = "test/Cut-GGUF:Q4_K_M";
const SHORT: &str = "test/Short-GGUF:Q4_K_M";
const LENGTH: &str = "test/Length-GGUF:Q4_K_M";
const HOLE: &str = "test/Hole-GGUF:Q4_K_M";

fn bench() -> FakeBench {
    FakeBench::new(&[
        (SYNTH, "synthesis"),
        (OTHER, "synthesis"),
        (SLOW, "synthesis-slow"),
        (THINKER, "synthesis-think"),
        (STRAY, "synthesis-stray"),
        (EMPTY, "synthesis-empty"),
        (CUT, "synthesis-cut"),
        (SHORT, "synthesis-short"),
        (LENGTH, "synthesis-length"),
        (HOLE, "synthesis-hole"),
    ])
}

/// États envoyés, dans l'ordre.
#[derive(Clone, Default)]
struct Sent(Arc<Mutex<Vec<SynthesisStatus>>>);

impl Sent {
    fn all(&self) -> Vec<SynthesisStatus> {
        self.0.lock().clone()
    }
}

/// Synthèses dont chaque morceau part vers la page (aucun écart imposé).
fn synthesizer() -> (Synthesizer, Sent) {
    let sent = Sent::default();
    let collected = sent.clone();
    let synthesizer = Synthesizer::with_timing(
        move |status| collected.0.lock().push(status.clone()),
        Timing {
            emit_gap: Duration::ZERO,
            stall: Duration::from_secs(20),
        },
    );
    (synthesizer, sent)
}

/// Transcription enregistrée : un paragraphe par texte (passages séparés
/// d'une longue pause).
fn add(bench: &FakeBench, id: &str, language: &str, paragraphs: &[String]) {
    let segments: Vec<Segment> = paragraphs
        .iter()
        .enumerate()
        .map(|(index, text)| Segment {
            start: index as f64 * 30.0,
            end: index as f64 * 30.0 + 10.0,
            text: text.clone(),
        })
        .collect();
    let (id, language) = (id.to_owned(), language.to_owned());
    bench
        .store()
        .blocking(move |c| {
            insert(
                c,
                &NewTranscription {
                    id: id.clone(),
                    source: SourceKind::File,
                    origin: "/home/me/cours.mp3".into(),
                    title: "Cours".into(),
                    engine: crate::dictation::models::SpeechEngine::Whisper,
                    model: "whisper-base".into(),
                    language: String::new(),
                },
            )?;
            if !language.is_empty() {
                set_language(c, &id, &language)?;
            }
            set_segments(c, &id, &segments)
        })
        .unwrap();
}

/// Paragraphe anglais d'environ 900 caractères, commencé par son numéro.
fn paragraph(index: usize) -> String {
    let mut text = format!("Paragraph {index} opens the topic number {index}.");
    for sentence in 0..10 {
        text.push_str(&format!(
            " The speaker explains the idea {index}.{sentence} with a few plain words about it."
        ));
    }
    text
}

fn summary_of(bench: &FakeBench, id: &str) -> Option<crate::transcription::Synthesis> {
    let id = id.to_owned();
    bench
        .store()
        .blocking(move |c| transcriptions::get(c, &id))
        .unwrap()
        .unwrap()
        .summary
}

/// Requêtes de chat reçues par le processus de `id`.
async fn chat_requests(bench: &FakeBench, id: &str) -> Vec<Value> {
    let session = bench
        .access
        .open(
            id,
            OpenOptions::default(),
            &CancellationToken::new(),
            &|| {},
        )
        .await
        .unwrap();
    let url = format!("{}/fake/requests", session.endpoint.base_url);
    let all: Vec<Value> = reqwest::Client::builder()
        .no_proxy()
        .build()
        .unwrap()
        .get(url)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    all.into_iter()
        .filter(|request| request["path"] == "/v1/chat/completions")
        .map(|request| request["body"].clone())
        .collect()
}

fn message(request: &Value, role: &str) -> String {
    request["messages"]
        .as_array()
        .unwrap()
        .iter()
        .rev()
        .find(|message| message["role"] == role)
        .and_then(|message| message["content"].as_str())
        .unwrap_or_default()
        .to_owned()
}

/// Fin de la synthèse en cours ; son dernier état.
async fn finished(synthesizer: &Synthesizer) -> SynthesisStatus {
    tokio::time::timeout(Duration::from_secs(60), synthesizer.wait())
        .await
        .expect("synthesis finished");
    synthesizer.status().unwrap()
}

#[test]
fn the_budget_follows_the_context_and_the_tokenizer() {
    let prompt = 1_200;
    let small = budget(4_096, 3.0, prompt).unwrap();
    assert_eq!(small.notes_tokens, 682);
    assert_eq!(small.document_tokens, 1_365);
    assert_eq!(small.part_chars, MAX_PART_CHARS);
    // 4 096 − 464 − 1 365 jetons, 3 caractères chacun.
    assert_eq!(small.source_chars, (4_096 - 464 - 1_365) * 3);
    let large = budget(32_768, 3.0, prompt).unwrap();
    assert_eq!(large.source_chars, MAX_SOURCE_CHARS);
    assert_eq!(large.document_tokens, 4_096);
    // Un texte qui prend plus de jetons (un caractère par jeton) : parties
    // plus courtes.
    let dense = budget(4_096, 1.0, prompt).unwrap();
    assert!(dense.part_chars < small.part_chars && dense.source_chars < small.source_chars);
    assert!(dense.part_chars >= MIN_PART_CHARS);
    // Trop petit.
    assert_eq!(budget(1_024, 3.0, prompt), None);
    assert_eq!(budget(2_048, 1.0, prompt), None);
    assert!(budget(2_048, 3.0, prompt).is_some());
}

#[test]
fn parts_follow_paragraphs_and_stay_within_the_limit() {
    let paragraphs: Vec<String> = (0..20).map(paragraph).collect();
    let text = paragraphs.join("\n\n");
    let parts = split_parts(&text, 4_000);
    assert!(parts.len() >= 5, "{}", parts.len());
    for part in &parts {
        assert!(part.chars().count() <= 4_000);
        // Coupées entre deux paragraphes.
        assert!(part.starts_with("Paragraph "), "{part}");
    }
    assert_eq!(parts.join("\n\n"), text);
    // Tailles voisines : la dernière n'est pas un reste minuscule.
    let sizes: Vec<usize> = parts.iter().map(|part| part.chars().count()).collect();
    let (min, max) = (sizes.iter().min().unwrap(), sizes.iter().max().unwrap());
    assert!(*min * 3 >= *max, "{sizes:?}");
    // Texte court : une partie.
    assert_eq!(split_parts("Bonjour.\n\nAu revoir.", 4_000).len(), 1);
}

#[test]
fn a_long_paragraph_is_cut_between_sentences_then_words() {
    let paragraph = paragraph(1);
    let parts = split_parts(&paragraph, 200);
    assert!(parts.len() > 3);
    for part in &parts {
        assert!(part.chars().count() <= 200, "{part}");
        assert!(part.ends_with('.'), "{part}");
    }
    let words = |text: &str| {
        text.split_whitespace()
            .map(str::to_owned)
            .collect::<Vec<_>>()
    };
    assert_eq!(words(&parts.join(" ")), words(&paragraph));
    // Phrase sans fin plus longue que la limite : coupée entre deux mots,
    // mot trop long coupé au caractère.
    let run_on = format!("{} {}", "mot ".repeat(100), "x".repeat(250));
    let parts = split_parts(&run_on, 100);
    assert!(parts.iter().all(|part| part.chars().count() <= 100));
    assert_eq!(parts.concat().replace(' ', ""), run_on.replace(' ', ""));
}

#[test]
fn notes_are_grouped_two_at_least() {
    let notes: Vec<String> = (0..5)
        .map(|i| format!("- {}", "n".repeat(40 + i)))
        .collect();
    let grouped = groups(&notes, 100);
    assert!(
        grouped[..grouped.len() - 1]
            .iter()
            .all(|group| group.len() >= 2)
    );
    assert!(grouped.len() < notes.len());
    assert_eq!(grouped.concat(), notes);
    // Tout tient : un seul paquet.
    assert_eq!(groups(&notes, 10_000).len(), 1);
    // Notes plus longues que la limite : deux par paquet quand même.
    assert_eq!(
        groups(&notes, 10).iter().map(Vec::len).collect::<Vec<_>>(),
        [2, 2, 1]
    );
}

#[test]
fn thinking_and_fences_are_never_shown() {
    assert_eq!(visible("<think>plan", false), "");
    assert_eq!(visible("<think>plan</think>\n\n# Titre", false), "# Titre");
    assert_eq!(visible("plan</think># Titre\n", true), "# Titre");
    assert_eq!(
        visible("```markdown\n# Titre\n\nTexte\n```\n", true),
        "# Titre\n\nTexte"
    );
    assert_eq!(visible("```markdown\n# Titre\n", false), "# Titre\n");
    assert_eq!(visible("```", false), "");
    assert_eq!(visible("  # Titre\n\n", true), "# Titre");
    assert_eq!(visible("# A `code` B", true), "# A `code` B");
}

#[test]
fn engine_errors_have_their_code() {
    let cancel = CancellationToken::new();
    let code = |error: LlmError| match engine_failure("m", &error, &cancel) {
        Stop::Failed(code, _) => Some(code),
        Stop::Cancelled => None,
    };
    assert_eq!(
        code(LlmError::Inference(InferenceError::ContextExceeded {
            n_prompt_tokens: Some(5_000),
            n_ctx: Some(4_096)
        })),
        Some(SynthesisErrorCode::ContextTooSmall)
    );
    assert_eq!(
        code(LlmError::NotInstalled("m".into())),
        Some(SynthesisErrorCode::ModelNotFound)
    );
    assert_eq!(
        code(LlmError::Engine(crate::engine::EngineError::Unavailable)),
        Some(SynthesisErrorCode::EngineFailed)
    );
    assert_eq!(
        code(LlmError::Inference(InferenceError::Interrupted("x".into()))),
        Some(SynthesisErrorCode::EngineFailed)
    );
    assert_eq!(
        code(LlmError::Inference(InferenceError::Http {
            status: 500,
            message: "x".into()
        })),
        Some(SynthesisErrorCode::Failed)
    );
    // Arrêt sans annulation demandée : modèle déchargé.
    assert_eq!(
        code(LlmError::Cancelled),
        Some(SynthesisErrorCode::EngineFailed)
    );
    cancel.cancel();
    assert_eq!(code(LlmError::Cancelled), None);
}

/// Transcription courte : document écrit d'un coup, montré en flux,
/// enregistré, réflexion coupée.
#[tokio::test(flavor = "multi_thread")]
async fn a_short_transcription_is_written_at_once_and_saved() {
    let bench = bench();
    add(&bench, "t1", "en", &[paragraph(1), paragraph(2)]);
    let (synthesizer, sent) = synthesizer();
    synthesizer
        .start(&bench.access, "t1".into(), Some(THINKER.into()))
        .await
        .unwrap();
    let last = finished(&synthesizer).await;
    assert_eq!(last.stage, SynthesisStage::Done, "{last:?}");
    assert_eq!(last.model.as_deref(), Some(THINKER));
    assert_eq!(last.error, None);
    let markdown = last.markdown.clone();
    assert!(markdown.starts_with("# Summary\n\n"), "{markdown}");
    assert!(
        markdown.contains("- Paragraph 1 opens the topic number 1."),
        "{markdown}"
    );
    assert!(!markdown.contains(SYNTHESIS_THINKING));
    // Flux : d'abord vide, puis des débuts du document, puis le document.
    let all = sent.all();
    assert_eq!(all[0].stage, SynthesisStage::Running);
    assert_eq!(all[0].markdown, "");
    let running: Vec<&SynthesisStatus> = all
        .iter()
        .filter(|status| status.stage == SynthesisStage::Running && !status.markdown.is_empty())
        .collect();
    assert!(running.len() > 5, "{}", running.len());
    for status in &running {
        assert!(
            markdown.starts_with(status.markdown.trim_end()),
            "{:?}",
            status.markdown
        );
    }
    assert_eq!(all.last(), Some(&last));
    // Enregistré avec la transcription.
    let saved = summary_of(&bench, "t1").unwrap();
    assert_eq!(
        (saved.markdown.as_str(), saved.model.as_str()),
        (markdown.as_str(), THINKER)
    );
    // Une seule requête : le document, réflexion coupée, réponse bornée.
    let requests = chat_requests(&bench, THINKER).await;
    assert_eq!(requests.len(), 1);
    let request = &requests[0];
    assert!(message(request, "system").contains(DOCUMENT_MARK));
    assert!(message(request, "system").contains("Write everything in English"));
    assert!(message(request, "user").contains("<transcript>\nParagraph 1"));
    assert_eq!(request["chat_template_kwargs"]["enable_thinking"], false);
    assert!(request["max_tokens"].as_u64().unwrap() >= 512);
    assert!((request["temperature"].as_f64().unwrap() - 0.3).abs() < 1e-6);
    // Une autre synthèse peut suivre (remplace la précédente).
    synthesizer
        .start(&bench.access, "t1".into(), Some(SYNTH.into()))
        .await
        .unwrap();
    assert_eq!(finished(&synthesizer).await.stage, SynthesisStage::Done);
    assert_eq!(summary_of(&bench, "t1").unwrap().model, SYNTH);
    bench.shutdown().await;
}

/// Transcription longue dans un petit contexte : notes de chaque partie,
/// puis document d'après les notes (seul montré en flux) ; « Auto » prend
/// le modèle du chat quand rien n'est en mémoire.
#[tokio::test(flavor = "multi_thread")]
async fn a_long_transcription_goes_through_notes() {
    let bench = bench();
    bench.set(SettingsPatch {
        context_length: Some(4_096),
        selected_model: Some(SYNTH.into()),
        ..SettingsPatch::default()
    });
    let paragraphs: Vec<String> = (1..=22).map(paragraph).collect();
    add(&bench, "long", "en", &paragraphs);
    let (synthesizer, sent) = synthesizer();
    synthesizer
        .start(&bench.access, "long".into(), None)
        .await
        .unwrap();
    let last = finished(&synthesizer).await;
    assert_eq!(last.stage, SynthesisStage::Done, "{last:?}");
    assert_eq!(last.model.as_deref(), Some(SYNTH));
    let requests = chat_requests(&bench, SYNTH).await;
    assert!(requests.len() >= 3, "{}", requests.len());
    let (document, notes) = requests.split_last().unwrap();
    for (index, request) in notes.iter().enumerate() {
        let user = message(request, "user");
        assert!(user.contains(PART_MARK), "{user}");
        assert!(
            user.contains(&format!("Part {} of {}", index + 1, notes.len())),
            "{user}"
        );
        assert!(!message(request, "system").contains(DOCUMENT_MARK));
        // Partie dans le contexte (le faux compte un mot par jeton).
        assert!(user.chars().count() <= MAX_PART_CHARS + 200);
    }
    assert!(message(document, "system").contains(DOCUMENT_MARK));
    assert!(message(document, "user").contains("<notes>\n- Paragraph 1 opens"));
    // Une ligne par partie, chacune d'après le début de sa partie.
    let markdown = &last.markdown;
    let points = markdown
        .lines()
        .filter(|line| line.starts_with("- Paragraph"))
        .count();
    assert_eq!(points, notes.len(), "{markdown}");
    // Rien de montré avant le document.
    let all = sent.all();
    let first_shown = all
        .iter()
        .position(|status| !status.markdown.is_empty())
        .unwrap();
    assert!(all[first_shown].markdown.starts_with('#'));
    assert!(summary_of(&bench, "long").is_some());
    bench.shutdown().await;
}

/// « Auto » prend le modèle prêt dans le moteur ; un modèle choisi passe
/// avant.
#[tokio::test(flavor = "multi_thread")]
async fn auto_takes_the_ready_model() {
    let bench = bench();
    bench.set(SettingsPatch {
        selected_model: Some(SYNTH.into()),
        ..SettingsPatch::default()
    });
    add(&bench, "t", "en", &[paragraph(1)]);
    let lease = bench
        .engine()
        .ensure(OTHER, crate::engine::LoadOptions::default())
        .await
        .unwrap();
    let (synthesizer, _) = synthesizer();
    synthesizer
        .start(&bench.access, "t".into(), None)
        .await
        .unwrap();
    assert_eq!(finished(&synthesizer).await.model.as_deref(), Some(OTHER));
    drop(lease);
    synthesizer
        .start(&bench.access, "t".into(), Some(SYNTH.into()))
        .await
        .unwrap();
    assert_eq!(finished(&synthesizer).await.model.as_deref(), Some(SYNTH));
    bench.shutdown().await;
}

/// Refus : aucun modèle, pas de texte, transcription inconnue, synthèse
/// déjà en cours.
#[tokio::test(flavor = "multi_thread")]
async fn refusals_come_before_anything_starts() {
    let bench = bench();
    add(&bench, "t", "en", &[paragraph(1)]);
    add(&bench, "blank", "", &[]);
    let (synthesizer, sent) = synthesizer();
    // Aucun modèle choisi, rien en mémoire, pas de modèle du chat.
    let error = synthesizer
        .start(&bench.access, "t".into(), None)
        .await
        .unwrap_err();
    assert!(matches!(error, AppError::Invalid(_)), "{error:?}");
    assert_eq!(error.to_string(), t!("synthesis.noModel"));
    let error = synthesizer
        .start(&bench.access, "blank".into(), Some(SYNTH.into()))
        .await
        .unwrap_err();
    assert_eq!(error.to_string(), t!("synthesis.noText"));
    let error = synthesizer
        .start(&bench.access, "nope".into(), Some(SYNTH.into()))
        .await
        .unwrap_err();
    assert!(matches!(error, AppError::NotFound(_)), "{error:?}");
    assert!(sent.all().is_empty());
    assert_eq!(synthesizer.status(), None);
    // Une synthèse tourne : la suivante est refusée, l'état ne bouge pas.
    synthesizer
        .start(&bench.access, "t".into(), Some(SLOW.into()))
        .await
        .unwrap();
    let error = synthesizer
        .start(&bench.access, "t".into(), Some(SYNTH.into()))
        .await
        .unwrap_err();
    assert_eq!(error.to_string(), t!("synthesis.busy"));
    assert_eq!(synthesizer.status().unwrap().model.as_deref(), Some(SLOW));
    synthesizer.cancel("t");
    assert_eq!(
        finished(&synthesizer).await.stage,
        SynthesisStage::Cancelled
    );
    bench.shutdown().await;
}

/// Annulation pendant le document : arrêt aussitôt, rien d'enregistré ;
/// pendant les notes aussi ; l'annulation d'une autre transcription est
/// sans effet.
#[tokio::test(flavor = "multi_thread")]
async fn a_synthesis_can_be_cancelled_at_any_time() {
    let bench = bench();
    add(&bench, "t", "en", &[paragraph(1), paragraph(2)]);
    let (synthesizer, sent) = synthesizer();
    synthesizer
        .start(&bench.access, "t".into(), Some(SLOW.into()))
        .await
        .unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while synthesizer.status().unwrap().markdown.is_empty() {
        assert!(tokio::time::Instant::now() < deadline, "nothing shown");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    synthesizer.cancel("other");
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(synthesizer.is_busy());
    let shown = synthesizer.status().unwrap().markdown;
    synthesizer.cancel("t");
    let last = finished(&synthesizer).await;
    assert_eq!(last.stage, SynthesisStage::Cancelled);
    assert_eq!(last.error, None);
    assert!(last.markdown.starts_with(shown.trim_end()));
    assert!(!last.markdown.contains("Conclusion"), "{}", last.markdown);
    assert_eq!(sent.all().last(), Some(&last));
    assert_eq!(summary_of(&bench, "t"), None);

    // Pendant les notes d'une transcription longue.
    bench.set(SettingsPatch {
        context_length: Some(4_096),
        ..SettingsPatch::default()
    });
    let paragraphs: Vec<String> = (1..=22).map(paragraph).collect();
    add(&bench, "long", "en", &paragraphs);
    synthesizer
        .start(&bench.access, "long".into(), Some(SLOW.into()))
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(400)).await;
    synthesizer.cancel("long");
    let last = finished(&synthesizer).await;
    assert_eq!(last.stage, SynthesisStage::Cancelled);
    assert_eq!(last.markdown, "");
    assert_eq!(summary_of(&bench, "long"), None);
    let requests = chat_requests(&bench, SLOW).await;
    assert!(
        !requests
            .iter()
            .any(|request| message(request, "system").contains(DOCUMENT_MARK)
                && message(request, "user").contains("<notes>"))
    );
    bench.shutdown().await;
}

/// Langue : celle de la transcription, sinon devinée sur son texte ; un
/// document écrit dans une autre langue est redemandé une fois.
#[tokio::test(flavor = "multi_thread")]
async fn the_document_is_written_in_the_language_of_the_transcription() {
    let bench = bench();
    let french = "Bonjour à tous, nous allons parler de la photosynthèse et des plantes \
                  qui sont dans le jardin avec nous ce matin."
        .to_owned();
    add(&bench, "fr", "fr", std::slice::from_ref(&french));
    // Langue non enregistrée (Parakeet) : devinée sur le texte.
    add(&bench, "guess", "", std::slice::from_ref(&french));
    add(&bench, "en", "en", &[paragraph(1)]);
    let (synthesizer, _) = synthesizer();
    for id in ["fr", "guess"] {
        synthesizer
            .start(&bench.access, id.into(), Some(SYNTH.into()))
            .await
            .unwrap();
        let last = finished(&synthesizer).await;
        assert!(
            last.markdown.starts_with("# Synthèse"),
            "{id}: {}",
            last.markdown
        );
    }
    // Garde : le premier document est en espagnol, redemandé avec un rappel.
    synthesizer
        .start(&bench.access, "en".into(), Some(STRAY.into()))
        .await
        .unwrap();
    let last = finished(&synthesizer).await;
    assert_eq!(last.stage, SynthesisStage::Done);
    assert!(last.markdown.starts_with("# Summary"), "{}", last.markdown);
    let requests = chat_requests(&bench, STRAY).await;
    assert_eq!(requests.len(), 2);
    assert!(!message(&requests[0], "user").contains(RETRY_MARK));
    assert!(message(&requests[1], "user").contains(RETRY_MARK));
    assert_eq!(summary_of(&bench, "en").unwrap().markdown, last.markdown);
    assert!(!last.markdown.contains(STRAY_DOCUMENT));
    bench.shutdown().await;
}

/// Relecture 107 : document coupé par la limite de longueur
/// (`finish_reason: length`) : redemandé une fois avec deux fois plus de
/// jetons (montré de nouveau depuis le début) ; encore coupé : échec
/// `Incomplete`, rien d'enregistré (jamais un document tronqué « fini »).
#[tokio::test(flavor = "multi_thread")]
async fn a_cut_document_is_asked_again_then_refused() {
    let bench = bench();
    bench.set(SettingsPatch {
        context_length: Some(4_096),
        ..SettingsPatch::default()
    });
    add(&bench, "short", "en", &[paragraph(1)]);
    add(&bench, "cut", "en", &[paragraph(1)]);
    let (synthesizer, sent) = synthesizer();

    synthesizer
        .start(&bench.access, "short".into(), Some(SHORT.into()))
        .await
        .unwrap();
    let last = finished(&synthesizer).await;
    assert_eq!(last.stage, SynthesisStage::Done, "{last:?}");
    assert!(last.markdown.starts_with("# Summary"), "{}", last.markdown);
    assert!(last.markdown.contains("## Conclusion"), "{}", last.markdown);
    assert_eq!(summary_of(&bench, "short").unwrap().markdown, last.markdown);
    let requests = chat_requests(&bench, SHORT).await;
    assert_eq!(requests.len(), 2);
    let first = requests[0]["max_tokens"].as_u64().unwrap();
    assert!(first < SHORT_LIMIT, "{first}");
    assert_eq!(requests[1]["max_tokens"].as_u64().unwrap(), first * 2);
    // Le flux repart de zéro avant la seconde réponse.
    let shown: Vec<String> = sent.all().into_iter().map(|s| s.markdown).collect();
    assert!(
        shown
            .windows(2)
            .any(|pair| !pair[0].is_empty() && pair[1].is_empty()),
        "{shown:?}"
    );

    synthesizer
        .start(&bench.access, "cut".into(), Some(LENGTH.into()))
        .await
        .unwrap();
    let last = finished(&synthesizer).await;
    assert_eq!(last.stage, SynthesisStage::Error, "{last:?}");
    let error = last.error.unwrap();
    assert_eq!(error.code, SynthesisErrorCode::Incomplete);
    assert!(
        !error.message.starts_with("synthesis."),
        "{}",
        error.message
    );
    assert_eq!(chat_requests(&bench, LENGTH).await.len(), 2);
    assert_eq!(summary_of(&bench, "cut"), None);
    bench.shutdown().await;
}

/// Relecture 107 : une note vide (partie 2) fait échouer la synthèse au lieu
/// d'omettre cette partie de la transcription en silence.
#[tokio::test(flavor = "multi_thread")]
async fn an_empty_note_fails_instead_of_dropping_a_part() {
    let bench = bench();
    bench.set(SettingsPatch {
        context_length: Some(4_096),
        ..SettingsPatch::default()
    });
    let paragraphs: Vec<String> = (1..=22).map(paragraph).collect();
    add(&bench, "long", "en", &paragraphs);
    let (synthesizer, _) = synthesizer();
    synthesizer
        .start(&bench.access, "long".into(), Some(HOLE.into()))
        .await
        .unwrap();
    let last = finished(&synthesizer).await;
    assert_eq!(last.stage, SynthesisStage::Error, "{last:?}");
    assert_eq!(last.error.unwrap().code, SynthesisErrorCode::Empty);
    // Arrêt à la partie vide : aucun document demandé.
    let requests = chat_requests(&bench, HOLE).await;
    assert!(
        requests
            .iter()
            .all(|request| !message(request, "system").contains(DOCUMENT_MARK))
    );
    assert_eq!(summary_of(&bench, "long"), None);
    bench.shutdown().await;
}

/// Relecture 107 : sans langue enregistrée, une transcription néerlandaise
/// n'est plus prise pour de l'anglais : consigne « dans la langue de la
/// transcription », sans garde.
#[tokio::test(flavor = "multi_thread")]
async fn an_unsure_language_is_left_to_the_transcript() {
    let bench = bench();
    let dutch = "Ik heb gisteren met mijn broer gesproken en hij zegt dat het weer in \
                 Amsterdam niet zo goed is, maar dat de stad nog steeds erg mooi is. We gaan \
                 er volgende week samen naartoe, want hij wil het museum zien."
        .to_owned();
    add(&bench, "nl", "", std::slice::from_ref(&dutch));
    let (synthesizer, _) = synthesizer();
    synthesizer
        .start(&bench.access, "nl".into(), Some(SYNTH.into()))
        .await
        .unwrap();
    assert_eq!(finished(&synthesizer).await.stage, SynthesisStage::Done);
    let requests = chat_requests(&bench, SYNTH).await;
    let system = message(&requests[0], "system");
    assert!(
        system.contains("in the language of the transcript"),
        "{system}"
    );
    assert!(!system.contains("English"), "{system}");
    bench.shutdown().await;
}

/// Échecs : réponse vide, connexion coupée, modèle absent ; rien
/// d'enregistré, une autre synthèse peut suivre.
#[tokio::test(flavor = "multi_thread")]
async fn failures_have_their_code_and_save_nothing() {
    let bench = bench();
    add(&bench, "t", "en", &[paragraph(1)]);
    let (synthesizer, _) = synthesizer();
    for (model, code) in [
        (EMPTY, SynthesisErrorCode::Empty),
        (CUT, SynthesisErrorCode::EngineFailed),
        (
            "test/Missing-GGUF:Q4_K_M",
            SynthesisErrorCode::ModelNotFound,
        ),
    ] {
        synthesizer
            .start(&bench.access, "t".into(), Some(model.into()))
            .await
            .unwrap();
        let last = finished(&synthesizer).await;
        assert_eq!(last.stage, SynthesisStage::Error, "{model}");
        let error = last.error.unwrap();
        assert_eq!(error.code, code, "{model}: {}", error.message);
        assert!(!error.message.is_empty());
        assert_eq!(summary_of(&bench, "t"), None);
    }
    assert!(!synthesizer.is_busy());
    bench.shutdown().await;
}

/// Contrat : vraie synthèse par le vrai `llama-server` et Qwen3 0.6B
/// (`RealBench`, rôle `Text`) d'une courte transcription anglaise : document
/// non vide, réflexion jamais montrée, enregistré. La qualité n'est pas
/// exigée.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs CHATONLM_LLAMA_TEST_BIN and the Text model (RealBench)"]
async fn contract_synthesis_with_a_small_model() {
    let Some(bench) = RealBench::new(Role::Text) else {
        return;
    };
    let text = "Today we talk about bees. Bees carry pollen from flower to flower, \
                and this helps plants make fruit. A single hive can hold fifty thousand \
                bees. In winter, they stay together to keep warm.";
    let segments = vec![Segment {
        start: 0.0,
        end: 20.0,
        text: text.into(),
    }];
    bench
        .access
        .store()
        .blocking(move |c| {
            insert(
                c,
                &NewTranscription {
                    id: "real".into(),
                    source: SourceKind::File,
                    origin: "/x.mp3".into(),
                    title: "Bees".into(),
                    engine: crate::dictation::models::SpeechEngine::Whisper,
                    model: "whisper-base".into(),
                    language: "en".into(),
                },
            )?;
            set_segments(c, "real", &segments)
        })
        .unwrap();
    let (synthesizer, _) = synthesizer();
    synthesizer
        .start(&bench.access, "real".into(), Some(bench.id.clone()))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(600), synthesizer.wait())
        .await
        .unwrap();
    let last = synthesizer.status().unwrap();
    eprintln!("{}", last.markdown);
    assert_eq!(last.stage, SynthesisStage::Done, "{last:?}");
    assert!(!last.markdown.trim().is_empty());
    assert!(!last.markdown.contains("<think"));
    let saved = bench
        .access
        .store()
        .blocking(|c| transcriptions::get(c, "real"))
        .unwrap()
        .unwrap()
        .summary
        .unwrap();
    assert_eq!(saved.markdown, last.markdown);
    bench.shutdown().await;
}
