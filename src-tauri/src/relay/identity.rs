//! Clé d'installation : graine Ed25519 gardée dans `<données>/relay/
//! identity.json`, avec la liste des relais auprès desquels elle est inscrite.
//!
//! Le fichier est écrit d'un coup (fichier temporaire renommé), lisible du
//! seul utilisateur : mode 0600 sous Linux ; sous Windows, le dossier des
//! données (`%LOCALAPPDATA%`) n'est ouvert qu'à son propriétaire. Perdue ou
//! illisible, la clé est refaite : il suffit de se réinscrire (quelques
//! secondes de calcul), le quota du jour repart pour la nouvelle clé.

use super::RelayError;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub use relay_proto::sign::Identity;

pub const DIR: &str = "relay";
pub const FILE: &str = "identity.json";

#[derive(Serialize, Deserialize)]
struct OnDisk {
    version: u32,
    seed: String,
    #[serde(default)]
    registered: Vec<String>,
}

/// Clé chargée.
#[derive(Clone)]
pub struct Loaded {
    pub key: Arc<Identity>,
    registered: Vec<String>,
}

impl Loaded {
    pub fn is_registered_with(&self, base: &str) -> bool {
        self.registered.iter().any(|url| url == base)
    }
}

pub struct Store {
    path: PathBuf,
    cache: Mutex<Option<Loaded>>,
}

impl Store {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            cache: Mutex::new(None),
        }
    }

    /// La clé : en mémoire, sinon lue, sinon créée (et écrite).
    pub fn load(&self) -> Result<Loaded, RelayError> {
        let mut cache = self.cache.lock();
        if let Some(loaded) = cache.as_ref() {
            return Ok(loaded.clone());
        }
        let loaded = match read(&self.path) {
            Some(loaded) => loaded,
            None => {
                let loaded = Loaded {
                    key: Arc::new(Identity::generate()),
                    registered: Vec::new(),
                };
                write(&self.path, &loaded)?;
                loaded
            }
        };
        *cache = Some(loaded.clone());
        Ok(loaded)
    }

    pub fn mark_registered(&self, base: &str) -> Result<(), RelayError> {
        self.update(|registered| {
            if !registered.iter().any(|url| url == base) {
                registered.push(base.to_owned());
            }
        })
    }

    pub fn forget_registration(&self, base: &str) -> Result<(), RelayError> {
        self.update(|registered| registered.retain(|url| url != base))
    }

    fn update(&self, change: impl FnOnce(&mut Vec<String>)) -> Result<(), RelayError> {
        let mut loaded = self.load()?;
        change(&mut loaded.registered);
        write(&self.path, &loaded)?;
        *self.cache.lock() = Some(loaded);
        Ok(())
    }
}

fn read(path: &Path) -> Option<Loaded> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return None,
        Err(error) => {
            crate::app_log!("chatonlm: relay key unreadable, a new one is made: {error}");
            return None;
        }
    };
    let parsed = serde_json::from_slice::<OnDisk>(&bytes)
        .ok()
        .filter(|file| file.version == 1)
        .and_then(|file| {
            Some((
                relay_proto::b64::decode_array::<32>(&file.seed)?,
                file.registered,
            ))
        });
    match parsed {
        Some((seed, registered)) => Some(Loaded {
            key: Arc::new(Identity::from_seed(seed)),
            registered,
        }),
        None => {
            crate::app_log!("chatonlm: relay key file malformed, a new one is made");
            None
        }
    }
}

fn write(path: &Path, loaded: &Loaded) -> Result<(), RelayError> {
    let file = OnDisk {
        version: 1,
        seed: relay_proto::b64::encode(loaded.key.seed()),
        registered: loaded.registered.clone(),
    };
    // Seule la sorte d'erreur est gardée (journal de l'app) : ni le message du
    // système ni le chemin.
    let bytes = serde_json::to_vec_pretty(&file)
        .map_err(|_| RelayError::Storage("serialization".to_owned()))?;
    write_private(path, &bytes).map_err(|error| RelayError::Storage(format!("{:?}", error.kind())))
}

/// Écrit un fichier lisible du seul utilisateur, d'un coup.
fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let dir = path
        .parent()
        .ok_or_else(|| std::io::Error::other("no parent"))?;
    std::fs::create_dir_all(dir)?;
    let temporary = path.with_extension("json.tmp");
    let _ = std::fs::remove_file(&temporary);
    {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        let mut file = options.open(&temporary)?;
        std::io::Write::write_all(&mut file, bytes)?;
        file.sync_all()?;
    }
    std::fs::rename(&temporary, path)
}
