//! Autocomplétion « fill in the middle » (FIM) par `/infill` de
//! `llama-server` (lot L5a) : le serveur construit le prompt avec les jetons
//! FIM du GGUF (`FIM_PRE`, `FIM_SUF`, `FIM_MID`), seulement pour un modèle qui
//! les a (capacité `fim` de la base locale ; sans eux, `/infill` rend du
//! texte sans valeur : refus clair, plus de voie `raw`). Le code de
//! l'utilisateur est découpé en jetons sans jetons spéciaux (b12110,
//! `format_prompt_infill`) : rien à neutraliser. Options figées (contexte,
//! `keep_alive`) : jamais de rechargement silencieux. Réponse entière
//! attendue au plus [`deadline`] (le premier jeton, puis un débit minimal),
//! sinon abandon (l'autocomplétion ne doit jamais faire attendre la frappe)
//! ; préfixe et suffixe ramenés au contexte. Proposition finie par
//! [`finish`] : sans la ligne inachevée qu'a coupée la limite de jetons, ni
//! la boucle d'un petit modèle (lignes de même forme). Mesures lues dans les
//! `timings` du serveur.
//!
//! Familles et jetons d'arrêt relevés dans `AutocompleteTemplate.ts` de
//! Continue (MIT : rapport 87-c § 3.1).

use std::time::{Duration, Instant};

use serde::Deserialize;
use tokio_util::sync::CancellationToken;

use crate::inference::{self, Endpoint, InfillRequest, InfillStop, Timings};

/// Délai du premier jeton.
pub const FIRST_TOKEN: Duration = Duration::from_millis(1500);
/// Durée d'une réponse, au plus.
const TOTAL: Duration = Duration::from_secs(15);
/// Temps permis par jeton produit après le premier (40 jetons/s au moins :
/// en deçà, la mesure dit « trop lent »).
const PER_TOKEN: Duration = Duration::from_millis(25);
/// Jetons produits sans `maxTokens` : plusieurs lignes, ou une seule.
pub const MULTILINE_TOKENS: u32 = 64;
pub const LINE_TOKENS: u32 = 16;
/// Jetons demandés au plus.
pub const MAX_TOKENS: u32 = 256;
/// Caractères du préfixe et du suffixe, au plus chacun.
pub const MAX_PREFIX: usize = 32_000;
pub const MAX_SUFFIX: usize = 16_000;
/// Température basse : la suite la plus probable.
const TEMPERATURE: f32 = 0.2;
/// Caractères par jeton comptés pour borner le préfixe (prudent : le code
/// en fait plutôt 3,5 à 4).
const CHARS_PER_TOKEN: usize = 3;
/// Jetons laissés aux jetons FIM et au gabarit.
const TEMPLATE_TOKENS: u32 = 64;
/// Lignes de même forme à la suite à partir desquelles une proposition est
/// vue comme une boucle du modèle (`int a = 0;`, `int b = 0;`…).
const LOOP_LINES: usize = 4;

/// Familles dont les jetons FIM sont connus (arrêts, et modèles de code
/// reconnus par leur nom : `engine::auto`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Family {
    QwenCoder,
    StarCoder,
    CodeGemma,
    DeepSeekCoder,
}

/// Famille d'après le nom du modèle (sans l'espace de noms ni l'étiquette).
pub fn family(model: &str) -> Option<Family> {
    let name = model
        .rsplit('/')
        .next()
        .unwrap_or(model)
        .to_ascii_lowercase();
    let name = name.split(':').next().unwrap_or_default();
    if name.contains("qwen") && name.contains("coder") {
        Some(Family::QwenCoder)
    } else if name.contains("starcoder") {
        Some(Family::StarCoder)
    } else if name.contains("codegemma") {
        Some(Family::CodeGemma)
    } else if name.contains("deepseek-coder") {
        Some(Family::DeepSeekCoder)
    } else {
        None
    }
}

impl Family {
    /// Jetons qui finissent une proposition.
    pub fn stops(self) -> &'static [&'static str] {
        match self {
            Family::QwenCoder => &[
                "<|endoftext|>",
                "<|fim_prefix|>",
                "<|fim_suffix|>",
                "<|fim_middle|>",
                "<|fim_pad|>",
                "<|file_sep|>",
                "<|repo_name|>",
                "<|im_start|>",
                "<|im_end|>",
            ],
            Family::StarCoder => &[
                "<|endoftext|>",
                "<fim_prefix>",
                "<fim_suffix>",
                "<fim_middle>",
                "<file_sep>",
            ],
            Family::CodeGemma => &[
                "<|fim_prefix|>",
                "<|fim_suffix|>",
                "<|fim_middle|>",
                "<|file_separator|>",
                "<end_of_turn>",
                "<eos>",
            ],
            Family::DeepSeekCoder => &[
                "<｜fim▁begin｜>",
                "<｜fim▁hole｜>",
                "<｜fim▁end｜>",
                "<｜end▁of▁sentence｜>",
            ],
        }
    }
}

/// Caractères de préfixe et de suffixe permis pour un modèle chargé avec
/// `context` jetons qui en écrira `tokens`.
pub fn budget(context: u32, tokens: u32) -> usize {
    let room = context
        .saturating_sub(tokens)
        .saturating_sub(TEMPLATE_TOKENS);
    usize::try_from(room)
        .unwrap_or(usize::MAX)
        .saturating_mul(CHARS_PER_TOKEN)
}

/// Préfixe et suffixe dans `budget` caractères : le suffixe garde au plus le
/// quart (coupé par la fin, à une ligne entière), le préfixe le reste (coupé
/// par le début, à une ligne entière : le code au plus près du curseur
/// reste).
pub fn fit<'a>(prefix: &'a str, suffix: &'a str, budget: usize) -> (&'a str, &'a str) {
    let count = |text: &str| text.chars().count();
    if count(prefix) + count(suffix) <= budget {
        return (prefix, suffix);
    }
    let suffix = if count(suffix) > budget / 4 {
        let end = suffix
            .char_indices()
            .nth(budget / 4)
            .map_or(suffix.len(), |(at, _)| at);
        // Jusqu'à la dernière fin de ligne gardée.
        &suffix[..suffix[..end].rfind('\n').map_or(0, |at| at + 1)]
    } else {
        suffix
    };
    let room = budget.saturating_sub(count(suffix));
    let prefix = if count(prefix) > room {
        let skip = count(prefix) - room;
        let start = prefix
            .char_indices()
            .nth(skip)
            .map_or(prefix.len(), |(at, _)| at);
        // Depuis le début de la ligne suivante.
        match prefix[start..].find('\n') {
            Some(at) => &prefix[start + at + 1..],
            None => &prefix[start..],
        }
    } else {
        prefix
    };
    (prefix, suffix)
}

/// Paramètres de `complete.fim`.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FimParams {
    pub model: String,
    pub prefix: String,
    #[serde(default)]
    pub suffix: String,
    // `language` et `path` (contrat) sont acceptés et ignorés : seul le
    // code autour du curseur part vers le modèle.
    #[serde(default)]
    pub max_tokens: Option<u32>,
    #[serde(default)]
    pub multiline: Option<bool>,
}

impl FimParams {
    /// Préfixe et suffixe bornés, jetons demandés dans les limites.
    pub fn valid(&self) -> bool {
        !self.model.trim().is_empty()
            && self.prefix.chars().count() <= MAX_PREFIX
            && self.suffix.chars().count() <= MAX_SUFFIX
            && self
                .max_tokens
                .is_none_or(|tokens| (1..=MAX_TOKENS).contains(&tokens))
    }

    pub fn multiline(&self) -> bool {
        self.multiline.unwrap_or(true)
    }

    pub fn tokens(&self) -> u32 {
        self.max_tokens.unwrap_or(if self.multiline() {
            MULTILINE_TOKENS
        } else {
            LINE_TOKENS
        })
    }
}

/// Options figées d'un modèle d'autocomplétion, comme les réglages de la
/// conversation : `num_ctx` (0 : contexte choisi comme pour la
/// conversation) et `keep_alive` (secondes ; 0 : défaut du moteur,
/// négatif : gardé).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fixed {
    pub num_ctx: u32,
    pub keep_alive: i64,
}

/// Proposition coupée comme la demande : à la première ligne vide en
/// plusieurs lignes, à la fin de la ligne sinon, et à tout jeton FIM qui
/// aurait échappé aux arrêts.
pub fn cut(text: &str, multiline: bool, family: Option<Family>) -> String {
    let mut end = text.len();
    let mut cut_at = |marker: &str| {
        if let Some(at) = text.find(marker) {
            end = end.min(at);
        }
    };
    cut_at(if multiline { "\n\n" } else { "\n" });
    if let Some(family) = family {
        for stop in family.stops() {
            cut_at(stop);
        }
    }
    text[..end].trim_end_matches([' ', '\t', '\r']).to_owned()
}

/// Forme d'une ligne : son retrait, puis ses signes, chaque mot ou nombre
/// réduit à `w` (`    int a = 0;` et `    int b = 1;` ont la même).
fn shape(line: &str) -> String {
    let body = line.trim_start();
    let mut out = line[..line.len() - body.len()].to_owned();
    let mut in_word = false;
    for c in body.chars() {
        if c.is_alphanumeric() || c == '_' {
            if !in_word {
                out.push('w');
            }
            in_word = true;
        } else {
            in_word = false;
            if !c.is_whitespace() {
                out.push(c);
            }
        }
    }
    out
}

/// Proposition finie : coupée comme la demande ([`cut`]) ; en plusieurs
/// lignes, sa dernière ligne retirée si la limite de jetons l'a coupée
/// (`truncated`), et arrêtée à la première ligne d'une suite de
/// [`LOOP_LINES`] lignes de même forme ou plus (boucle du modèle, qui
/// remplirait sinon toute la limite).
pub fn finish(text: &str, multiline: bool, family: Option<Family>, truncated: bool) -> String {
    let mut text = cut(text, multiline, family);
    if !multiline {
        return text;
    }
    if truncated && let Some(at) = text.rfind('\n') {
        text.truncate(at);
        text.truncate(text.trim_end_matches([' ', '\t', '\r']).len());
    }
    let lines: Vec<&str> = text.split('\n').collect();
    let shapes: Vec<String> = lines.iter().map(|line| shape(line)).collect();
    let mut start = 0;
    for index in 1..=shapes.len() {
        let same = index < shapes.len()
            && !shapes[index].trim().is_empty()
            && shapes[index] == shapes[start];
        if same {
            continue;
        }
        if index - start >= LOOP_LINES && !shapes[start].trim().is_empty() {
            return lines[..=start].join("\n");
        }
        start = index;
    }
    text
}

/// Proposition reçue.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Completed {
    pub text: String,
    /// Du départ de la requête au premier jeton (lu dans les mesures du
    /// serveur), ou à l'abandon.
    pub first_token_ms: u64,
    pub total_ms: u64,
    /// Jetons du prompt repris du cache du serveur.
    pub cached_tokens: u64,
    /// Réponse pas arrivée à temps : abandonnée, texte vide.
    pub timed_out: bool,
}

/// Échec d'une requête.
#[derive(Debug, PartialEq, Eq)]
pub enum FimFailure {
    /// Texte traduit (langue du moment), à montrer tel quel.
    Failed(String),
}

fn failed(error: inference::InferenceError) -> FimFailure {
    FimFailure::Failed(crate::llm::LlmError::Inference(error).message())
}

/// Attente d'une proposition de `tokens` jetons : le premier jeton, puis
/// [`PER_TOKEN`] par jeton, au plus [`TOTAL`] (`/infill` rend la réponse
/// entière).
pub fn deadline(tokens: u32) -> Duration {
    (FIRST_TOKEN + PER_TOKEN * tokens).min(TOTAL)
}

/// Arrêts d'une proposition : la ligne vide (plusieurs lignes) ou la fin de
/// ligne (une seule), puis les jetons de la famille.
pub fn stops(multiline: bool, family: Option<Family>) -> Vec<String> {
    let mut stops = vec![if multiline { "\n\n" } else { "\n" }.to_owned()];
    if let Some(family) = family {
        stops.extend(family.stops().iter().map(|stop| (*stop).to_owned()));
    }
    stops
}

/// Demande `/infill` d'une proposition (température basse, `tokens` au
/// plus, arrêts de [`stops`]).
pub fn request(
    prefix: &str,
    suffix: &str,
    tokens: u32,
    multiline: bool,
    family: Option<Family>,
) -> InfillRequest {
    InfillRequest {
        input_prefix: prefix.to_owned(),
        input_suffix: suffix.to_owned(),
        input_extra: Vec::new(),
        n_predict: Some(tokens),
        temperature: Some(TEMPERATURE),
        stop: stops(multiline, family),
    }
}

/// Envoie la demande au modèle tenu (`endpoint` de son `Lease`) ; au-delà
/// de [`deadline`], abandon (requête lâchée : le serveur arrête la
/// génération ou la retire de sa file).
pub async fn complete(
    endpoint: &Endpoint,
    request: &InfillRequest,
    multiline: bool,
    family: Option<Family>,
) -> Result<Completed, FimFailure> {
    let started = Instant::now();
    let tokens = request.n_predict.unwrap_or(MULTILINE_TOKENS);
    let cancel = CancellationToken::new();
    let _cancel_on_drop = cancel.clone().drop_guard();
    let answer = tokio::time::timeout(
        deadline(tokens),
        inference::infill(endpoint, request, cancel.clone()),
    )
    .await;
    let Ok(answer) = answer else {
        let elapsed = millis(started.elapsed());
        return Ok(Completed {
            first_token_ms: elapsed,
            total_ms: elapsed,
            timed_out: true,
            ..Completed::default()
        });
    };
    let answer = answer.map_err(failed)?;
    let total_ms = millis(started.elapsed());
    let first_token_ms = answer
        .timings
        .map_or(total_ms, |timings| speed(&timings).0.min(total_ms));
    Ok(Completed {
        text: finish(
            &answer.content,
            multiline,
            family,
            answer.stop == InfillStop::Limit,
        ),
        first_token_ms,
        total_ms,
        cached_tokens: answer
            .timings
            .map_or(0, |timings| u64::from(timings.cache_n)),
        timed_out: false,
    })
}

/// Une requête de mesure (32 jetons, seuls les jetons de fin arrêtent) :
/// mesures du serveur (indépendantes de la charge du pont).
pub async fn measured(endpoint: &Endpoint, request: &InfillRequest) -> Result<Timings, FimFailure> {
    let cancel = CancellationToken::new();
    let _cancel_on_drop = cancel.clone().drop_guard();
    let answer = tokio::time::timeout(
        Duration::from_secs(120),
        inference::infill(endpoint, request, cancel.clone()),
    )
    .await
    .map_err(|_| failed(inference::InferenceError::Timeout))?
    .map_err(failed)?;
    answer
        .timings
        .ok_or_else(|| failed(inference::InferenceError::Malformed("no timings".into())))
}

fn millis(elapsed: Duration) -> u64 {
    u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
}

/// Verdict d'une mesure.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Rating {
    Smooth,
    Usable,
    TooSlow,
}

/// Jetons d'une proposition type, pour le verdict.
pub const RATED_TOKENS: f64 = 32.0;

/// Premier jeton (ms) et vitesse (jetons/s) d'une réponse mesurée
/// (`timings` du serveur) : lecture du prompt, puis un jeton.
pub fn speed(timings: &Timings) -> (u64, f64) {
    let per_token = if timings.predicted_n > 0 {
        timings.predicted_ms / f64::from(timings.predicted_n)
    } else {
        0.0
    };
    let first = (timings.prompt_ms + per_token).max(0.0) as u64;
    let tokens_per_second = if timings.predicted_ms > 0.0 {
        f64::from(timings.predicted_n) / (timings.predicted_ms / 1000.0)
    } else {
        0.0
    };
    (first, tokens_per_second)
}

/// Fluide sous 0,5 s pour une proposition de 32 jetons, utilisable jusqu'à
/// 1,5 s, déconseillé au-delà (rapport 87-c § 3.5).
pub fn rate(first_token_ms: u64, tokens_per_second: f64) -> Rating {
    let generating = if tokens_per_second > 0.0 {
        RATED_TOKENS / tokens_per_second * 1000.0
    } else {
        f64::INFINITY
    };
    let latency = first_token_ms as f64 + generating;
    if latency < 500.0 {
        Rating::Smooth
    } else if latency <= 1500.0 {
        Rating::Usable
    } else {
        Rating::TooSlow
    }
}

/// Code fixe de la mesure : environ 1 000 jetons de préfixe ; `round`
/// change la dernière ligne (comme une frappe : le début reste en cache).
pub fn measure_prompt(round: usize) -> (String, String) {
    let mut prefix = String::new();
    for index in 0..24 {
        // i18n-ignore : code donné au modèle pour la mesure.
        prefix.push_str(&format!(
            "def total_{index}(items, rate={index}):\n    \"\"\"Sum of the prices of the items, with the rate applied.\"\"\"\n    result = 0\n    for item in items:\n        result += item.price * (1 + rate / 100)\n    return round(result, 2)\n\n\n"
        ));
    }
    // i18n-ignore : code donné au modèle pour la mesure.
    prefix.push_str(&format!("def average_{round}(values):\n    "));
    // i18n-ignore : code donné au modèle pour la mesure.
    let suffix = "\n\n\nif __name__ == \"__main__\":\n    print(average_0([1, 2, 3]))\n".to_owned();
    (prefix, suffix)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn families_are_read_from_the_name() {
        for (name, expected) in [
            ("qwen2.5-coder:1.5b-base", Some(Family::QwenCoder)),
            ("qwen3-coder:30b", Some(Family::QwenCoder)),
            (
                "hf.co/Qwen/Qwen2.5-Coder-7B-GGUF:Q4_K_M",
                Some(Family::QwenCoder),
            ),
            ("starcoder2:3b", Some(Family::StarCoder)),
            ("codegemma:2b", Some(Family::CodeGemma)),
            ("deepseek-coder:6.7b-base", Some(Family::DeepSeekCoder)),
            ("qwen3:0.6b", None),
            ("llama3.2:3b", None),
            // L'étiquette ne compte pas.
            ("mymodel:coder-qwen", None),
        ] {
            assert_eq!(family(name), expected, "{name}");
        }
    }

    /// `/infill` : contexte de la famille, arrêts, jetons et température ;
    /// le serveur construit le prompt (aucun jeton écrit par l'app).
    #[test]
    fn the_request_carries_the_stops_and_the_limit() {
        let multi = request("def f(", ")\n", 64, true, Some(Family::QwenCoder));
        assert_eq!(multi.input_prefix, "def f(");
        assert_eq!(multi.input_suffix, ")\n");
        assert_eq!(multi.n_predict, Some(64));
        assert_eq!(multi.temperature, Some(TEMPERATURE));
        assert_eq!(multi.stop[0], "\n\n");
        assert!(multi.stop.iter().any(|stop| stop == "<|fim_middle|>"));
        let body = multi.body();
        assert!(body.get("prompt").is_none(), "{body}");
        let line = request("x = ", "", 16, false, None);
        assert_eq!(line.stop, ["\n"]);
        // Le code de l'utilisateur part tel quel (le serveur ne lit pas ses
        // jetons spéciaux).
        let code = "s = '<|fim_middle|>'";
        assert_eq!(request(code, "", 16, false, None).input_prefix, code);
    }

    /// Le premier jeton, puis un débit minimal ; jamais plus de 15 s.
    #[test]
    fn the_deadline_follows_the_tokens() {
        assert_eq!(deadline(0), FIRST_TOKEN);
        assert_eq!(deadline(16), Duration::from_millis(1500 + 16 * 25));
        assert_eq!(deadline(64), Duration::from_millis(1500 + 64 * 25));
        assert_eq!(deadline(MAX_TOKENS), Duration::from_millis(1500 + 256 * 25));
        assert_eq!(deadline(10_000), TOTAL);
    }

    #[test]
    fn params_are_bounded() {
        let params = |max_tokens: Option<u32>, prefix: usize| FimParams {
            model: "m".into(),
            prefix: "x".repeat(prefix),
            suffix: String::new(),
            max_tokens,
            multiline: None,
        };
        assert!(params(None, MAX_PREFIX).valid());
        assert!(!params(None, MAX_PREFIX + 1).valid());
        assert!(!params(Some(0), 1).valid());
        assert!(!params(Some(MAX_TOKENS + 1), 1).valid());
        assert_eq!(params(None, 1).tokens(), MULTILINE_TOKENS);
        let line = FimParams {
            multiline: Some(false),
            ..params(None, 1)
        };
        assert_eq!(line.tokens(), LINE_TOKENS);
        assert_eq!(params(Some(8), 1).tokens(), 8);
    }

    /// Rapport 92, m3 : préfixe et suffixe ramenés au contexte, à des lignes
    /// entières, le code près du curseur gardé.
    #[test]
    fn prefix_and_suffix_fit_the_context() {
        assert_eq!(budget(4096, 64), (4096 - 64 - 64) * 3);
        assert_eq!(budget(100, 64), 0);
        let prefix: String = (0..100).map(|line| format!("line {line:03}\n")).collect();
        let suffix: String = (0..100).map(|line| format!("after {line:03}\n")).collect();
        assert_eq!(fit(&prefix, &suffix, 10_000), (&prefix[..], &suffix[..]));
        let (kept_prefix, kept_suffix) = fit(&prefix, &suffix, 200);
        assert!(kept_prefix.chars().count() + kept_suffix.chars().count() <= 200);
        assert!(kept_prefix.ends_with("line 099\n"), "{kept_prefix}");
        assert!(kept_prefix.starts_with("line "), "{kept_prefix}");
        assert!(kept_suffix.starts_with("after 000\n"), "{kept_suffix}");
        assert!(kept_suffix.ends_with('\n'));
        assert!(kept_suffix.chars().count() <= 50);
        // Préfixe d'une seule ligne : coupé par le début.
        let long = "x".repeat(500);
        assert_eq!(fit(&long, "", 100).0.chars().count(), 100);
    }

    #[test]
    fn a_proposal_is_cut_like_the_request() {
        let family = Some(Family::QwenCoder);
        assert_eq!(cut("a + b\n    c\n\nd", true, family), "a + b\n    c");
        assert_eq!(cut("a + b\n    c", false, family), "a + b");
        assert_eq!(cut("a<|endoftext|>junk", true, family), "a");
        assert_eq!(cut("a<|endoftext|>junk", true, None), "a<|endoftext|>junk");
        assert_eq!(cut("value  \t", true, family), "value");
    }

    /// Proposition coupée par la limite de jetons : la ligne inachevée part
    /// (plusieurs lignes seulement ; une seule ligne inachevée reste).
    #[test]
    fn a_line_cut_by_the_token_limit_is_dropped() {
        let family = Some(Family::QwenCoder);
        let text = "\n    int *p = &a;\n    printf(\"a = %";
        assert_eq!(finish(text, true, family, true), "\n    int *p = &a;");
        assert_eq!(finish(text, true, family, false), text);
        assert_eq!(finish("value(6|", true, family, true), "value(6|");
        assert_eq!(finish("value(6|", false, family, true), "value(6|");
        assert_eq!(
            finish("a = 1\n    b = \t", true, family, true),
            "a = 1",
            "trailing spaces trimmed"
        );
    }

    /// Boucle du modèle (relevée à la recette : `int a = 0;` à `int i = 0`
    /// dans un `main` vide) : arrêtée à la première ligne de la suite.
    #[test]
    fn a_looping_proposal_stops_at_its_first_line() {
        let family = Some(Family::QwenCoder);
        let looping: String = ('a'..='i')
            .map(|name| format!("\n    int {name} = 0;"))
            .collect();
        assert_eq!(finish(&looping, true, family, false), "\n    int a = 0;");
        // Après des lignes utiles, la boucle est coupée de même.
        let after = format!("\n    int x = f();{looping}");
        assert_eq!(
            finish(&after, true, family, false),
            "\n    int x = f();\n    int a = 0;"
        );
        // Trois lignes de même forme : un motif ordinaire, gardé.
        let three = "\n    p.x = 1;\n    p.y = 2;\n    p.z = 3;\n    return p;";
        assert_eq!(finish(three, true, family, false), three);
        // Même forme mais retraits différents (accolades imbriquées) : gardé.
        let braces = "x;\n      }\n    }\n  }\n}";
        assert_eq!(finish(braces, true, family, false), braces);
        // Une seule ligne demandée : rien de plus que la coupe.
        assert_eq!(finish("int a = 0;", false, family, false), "int a = 0;");
        assert_eq!(shape("    int a = 0;"), "    ww=w;");
        assert_eq!(shape("    int count_2 = 10;"), "    ww=w;");
    }

    #[test]
    fn a_measure_is_rated() {
        let timings = Timings {
            cache_n: 900,
            prompt_n: 100,
            prompt_ms: 60.5,
            predicted_n: 40,
            predicted_ms: 100.0,
        };
        let (first, per_second) = speed(&timings);
        // 60,5 + 2,5 ms.
        assert_eq!(first, 63);
        assert!((per_second - 400.0).abs() < 1e-9);
        assert_eq!(rate(first, per_second), Rating::Smooth);
        assert_eq!(rate(400, 100.0), Rating::Usable);
        assert_eq!(rate(1000, 20.0), Rating::TooSlow);
        assert_eq!(rate(10, 0.0), Rating::TooSlow);
        // Rien de produit : vitesse nulle, pas de division par zéro.
        assert_eq!(speed(&Timings::default()), (0, 0.0));
        let (prefix, suffix) = measure_prompt(1);
        assert!(prefix.ends_with("def average_1(values):\n    "));
        assert!(prefix.len() > 3_000, "{}", prefix.len());
        assert!(suffix.contains("__main__"));
        assert_ne!(measure_prompt(2).0, prefix);
    }
}
