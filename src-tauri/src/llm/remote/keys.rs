//! Clés d'API des fournisseurs distants : `<données>/remote/keys.json`
//! (`{"version": 1, "keys": {"<fournisseur>": "<clé>"}}`), jamais en base.
//!
//! Écrit d'un coup (fichier temporaire renommé), lisible du seul
//! utilisateur : mode 0600 sous Linux ; sous Windows, le dossier des données
//! (`%LOCALAPPDATA%`) n'est ouvert qu'à son propriétaire. Une clé ne sort d'ici que vers l'en-tête
//! `Authorization` d'un fournisseur : la page n'en reçoit que l'indice
//! ([`hint`]), le journal rien.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

pub const DIR: &str = "remote";
pub const FILE: &str = "keys.json";

/// Une écriture à la fois (lecture, changement, écriture).
static WRITING: Mutex<()> = Mutex::new(());

#[derive(Default, Serialize, Deserialize)]
struct OnDisk {
    version: u32,
    #[serde(default)]
    keys: BTreeMap<String, String>,
}

/// Fichier des clés d'une installation.
pub struct Keys {
    path: PathBuf,
}

impl Keys {
    /// `data_dir` : dossier des données de l'app.
    pub fn new(data_dir: &Path) -> Keys {
        Keys {
            path: data_dir.join(DIR).join(FILE),
        }
    }

    #[cfg_attr(
        not(test),
        allow(dead_code, reason = "lu par les essais (droits du fichier)")
    )]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Clé du fournisseur `id`, s'il en a une (fichier illisible : aucune).
    pub fn get(&self, id: &str) -> Option<String> {
        self.read().ok()?.keys.get(id).cloned()
    }

    /// Pose (ou remplace) la clé du fournisseur `id`. Fichier illisible un
    /// instant (droits, antivirus qui le tient ouvert sous Windows) : échec,
    /// rien n'est écrit (les autres clés ne sont pas perdues).
    pub fn set(&self, id: &str, key: &str) -> std::io::Result<()> {
        let _writing = WRITING.lock();
        let mut file = self.read()?;
        file.keys.insert(id.to_owned(), key.to_owned());
        self.write(&file)
    }

    /// Retire la clé du fournisseur `id` (rien s'il n'en a pas) ; fichier
    /// illisible : échec, rien n'est écrit.
    pub fn remove(&self, id: &str) -> std::io::Result<()> {
        let _writing = WRITING.lock();
        let mut file = self.read()?;
        if file.keys.remove(id).is_some() {
            self.write(&file)?;
        }
        Ok(())
    }

    /// Contenu du fichier : absent → vide ; illisible → erreur ; mal formé
    /// (jamais écrit ainsi par l'app) → vide, les clés sont à ressaisir.
    fn read(&self) -> std::io::Result<OnDisk> {
        let bytes = match std::fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(OnDisk::default());
            }
            Err(error) => {
                super::note(&format!("key file unreadable ({:?})", error.kind()));
                return Err(error);
            }
        };
        Ok(serde_json::from_slice::<OnDisk>(&bytes)
            .ok()
            .filter(|file| file.version == 1)
            .unwrap_or_else(|| {
                super::note("key file malformed: keys to enter again");
                OnDisk::default()
            }))
    }

    fn write(&self, file: &OnDisk) -> std::io::Result<()> {
        let file = OnDisk {
            version: 1,
            keys: file.keys.clone(),
        };
        let bytes = serde_json::to_vec_pretty(&file).map_err(std::io::Error::other)?;
        write_private(&self.path, &bytes)
    }
}

/// Indice d'une clé pour la page : ses quatre derniers caractères, si elle
/// en a au moins huit (une clé plus courte ne se montre pas du tout).
pub fn hint(key: &str) -> Option<String> {
    let count = key.chars().count();
    (count >= 8).then(|| key.chars().skip(count - 4).collect())
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
