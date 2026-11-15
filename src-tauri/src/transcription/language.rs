//! Langue d'une transcription devinée sur son texte, seulement quand elle
//! est sûre (relecture 107) : transcribe-rs ne rend pas la langue que
//! Whisper détecte lui-même (`whisper_full_lang_id` : état privé de son
//! moteur).
//!
//! Même principe que `proofread::language` (mots outils propres à une
//! langue), mais avec bien plus de langues concurrentes : sans elles, le
//! néerlandais, le danois, le suédois ou le norvégien (« i », « is », « at »,
//! « for », « her »…) passaient pour de l'anglais. Seules les six langues
//! de la correction peuvent être rendues ; les autres ne servent qu'à
//! retirer leurs mots communs et à faire douter. Un texte écrit surtout hors
//! de l'alphabet latin, trop court, ou sans majorité très nette : `None`
//! (la langue reste « auto »).
//!
//! Module sans dépendance (essayé aussi hors de cargo).

use std::collections::HashMap;
use std::sync::LazyLock;

/// Langue d'un mot outil. Les variantes sans code ne sont jamais rendues.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Lang {
    English,
    French,
    Spanish,
    German,
    Italian,
    Portuguese,
    Dutch,
    /// Danois, norvégien et suédois, trop proches pour être séparés ici.
    Nordic,
    Catalan,
    Polish,
    Romanian,
    /// Tchèque et slovaque.
    Czech,
    Finnish,
    Turkish,
    /// Indonésien et malais.
    Indonesian,
    Hungarian,
}

impl Lang {
    /// Code rendu (langues de la correction seulement).
    fn code(self) -> Option<&'static str> {
        Some(match self {
            Lang::English => "en",
            Lang::French => "fr",
            Lang::Spanish => "es",
            Lang::German => "de",
            Lang::Italian => "it",
            Lang::Portuguese => "pt",
            _ => return None,
        })
    }
}

/// Mots outils courants ; un mot cité par plusieurs langues ne compte pour
/// aucune (c'est ce qui retire « i », « is », « at »… de l'anglais).
const STOPWORDS: &[(Lang, &str)] = &[
    (
        Lang::English,
        "the and is are was were you your this that with for have has not what does do i my we \
         they be will can from but there their would should could been which about it's don't \
         i'm it he she him her them our us how when where why who an of to in on at by or if so \
         no just very too going want think know because these those than then",
    ),
    (
        Lang::French,
        "le les des est et je tu il elle nous vous ils elles une du au aux pour pas ne qui dans \
         sur avec ce cette ces mais ou où mon ma mes ton ta tes son sa ses notre votre leur leurs \
         sont être très ça c j l d qu n s ai as avez été fait beaucoup aussi plus moins bien \
         toujours jamais déjà encore comme parce quand alors donc chez vers sans sous peut faut",
    ),
    (
        Lang::Spanish,
        "el los las es y yo tú una del al por para con pero muy está están son mi mis su sus \
         como qué cómo hay lo se también cuando porque esta este mucho mucha muchos muchas donde \
         dónde cuál quién nosotros ellos ella puedes puede tengo tiene estoy fue fuimos era había \
         habia sin sobre ya aquí ahora hace eso bien",
    ),
    (
        Lang::German,
        "der die das und ist nicht ich du sie wir ein eine mit für auf zu den dem von auch sich \
         es sind war haben wird kann ihr mein dein unser noch nur schon wenn oder aber so in \
         einen einem dass hat werden wie",
    ),
    (
        Lang::Italian,
        "il lo gli della di e è che non per sono con mi ti ci questo questa ma anche come perché \
         nel alla dello sei siamo hanno molto in",
    ),
    (
        Lang::Portuguese,
        "o os as um uma é não do da dos das em no na com para por você eu ele ela mas muito \
         também isso e são está estão como sobre era foi tem ter eles elas pelo pela nisso \
         disso ao aos às mesmo",
    ),
    (
        Lang::Dutch,
        "de het een van en is in op te dat die niet voor met zijn er aan ook als maar om bij nog \
         wat naar kan worden wordt dit uit hij ze zij wij jij ik u mijn hun geen heeft hebben was \
         waren werd zo dan toch nu hier daar wel heel veel door tot deze dus omdat want welke wie \
         waar hoe je",
    ),
    (
        Lang::Nordic,
        "och att det som en ett är på för med av till den inte jag har de om så men var vi kan \
         du han hon eller ska skal vil vill også också og ikke jeg er at af et der fra hvad vad \
         hvor være vara blev bliver blir mig meg dig deg sig seg nu nå når när her här där meget \
         mycket mye bare kun noget något noe i for fordi eftersom efter etter kommer skulle hade \
         havde hadde",
    ),
    (
        Lang::Catalan,
        "els les i del per amb una és no són aquest aquesta això però molt també com més ha \
         han hi ho seu seva jo ell nosaltres vosaltres ells està estan perquè quan on ja ara aquí \
         fer pot doncs al el un la",
    ),
    (
        Lang::Polish,
        "się nie jest że to na w z do co jak ale tak czy już być był była było są mnie ja ty on \
         ona my wy oni tylko jeszcze bardzo może przez dla tego jego jej który która które gdzie \
         kiedy bo też",
    ),
    (
        Lang::Romanian,
        "și în la de cu pe nu este un o care mai ce sunt din pentru că dar fost foarte acest \
         această așa cum tu el ea noi voi ele sau sa se lui",
    ),
    (
        Lang::Czech,
        "je se na to že v a s z do jsem jsi jsou byl byla bylo není ale jak tak co jen už také \
         který která které když protože pro od po by být mám máme jsme ještě velmi",
    ),
    (
        Lang::Finnish,
        "ja on ei se että oli ovat olen olet hän he me te minä sinä mutta kuin niin myös vain kun \
         jos tai mitä mikä joka tämä tuo nyt sitten vielä hyvin paljon",
    ),
    (
        Lang::Turkish,
        "ve bir bu da de için ile çok ama ne gibi daha olarak var yok ben sen o biz siz onlar şey \
         değil kadar sonra çünkü mi mı mu",
    ),
    (
        Lang::Indonesian,
        "dan yang di ini itu dengan untuk tidak ada dari ke akan pada juga saya kami kita mereka \
         anda dia sudah bisa atau karena tetapi lebih sangat",
    ),
    (
        Lang::Hungarian,
        "a az és hogy nem is egy van volt meg de csak már még mint vagy ez azt ami amit így \
         nagyon lesz kell",
    ),
];

/// Mot outil → langue, pour les mots d'une seule langue.
static EXCLUSIVE: LazyLock<HashMap<&'static str, Lang>> = LazyLock::new(|| {
    let mut languages: HashMap<&'static str, Vec<Lang>> = HashMap::new();
    for (language, words) in STOPWORDS {
        for word in words.split_whitespace() {
            let seen = languages.entry(word).or_default();
            if !seen.contains(language) {
                seen.push(*language);
            }
        }
    }
    languages
        .into_iter()
        .filter_map(|(word, languages)| match languages[..] {
            [language] => Some((word, language)),
            _ => None,
        })
        .collect()
});

/// Mots au moins pour juger.
const MIN_WORDS: usize = 20;
/// Mots outils de la langue au moins…
const MIN_HITS: usize = 6;
/// … et au moins cette part des mots (pour cent) : une langue absente des
/// listes ne donne que quelques mots isolés.
const MIN_SHARE_PERCENT: usize = 10;
/// Avance sur la langue suivante (fois).
const LEAD: usize = 4;
/// Part des lettres latines (pour cent) au moins.
const MIN_LATIN_PERCENT: usize = 90;

/// Lettre de l'alphabet latin (accents compris).
fn is_latin(c: char) -> bool {
    c.is_ascii_alphabetic()
        || matches!(c, '\u{00C0}'..='\u{024F}' | '\u{1E00}'..='\u{1EFF}')
            && c != '\u{00D7}'
            && c != '\u{00F7}'
}

/// Code de la langue de `text` si elle est sûre (voir l'en-tête), sinon
/// `None`.
pub fn sure(text: &str) -> Option<&'static str> {
    let letters = text.chars().filter(|c| c.is_alphabetic()).count();
    let latin = text.chars().filter(|c| is_latin(*c)).count();
    if letters == 0 || latin * 100 < letters * MIN_LATIN_PERCENT {
        return None;
    }
    let lowered = text.to_lowercase();
    let mut words = 0usize;
    let mut hits: HashMap<Lang, usize> = HashMap::new();
    let tokens = lowered
        .split(|c: char| !(c.is_alphanumeric() || c == '\'' || c == '’'))
        .map(|token| token.replace('’', "'"));
    for token in tokens {
        let token = token.trim_matches('\'');
        if token.is_empty() || !token.chars().any(char::is_alphabetic) {
            continue;
        }
        words += 1;
        // « don't » entier, sinon « l'école » en « l » et « école ».
        if let Some(language) = EXCLUSIVE.get(token) {
            *hits.entry(*language).or_default() += 1;
        } else if token.contains('\'') {
            for part in token.split('\'') {
                if let Some(language) = EXCLUSIVE.get(part) {
                    *hits.entry(*language).or_default() += 1;
                }
            }
        }
    }
    if words < MIN_WORDS {
        return None;
    }
    let mut scores: Vec<(usize, Lang)> = hits.into_iter().map(|(l, n)| (n, l)).collect();
    scores.sort_by_key(|score| std::cmp::Reverse(score.0));
    let (best, language) = *scores.first()?;
    let second = scores.get(1).map_or(0, |score| score.0);
    let sure =
        best >= MIN_HITS && best * 100 >= words * MIN_SHARE_PERCENT && best >= LEAD * second.max(1);
    if sure { language.code() } else { None }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Phrases ordinaires d'une quarantaine de mots, comme un morceau de
    /// transcription.
    pub(crate) const SAMPLES: &[(&str, Option<&str>)] = &[
        (
            "Hello everyone, today we are going to talk about the history of copyright and how \
             it has changed over the last century. The first thing you need to know is that the \
             law was very different, and most people did not think about it at all.",
            Some("en"),
        ),
        (
            "Bonjour à tous, aujourd'hui nous allons parler de l'histoire du droit d'auteur et de \
             la façon dont il a changé au cours du siècle dernier. La première chose à savoir, \
             c'est que la loi était très différente, et que la plupart des gens n'y pensaient pas.",
            Some("fr"),
        ),
        (
            "Hola a todos, hoy vamos a hablar de la historia de los derechos de autor y de cómo \
             han cambiado durante el último siglo. Lo primero que hay que saber es que la ley era \
             muy diferente, y que la mayoría de las personas no pensaba en ella.",
            Some("es"),
        ),
        (
            "Hallo zusammen, heute sprechen wir über die Geschichte des Urheberrechts und wie es \
             sich im letzten Jahrhundert verändert hat. Das Erste, was man wissen muss, ist, dass \
             das Gesetz damals ganz anders war und die meisten Menschen nicht daran dachten.",
            Some("de"),
        ),
        (
            "Ciao a tutti, oggi parliamo della storia del diritto d'autore e di come è cambiato \
             nel corso dell'ultimo secolo. La prima cosa da sapere è che la legge era molto \
             diversa, e che la maggior parte delle persone non ci pensava per niente.",
            Some("it"),
        ),
        (
            "Olá a todos, hoje vamos falar sobre a história dos direitos autorais e como eles \
             mudaram ao longo do último século. A primeira coisa que você precisa saber é que a \
             lei era muito diferente, e que a maioria das pessoas não pensava nisso.",
            Some("pt"),
        ),
        (
            "Ik heb gisteren met mijn broer gesproken en hij zegt dat het weer in Amsterdam niet \
             zo goed is, maar dat de stad nog steeds erg mooi is. We gaan er volgende week samen \
             naartoe, want hij wil het museum zien en ik wil vooral wandelen.",
            None,
        ),
        (
            "Jag har pratat med min bror i går och han säger att vädret i Stockholm inte är så \
             bra, men att staden fortfarande är mycket vacker. Vi ska åka dit tillsammans nästa \
             vecka, för han vill se museet och jag vill mest promenera.",
            None,
        ),
        (
            "Jeg har talt med min bror i går, og han siger at vejret i København ikke er så godt, \
             men at byen stadig er meget smuk. Vi skal tage derhen sammen i næste uge, for han vil \
             se museet og jeg vil mest gå en tur.",
            None,
        ),
        (
            "Jeg har snakket med broren min i går, og han sier at været i Oslo ikke er så bra, men \
             at byen fortsatt er veldig fin. Vi skal reise dit sammen neste uke, for han vil se \
             museet og jeg vil mest gå en tur.",
            None,
        ),
        (
            "Doncs jo crec que el que hem de fer és parlar amb ells i veure què volen fer amb el \
             projecte, perquè si no ho fem ara després serà massa tard i no podrem canviar res \
             del que ja està decidit.",
            None,
        ),
        (
            "Wczoraj rozmawiałem z moim bratem i mówi, że pogoda w Warszawie nie jest zbyt dobra, \
             ale miasto jest nadal bardzo piękne. Pojedziemy tam razem w przyszłym tygodniu, bo \
             on chce zobaczyć muzeum, a ja chcę głównie spacerować.",
            None,
        ),
        (
            "Ieri am vorbit cu fratele meu și spune că vremea în București nu este prea bună, dar \
             că orașul este încă foarte frumos. Vom merge acolo împreună săptămâna viitoare, \
             pentru că el vrea să vadă muzeul și eu vreau mai ales să mă plimb.",
            None,
        ),
        (
            "Eilen puhuin veljeni kanssa ja hän sanoo, että sää Helsingissä ei ole kovin hyvä, \
             mutta kaupunki on silti hyvin kaunis. Menemme sinne yhdessä ensi viikolla, koska hän \
             haluaa nähdä museon ja minä haluan lähinnä kävellä.",
            None,
        ),
        (
            "Вчера я говорил с братом, и он говорит, что погода в Москве не очень хорошая, но \
             город всё ещё очень красивый. Мы поедем туда вместе на следующей неделе.",
            None,
        ),
    ];

    #[test]
    fn only_a_sure_language_is_given() {
        for (text, expected) in SAMPLES {
            assert_eq!(sure(text), *expected, "{text}");
        }
    }

    #[test]
    fn short_or_foreign_texts_stay_unknown() {
        assert_eq!(sure("ChatonLM"), None);
        assert_eq!(
            sure("The cat and the children are in the house with us."),
            None
        );
        assert_eq!(sure(""), None);
        assert_eq!(sure("12 34 56 78"), None);
        // Mélange : ni l'une ni l'autre n'a une avance nette.
        let mixed = format!("{} {}", SAMPLES[0].0, SAMPLES[1].0);
        assert_eq!(sure(&mixed), None);
    }
}
