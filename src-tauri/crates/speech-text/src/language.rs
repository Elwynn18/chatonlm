//! Langue d'une réponse, pour choisir la voix.
//!
//! Détection sur le début du texte parlé (600 caractères) par les mots
//! outils (articles, prépositions, pronoms, auxiliaires) du français, de
//! l'anglais et de l'espagnol, et de quatre langues proches qui ne sont pas
//! lues (allemand, italien, portugais, néerlandais) ; une écriture non
//! latine majoritaire est une autre langue. Repli sur la langue de
//! l'interface quand le texte est trop court ou ambigu.
//!
//! Choix d'une heuristique plutôt que `whatlang` (trigrammes, 69 langues) :
//! pas de dépendance ni de tables de plusieurs centaines de Ko, et une réponse
//! technique française pleine de termes anglais (« npm run dev »,
//! « timeout », « streaming ») reste française, puisque seuls les mots outils
//! comptent ; trois langues à reconnaître, le reste n'a qu'à être « autre ».

use crate::Language;

/// Résultat de la détection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Detected {
    /// Une langue lue par une voix.
    Voice(Language),
    /// Une autre langue, détectée avec assurance (allemand, japonais…).
    Other,
    /// Trop court ou ambigu : prendre la langue de l'interface.
    Unknown,
}

/// Caractères examinés au début du texte.
const SAMPLE_CHARS: usize = 600;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Lang {
    Fr,
    En,
    Es,
    De,
    It,
    Pt,
    Nl,
}

const LANGS: [Lang; 7] = [
    Lang::Fr,
    Lang::En,
    Lang::Es,
    Lang::De,
    Lang::It,
    Lang::Pt,
    Lang::Nl,
];

/// Mots outils par langue ; `x'` : élision (« l'erreur »), `'x` : fin de
/// contraction (« don't »).
fn function_words(lang: Lang) -> &'static [&'static str] {
    match lang {
        Lang::Fr => &[
            "le", "la", "les", "des", "du", "de", "un", "une", "et", "est", "sont", "pas", "ne",
            "ce", "cette", "ces", "qui", "que", "dans", "pour", "avec", "sur", "au", "aux", "il",
            "elle", "ils", "elles", "nous", "vous", "tu", "je", "on", "mais", "ou", "où", "plus",
            "très", "aussi", "comme", "être", "avoir", "fait", "peut", "si", "son", "sa", "ses",
            "leur", "tout", "tous", "ça", "à", "y", "en", "se", "me", "te", "lui", "été", "était",
            "sans", "alors", "donc", "car", "déjà", "encore", "bien", "oui", "non", "voici",
            "voilà", "chaque", "entre", "puis", "ton", "ta", "tes", "votre", "vos", "notre", "l'",
            "d'", "j'", "c'", "n'", "s'", "qu'", "m'", "t'", "jusqu'", "lorsqu'", "puisqu'",
        ],
        Lang::En => &[
            "the", "a", "an", "and", "is", "are", "was", "were", "be", "been", "of", "to", "in",
            "on", "at", "for", "with", "that", "this", "these", "those", "it", "its", "you",
            "your", "we", "they", "he", "she", "i", "not", "but", "or", "as", "by", "from", "have",
            "has", "had", "will", "would", "can", "could", "should", "do", "does", "did", "there",
            "their", "what", "which", "who", "when", "if", "then", "than", "so", "all", "also",
            "just", "more", "most", "about", "into", "how", "here", "yes", "no", "my", "our", "'s",
            "'t", "'re", "'ll", "'ve", "'m", "'d",
        ],
        Lang::Es => &[
            "el", "la", "los", "las", "de", "del", "y", "que", "en", "es", "son", "un", "una",
            "unos", "unas", "por", "para", "con", "sin", "no", "se", "lo", "al", "como", "pero",
            "más", "muy", "también", "este", "esta", "estos", "estas", "ese", "esa", "su", "sus",
            "le", "les", "me", "te", "nos", "yo", "tú", "él", "ella", "ellos", "usted", "hay",
            "está", "están", "ser", "puede", "cuando", "donde", "porque", "si", "sí", "ya", "todo",
            "todos", "entre", "sobre", "hasta", "desde", "tu", "tus", "mi", "mis",
        ],
        Lang::De => &[
            "der", "die", "das", "und", "ist", "nicht", "ein", "eine", "einen", "dem", "den",
            "des", "zu", "mit", "auf", "für", "von", "sich", "auch", "es", "ich", "sie", "wir",
            "ihr", "du", "er", "wird", "werden", "sind", "war", "kann", "oder", "aber", "wie",
            "wenn", "dass", "noch", "nur", "schon", "bei", "nach", "aus", "im", "am", "zum", "zur",
            "über",
        ],
        Lang::It => &[
            "il", "lo", "la", "gli", "le", "di", "del", "della", "dei", "delle", "che", "e", "è",
            "un", "una", "uno", "per", "con", "non", "sono", "si", "come", "anche", "ma", "più",
            "questo", "questa", "nel", "nella", "al", "alla", "da", "dal", "ci", "mi", "ti", "ha",
            "hanno", "essere", "molto", "perché", "quando", "se", "tutto", "cosa", "l'", "dell'",
            "all'", "nell'", "un'", "c'",
        ],
        Lang::Pt => &[
            "o", "os", "a", "as", "de", "do", "da", "dos", "das", "e", "é", "um", "uma", "que",
            "não", "em", "no", "na", "nos", "nas", "para", "com", "por", "se", "mais", "como",
            "mas", "ao", "você", "isso", "este", "esta", "também", "muito", "são", "está", "ser",
            "tem", "pelo", "pela", "já", "ou", "seu", "sua", "quando", "onde", "porque",
        ],
        Lang::Nl => &[
            "de", "het", "een", "en", "is", "van", "dat", "die", "niet", "op", "te", "met", "voor",
            "zijn", "ik", "je", "we", "er", "maar", "ook", "als", "dan", "bij", "aan", "om", "nog",
            "wat", "naar", "hoe", "wordt", "kan", "deze", "dit",
        ],
    }
}

/// Mots outils d'une langue lue (sans les formes élidées), pour le
/// prétraitement : un mot en capitales qui en fait partie se lit comme un mot.
pub(crate) fn common_words(language: Language) -> &'static [&'static str] {
    function_words(match language {
        Language::Fr => Lang::Fr,
        Language::En => Lang::En,
        Language::Es => Lang::Es,
    })
}

/// Lettre d'une écriture latine (ASCII, Latin-1, Latin étendu).
fn is_latin(c: char) -> bool {
    c.is_ascii_alphabetic() || matches!(c as u32, 0xC0..=0x24F | 0x1E00..=0x1EFF)
}

/// Mots qui s'élident devant une apostrophe (« l'erreur », « dell'acqua »).
const ELISIONS: &[&str] = &[
    "l", "d", "j", "c", "n", "s", "qu", "m", "t", "jusqu", "lorsqu", "puisqu", "dell", "all",
    "nell", "un",
];

/// Mots en minuscules ; entre deux lettres, une apostrophe colle « l' » au
/// mot élidé qui la précède, sinon « 's » au mot qui la suit (contraction).
fn tokens(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut word = String::new();
    let mut contraction = false;
    let chars: Vec<char> = text.chars().collect();
    for (i, &c) in chars.iter().enumerate() {
        if c.is_alphabetic() {
            if word.is_empty() && contraction {
                word.push('\'');
            }
            word.extend(c.to_lowercase());
            continue;
        }
        let inner_apostrophe = matches!(c, '\'' | '’')
            && chars.get(i + 1).is_some_and(|c| c.is_alphabetic())
            && !word.is_empty();
        contraction = false;
        if inner_apostrophe {
            if ELISIONS.contains(&word.as_str()) {
                word.push('\'');
            } else {
                contraction = true;
            }
        }
        if !word.is_empty() {
            out.push(std::mem::take(&mut word));
        }
    }
    if !word.is_empty() {
        out.push(word);
    }
    out
}

/// Langue de `spoken` (texte parlé, déjà tiré du Markdown).
pub fn detect(spoken: &str) -> Detected {
    // Une citation dans une autre langue (« "To be or not to be" est une
    // citation de Hamlet ») ne compte pas, sauf s'il ne reste presque rien.
    let unquoted = without_quotes(spoken);
    let source = if unquoted.chars().filter(|c| c.is_alphabetic()).count() >= 20 {
        &unquoted
    } else {
        spoken
    };
    let sample: String = source.chars().take(SAMPLE_CHARS).collect();
    let (latin, other) = sample
        .chars()
        .filter(|c| c.is_alphabetic())
        .fold(
            (0, 0),
            |(l, o), c| {
                if is_latin(c) { (l + 1, o) } else { (l, o + 1) }
            },
        );
    if other >= 8 && other > latin {
        return Detected::Other;
    }
    // Chaque mot outil vaut 1, partagé entre les langues qui l'emploient.
    let mut scores = [0.0f32; LANGS.len()];
    for token in tokens(&sample) {
        let langs: Vec<usize> = (0..LANGS.len())
            .filter(|&i| function_words(LANGS[i]).contains(&token.as_str()))
            .collect();
        for &i in &langs {
            scores[i] += 1.0 / langs.len() as f32;
        }
    }
    let mut ranked: Vec<usize> = (0..LANGS.len()).collect();
    ranked.sort_by(|&a, &b| scores[b].total_cmp(&scores[a]));
    let (best, second) = (scores[ranked[0]], scores[ranked[1]]);
    if best < 1.5 || best < 2.0 * second {
        return Detected::Unknown;
    }
    match LANGS[ranked[0]] {
        Lang::Fr => Detected::Voice(Language::Fr),
        Lang::En => Detected::Voice(Language::En),
        Lang::Es => Detected::Voice(Language::Es),
        _ => Detected::Other,
    }
}

/// Texte sans ce qui est entre guillemets (« », “ ”, " ").
fn without_quotes(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut closing: Option<char> = None;
    for c in text.chars() {
        match closing {
            Some(close) if c == close => closing = None,
            Some(_) => {}
            None => match c {
                '«' => closing = Some('»'),
                '“' => closing = Some('”'),
                '"' => closing = Some('"'),
                _ => out.push(c),
            },
        }
    }
    // Guillemet jamais fermé : rien n'est retiré.
    if closing.is_some() {
        text.to_owned()
    } else {
        out
    }
}

/// Voix à utiliser : la langue détectée, sinon `interface` ; `None` : autre
/// langue (lecture refusée).
pub fn choose(detected: Detected, interface: Language) -> Option<Language> {
    match detected {
        Detected::Voice(language) => Some(language),
        Detected::Unknown => Some(interface),
        Detected::Other => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn voice(language: Language) -> Detected {
        Detected::Voice(language)
    }

    #[test]
    fn plain_paragraphs() {
        assert_eq!(
            detect(
                "Bonne question ! En résumé, oui : tu peux utiliser ce modèle hors ligne, sans compte et sans envoyer tes données sur Internet."
            ),
            voice(Language::Fr)
        );
        assert_eq!(
            detect(
                "Great question! In short, yes: you can run this model offline, without an account and without sending your data to the internet."
            ),
            voice(Language::En)
        );
        assert_eq!(
            detect(
                "¡Buena pregunta! En resumen, sí: puedes usar este modelo sin conexión, sin cuenta y sin enviar tus datos a internet."
            ),
            voice(Language::Es)
        );
    }

    #[test]
    fn technical_answers_mixed_with_english_terms() {
        assert_eq!(
            detect(
                "Pour corriger l'erreur, ouvre le fichier de configuration et règle le paramètre timeout sur trente secondes. Relance ensuite le serveur avec npm run dev, puis vérifie que le streaming fonctionne."
            ),
            voice(Language::Fr)
        );
        assert_eq!(
            detect(
                "Para corregir el error, abre el archivo de configuración y cambia el valor del parámetro timeout. Después, reinicia el servidor con npm run dev."
            ),
            voice(Language::Es)
        );
        assert_eq!(
            detect(
                "Run npm install, then npm run dev. If the build fails, check that the timeout is set and that streaming is enabled in your config."
            ),
            voice(Language::En)
        );
        // Beaucoup de termes anglais, peu de mots outils : toujours du français.
        assert_eq!(
            detect(
                "Le backend Rust expose un endpoint REST ; le frontend React utilise TanStack Query avec un cache et du streaming SSE."
            ),
            voice(Language::Fr)
        );
    }

    #[test]
    fn other_languages() {
        assert_eq!(
            detect(
                "Das ist eine gute Frage. Du kannst das Modell auch ohne Internet nutzen, und es ist nicht nötig, ein Konto zu haben."
            ),
            Detected::Other
        );
        assert_eq!(
            detect(
                "È una buona domanda. Puoi usare questo modello anche senza connessione, e non è necessario avere un account per farlo."
            ),
            Detected::Other
        );
        assert_eq!(
            detect(
                "É uma boa pergunta. Você pode usar este modelo sem conexão, e não é preciso ter uma conta para isso."
            ),
            Detected::Other
        );
        assert_eq!(
            detect("これは良い質問です。このモデルはオフラインでも使えます。"),
            Detected::Other
        );
        assert_eq!(
            detect("Это хороший вопрос. Модель работает без подключения к интернету."),
            Detected::Other
        );
    }

    #[test]
    fn short_or_ambiguous_text() {
        assert_eq!(detect(""), Detected::Unknown);
        assert_eq!(detect("Oui."), Detected::Unknown);
        assert_eq!(detect("OK"), Detected::Unknown);
        assert_eq!(detect("42"), Detected::Unknown);
        assert_eq!(detect("La la la."), Detected::Unknown);
        assert_eq!(detect("npm run dev"), Detected::Unknown);
        assert_eq!(detect("はい"), Detected::Unknown);
        // Deux mots outils sans concurrent suffisent.
        assert_eq!(detect("Yes, of course."), voice(Language::En));
        assert_eq!(detect("Oui, bien sûr."), voice(Language::Fr));
    }

    #[test]
    fn only_the_beginning_counts() {
        let french =
            "Voici la réponse que tu attendais, avec les détails dans la suite. ".repeat(10);
        let english =
            "This is the part that comes after, and it is written in English. ".repeat(20);
        assert_eq!(detect(&format!("{french}{english}")), voice(Language::Fr));
    }

    #[test]
    fn elisions_and_contractions() {
        assert_eq!(
            tokens("L'API d’OpenAI don't"),
            ["l'", "api", "d'", "openai", "don", "'t"]
        );
        assert_eq!(detect("C'est l'heure qu'il faut."), voice(Language::Fr));
        assert_eq!(
            detect("It's what you'd expect, isn't it?"),
            voice(Language::En)
        );
    }

    #[test]
    fn quotations_do_not_count() {
        // Constat R3-20 : une citation anglaise en tête d'une réponse française.
        assert_eq!(
            detect(
                "\"To be or not to be, that is the question\" est une citation de Hamlet, la pièce de Shakespeare."
            ),
            voice(Language::Fr)
        );
        assert_eq!(
            detect(
                "Il a répondu « I don't know what you are talking about, and it is not my job » puis il est parti sans un mot."
            ),
            voice(Language::Fr)
        );
        // Tout entre guillemets : la citation compte.
        assert_eq!(
            detect("« This is the answer that you were looking for, and it is in English. »"),
            voice(Language::En)
        );
        // Guillemet jamais fermé : rien n'est retiré.
        assert_eq!(without_quotes("a \"b c"), "a \"b c");
    }

    #[test]
    fn interface_fallback() {
        assert_eq!(choose(Detected::Unknown, Language::Es), Some(Language::Es));
        assert_eq!(
            choose(voice(Language::En), Language::Fr),
            Some(Language::En)
        );
        assert_eq!(choose(Detected::Other, Language::Fr), None);
    }
}
