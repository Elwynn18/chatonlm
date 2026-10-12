//! Dossier des modèles d'Ollama (Settings > Model location).
//!
//! Le dossier appartient au serveur, pas à ChatonLM : c'est `OLLAMA_MODELS` du
//! processus `ollama serve` auquel l'app parle. Ce module porte ce qui est
//! commun aux deux systèmes : les types lus par la page, le contenu d'un
//! dossier ([`folder_kind`]), les modèles du serveur ([`models_summary`]),
//! le verrou d'une opération. Le reste dépend du système, sous les mêmes
//! noms (`detect`, `check_folder`) :
//! - Linux (`models_dir/linux.rs`) : détection par `/proc` et D-Bus,
//!   changement du dossier du service systemd par un script lancé en root
//!   (`pkexec`), commandes à copier ailleurs ; dossier `.ollama` caché au
//!   service dans [`home`] ;
//! - Windows (`models_dir/windows.rs`) : détection par la table TCP du
//!   système ; l'Ollama de la machine n'est jamais changé par ChatonLM
//!   (guidage seulement : son dossier est un réglage de l'app officielle).
//!
//! L'Ollama intégré, lui, appartient à ChatonLM : son dossier se change de la
//! même façon sur les deux systèmes ([`builtin`]).
//!
//! Conception, preuves et banc d'essai : `.tools/research/36-modelhub-models-dir.md`.

use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use ts_rs::TS;
use url::Url;

use super::client;

/// Place libre exigée en plus des modèles copiés vers un autre disque (comme
/// le script).
pub const SPACE_MARGIN: u64 = 100 * 1024 * 1024;

pub mod builtin;
#[cfg(target_os = "linux")]
pub mod home;
#[cfg(target_os = "linux")]
mod linux;
pub mod rules;
#[cfg(windows)]
mod windows;

// Mêmes noms qu'avant le découpage : les éléments privés de la partie Linux
// (visibles de ce module seulement) le restent pour `home` et les essais.
#[cfg(target_os = "linux")]
pub use linux::*;
#[cfg(windows)]
pub use windows::*;

/// Délai de la liste des modèles (`/api/tags`), pour leur nombre et leur taille.
const SUMMARY_TIMEOUT: Duration = Duration::from_secs(5);

/// Intervalle des relevés de taille du fichier en cours de copie.
const PROGRESS_INTERVAL: Duration = Duration::from_millis(500);

/// Où Ollama range ses modèles, et ce que l'app peut y faire.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[ts(export)]
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub enum ModelsLocation {
    /// Service systemd du système (`ollama.service`) : l'app peut le modifier.
    #[serde(rename_all = "camelCase")]
    SystemService {
        /// Dossier actuel (ou configuré, service arrêté) ; `null` si inconnu.
        dir: Option<String>,
        /// Utilisateur du service ; `root` sans `User=` ou pour un uid 0 :
        /// l'app ne change pas alors le dossier elle-même (commandes à
        /// copier).
        user: String,
        /// `Group=` ; vide : groupe principal de l'utilisateur.
        group: String,
        /// Le service tourne : il sera redémarré.
        running: bool,
        /// Le drop-in de ChatonLM existe déjà.
        managed: bool,
        /// pkexec (setuid root) est présent.
        can_elevate: bool,
        /// Fichier de `EnvironmentFile=` qui pose `OLLAMA_MODELS` : à changer
        /// là, l'app ne le peut pas.
        env_file: Option<String>,
    },
    /// Service systemd de l'utilisateur (`systemctl --user`).
    UserService { dir: Option<String> },
    /// `ollama serve` lancé à la main.
    #[serde(rename_all = "camelCase")]
    Manual {
        /// Lu dans son environnement.
        dir: Option<String>,
        /// Valeur de `OLLAMA_HOST` à redonner à `ollama serve` (port non
        /// standard).
        host_env: Option<String>,
    },
    /// Windows : l'Ollama de l'app de bureau officielle (`ollama app.exe`,
    /// zone de notification). Son dossier est un réglage de cette app :
    /// ChatonLM ne le change pas (guidage seulement). `dir` : dossier probable
    /// (variable d'utilisateur `OLLAMA_MODELS`, sinon
    /// `%USERPROFILE%\.ollama\models`).
    DesktopApp { dir: Option<String> },
    /// Ollama dans un conteneur.
    Container,
    /// Serveur d'une autre machine.
    Remote { host: String },
    /// Lancement inconnu ; `running` : quelque chose répond à l'adresse.
    Unknown { running: bool },
    /// Ollama intégré : ChatonLM change lui-même son dossier
    /// (`change_builtin_models_dir`), sans droits ni invite.
    #[serde(rename_all = "camelCase")]
    Builtin {
        dir: String,
        /// Dossier par défaut (`<données de l'app>/ollama/models`).
        is_default: bool,
    },
}

/// Vérification d'un dossier choisi, sans droits ni invite.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct FolderCheck {
    /// Chemin canonique (liens résolus) : celui que reçoit le script.
    pub path: String,
    /// Raison d'un refus, avant toute invite.
    pub problem: Option<FolderProblem>,
    /// Dossier actuel, sous la forme que le script accepte ; `null` : les
    /// modèles ne peuvent pas être déplacés (dossier inconnu).
    pub old: Option<String>,
    /// Le dossier choisi contient déjà des modèles (fusion).
    pub has_models: bool,
    /// Même système de fichiers que l'ancien dossier : renommage instantané.
    pub same_drive: bool,
    /// Modèles du serveur (`/api/tags`), si le serveur répond.
    pub models: Option<ModelsSummary>,
    /// Place libre à cet endroit.
    pub free_bytes: Option<u64>,
    /// Commandes à copier (lancement à la main, service de l'utilisateur,
    /// service système sans pkexec ou sans agent d'authentification).
    pub commands: Option<String>,
    /// Des réponses du chat sont en cours : elles s'arrêteront.
    pub replies_running: bool,
    /// Des téléchargements de modèles sont en cours : changement refusé.
    pub downloads_running: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub enum FolderProblem {
    /// Dossier du système, ou racine dont le propriétaire changerait.
    System,
    /// Caractère que systemd interpréterait (`" \ : $` et accent grave),
    /// caractère de contrôle, chemin trop long ou non UTF-8.
    BadCharacters,
    /// Ni vide, ni dossier de modèles d'Ollama.
    NotEmpty,
    /// Dans l'ancien dossier, ou le contient.
    Inside,
    /// C'est déjà le dossier des modèles.
    Same,
    /// Dossier (ou dossier parent) introuvable.
    Missing,
    /// Dossier existant d'un autre compte (ni l'utilisateur de l'app, ni
    /// celui du service, root compris : `/var/empty`, `/srv/x`) : il serait
    /// donné au service.
    NotYours,
}

/// Modèles installés, d'après le serveur.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ModelsSummary {
    pub count: u32,
    pub bytes: u64,
}

/// Progression d'un changement.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(tag = "type", rename_all = "camelCase")]
#[ts(export)]
pub enum MoveEvent {
    /// Étape commencée.
    Step { step: MoveStep },
    /// Octets à copier (0 : renommages seulement).
    Total { bytes: u64 },
    /// Octets copiés jusqu'ici (fichier en cours compris).
    Progress { done: u64 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum MoveStep {
    Prepare,
    Stop,
    Move,
    Config,
    Start,
    Cleanup,
}

/// Fin d'un changement.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[ts(export)]
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub enum MoveOutcome {
    #[serde(rename_all = "camelCase")]
    Done {
        /// Le service tournait et a redémarré sur le nouveau dossier.
        restarted: bool,
        renamed: u32,
        copied: u32,
        /// Fichiers déjà présents dans le nouveau dossier (fusion).
        skipped: u32,
        /// Les anciennes copies n'ont pas pu être effacées.
        cleanup_failed: bool,
    },
    /// Invite fermée : rien n'a été exécuté.
    Cancelled,
    Failed {
        reason: MoveFailure,
        /// Échec après modification : tout a été remis comme avant.
        restored: bool,
        /// Sortie d'erreur (lignes `chatonlm: …` du script, message de pkexec).
        detail: String,
    },
}

/// Raison d'un échec (codes de sortie du script et de pkexec).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[ts(export)]
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub enum MoveFailure {
    /// 127, pas d'agent d'authentification dans la session.
    NoAgent,
    /// 127, mot de passe refusé ou autre refus de polkit.
    NotAuthorized,
    /// 10, 12 : chemin refusé.
    Refused,
    /// 11 : unité absente ou changée.
    UnitChanged,
    /// 13.
    NotEmpty,
    /// 14, ou vérification de l'app avant l'invite ; en octets.
    NoSpace {
        need: Option<u64>,
        free: Option<u64>,
    },
    /// 15 : le service ne peut pas écrire dans le dossier.
    CannotWrite,
    /// 16 : `OLLAMA_MODELS` posé par ce fichier.
    EnvironmentFile { file: Option<String> },
    /// 20.
    StopFailed,
    /// 21.
    MoveFailed,
    /// 22.
    ConfigFailed,
    /// 23 : un autre réglage du service l'emporte.
    Overridden,
    /// 24.
    StartFailed,
    /// 30 : restauration incomplète.
    RestoreFailed,
    /// 17, signal, sortie illisible.
    Unexpected,
    /// Intégré : le nouveau dossier a déjà un modèle de ce nom, d'un autre
    /// contenu ; rien n'a été déplacé (la fusion perdrait l'une des deux
    /// versions).
    ModelConflict { model: String },
    /// Intégré : lien symbolique ou fichier spécial dans le dossier des
    /// modèles (chemin complet), que ChatonLM ne déplace pas ; rien n'a été
    /// déplacé.
    Symlink { path: String },
}

/// Dossier `.ollama` que le service système ne voit pas.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct HiddenHome {
    /// `$HOME/.ollama` du service.
    pub dir: String,
    /// Dossier que `ProtectHome=` masque : `/home`, `/root` ou `/run/user`.
    pub hidden_root: String,
    /// Utilisateur du service (`root` sans `User=`).
    pub user: String,
    /// Le service tourne : il redémarrera.
    pub running: bool,
    /// pkexec (setuid root) est présent : « Fix » possible (sauf service
    /// root).
    pub can_elevate: bool,
    /// Commandes à copier (pkexec absent, pas d'agent d'authentification) ;
    /// `None` pour un service root, que l'app ne change pas : avec
    /// `BindPaths=`, systemd suivrait, en root et en écriture, un lien glissé
    /// par le propriétaire du `HOME` à la place de `.ollama`.
    pub commands: Option<String>,
}

/// Fin d'une correction.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[ts(export)]
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub enum HomeFixOutcome {
    /// `changed` faux : rien à faire (le dossier n'était plus caché).
    Done { changed: bool, restarted: bool },
    /// Invite fermée : rien n'a été exécuté.
    Cancelled,
    Failed {
        reason: HomeFixFailure,
        /// Échec après modification : tout a été remis comme avant.
        restored: bool,
        /// Sortie d'erreur (lignes `chatonlm: …` du script, message de pkexec).
        detail: String,
    },
}

/// Raison d'un échec (codes de sortie du script et de pkexec).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub enum HomeFixFailure {
    /// 127, pas d'agent d'authentification dans la session.
    NoAgent,
    /// 127, mot de passe refusé ou autre refus de polkit.
    NotAuthorized,
    /// 11 : unité absente, ou autre `HOME`.
    UnitChanged,
    /// 12 : `.ollama` n'est pas un dossier, ou appartient à un autre compte.
    NotYours,
    /// 15 : le service ne peut pas créer `.ollama`.
    CannotCreate,
    /// 22.
    ConfigFailed,
    /// 23 : un autre réglage du service l'emporte.
    Overridden,
    /// 24 : le service ne redémarre pas, ou ne voit toujours pas le dossier.
    StartFailed,
    /// 30 : restauration incomplète.
    RestoreFailed,
    /// 10, 17, signal, sortie illisible.
    Unexpected,
}

/// Résultat de la détection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Detection {
    pub location: ModelsLocation,
    /// `ProtectHome=` et `ProtectSystem=` du service système (drop-in des
    /// commandes à copier).
    pub protect_home: String,
    pub protect_system: String,
    /// `OLLAMA_HOST` pour lancer `ollama serve` à l'adresse réglée.
    pub host_env: Option<String>,
    /// Dossier `.ollama` du service système (`$HOME/.ollama`), rendu visible
    /// avec un dossier des modèles monté par `BindPaths=`.
    pub ollama_dir: Option<String>,
    /// Ce dossier, quand `ProtectHome=` le cache au service (alerte de
    /// Settings > General et « Fix »).
    pub hidden_home: Option<HiddenHome>,
}

impl Detection {
    fn new(location: ModelsLocation, host_env: Option<String>) -> Detection {
        Detection {
            location,
            protect_home: String::new(),
            protect_system: String::new(),
            host_env,
            ollama_dir: None,
            hidden_home: None,
        }
    }
}

/// Contenu d'un dossier cible, comme `kind` du script.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FolderKind {
    Missing,
    NotFolder,
    Empty,
    /// Seulement `blobs`, `manifests`, `metadata`.
    Models,
    Other,
}

pub fn folder_kind(path: &Path) -> FolderKind {
    let meta = match fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return FolderKind::Missing,
        Err(_) => return FolderKind::Other,
    };
    if !meta.is_dir() {
        return FolderKind::NotFolder;
    }
    let Ok(entries) = fs::read_dir(path) else {
        // Illisible : rien ne dit qu'il est vide.
        return FolderKind::Other;
    };
    let mut kind = FolderKind::Empty;
    for entry in entries {
        let Ok(entry) = entry else {
            return FolderKind::Other;
        };
        match entry.file_name().to_str() {
            Some("blobs" | "manifests" | "metadata") => kind = FolderKind::Models,
            _ => return FolderKind::Other,
        }
    }
    kind
}

/// Dossier actuel d'après la détection.
pub fn current_dir(location: &ModelsLocation) -> Option<&str> {
    match location {
        ModelsLocation::SystemService { dir, .. }
        | ModelsLocation::UserService { dir }
        | ModelsLocation::Manual { dir, .. }
        | ModelsLocation::DesktopApp { dir } => dir.as_deref(),
        ModelsLocation::Builtin { dir, .. } => Some(dir),
        _ => None,
    }
}

#[derive(Deserialize)]
struct Tags {
    #[serde(default)]
    models: Vec<Tag>,
}

#[derive(Deserialize)]
struct Tag {
    #[serde(default)]
    size: u64,
    #[serde(default)]
    remote_host: Option<String>,
}

/// Nombre et taille des modèles du serveur (`/api/tags`, modèles cloud
/// exclus : ils n'ont rien sur le disque).
pub async fn models_summary(base: &str) -> Option<ModelsSummary> {
    let url = Url::parse(base).ok()?;
    let client = client::client(&url, Some(SUMMARY_TIMEOUT)).ok()?;
    let response = client.get(format!("{base}/api/tags")).send().await.ok()?;
    if !response.status().is_success() {
        return None;
    }
    let tags: Tags = response.json().await.ok()?;
    let local: Vec<&Tag> = tags
        .models
        .iter()
        .filter(|tag| tag.remote_host.as_deref().is_none_or(str::is_empty))
        .collect();
    Some(ModelsSummary {
        count: u32::try_from(local.len()).unwrap_or(u32::MAX),
        bytes: local.iter().map(|tag| tag.size).sum(),
    })
}

/// Octets à copier vers un autre disque, et refus si la place manque (comme
/// le script, avant l'invite).
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn space_shortage(check: &FolderCheck, move_models: bool) -> Option<MoveFailure> {
    if !move_models || check.same_drive {
        return None;
    }
    let need = check.models?.bytes;
    let free = check.free_bytes?;
    (need.saturating_add(SPACE_MARGIN) > free).then_some(MoveFailure::NoSpace {
        need: Some(need),
        free: Some(free),
    })
}

/// Une seule opération à la fois.
pub struct OperationLock(AtomicBool);

/// Tenu pendant l'opération ; la libère en disparaissant.
pub struct OperationGuard(&'static OperationLock);

impl OperationLock {
    pub const fn new() -> OperationLock {
        OperationLock(AtomicBool::new(false))
    }

    /// `None` si une opération est déjà en cours.
    pub fn try_acquire(&'static self) -> Option<OperationGuard> {
        self.0
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
            .then_some(OperationGuard(self))
    }

    /// Une opération est en cours (un téléchargement ne doit pas démarrer).
    pub fn is_held(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

impl Drop for OperationGuard {
    fn drop(&mut self) {
        self.0.0.store(false, Ordering::Release);
    }
}

// Essais de la partie Linux : avec elle (`linux.rs`) ; ceux de `home`
// reprennent leurs aides sous ce nom.
#[cfg(all(test, target_os = "linux"))]
use linux::tests;
