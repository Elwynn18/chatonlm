//! Cache disque du ModelHub (35 R2, contrat § 3) : réponses normalisées de
//! Hugging Face, dans `app_data_dir()/modelhub/<genre>/<empreinte>.json`.
//! Écriture atomique (fichier temporaire puis renommage) ; jamais vidé sur une
//! erreur ; seules les réponses analysées sans erreur y sont écrites (par
//! l'appelant). Les genres bornés gardent les entrées les plus récemment
//! utilisées (date de modification du fichier, mise à jour à chaque lecture).
//! Les listes de l'accueil restent aussi en mémoire.

use std::any::Any;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::SystemTime;

use parking_lot::Mutex;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

/// Version du format des fichiers : une autre version est ignorée (relue sur
/// le réseau).
const FORMAT: u32 = 1;

/// Limite d'un genre d'entrées.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Limit {
    None,
    /// Nombre d'entrées.
    Count(usize),
    /// Taille totale des fichiers.
    Bytes(u64),
}

/// Genre d'entrées : sous-dossier, limite, gardé en mémoire.
#[derive(Clone, Copy, Debug)]
pub struct Kind {
    pub dir: &'static str,
    pub limit: Limit,
    pub memory: bool,
}

/// Entrée du cache.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry<T> {
    /// Date de la réponse de Hugging Face (ms depuis 1970).
    pub fetched_at: i64,
    pub etag: Option<String>,
    pub data: T,
}

/// Fichier écrit : l'entrée, avec le format et la clé complète.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StoredRef<'a, T> {
    format: u32,
    key: &'a str,
    fetched_at: i64,
    etag: &'a Option<String>,
    data: &'a T,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Stored<T> {
    format: u32,
    key: String,
    fetched_at: i64,
    etag: Option<String>,
    data: T,
}

pub struct Cache {
    dir: PathBuf,
    memory: Mutex<HashMap<String, Arc<dyn Any + Send + Sync>>>,
}

/// Empreinte FNV-1a 64 bits d'une clé (stable d'une version de Rust à
/// l'autre) ; la clé complète est aussi écrite dans le fichier.
fn fingerprint(key: &str) -> String {
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in key.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

impl Cache {
    pub fn new(dir: PathBuf) -> Self {
        Self {
            dir,
            memory: Mutex::new(HashMap::new()),
        }
    }

    fn path(&self, kind: Kind, key: &str) -> PathBuf {
        self.dir
            .join(kind.dir)
            .join(format!("{}.json", fingerprint(key)))
    }

    /// Entrée de `key`, si elle existe et se lit.
    pub async fn read<T>(&self, kind: Kind, key: &str) -> Option<Arc<Entry<T>>>
    where
        T: DeserializeOwned + Send + Sync + 'static,
    {
        if kind.memory
            && let Some(found) = self.memory.lock().get(key).cloned()
        {
            return found.downcast::<Entry<T>>().ok();
        }
        let path = self.path(kind, key);
        let owned_key = key.to_owned();
        let entry = tauri::async_runtime::spawn_blocking(move || read_file::<T>(&path, &owned_key))
            .await
            .ok()??;
        let entry = Arc::new(entry);
        if kind.memory {
            self.memory.lock().insert(key.to_owned(), entry.clone());
        }
        Some(entry)
    }

    /// Écrit l'entrée de `key` (atomique), puis applique la limite du genre.
    /// Un échec d'écriture laisse l'ancienne entrée.
    pub async fn write<T>(&self, kind: Kind, key: &str, entry: Arc<Entry<T>>)
    where
        T: Serialize + Send + Sync + 'static,
    {
        if kind.memory {
            self.memory.lock().insert(key.to_owned(), entry.clone());
        }
        let path = self.path(kind, key);
        let owned_key = key.to_owned();
        let written = tauri::async_runtime::spawn_blocking(move || {
            let result = write_file(&path, &owned_key, &entry);
            if let Some(dir) = path.parent() {
                evict(dir, kind.limit, &path);
            }
            result
        })
        .await;
        if let Ok(Err(error)) = written {
            eprintln!("chatonlm: cache du ModelHub non écrit : {error}");
        }
    }
}

fn read_file<T: DeserializeOwned>(path: &Path, key: &str) -> Option<Entry<T>> {
    let bytes = fs::read(path).ok()?;
    let stored: Stored<T> = serde_json::from_slice(&bytes).ok()?;
    if stored.format != FORMAT || stored.key != key {
        return None;
    }
    // Utilisée : la plus récente pour la limite de son genre.
    if let Ok(file) = fs::File::options().write(true).open(path) {
        let _ = file.set_modified(SystemTime::now());
    }
    Some(Entry {
        fetched_at: stored.fetched_at,
        etag: stored.etag,
        data: stored.data,
    })
}

fn write_file<T: Serialize>(path: &Path, key: &str, entry: &Entry<T>) -> std::io::Result<()> {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let dir = path.parent().expect("cache file in a directory");
    fs::create_dir_all(dir)?;
    let bytes = serde_json::to_vec(&StoredRef {
        format: FORMAT,
        key,
        fetched_at: entry.fetched_at,
        etag: &entry.etag,
        data: &entry.data,
    })
    .map_err(std::io::Error::other)?;
    let temporary = dir.join(format!(
        ".{}-{}.tmp",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    fs::write(&temporary, &bytes)?;
    fs::rename(&temporary, path).inspect_err(|_| {
        let _ = fs::remove_file(&temporary);
    })
}

/// Retire les entrées les moins récemment utilisées au-delà de la limite,
/// jamais `keep` (celle qu'on vient d'écrire).
fn evict(dir: &Path, limit: Limit, keep: &Path) {
    if limit == Limit::None {
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut files: Vec<(SystemTime, u64, PathBuf)> = entries
        .filter_map(Result::ok)
        .filter(|entry| entry.path().extension().is_some_and(|e| e == "json"))
        .filter_map(|entry| {
            let meta = entry.metadata().ok()?;
            Some((meta.modified().ok()?, meta.len(), entry.path()))
        })
        .collect();
    // Les plus anciennes d'abord.
    files.sort_by(|a, b| {
        a.0.cmp(&b.0)
            .then_with(|| (a.2 == keep).cmp(&(b.2 == keep)))
    });
    let mut count = files.len();
    let mut bytes: u64 = files.iter().map(|f| f.1).sum();
    for (_, size, path) in files {
        let over = match limit {
            Limit::Count(max) => count > max,
            Limit::Bytes(max) => bytes > max,
            Limit::None => false,
        };
        if !over {
            break;
        }
        if path == keep {
            continue;
        }
        if fs::remove_file(&path).is_ok() {
            count -= 1;
            bytes = bytes.saturating_sub(size);
        }
    }
}

#[cfg(test)]
impl Cache {
    /// Vieillit une entrée de `ms` (essais de la fraîcheur).
    pub async fn age<T>(&self, kind: Kind, key: &str, ms: i64)
    where
        T: Serialize + DeserializeOwned + Clone + Send + Sync + 'static,
    {
        let entry = self.read::<T>(kind, key).await.expect("entry to age");
        let mut aged = (*entry).clone();
        aged.fetched_at -= ms;
        self.write(kind, key, Arc::new(aged)).await;
    }

    /// Fichiers `.json` d'un genre.
    pub fn files(&self, kind: Kind) -> Vec<PathBuf> {
        fs::read_dir(self.dir.join(kind.dir))
            .map(|entries| {
                entries
                    .filter_map(Result::ok)
                    .map(|entry| entry.path())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Oublie la copie en mémoire (relecture du disque).
    pub fn forget_memory(&self) {
        self.memory.lock().clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PLAIN: Kind = Kind {
        dir: "plain",
        limit: Limit::None,
        memory: false,
    };

    fn entry(data: &str) -> Arc<Entry<String>> {
        Arc::new(Entry {
            fetched_at: 1,
            etag: Some("W/\"1\"".into()),
            data: data.to_owned(),
        })
    }

    #[tokio::test]
    async fn entries_are_written_and_read_back() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path().join("modelhub"));
        assert!(cache.read::<String>(PLAIN, "a").await.is_none());
        cache.write(PLAIN, "a", entry("hello")).await;
        let read = cache.read::<String>(PLAIN, "a").await.unwrap();
        assert_eq!(read.data, "hello");
        assert_eq!(read.etag.as_deref(), Some("W/\"1\""));
        // Pas de fichier temporaire laissé.
        let files = cache.files(PLAIN);
        assert_eq!(files.len(), 1);
        assert!(files[0].extension().is_some_and(|e| e == "json"));
        // Autre type, fichier abîmé, autre format : ignorés.
        assert!(cache.read::<u32>(PLAIN, "a").await.is_none());
        fs::write(&files[0], b"{ broken").unwrap();
        assert!(cache.read::<String>(PLAIN, "a").await.is_none());
        let other_format = serde_json::json!({
            "format": FORMAT + 1, "key": "a", "fetchedAt": 1, "etag": null, "data": "x"
        });
        fs::write(&files[0], other_format.to_string()).unwrap();
        assert!(cache.read::<String>(PLAIN, "a").await.is_none());
    }

    #[tokio::test]
    async fn bounded_kinds_keep_the_most_recently_used() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path().to_owned());
        let counted = Kind {
            dir: "counted",
            limit: Limit::Count(2),
            memory: false,
        };
        for key in ["a", "b"] {
            cache.write(counted, key, entry(key)).await;
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        // « a » relu : « b » devient la plus ancienne.
        assert!(cache.read::<String>(counted, "a").await.is_some());
        std::thread::sleep(std::time::Duration::from_millis(20));
        cache.write(counted, "c", entry("c")).await;
        assert_eq!(cache.files(counted).len(), 2);
        assert!(cache.read::<String>(counted, "b").await.is_none());
        assert!(cache.read::<String>(counted, "a").await.is_some());
        assert!(cache.read::<String>(counted, "c").await.is_some());

        let sized = Kind {
            dir: "sized",
            limit: Limit::Bytes(400),
            memory: false,
        };
        for key in ["x", "y", "z"] {
            cache.write(sized, key, entry(&"-".repeat(150))).await;
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let left = cache.files(sized);
        assert!(left.len() < 3, "{left:?}");
        assert!(cache.read::<String>(sized, "z").await.is_some());
    }

    #[tokio::test]
    async fn memory_kinds_are_kept_in_memory() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path().to_owned());
        let lists = Kind {
            dir: "lists",
            limit: Limit::None,
            memory: true,
        };
        cache.write(lists, "popular", entry("p")).await;
        fs::remove_dir_all(dir.path().join("lists")).unwrap();
        assert_eq!(
            cache.read::<String>(lists, "popular").await.unwrap().data,
            "p"
        );
        cache.forget_memory();
        assert!(cache.read::<String>(lists, "popular").await.is_none());
    }

    #[test]
    fn fingerprints_are_stable() {
        assert_eq!(fingerprint(""), "cbf29ce484222325");
        assert_eq!(fingerprint("a"), "af63dc4c8601ec8c");
        assert_ne!(fingerprint("repo:a/b"), fingerprint("repo:a/c"));
    }
}
