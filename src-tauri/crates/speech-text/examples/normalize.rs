//! Prétraitement en ligne de commande, pour le banc d'écoute.
//!
//! Usage : `cargo run -p speech-text --example normalize -- <fr|en|es> [--markdown] [--time]`
//! Lit le texte sur l'entrée standard et imprime le texte prêt pour la
//! synthèse. `--markdown` : tire d'abord le texte parlé (une phrase par
//! ligne) ; `--time` : temps moyen par appel sur 1000 appels (sur la sortie
//! d'erreur).

use speech_text::{Language, markdown, normalize::normalize};
use std::io::Read;
use std::time::Instant;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let language = match args.first().map(String::as_str) {
        Some("fr") => Language::Fr,
        Some("en") => Language::En,
        Some("es") => Language::Es,
        _ => {
            eprintln!("usage : normalize <fr|en|es> [--markdown] [--time] < texte");
            std::process::exit(2);
        }
    };
    let mut input = String::new();
    std::io::stdin()
        .read_to_string(&mut input)
        .expect("entrée standard lisible");
    let lines: Vec<String> = if args.iter().any(|a| a == "--markdown") {
        let mut segmenter = markdown::Segmenter::new();
        let mut sentences = segmenter.push(&input);
        sentences.extend(segmenter.finish());
        sentences
    } else {
        vec![input.trim_end_matches('\n').to_owned()]
    };
    for line in &lines {
        println!("{}", normalize(line, language));
    }
    if args.iter().any(|a| a == "--time") {
        let start = Instant::now();
        for _ in 0..1000 {
            for line in &lines {
                std::hint::black_box(normalize(std::hint::black_box(line), language));
            }
        }
        let micros = start.elapsed().as_secs_f64() * 1e6 / 1000.0;
        let chars: usize = lines.iter().map(|l| l.chars().count()).sum();
        eprintln!("{micros:.1} µs par appel ({chars} caractères)");
    }
}
