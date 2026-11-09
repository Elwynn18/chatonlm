//! Journal du relais : quelques événements de service, jamais de contenu.
//!
//! Règle : aucune requête cherchée, aucune adresse IP, aucune clé (ni de
//! fournisseur ni d'installation), aucune réponse. Seuls des faits agrégés ou
//! techniques : démarrage, fournisseur en échec (code HTTP), seuils de budget.
//! Sortie : erreur standard avec le préfixe de priorité de journald (`<4>`),
//! sous systemd. Les essais capturent les lignes pour vérifier la règle.

use std::sync::{Arc, Mutex};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Error,
    Warn,
    Info,
}

impl Level {
    fn prefix(self) -> &'static str {
        match self {
            Level::Error => "<3>",
            Level::Warn => "<4>",
            Level::Info => "<6>",
        }
    }
}

#[derive(Clone, Default)]
pub struct Log {
    capture: Option<Arc<Mutex<Vec<String>>>>,
}

impl Log {
    /// Journal qui garde les lignes en mémoire (essais) au lieu de les écrire.
    pub fn capturing() -> (Self, Arc<Mutex<Vec<String>>>) {
        let lines = Arc::new(Mutex::new(Vec::new()));
        (
            Self {
                capture: Some(lines.clone()),
            },
            lines,
        )
    }

    pub fn write(&self, level: Level, message: &str) {
        match &self.capture {
            Some(lines) => lines
                .lock()
                .expect("log")
                .push(format!("{level:?} {message}")),
            None => eprintln!("{}{message}", level.prefix()),
        }
    }

    pub fn info(&self, message: &str) {
        self.write(Level::Info, message);
    }

    pub fn warn(&self, message: &str) {
        self.write(Level::Warn, message);
    }

    pub fn error(&self, message: &str) {
        self.write(Level::Error, message);
    }
}

/// Ligne écrite pour une panique : l'emplacement seul, jamais le message. Le
/// message d'une panique peut recopier ce qui l'a causée (jusqu'à 256 octets
/// d'une chaîne : un titre de résultat, une requête) ; le journal n'en garde
/// rien.
pub fn panic_line(location: Option<&std::panic::Location<'_>>) -> String {
    match location {
        Some(at) => format!(
            "{}panic at {}:{}",
            Level::Error.prefix(),
            at.file(),
            at.line()
        ),
        None => format!("{}panic", Level::Error.prefix()),
    }
}

/// Remplace le crochet de panique par défaut (qui écrit le message sur
/// l'erreur standard, donc dans journald) par [`panic_line`].
pub fn install_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        eprintln!("{}", panic_line(info.location()))
    }));
}

/// Horloge du relais : celle du système, ou réglée à la main (essais).
#[derive(Clone, Default)]
pub struct Clock {
    manual: Option<Arc<std::sync::atomic::AtomicU64>>,
}

impl Clock {
    pub fn manual(start: u64) -> (Self, Arc<std::sync::atomic::AtomicU64>) {
        let value = Arc::new(std::sync::atomic::AtomicU64::new(start));
        (
            Self {
                manual: Some(value.clone()),
            },
            value,
        )
    }

    /// Secondes Unix.
    pub fn now(&self) -> u64 {
        match &self.manual {
            Some(value) => value.load(std::sync::atomic::Ordering::SeqCst),
            None => std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|elapsed| elapsed.as_secs())
                .unwrap_or_default(),
        }
    }
}
