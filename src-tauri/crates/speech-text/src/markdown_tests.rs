//! Essais du texte parlé et du segmenteur.

use super::{Segmenter, spoken_text};

#[test]
fn paragraphs_and_headings() {
    assert_eq!(spoken_text("Bonjour à tous"), "Bonjour à tous.");
    assert_eq!(
        spoken_text("# Installation\n\nOuvre le terminal."),
        "Installation. Ouvre le terminal."
    );
    assert_eq!(
        spoken_text("Titre\n=====\n\nSous-titre\n---\n\nTexte !"),
        "Titre. Sous-titre. Texte !"
    );
    assert_eq!(
        spoken_text("Ligne une\nligne deux,"),
        "Ligne une ligne deux."
    );
    assert_eq!(spoken_text("Voici les étapes :"), "Voici les étapes :");
    assert_eq!(spoken_text(""), "");
}

#[test]
fn emphasis_links_and_inline_code() {
    assert_eq!(
        spoken_text("C'est **très** *simple* et ~~faux~~ : lance `npm run dev`."),
        "C'est très simple et faux : lance npm run dev."
    );
    assert_eq!(
        spoken_text(
            "Voir [la documentation](https://ollama.com/library) ou [ici][ref].\n\n[ref]: https://example.com"
        ),
        "Voir la documentation ou ici."
    );
    // Lien automatique et adresse nue : l'hôte seulement.
    assert_eq!(
        spoken_text(
            "Site : <https://www.ollama.com/library>, ou https://github.com/ollama/ollama."
        ),
        "Site : ollama.com, ou github.com."
    );
    assert_eq!(
        spoken_text("Écris à <contact@example.com>."),
        "Écris à contact@example.com."
    );
}

#[test]
fn lists_get_a_final_period() {
    assert_eq!(
        spoken_text("- pommes\n- poires;\n- prunes !"),
        "pommes. poires. prunes !"
    );
    assert_eq!(
        spoken_text("1. Installe Ollama\n2. Tire un modèle\n   - petit\n   - rapide\n3. Discute"),
        "Installe Ollama. Tire un modèle. petit. rapide. Discute."
    );
    // Liste lâche (paragraphes dans les éléments) et cases à cocher.
    assert_eq!(
        spoken_text("- a\n\n  suite\n\n- [x] fait\n- [ ] à faire"),
        "a. suite. fait. à faire."
    );
}

#[test]
fn quotes_are_read() {
    assert_eq!(
        spoken_text("> Citation\n> sur deux lignes.\n\nFin"),
        "Citation sur deux lignes. Fin."
    );
    assert_eq!(
        spoken_text("> [!NOTE]\n> Remarque utile"),
        "Remarque utile."
    );
}

#[test]
fn tables_one_sentence_per_row() {
    let table = "| Modèle | Taille |\n|:--|--:|\n| **qwen3** | `523 MB` |\n| [lib](https://ollama.com) | ~~1 GB~~ |\n| vide |  |\n\nAprès le tableau.";
    assert_eq!(
        spoken_text(table),
        "Modèle, Taille. qwen3, 523 MB. lib, 1 GB. vide. Après le tableau."
    );
}

#[test]
fn code_blocks_are_skipped() {
    assert_eq!(
        spoken_text(
            "Lance :\n\n```bash\nnpm run dev\n```\n\nPuis ouvre la page.\n\n    code indenté\n\nFin."
        ),
        "Lance : Puis ouvre la page. Fin."
    );
    // Bloc jamais fermé : sauté jusqu'au bout.
    assert_eq!(spoken_text("Avant.\n\n~~~\nreste du code"), "Avant.");
}

#[test]
fn formulas_like_the_app() {
    // `$$…$$`, `\(…\)`, `\[…\]` : formules ; un `$` seul est du texte.
    assert_eq!(
        spoken_text(
            "Soit \\(a^2 + b^2\\) et $$c^2$$ donc.\n\n\\[\nE = mc^2\n\\]\n\n$$\n\\sum i\n\n= n\n$$\n\nIl coûte $5 et $10."
        ),
        "Soit et donc. Il coûte $5 et $10."
    );
    // Formule dans du code : gardée.
    assert_eq!(spoken_text("Écris `$$x$$` ici."), "Écris $$x$$ ici.");
    // Formule ouverte jusqu'à la fin du paragraphe.
    assert_eq!(spoken_text("Calcul $$x + y\n\nSuite."), "Calcul. Suite.");
}

#[test]
fn images_html_and_footnotes_are_skipped() {
    assert_eq!(spoken_text("Logo ![un chat](chat.png) ici."), "Logo ici.");
    assert_eq!(
        spoken_text("<div>\nbloc HTML\n</div>\n\nTexte <b>gras</b> fin."),
        "Texte gras fin."
    );
    assert_eq!(
        spoken_text("Une affirmation[^a] et une autre[^b].\n\n[^a]: Note *a*.\n[^b]: Note b."),
        "Une affirmation et une autre."
    );
    // Appel sans définition : retiré aussi.
    assert_eq!(spoken_text("Voir[^9] plus loin."), "Voir plus loin.");
    // Paragraphe fait d'une image seule : rien à lire.
    assert_eq!(spoken_text("![seule](a.png)\n\nTexte."), "Texte.");
}

#[test]
fn emojis_are_left_to_normalization_but_empty_sentences_dropped() {
    assert_eq!(spoken_text("Bravo 🎉 !\n\n🚀"), "Bravo 🎉 !");
    assert_eq!(spoken_text("---\n\nAprès la règle."), "Après la règle.");
}

#[test]
fn sentences_and_abbreviations() {
    let mut seg = Segmenter::new();
    let mut out = seg.push("M. Dupont arrive. Il est 9 h. Le Dr. Watson aussi, p. ex. demain. Fin");
    out.extend(seg.finish());
    assert_eq!(
        out,
        [
            "M. Dupont arrive.",
            "Il est 9 h.",
            "Le Dr. Watson aussi, p. ex. demain.",
            "Fin."
        ]
    );
}

// ---------------------------------------------------------------- segmenteur

const DOCUMENTS: &[&str] = &[
    "Bonjour ! Voici une réponse. Elle a trois phrases.",
    "# Titre\n\nPremier paragraphe. Deuxième phrase.\n\n- élément un\n- élément **deux**\n  - imbriqué\n\n1. a\n2. b\n\nFin.",
    "Avant le code.\n\n```python\nprint(\"Bonjour. Au revoir.\")\n# Ceci n'est pas lu. Jamais.\n```\n\nAprès le code. Encore.",
    "| A | B |\n|---|---|\n| 1 | 2 |\n| trois. quatre | cinq |\n\nTexte. Fin",
    "Texte *en. italique* puis **gras. encore** et `code. ici` puis [lien. texte](https://x.y/z). Fin.",
    "Formule \\(x. y\\) puis $$a. b$$ et\n\n$$\nE = mc^2. Oui.\n$$\n\nSuite. Et $5 ou $10.",
    "> Citation. Deux.\n> Trois.\n\nNote[^1]. Suite.\n\n[^1]: Définition. Longue.",
    "Titre\n===\n\nUn paragraphe | avec barre. Encore.\n\nFin. Vraiment.",
    "Étapes :\n\n1. Installe\n\n   Puis vérifie.\n2. Lance `npm run dev`. Ouvre http://localhost:5173. C'est prêt.",
    "<details>\n<summary>Voir</summary>\n\nCaché **gras**. Ici.\n\nToujours [dedans](https://ollama.com).",
    "Ligne **un**\r\n\r\n- élément\r\n- `deux` et [lien](https://ollama.com)\r\n\r\n```js\r\nlet x = 1;\r\n```\r\n\r\nFin **ici**",
    "Prix : 5 * 3 = 15. Le fichier nom_de_fichier.txt existe. Ensuite 2 < 3. Enfin.",
    "Intro. Suite.\n\nNom | Valeur. Autre\n---|---\nA. B | C\n\nFin.",
    "Emoji 🎉 au milieu. 日本語のテキスト。Voilà. Ça marche ? Oui… Peut-être!",
    // Cas trouvés par l'exploration (relecture 61) :
    // ligne devenue en-tête de tableau après une coupe, cellule sans espace de tête ;
    "<b>HTML. Ici</b> et <span Nom | Valeur. Autre\n---|---\nA. B | C\n\nFin.",
    // en-tête d'une colonne qui redevient du texte quand le délimiteur s'allonge ;
    "Intro. Texte\n|a\n|---|---\nSuite. Fin",
    // appel de note dont la définition sort de la fenêtre ;
    "Prix : 5 $ et 10 $. Oui\n\n[^1]: Définition. Longue\n\n~~barré. X~~ et ~simple. Y~ [^1]: Définition. Longue\nPrix : 5 $ et 10 $. Oui\n\n",
    // citation retirée juste après une coupe ;
    "Résultat. [1]. Suite et fin. Puis [2], encore.",
    // code en ligne fermé une ligne plus loin, formule entre les deux.
    "- un `a $$x$$\n- deux.\n- trois `fin\n- quatre. Cinq",
];

/// Phrases rendues en envoyant `parts` un à un, puis `finish`.
fn streamed(parts: &[&str]) -> Vec<String> {
    let mut seg = Segmenter::new();
    let mut out = Vec::new();
    for part in parts {
        out.extend(seg.push(part));
    }
    out.extend(seg.finish());
    out
}

/// Découpage par caractère.
fn by_char(doc: &str) -> Vec<&str> {
    doc.char_indices()
        .map(|(i, c)| &doc[i..i + c.len_utf8()])
        .collect()
}

/// Découpage pseudo-aléatoire déterministe (générateur congruentiel),
/// toujours sur des frontières de caractère.
fn random_parts(doc: &str, seed: u64) -> Vec<&str> {
    let mut state = seed;
    let mut parts = Vec::new();
    let mut at = 0;
    while at < doc.len() {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        let mut end = (at + 1 + (state >> 33) as usize % 12).min(doc.len());
        while !doc.is_char_boundary(end) {
            end += 1;
        }
        parts.push(&doc[at..end]);
        at = end;
    }
    parts
}

#[test]
fn streaming_equals_whole_text_for_any_split() {
    for doc in DOCUMENTS {
        let whole = spoken_text(doc);
        assert_eq!(streamed(&[doc]).join(" "), whole, "envoi unique : {doc:?}");
        assert_eq!(
            streamed(&by_char(doc)).join(" "),
            whole,
            "par caractère : {doc:?}"
        );
        for seed in 0..20 {
            assert_eq!(
                streamed(&random_parts(doc, seed)).join(" "),
                whole,
                "graine {seed} : {doc:?}"
            );
        }
    }
}

#[test]
fn streaming_reads_finished_sentences_early() {
    let mut seg = Segmenter::new();
    assert!(
        seg.push("Première phrase. Deuxi")
            .contains(&"Première phrase.".to_owned())
    );
    // Phrase inachevée gardée.
    assert!(seg.push("ème en cours").is_empty());
    assert_eq!(seg.push(". Troisième"), ["Deuxième en cours."]);
    assert_eq!(seg.finish(), ["Troisième."]);
}

#[test]
fn open_code_block_is_never_read() {
    let doc = "Intro.\n\n```text\nPhrase de code. Autre phrase. Encore une.\n";
    let mut seg = Segmenter::new();
    let mut early = Vec::new();
    for part in by_char(doc) {
        early.extend(seg.push(part));
    }
    assert_eq!(early, ["Intro."]);
    assert!(seg.push("```\n\nDehors. ").is_empty());
    assert_eq!(seg.push("Suite"), ["Dehors."]);
    assert_eq!(seg.finish(), ["Suite."]);
}

#[test]
fn unfinished_table_row_waits() {
    let mut seg = Segmenter::new();
    // L'en-tête est fini dès que la ligne suivante commence ; elle, non, même
    // avec une phrase terminée dans sa première cellule.
    assert_eq!(
        seg.push("| A | B |\n|---|---|\n| Un. Deux | trois"),
        ["A, B."]
    );
    assert!(seg.push(" |").is_empty());
    assert!(seg.push("\n").is_empty());
    assert_eq!(seg.push("| quatre"), ["Un.", "Deux, trois."]);
    assert_eq!(seg.finish(), ["quatre."]);
}

#[test]
fn unclosed_inline_markers_hold_the_sentence() {
    let mut seg = Segmenter::new();
    // « *en. » pourrait encore devenir de l'italique : rien n'est lu.
    assert!(seg.push("Texte *en. Italique").is_empty());
    assert_eq!(seg.push("* suite. Fin"), ["Texte en.", "Italique suite."]);
    assert_eq!(seg.finish(), ["Fin."]);
    // Une ligne avec « | » pourrait devenir l'en-tête d'un tableau.
    let mut seg = Segmenter::new();
    assert!(seg.push("a | b. C\n").is_empty());
    assert!(seg.push("|---|---|\n").is_empty());
    assert_eq!(seg.finish(), ["a, b.", "C."]);
}

/// Morceaux piégeux assemblés au hasard (graine fixe) : titres soulignés,
/// barres, listes, citations, code, formules, marqueurs ouverts, notes.
const PIECES: &[&str] = &[
    "Phrase simple. Autre phrase",
    "a | b. C d",
    "Nom | Valeur. Autre\n---|---\nA. B | C",
    "Titre. Deux\n===",
    "-",
    "---",
    "- élément. Suite\n- Deux. Trois",
    "1. Un. Deux\n2. Trois",
    "> Citation. Suite\n> Fin. Là",
    "```\ncode. Code\n```",
    "~~~\nouvert. Jamais",
    "$$\nE = mc^2. X\n$$",
    "Formule $$a. B$$ ici. Et \\(x. Y\\) là",
    "*Italique. Encore* puis **gras. Fort**",
    "Un *marqueur. Ouvert",
    "`code. Ici` et `ouvert. Là",
    "[lien. Texte](https://ollama.com) ou [ref. X]",
    "Note[^1]. Après",
    "[^1]: Définition. Longue",
    "<b>HTML. Ici</b> et <span",
    "| A | B |\n|---|---|\n| Un. Deux | trois |",
    "M. Dupont. Le Dr. Watson. Fin",
    "Prix : 5 $ et 10 $. Oui",
    "    indenté. Code",
    "Ligne\nsuite. Encore",
    "🎉. Bravo ! Ça va ? Oui… Non",
    "#",
    "#x. Y",
    "```a`b. C",
    "<div",
    "* * *",
    "Souligné. X\n-",
    "- [ ] tâche. X\n- [x] faite",
    "1) Un. Deux",
    "Ouvert \\(x. Y",
    "Ouvert $$x. Y",
    "&amp;. Y &eacute. Z",
    "<https://ollama.com/a>. B",
    "~~barré. X~~ et ~simple. Y~",
    "*Attention… Le motif `*.txt` suffit. Ensuite",
    "*… ``* ``",
    "« Bonjour… » dit-il. « Au revoir. » Fin”",
    "D'après [1][2], c'est vrai [3]. Voir [la doc](https://ollama.com) [4](https://x.y).",
    "Lance `ps aux | grep ollama` pour voir. Ensuite redémarre",
    "nom_de_fichier. Ok et _souligné. X_",
];

fn generated_documents(count: usize) -> Vec<String> {
    let mut state = 0x2545_f491_4f6c_dd1d_u64;
    let mut next = |n: usize| {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        (state % n as u64) as usize
    };
    (0..count)
        .map(|_| {
            let len = 2 + next(5);
            (0..len)
                .map(|_| {
                    let sep = ["\n\n", "\n", " ", "\n\n"][next(4)];
                    format!("{}{sep}", PIECES[next(PIECES.len())])
                })
                .collect::<String>()
        })
        .collect()
}

#[test]
fn streaming_equals_whole_text_on_generated_documents() {
    for doc in generated_documents(400) {
        let whole = spoken_text(&doc);
        assert_eq!(
            streamed(&by_char(&doc)).join(" "),
            whole,
            "par caractère : {doc:?}"
        );
        for seed in 0..3 {
            assert_eq!(
                streamed(&random_parts(&doc, seed)).join(" "),
                whole,
                "graine {seed} : {doc:?}"
            );
        }
    }
}

#[test]
fn emphasis_undone_by_a_later_code_span_never_panics() {
    // Constat R3-1 : « *Attention… » lu, puis le code `*.txt` défait
    // l'emphase ; l'ancien décalage en octets tombait au milieu de « … ».
    for doc in [
        "*Attention… Le motif `*.txt` suffit. Ensuite",
        "*Attention… Le motif `*.txt` suffit.\n\nSuite.",
        "*… ``* ``",
    ] {
        let whole = spoken_text(doc);
        assert_eq!(streamed(&by_char(doc)).join(" "), whole, "{doc:?}");
    }
    // L'intérieur d'une emphase qui contient un « ` » littéral n'est pas lu.
    let mut seg = Segmenter::new();
    let early: Vec<String> = by_char("*Attention… Le motif `*")
        .into_iter()
        .flat_map(|p| seg.push(p))
        .collect();
    assert!(early.is_empty(), "{early:?}");
}

/// Texte arbitraire fait de caractères Markdown, d'espaces, de retours à la
/// ligne et de caractères de plusieurs octets.
fn soup(seed: u64, len: usize) -> String {
    const ALPHABET: &[&str] = &[
        "*",
        "_",
        "`",
        "~",
        "[",
        "]",
        "(",
        ")",
        "<",
        ">",
        "|",
        "-",
        "#",
        "$",
        "\\",
        "!",
        "^",
        ":",
        ".",
        " ",
        " ",
        " ",
        "\n",
        "\n\n",
        "a",
        "B",
        "c",
        "1",
        "…",
        "»",
        "é",
        "🎉",
        "日",
        "\r\n",
        "- ",
        "1. ",
        "```",
        "$$",
        "> ",
        "    ",
        "|---|",
        "http://a.b",
    ];
    let mut state = seed | 1;
    (0..len)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            ALPHABET[(state % ALPHABET.len() as u64) as usize]
        })
        .collect()
}

/// Aucune panique sur du texte arbitraire, quel que soit le découpage (à
/// chaque caractère, frontières UTF-8 comprises, ou au hasard), ni marqueur
/// interne dans les phrases. L'égalité avec `spoken_text` n'est garantie que
/// pour du Markdown réaliste (essai précédent) : sur ce texte, environ un
/// cas sur mille diffère (formule `$$` collée à un délimiteur d'emphase,
/// échappements devant un retour CRLF).
#[test]
fn arbitrary_fragments_never_panic() {
    for seed in 0..300u64 {
        let doc = soup(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15), 60);
        let _ = spoken_text(&doc);
        for out in [
            streamed(&by_char(&doc)),
            streamed(&random_parts(&doc, seed)),
        ] {
            for sentence in out {
                assert!(
                    !sentence.is_empty() && !sentence.contains(['\u{E000}', '\u{E001}']),
                    "{doc:?}"
                );
            }
        }
    }
}

#[test]
fn citations_are_dropped() {
    assert_eq!(
        spoken_text(
            "D'après les résultats [1][2], la version 0.12 est sortie le 3 octobre 2025 [3]."
        ),
        "D'après les résultats, la version 0.12 est sortie le 3 octobre 2025."
    );
    assert_eq!(
        spoken_text("See [1, 2] and [3-5], or [the docs](https://ollama.com) [6](https://x.y)."),
        "See and, or the docs."
    );
    // Le texte d'un lien qui n'est pas un numéro reste ; une définition
    // n'est pas nécessaire.
    assert_eq!(spoken_text("Voir [ici][doc] et [là][]."), "Voir ici et là.");
}

#[test]
fn a_pipe_inside_inline_code_does_not_hold_the_paragraph() {
    let mut seg = Segmenter::new();
    let out =
        seg.push("Lance `ps aux | grep ollama` pour voir. Ensuite redémarre le service. Puis");
    assert_eq!(
        out,
        [
            "Lance ps aux | grep ollama pour voir.",
            "Ensuite redémarre le service."
        ]
    );
    // Un « | » littéral sur la dernière ligne la retient ; sur une ligne
    // finie suivie d'une ligne ordinaire, plus.
    let mut seg = Segmenter::new();
    assert!(seg.push("Choix a | b. Ensuite").is_empty());
    assert_eq!(seg.push(" ici.\nSuite"), ["Choix a | b.", "Ensuite ici."]);
    // Dernière ligne qui commence comme une ligne de délimiteurs : la
    // ligne d'avant attend.
    let mut seg = Segmenter::new();
    assert!(seg.push("Nom | Valeur. Autre\n-").is_empty());
    assert!(seg.push("--|---\n").is_empty());
    assert_eq!(seg.finish(), ["Nom, Valeur.", "Autre."]);
}

#[test]
fn the_window_moves_forward() {
    let mut seg = Segmenter::new();
    let mut out = Vec::new();
    for i in 0..50 {
        out.extend(seg.push(&format!("Paragraphe {i}. Fin du paragraphe.\n\n")));
    }
    out.extend(seg.push("Dernier"));
    // Seul le dernier bloc reste dans la fenêtre analysée.
    assert!(
        seg.markdown.len() - seg.base < 60,
        "fenêtre de {} octets : {:?}",
        seg.markdown.len() - seg.base,
        &seg.markdown[seg.base..]
    );
    out.extend(seg.finish());
    assert_eq!(out.len(), 101);
    // Pas de reprise dans un bloc de code ni dans une liste serrée.
    let mut seg = Segmenter::new();
    seg.push("Avant.\n\n```\na\n\nb\n\nc\n");
    assert_eq!(
        seg.base,
        "Avant.\n\n".len(),
        "{:?}",
        &seg.markdown[seg.base..]
    );
    // Reprise au premier bloc dont la ligne est finie, jamais dans le code.
    seg.push("```\n\nAprès.\n\nFin");
    assert!(
        seg.markdown[seg.base..].starts_with("Après."),
        "{:?}",
        &seg.markdown[seg.base..]
    );
}
