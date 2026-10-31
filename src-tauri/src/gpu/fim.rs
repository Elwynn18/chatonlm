//! Requête d'autocomplétion « fill in the middle » (FIM) à Ollama
//! (`/api/generate`) : `suffix` si le modèle annonce la capacité `insert`
//! (son gabarit construit le prompt), sinon `raw` avec les jetons FIM de sa
//! famille (relevés dans `AutocompleteTemplate.ts` de Continue, MIT :
//! rapport 87-c § 3.1). Options figées (`num_ctx`, `keep_alive`) : jamais de
//! rechargement silencieux. Réponse en flux, coupée tôt ; premier jeton
//! attendu au plus [`FIRST_TOKEN`], envoi compris (Ollama n'écrit les
//! en-têtes de sa réponse qu'avec le premier jeton), sinon abandon
//! (l'autocomplétion ne doit jamais faire attendre la frappe). Les jetons
//! spéciaux écrits dans le code de l'utilisateur sont neutralisés (ils
//! casseraient la structure FIM) ; préfixe et suffixe ramenés au contexte.
//! Proposition finie par [`finish`] : sans la ligne inachevée qu'a coupée la
//! limite de jetons, ni la boucle d'un petit modèle (lignes de même forme).

use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::{Value, json};

use crate::ollama::client;

/// Délai du premier jeton.
pub const FIRST_TOKEN: Duration = Duration::from_millis(1500);
/// Durée d'une réponse, au plus.
const TOTAL: Duration = Duration::from_secs(15);
/// Jetons produits sans `maxTokens` : plusieurs lignes, ou une seule.
pub const MULTILINE_TOKENS: u32 = 64;
pub const LINE_TOKENS: u32 = 16;
/// Jetons demandés au plus.
pub const MAX_TOKENS: u32 = 256;
/// Caractères du préfixe et du suffixe, au plus chacun.
pub const MAX_PREFIX: usize = 32_000;
pub const MAX_SUFFIX: usize = 16_000;
/// Température basse : la suite la plus probable.
const TEMPERATURE: f64 = 0.2;
/// Caractères par jeton comptés pour borner le préfixe (prudent : le code
/// en fait plutôt 3,5 à 4).
const CHARS_PER_TOKEN: usize = 3;
/// Jetons laissés aux jetons FIM et au gabarit.
const TEMPLATE_TOKENS: u32 = 64;
/// Lignes de même forme à la suite à partir desquelles une proposition est
/// vue comme une boucle du modèle (`int a = 0;`, `int b = 0;`…).
const LOOP_LINES: usize = 4;
/// Insécable de largeur nulle glissée après le premier caractère d'un jeton
/// spécial écrit dans le code : le modèle le lit comme du texte.
const BREAK: char = '\u{200B}';

/// Familles dont les jetons FIM sont connus.
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
    /// Prompt `raw` : préfixe, suffixe, puis le milieu à écrire.
    pub fn prompt(self, prefix: &str, suffix: &str) -> String {
        match self {
            Family::QwenCoder | Family::CodeGemma => {
                format!("<|fim_prefix|>{prefix}<|fim_suffix|>{suffix}<|fim_middle|>")
            }
            Family::StarCoder => format!("<fim_prefix>{prefix}<fim_suffix>{suffix}<fim_middle>"),
            Family::DeepSeekCoder => {
                format!("<｜fim▁begin｜>{prefix}<｜fim▁hole｜>{suffix}<｜fim▁end｜>")
            }
        }
    }

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

/// Jetons spéciaux de toutes les familles connues (FIM, fin de texte,
/// tours de chat).
fn special_tokens() -> impl Iterator<Item = &'static str> {
    [
        Family::QwenCoder,
        Family::StarCoder,
        Family::CodeGemma,
        Family::DeepSeekCoder,
    ]
    .into_iter()
    .flat_map(|family| family.stops().iter().copied())
}

/// Le code de l'utilisateur, ses jetons spéciaux neutralisés (un fichier
/// qui parle de FIM, comme celui-ci, ne casse pas la demande).
pub fn neutralize(code: &str) -> std::borrow::Cow<'_, str> {
    let mut text = std::borrow::Cow::Borrowed(code);
    for token in special_tokens() {
        if text.contains(token) {
            let mut chars = token.chars();
            let first = chars.next().unwrap_or_default();
            let broken = format!("{first}{BREAK}{}", chars.as_str());
            text = std::borrow::Cow::Owned(text.replace(token, &broken));
        }
    }
    text
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

/// Options figées d'un modèle d'autocomplétion : `num_ctx` (0 : rien
/// d'envoyé, défaut du serveur) et `keep_alive` (0 : rien d'envoyé).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fixed {
    pub num_ctx: u32,
    pub keep_alive: i64,
}

/// Comment demander le milieu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Way {
    /// `suffix` : le gabarit du modèle construit le prompt.
    Insert(Option<Family>),
    /// `raw` : jetons de la famille écrits ici.
    Raw(Family),
}

/// Manière de demander le milieu à ce modèle ; `None` : il ne sait pas.
pub fn way(model: &str, insert: bool) -> Option<Way> {
    let family = family(model);
    if insert {
        Some(Way::Insert(family))
    } else {
        family.map(Way::Raw)
    }
}

/// Manière de demander le milieu pour ce suffixe : Ollama n'applique le
/// gabarit FIM que si le suffixe n'est pas vide (sinon un modèle instruit
/// répondrait comme dans une conversation) ; suffixe vide et famille
/// connue : `raw` avec ses jetons.
pub fn way_for(way: Way, suffix: &str) -> Way {
    match way {
        Way::Insert(Some(family)) if suffix.is_empty() => Way::Raw(family),
        way => way,
    }
}

/// Corps de `/api/generate`, en flux ; jetons spéciaux du code neutralisés.
pub fn body(
    model: &str,
    prefix: &str,
    suffix: &str,
    way: Way,
    fixed: Fixed,
    tokens: u32,
    multiline: bool,
) -> Value {
    let family = match way {
        Way::Insert(family) => family,
        Way::Raw(family) => Some(family),
    };
    let mut stop: Vec<&str> = vec![if multiline { "\n\n" } else { "\n" }];
    if let Some(family) = family {
        stop.extend_from_slice(family.stops());
    }
    let mut options = json!({
        "num_predict": tokens,
        "temperature": TEMPERATURE,
        "stop": stop,
    });
    if fixed.num_ctx > 0 {
        options["num_ctx"] = json!(fixed.num_ctx);
    }
    let mut body = json!({
        "model": model,
        "stream": true,
        "think": false,
        "options": options,
    });
    let (prefix, suffix) = (neutralize(prefix), neutralize(suffix));
    match way {
        Way::Insert(_) => {
            body["prompt"] = json!(prefix);
            body["suffix"] = json!(suffix);
        }
        Way::Raw(family) => {
            body["prompt"] = json!(family.prompt(&prefix, &suffix));
            body["raw"] = json!(true);
        }
    }
    if fixed.keep_alive != 0 {
        body["keep_alive"] = json!(fixed.keep_alive);
    }
    body
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
    /// Du départ de la requête au premier jeton (ou à la fin s'il n'y en a
    /// aucun).
    pub first_token_ms: u64,
    pub total_ms: u64,
    /// Jetons du prompt repris du cache d'Ollama.
    pub cached_tokens: u64,
    /// Premier jeton pas arrivé à temps : abandonnée, texte vide.
    pub timed_out: bool,
}

/// Mesures d'une réponse finie (`/api/generate`, durées en ns).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
pub struct Durations {
    #[serde(default)]
    pub total_duration: u64,
    #[serde(default)]
    pub load_duration: u64,
    #[serde(default)]
    pub prompt_eval_count: u64,
    #[serde(default)]
    pub prompt_eval_cached_count: u64,
    #[serde(default)]
    pub prompt_eval_duration: u64,
    #[serde(default)]
    pub eval_count: u64,
    #[serde(default)]
    pub eval_duration: u64,
}

/// Échec d'une requête.
#[derive(Debug, PartialEq, Eq)]
pub enum FimFailure {
    /// Texte d'Ollama (ou de la connexion), à montrer tel quel.
    Failed(String),
}

fn failed(error: impl std::fmt::Display) -> FimFailure {
    FimFailure::Failed(error.to_string())
}

/// Envoie la requête et lit le flux ; lâcher le futur ferme la requête
/// (Ollama arrête de générer).
pub async fn complete(
    base: &str,
    body: &Value,
    multiline: bool,
    family: Option<Family>,
) -> Result<Completed, FimFailure> {
    let started = Instant::now();
    let deadline = tokio::time::Instant::from_std(started + FIRST_TOKEN);
    let late = || Completed {
        first_token_ms: millis(started.elapsed()),
        total_ms: millis(started.elapsed()),
        timed_out: true,
        ..Completed::default()
    };
    let url = client::server_url(base).map_err(failed)?;
    let http = client::client(&url, Some(TOTAL)).map_err(failed)?;
    // Ollama n'écrit les en-têtes qu'avec le premier jeton (après la file,
    // le chargement et la lecture du prompt) : le délai couvre l'envoi.
    // Lâcher l'envoi ferme la connexion ; Ollama saute une requête en file
    // dont le client est parti.
    let sending = http.post(format!("{base}/api/generate")).json(body).send();
    let mut response = match tokio::time::timeout_at(deadline, sending).await {
        Err(_) => return Ok(late()),
        Ok(sent) => sent.map_err(|error| failed(error.without_url()))?,
    };
    let status = response.status();
    if !status.is_success() {
        let text = response.text().await.unwrap_or_default();
        let message = crate::chat::error_line(&text);
        return Err(FimFailure::Failed(if message.trim().is_empty() {
            crate::i18n::t!("ollama.httpStatus", status = status.as_u16())
        } else {
            message
        }));
    }
    let mut lines = crate::chat::stream::LineBuffer::default();
    let mut text = String::new();
    let mut first: Option<u64> = None;
    let mut cached = 0;
    // Fin à la limite de jetons (`num_predict`) : dernière ligne inachevée.
    let mut truncated = false;
    'reading: loop {
        let chunk = if first.is_none() {
            match tokio::time::timeout_at(deadline, response.chunk()).await {
                Ok(chunk) => chunk,
                Err(_) => return Ok(late()),
            }
        } else {
            response.chunk().await
        };
        let Some(chunk) = chunk.map_err(|error| failed(error.without_url()))? else {
            break;
        };
        for line in lines.push(&chunk) {
            let value: Value = serde_json::from_str(&line).unwrap_or(Value::Null);
            if let Some(error) = value["error"].as_str() {
                return Err(FimFailure::Failed(error.to_owned()));
            }
            if let Some(piece) = value["response"].as_str()
                && !piece.is_empty()
            {
                first.get_or_insert_with(|| millis(started.elapsed()));
                text.push_str(piece);
            }
            if value["done"] == true {
                cached = value["prompt_eval_cached_count"].as_u64().unwrap_or(0);
                truncated = value["done_reason"] == "length";
                first.get_or_insert_with(|| millis(started.elapsed()));
                break 'reading;
            }
            // Coupe atteinte : inutile d'attendre la fin.
            if cut(&text, multiline, family).len() < text.trim_end_matches([' ', '\t', '\r']).len()
            {
                first.get_or_insert_with(|| millis(started.elapsed()));
                break 'reading;
            }
        }
    }
    Ok(Completed {
        text: finish(&text, multiline, family, truncated),
        first_token_ms: first.unwrap_or_else(|| millis(started.elapsed())),
        total_ms: millis(started.elapsed()),
        cached_tokens: cached,
        timed_out: false,
    })
}

/// Une requête de mesure, sans flux : durées lues dans la réponse
/// d'Ollama (indépendantes de la charge du pont).
pub async fn measured(base: &str, body: &Value) -> Result<Durations, FimFailure> {
    let url = client::server_url(base).map_err(failed)?;
    let http = client::client(&url, Some(Duration::from_secs(120))).map_err(failed)?;
    let mut body = body.clone();
    body["stream"] = json!(false);
    let response = http
        .post(format!("{base}/api/generate"))
        .json(&body)
        .send()
        .await
        .map_err(|error| failed(error.without_url()))?;
    let status = response.status();
    let text = response
        .text()
        .await
        .map_err(|error| failed(error.without_url()))?;
    if !status.is_success() {
        let message = crate::chat::error_line(&text);
        return Err(FimFailure::Failed(if message.trim().is_empty() {
            crate::i18n::t!("ollama.httpStatus", status = status.as_u16())
        } else {
            message
        }));
    }
    serde_json::from_str(&text).map_err(failed)
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

/// Premier jeton (ms) et vitesse (jetons/s) d'une réponse mesurée.
pub fn speed(durations: &Durations) -> (u64, f64) {
    // Premier jeton : chargement (presque nul, modèle en mémoire), lecture du
    // prompt, puis un jeton.
    let per_token = durations
        .eval_duration
        .checked_div(durations.eval_count)
        .unwrap_or(0);
    let first = durations.load_duration + durations.prompt_eval_duration + per_token;
    let tokens_per_second = if durations.eval_duration > 0 {
        durations.eval_count as f64 / (durations.eval_duration as f64 / 1e9)
    } else {
        0.0
    };
    (first / 1_000_000, tokens_per_second)
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

    #[test]
    fn raw_prompts_use_the_family_tokens() {
        assert_eq!(
            Family::QwenCoder.prompt("a", "b"),
            "<|fim_prefix|>a<|fim_suffix|>b<|fim_middle|>"
        );
        assert_eq!(
            Family::StarCoder.prompt("a", "b"),
            "<fim_prefix>a<fim_suffix>b<fim_middle>"
        );
        assert_eq!(
            Family::CodeGemma.prompt("a", "b"),
            "<|fim_prefix|>a<|fim_suffix|>b<|fim_middle|>"
        );
        assert_eq!(
            Family::DeepSeekCoder.prompt("a", "b"),
            "<｜fim▁begin｜>a<｜fim▁hole｜>b<｜fim▁end｜>"
        );
    }

    #[test]
    fn insert_is_preferred_then_raw_then_nothing() {
        assert_eq!(
            way("qwen2.5-coder:1.5b-base", true),
            Some(Way::Insert(Some(Family::QwenCoder)))
        );
        assert_eq!(way("mystery", true), Some(Way::Insert(None)));
        assert_eq!(
            way("qwen3-coder:30b", false),
            Some(Way::Raw(Family::QwenCoder))
        );
        assert_eq!(way("qwen3:0.6b", false), None);
    }

    #[test]
    fn the_body_keeps_fixed_options() {
        let fixed = Fixed {
            num_ctx: 4096,
            keep_alive: 600,
        };
        let insert = body(
            "qwen2.5-coder:1.5b-base",
            "def f(",
            ")\n",
            Way::Insert(Some(Family::QwenCoder)),
            fixed,
            64,
            true,
        );
        assert_eq!(insert["prompt"], "def f(");
        assert_eq!(insert["suffix"], ")\n");
        assert_eq!(insert["think"], false);
        assert_eq!(insert["stream"], true);
        assert_eq!(insert["keep_alive"], 600);
        assert_eq!(insert["options"]["num_ctx"], 4096);
        assert_eq!(insert["options"]["num_predict"], 64);
        assert_eq!(insert["options"]["stop"][0], "\n\n");
        assert!(insert.get("raw").is_none());
        let raw = body(
            "starcoder2:3b",
            "x = ",
            "",
            Way::Raw(Family::StarCoder),
            Fixed {
                num_ctx: 0,
                keep_alive: 0,
            },
            16,
            false,
        );
        assert_eq!(raw["prompt"], "<fim_prefix>x = <fim_suffix><fim_middle>");
        assert_eq!(raw["raw"], true);
        assert!(raw.get("suffix").is_none());
        // 0 : rien d'envoyé (défaut du serveur, comme la conversation).
        assert!(raw.get("keep_alive").is_none());
        assert!(raw["options"].get("num_ctx").is_none());
        assert_eq!(raw["options"]["stop"][0], "\n");
        assert!(
            raw["options"]["stop"]
                .as_array()
                .unwrap()
                .contains(&json!("<fim_middle>"))
        );
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

    /// Rapport 92, m1 : un jeton spécial écrit dans le code ne casse pas la
    /// demande, dans les deux manières.
    #[test]
    fn special_tokens_in_the_code_are_neutralized() {
        let code = "s = '<|fim_middle|>' + \"<fim_prefix>\" + '<｜fim▁hole｜>'";
        let raw = body(
            "qwen2.5-coder:1.5b-base",
            code,
            "<|endoftext|>",
            Way::Raw(Family::QwenCoder),
            Fixed {
                num_ctx: 4096,
                keep_alive: 240,
            },
            16,
            false,
        );
        let prompt = raw["prompt"].as_str().unwrap();
        assert_eq!(prompt.matches("<|fim_middle|>").count(), 1, "{prompt}");
        assert!(prompt.ends_with("<|fim_middle|>"));
        assert_eq!(prompt.matches("<|fim_suffix|>").count(), 1);
        for token in ["<fim_prefix>", "<｜fim▁hole｜>", "<|endoftext|>"] {
            assert!(!prompt.contains(token), "{token} in {prompt}");
        }
        let insert = body(
            "chatonlm-fim",
            code,
            "<|endoftext|>",
            Way::Insert(None),
            Fixed {
                num_ctx: 4096,
                keep_alive: 240,
            },
            16,
            false,
        );
        let prompt = insert["prompt"].as_str().unwrap();
        assert!(
            special_tokens().all(|token| !prompt.contains(token)),
            "{prompt}"
        );
        assert!(!insert["suffix"].as_str().unwrap().contains("<|endoftext|>"));
        // Le texte reste lisible : seul un caractère invisible est ajouté.
        assert_eq!(prompt.replace(BREAK, ""), code);
        assert_eq!(neutralize("plain code"), "plain code");
    }

    /// Rapport 92, m2 : suffixe vide, Ollama n'appliquerait pas le gabarit
    /// FIM ; famille connue : `raw`.
    #[test]
    fn an_empty_suffix_goes_raw_when_the_family_is_known() {
        let qwen = Way::Insert(Some(Family::QwenCoder));
        assert_eq!(way_for(qwen, ""), Way::Raw(Family::QwenCoder));
        assert_eq!(way_for(qwen, "\n"), qwen);
        assert_eq!(way_for(Way::Insert(None), ""), Way::Insert(None));
        assert_eq!(
            way_for(Way::Raw(Family::StarCoder), ""),
            Way::Raw(Family::StarCoder)
        );
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
        let durations = Durations {
            load_duration: 1_000_000,
            prompt_eval_duration: 60_000_000,
            eval_count: 40,
            eval_duration: 100_000_000,
            ..Durations::default()
        };
        let (first, speed) = speed(&durations);
        // 1 + 60 + 2,5 ms.
        assert_eq!(first, 63);
        assert!((speed - 400.0).abs() < 1e-9);
        assert_eq!(rate(first, speed), Rating::Smooth);
        assert_eq!(rate(400, 100.0), Rating::Usable);
        assert_eq!(rate(1000, 20.0), Rating::TooSlow);
        assert_eq!(rate(10, 0.0), Rating::TooSlow);
        let (prefix, suffix) = measure_prompt(1);
        assert!(prefix.ends_with("def average_1(values):\n    "));
        assert!(prefix.len() > 3_000, "{}", prefix.len());
        assert!(suffix.contains("__main__"));
        assert_ne!(measure_prompt(2).0, prefix);
    }
}
