//! Essais du prétraitement français : les `CASES` du prototype (sortie
//! identique), puis des cas tirés du prototype (sortie relevée par
//! `normalize_fr.py`) et les écarts voulus.

use super::{cardinal, normalize, ordinal};

/// `CASES` de `.tools/tts-bench/bin/normalize_fr.py`, recopiés tels quels.
const PROTOTYPE_CASES: &[(&str, &str)] = &[
    (
        "Le 9 octobre 2026, la version 0.9.0 pesait 3,5 Go.",
        "Le neuf octobre deux mille vingt-six, la version zéro point neuf point zéro pesait trois virgule cinq gigaoctets.",
    ),
    (
        "Comptez environ 15 € par mois, soit 20 % de moins qu'en 2025.",
        "Comptez environ quinze euros par mois, soit vingt pour cent de moins qu'en deux mille vingt-cinq.",
    ),
    (
        "L'API REST répond en 120 ms sur un GPU NVIDIA.",
        "L'A P I rest répond en cent vingt millisecondes sur un G P U ènvidia.",
    ),
    (
        "Le RER B part à 14h30 du quai n°3 : comptez 2,50 € et 1 h 15.",
        "Le R E R B part à quatorze heures trente du quai numéro trois : comptez deux euros cinquante et une heure quinze.",
    ),
    (
        "21 minutes, 1 seconde, 1,5 heure, 80 ans, 71 jours, 91 %, 200 000 €.",
        "vingt et une minutes, une seconde, une virgule cinq heure, quatre-vingts ans, soixante et onze jours, quatre-vingt-onze pour cent, deux cent mille euros.",
    ),
    (
        "Le 1er janvier, M. Dupont lit le chapitre II sur Louis XIV et le XXIe siècle.",
        "Le premier janvier, monsieur Dupont lit le chapitre deux sur Louis quatorze et le vingt et unième siècle.",
    ),
    (
        "Installe Python 3.12, lance --verbose, ouvre le JSON de 250 Ko et du MP3 en 4K.",
        "Installe Python trois point douze, lance verbose, ouvre le jason de deux cent cinquante kilooctets et du M P trois en quatre K.",
    ),
    (
        "De 10-20 Go, 0,50 €, 1 200 000 habitants, -5 °C, 09/10/2026, 80 000 et 300 000.",
        "De dix à vingt gigaoctets, cinquante centimes, un million deux cent mille habitants, moins cinq degrés Celsius, neuf octobre deux mille vingt-six, quatre-vingt mille et trois cent mille.",
    ),
    (
        "ATTENTION : la SNCF et l'UNESCO, etc. Le 2e et la 5e.",
        "attention : la S N C F et l'unesco, et cetera. Le deuxième et la cinquième.",
    ),
    (
        "Un SSD de 1 To, une hausse de 1,5 % et jusque 1 h.",
        "Un S S D d'un téraoctet, une hausse d'un virgule cinq pour cent et jusqu'une heure.",
    ),
];

/// Cas supplémentaires, sortie relevée sur le prototype (`make_norm.py` et
/// pièges par règle).
const MORE_CASES: &[(&str, &str)] = &[
    (
        "Ton PC a 16 Go de RAM, un CPU AMD à 3,6 GHz et un SSD NVMe de 1 To.",
        "Ton P C a seize gigaoctets de ram, un C P U A M D à trois virgule six gigahertz et un S S D N V M E d'un téraoctet.",
    ),
    (
        "Entre 1990 et 2024, le PIB a progressé de 1,5 % par an ; en 2024, il atteignait 2 920 milliards d'euros, soit environ 42 800 € par habitant.",
        "Entre mille neuf cent quatre-vingt-dix et deux mille vingt-quatre, le P I B a progressé d'un virgule cinq pour cent par an ; en deux mille vingt-quatre, il atteignait deux mille neuf cent vingt milliards d'euros, soit environ quarante-deux mille huit cents euros par habitant.",
    ),
    (
        "Le 1er janvier, M. Dupont a reçu 71 messages et 80 appels ; 91 % venaient de l'appli iOS (cf. chapitre II).",
        "Le premier janvier, monsieur Dupont a reçu soixante et onze messages et quatre-vingts appels ; quatre-vingt-onze pour cent venaient de l'appli I O S (voir chapitre deux).",
    ),
    (
        "Il reste 1 personne, 21 heures et 31 pages ; plus que 1 étape.",
        "Il reste une personne, vingt et une heures et trente et une pages ; plus qu'une étape.",
    ),
    (
        "Au XIXe siècle, la 1re et la 2nde guerre ; Louis XVI, Henri IV et Jean-Paul II ; acte III, tome 2.",
        "Au dix-neuvième siècle, la première et la seconde guerre ; Louis seize, Henri quatre et Jean-Paul deux ; acte trois, tome deux.",
    ),
    (
        "De 2020-2024, -12 % ; ~10 ms ou ≈ 3 Go, 1920x1080 et/ou 4 Mo/s.",
        "De deux mille vingt à deux mille vingt-quatre, moins douze pour cent ; environ dix millisecondes ou environ trois gigaoctets, mille neuf cent vingt par mille quatre-vingts et ou quatre mégaoctets par seconde.",
    ),
    (
        "Prix : 1,99 $, $5, 3 k€, 1,5 M€, £10 et 1 €.",
        "Prix : un dollar quatre-vingt-dix-neuf, cinq dollars, trois mille euros, un virgule cinq million d'euros, dix livres et un euro.",
    ),
    (
        "Rendez-vous à 8h00, à 23:59 ou à 9 h 05 le 2026-10-09 ; le 1 mai aussi.",
        "Rendez-vous à huit heures, à vingt-trois heures cinquante-neuf ou à neuf heures cinq le neuf octobre deux mille vingt-six ; le premier mai aussi.",
    ),
    (
        "Mme Curie, Mlle Dubois, MM. Martin et Dr Watson ; voir p. ex. le § 3, n° 5, etc.",
        "madame Curie, mademoiselle Dubois, messieurs Martin et docteur Watson ; voir par exemple le paragraphe trois, numéro cinq, et cetera.",
    ),
    (
        "L'IA de ChatGPT et d'OpenAI tourne sous macOS ; npm et ChatonLM aussi, via HTTPS en PDF.",
        "L'I A de tchatte G P T et d'open A I tourne sous mac O S ; N P M et libre L M aussi, via H T T P S en P D F.",
    ),
    (
        "La version v2.1 et l'adresse 192.168.0.1 répondent ; Python 3.12.1 aussi.",
        "La version deux point un et l'adresse cent quatre-vingt-douze point cent soixante-huit point zéro point un répondent ; Python trois point douze point un aussi.",
    ),
    (
        "Il fait -5 °C, 25 °C et 100 km/h ; 3 kWh, 5 mAh, 60 fps et 2 x plus.",
        "Il fait moins cinq degrés Celsius, vingt-cinq degrés Celsius et cent kilomètres par heure ; trois kilowattheures, cinq milliampères-heures, soixante images par seconde et deux fois plus.",
    ),
    (
        "Le plan B de la SNCF, l'UE et la BCE.",
        "Le plan B de la S N C F, l'U E et la B C E.",
    ),
    (
        "Le 31/12/1999 à 0h00, le compte passa à 007 et 0,05 € ; 1,005 kg.",
        "Le trente et un décembre mille neuf cent quatre-vingt-dix-neuf à zéro heure, le compte passa à zéro zéro sept et cinq centimes ; un virgule zéro zéro cinq kilogramme.",
    ),
    (
        "Choisis 1 option sur 2 ; prends-en 1.",
        "Choisis une option sur deux ; prends-en un.",
    ),
    (
        "Il y a 1 000 000 de personnes et 1 001 jours.",
        "Il y a un million de personnes et mille un jours.",
    ),
];

#[test]
fn prototype_cases_are_identical() {
    for (given, expected) in PROTOTYPE_CASES {
        assert_eq!(normalize(given), *expected, "entrée : {given}");
    }
}

#[test]
fn more_cases_match_the_prototype() {
    for (given, expected) in MORE_CASES {
        assert_eq!(normalize(given), *expected, "entrée : {given}");
    }
}

#[test]
fn deliberate_departures_from_the_prototype() {
    // Modificateurs de teinte retirés avec l'émoji (le prototype gardait « 🏽 »).
    assert_eq!(
        normalize("C'est prêt 🚀 ! Bravo 👏🏽 & merci @Paul → suite."),
        "C'est prêt ! Bravo et merci arobase Paul, suite."
    );
    // « de » devant l'unité après un million ou un milliard exact.
    assert_eq!(
        normalize("Budget : 2 000 000 000 €."),
        "Budget : deux milliards d'euros."
    );
    assert_eq!(
        normalize("Budget : 1 000 000 $."),
        "Budget : un million de dollars."
    );
    // Mots courts en capitales lus comme des mots.
    assert_eq!(
        normalize("IMPORTANT : la NOTE dit NON."),
        "important : la note dit non."
    );
    // Une seule lettre après une section : I, V, X seulement.
    assert_eq!(
        normalize("Voir l'annexe C et la partie V."),
        "Voir l'annexe C et la partie cinq."
    );
    assert_eq!(normalize("Louis C a écrit."), "Louis C a écrit.");
}

#[test]
fn rules_one_by_one() {
    let cases = [
        // Élision après « de », « que », « jusque » seulement devant un/une produits.
        ("plus de 1 page", "plus d'une page"),
        ("autant que 1 €", "autant qu'un euro"),
        ("de une à deux", "d'une à deux"),
        // Ordinaux.
        (
            "le 3e étage, le 9e, le 21e",
            "le troisième étage, le neuvième, le vingt et unième",
        ),
        ("la 1ère et le 2nd", "la première et le second"),
        // Romains : siècles ordinaux, règnes, sections ; « Le », « De » restent des mots.
        ("Le Ve siècle De Gaulle", "Le cinquième siècle De Gaulle"),
        (
            "Napoléon Ier et Napoléon III",
            "Napoléon premier et Napoléon trois",
        ),
        ("Benoît XVI", "Benoît seize"),
        // Intervalles et négatifs.
        ("de 5-10 min", "de cinq à dix minutes"),
        ("−3 et -4", "moins trois et moins quatre"),
        ("version 1.2-3", "version un point deux-trois"),
        // Monnaies.
        (
            "0,99 €, 2 €, 1 £",
            "quatre-vingt-dix-neuf centimes, deux euros, une livre",
        ),
        (
            "$0.50 et $3.75",
            "cinquante cents et trois dollars soixante-quinze",
        ),
        // Heures.
        ("à 12:00 et à 0:30", "à douze heures et à zéro heure trente"),
        // Dates.
        (
            "le 1er mars et le 01/02/2003",
            "le premier mars et le premier février deux mille trois",
        ),
        // Décimaux : virgule et point.
        ("3,14 et 2.5", "trois virgule quatorze et deux point cinq"),
        // Lexique et sigles ; un mot en capitales de 5 lettres ou plus avec voyelle est lu.
        (
            "la NASA, le CNRS et le YAML",
            "la nasa, le C N R S et le yamel",
        ),
        ("BRAVO et HTTP", "bravo et H T T P"),
        // Sigle collé à un trait d'union : laissé tel quel.
        ("le Wi-FI et COVID-19", "le Wi-FI et covid-dix-neuf"),
        // Abréviations.
        (
            "Pr Martin, env. 3 km, vs le c.-à-d.",
            "professeur Martin, environ trois kilomètres, contre le c'est-à-dire",
        ),
        ("ex. le Pr. Durand", "par exemple le professeur Durand"),
    ];
    for (given, expected) in cases {
        assert_eq!(normalize(given), expected, "entrée : {given}");
    }
}

#[test]
fn cardinals_and_ordinals() {
    let cases = [
        (0, "zéro"),
        (17, "dix-sept"),
        (70, "soixante-dix"),
        (71, "soixante et onze"),
        (80, "quatre-vingts"),
        (81, "quatre-vingt-un"),
        (99, "quatre-vingt-dix-neuf"),
        (100, "cent"),
        (200, "deux cents"),
        (201, "deux cent un"),
        (1000, "mille"),
        (80_000, "quatre-vingt mille"),
        (1_000_000, "un million"),
        (2_500_000_000, "deux milliards cinq cents millions"),
    ];
    for (n, words) in cases {
        assert_eq!(cardinal(n, false), words, "{n}");
    }
    assert_eq!(cardinal(21, true), "vingt et une");
    assert_eq!(
        cardinal(1_000_000_000_000, false),
        "un zéro zéro zéro zéro zéro zéro zéro zéro zéro zéro zéro zéro"
    );
    assert_eq!(ordinal(1, true), "première");
    assert_eq!(ordinal(5, false), "cinquième");
    assert_eq!(ordinal(9, false), "neuvième");
    assert_eq!(ordinal(80, false), "quatre-vingtième");
    assert_eq!(ordinal(1000, false), "millième");
}

#[test]
fn spaces_and_empty_text() {
    assert_eq!(normalize(""), "");
    assert_eq!(normalize("  Bonjour\u{a0}!  "), "Bonjour\u{a0}!");
    assert_eq!(normalize("Ligne\nsuivante"), "Ligne\nsuivante");
}

/// Constats de la relecture (61, R3-*).
#[test]
fn review_cases() {
    let cases = [
        // R3-4 : élision après un nombre, pas une unité.
        (
            "Les 2 l'utilisent et les 3 m'ont aidé, mais les 2 s'arrêtent.",
            "Les deux l'utilisent et les trois m'ont aidé, mais les deux s'arrêtent.",
        ),
        // R3-5 : sigle collé à un nombre ou à une lettre par un trait d'union.
        (
            "Utilise GPT-4 ou GPT-4o, en UTF-8, avec SHA-256 et l'USB-C.",
            "Utilise G P T quatre ou G P T quatre O, en U T F huit, avec S H A deux cent cinquante-six et l'U S B C.",
        ),
        // R3-6 : mots courants en capitales lus comme des mots.
        (
            "NE PAS supprimer ce fichier. ET surtout NE JAMAIS le modifier. Il FAUT redémarrer, c'est VRAI, TOUS, CECI.",
            "ne pas supprimer ce fichier. et surtout ne jamais le modifier. Il faut redémarrer, c'est vrai, tous, ceci.",
        ),
        ("Le marquage CE de la SA.", "Le marquage C E de la S A."),
        // R3-7 : soustraction.
        (
            "Calcul : 10 - 3 = 7, puis 15-5 = 10.",
            "Calcul : dix moins trois égal sept, puis quinze moins cinq égal dix.",
        ),
        // R3-8 : notation scientifique.
        (
            "Un taux de 1e-5 et 2,5e-3, ou 3E8.",
            "Un taux de dix puissance moins cinq et deux virgule cinq fois dix puissance moins trois, ou trois fois dix puissance huit.",
        ),
        // R3-10 : « etc » dans un chemin.
        (
            "Le fichier /etc/hosts, etc.",
            "Le fichier /etc/hosts, et cetera.",
        ),
        // R3-13 : barre oblique.
        (
            "Les prix : 9,99 €/mois ou 99 €/an.",
            "Les prix : neuf euros quatre-vingt-dix-neuf par mois ou quatre-vingt-dix-neuf euros par an.",
        ),
        (
            "Ouvert 24h/24 et 7j/7, 1/3 des cas, note 18/20, mardi 14/10.",
            "Ouvert vingt-quatre heures sur vingt-quatre et sept jours sur sept, un tiers des cas, note dix-huit sur vingt, mardi quatorze octobre.",
        ),
        // R3-14 : « M. » qui n'est pas « monsieur ».
        (
            "Soit le point M. La droite passe par A. Choisis la taille M. Elle est grande. M. Durand vient.",
            "Soit le point M. La droite passe par A. Choisis la taille M. Elle est grande. monsieur Durand vient.",
        ),
        (
            "Le point M. Ses coordonnées sont simples.",
            "Le point M. Ses coordonnées sont simples.",
        ),
        // R3-15 : langages à symbole.
        (
            "En C# et en C++, comme en F#. Le framework .NET.",
            "En C sharp et en C plus plus, comme en F sharp. Le framework dotnet.",
        ),
        // R3-17 : durée, rapport, verset.
        (
            "Écran 16:10, verset Jean 3:16, à 16:10. La vidéo dure 1:30:45, il est 10:30:00.",
            "Écran seize sur dix, verset Jean trois seize, à seize heures dix. La vidéo dure une heure trente minutes quarante-cinq secondes, il est dix heures trente minutes.",
        ),
        // R3-21 : féminins réguliers.
        (
            "Il y a 1 variable, 1 instance, 1 liste, 21 entrées et 1 musée.",
            "Il y a une variable, une instance, une liste, vingt et une entrées et un musée.",
        ),
        (
            "1 fonctionnalité, 1 dépendance, 1 occurrence et 21 conditions.",
            "une fonctionnalité, une dépendance, une occurrence et vingt et une conditions.",
        ),
        // R3-23 : monnaie après « millions », °F.
        (
            "Coût : 2 millions € ou 3 milliards $, à 77°F.",
            "Coût : deux millions d'euros ou trois milliards de dollars, à soixante-dix-sept degrés Fahrenheit.",
        ),
    ];
    for (given, expected) in cases {
        assert_eq!(normalize(given), expected, "entrée : {given}");
    }
}
