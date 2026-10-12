//! Mesure du `Segmenter` sur une réponse réaliste (paragraphes, listes,
//! tableaux, code, formules), envoyée en fragments de 4 caractères.
//!
//! Usage : `cargo run --release -p speech-text --example segment -- [caractères]`
//! (50 000 par défaut). Vérifie aussi que les phrases rendues valent
//! `spoken_text` du texte entier.

use speech_text::markdown::{Segmenter, spoken_text};
use std::time::{Duration, Instant};

const BLOCK: &str = "## Installation\n\n\
Pour installer le modèle, ouvre un terminal et lance `ollama pull qwen3`. Le téléchargement \
pèse environ 5,2 Go et prend quelques minutes. Ensuite, vérifie la version avec `ollama -v`.\n\n\
- Première étape : **installer** les dépendances.\n\
- Deuxième étape : configurer le *fichier* `config.toml`.\n\
  - Sous-étape avec un [lien](https://ollama.com/library).\n\
- Troisième étape : relancer le service.\n\n\
| Modèle | Taille | Contexte |\n|---|---|---|\n| qwen3:0.6b | 523 Mo | 40 000 |\n| gemma3:270m | 292 Mo | 32 000 |\n\n\
```bash\nollama run qwen3 \"Bonjour. Comment vas-tu ?\"\nexport OLLAMA_HOST=0.0.0.0\n```\n\n\
> Remarque : la formule $$E = mc^2$$ n'a rien à voir ici. Le prix est de 5 $ par mois.\n\n\
Enfin, un paragraphe plus long qui explique les choses en détail, avec plusieurs phrases. \
La mémoire vive nécessaire dépend du modèle choisi. Les modèles quantifiés consomment moins. \
En cas d'erreur, consulte les journaux du service et relance-le.\n\n";

fn main() {
    let size: usize = std::env::args()
        .nth(1)
        .and_then(|a| a.parse().ok())
        .unwrap_or(50_000);
    let mut doc = String::new();
    while doc.chars().count() < size {
        doc.push_str(BLOCK);
    }
    let chars: Vec<char> = doc.chars().collect();
    let fragments: Vec<String> = chars.chunks(4).map(|c| c.iter().collect()).collect();
    let mut seg = Segmenter::new();
    let mut out = Vec::new();
    let mut worst = Duration::ZERO;
    let start = Instant::now();
    for fragment in &fragments {
        let t = Instant::now();
        out.extend(seg.push(fragment));
        worst = worst.max(t.elapsed());
    }
    out.extend(seg.finish());
    let total = start.elapsed();
    let same = out.join(" ") == spoken_text(&doc);
    println!(
        "{} caractères, {} fragments : {:.0} ms au total, pire push {:.2} ms, {} phrases, identique à spoken_text : {same}",
        chars.len(),
        fragments.len(),
        total.as_secs_f64() * 1e3,
        worst.as_secs_f64() * 1e3,
        out.len()
    );
}
