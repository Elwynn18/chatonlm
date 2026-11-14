//! Outils tiers téléchargés à la demande pour l'outil Transcription :
//! **yt-dlp** (Unlicense ; composants embarqués sous leurs licences, dont
//! GPL et LGPL : `_internal/THIRD_PARTY_LICENSES.txt`) et **Deno** (MIT),
//! le moteur JavaScript que yt-dlp emploie pour YouTube. ChatonLM ne les
//! livre jamais : l'utilisateur les fait télécharger depuis leurs Releases
//! GitHub officielles, licences acceptées sur la page (comme le pack
//! NVIDIA, `engine::cuda`).
//!
//! - [`release`] : dernière version par la redirection de
//!   `releases/latest` (sans l'API, limitée à 60 requêtes par heure),
//!   sommes (`SHA2-256SUMS` de yt-dlp, `.sha256sum` de Deno aux formats de
//!   Linux et de PowerShell), taille par `Range: bytes=0-0`.
//! - [`openpgp`] : signature de `SHA2-256SUMS` vérifiée par `ring` avec la
//!   clé de yt-dlp épinglée (yt-dlp, lui, ne la vérifie pas).
//! - [`install`] : archive onedir extraite avec son arborescence assainie
//!   (ni `..`, ni chemin absolu, ni lien, ni nom réservé de Windows), droits
//!   d'exécution (Linux), `--version` lancé une fois avant la bascule du
//!   pointeur `current`, anciennes versions effacées après (ou au
//!   lancement suivant, si un fichier est encore ouvert sous Windows).
//! - [`manager`] : état Tauri [`Tools`], événement [`STATUS_EVENT`], file
//!   commune des téléchargements (reprise, SHA-256, progression), suivi des
//!   versions, et ce que le lot 4 (lancement de yt-dlp) reçoit :
//!   [`Tools::ready`], [`ToolsLease`], [`ToolPaths`].
//!
//! Dossier : `<données>/tools/` : `yt-dlp/<version>/` et `deno/<version>/`
//! (installés, `current` = version en service, `checked.json` = dernier
//! relevé de la dernière version), `downloads/` (archives, reprises),
//! `deno-cache/` (`DENO_DIR`, jamais `~/.cache/deno`), `yt-dlp-cache/`,
//! `no-ffmpeg/` (dossier vide donné à `--ffmpeg-location`).
//!
//! Adresses : `https://github.com/yt-dlp/yt-dlp` et
//! `https://github.com/denoland/deno` ; essais (builds de débogage et e2e
//! seulement) : `CHATONLM_YTDLP_RELEASES_URL`, `CHATONLM_DENO_RELEASES_URL`
//! (dépôts) et `CHATONLM_TOOLS_PUBKEY` (clé qui signe les sommes du faux).

pub mod install;
pub mod manager;
pub mod openpgp;
pub mod release;

#[cfg(target_os = "linux")]
#[path = "linux.rs"]
mod os;
#[cfg(windows)]
#[path = "windows.rs"]
mod os;
#[cfg(not(any(target_os = "linux", windows)))]
mod os {
    pub const PLATFORM: Option<super::Platform> = None;
    pub fn set_executable(_path: &std::path::Path, _executable: bool) -> std::io::Result<()> {
        Ok(())
    }
}

use serde::{Deserialize, Serialize};
use ts_rs::TS;

pub use manager::{STATUS_EVENT, ToolPaths, Tools, ToolsLease};

/// Système des archives.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Platform {
    Linux,
    // Construit par `windows.rs` seulement (et par les essais).
    #[cfg_attr(not(windows), allow(dead_code))]
    Windows,
}

impl Platform {
    /// Système de cette app (x86-64 seulement), sinon `None`.
    pub fn current() -> Option<Platform> {
        os::PLATFORM
    }
}

/// Outil téléchargé à la demande.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum ToolId {
    YtDlp,
    Deno,
}

impl ToolId {
    pub const ALL: [ToolId; 2] = [ToolId::YtDlp, ToolId::Deno];

    /// Nom de dossier, d'élément de la file et de journal.
    pub fn key(self) -> &'static str {
        match self {
            ToolId::YtDlp => "yt-dlp",
            ToolId::Deno => "deno",
        }
    }

    /// Nom montré (nom propre, non traduit).
    pub fn name(self) -> &'static str {
        match self {
            ToolId::YtDlp => "yt-dlp",
            ToolId::Deno => "Deno",
        }
    }

    /// Archive de la Release.
    pub fn asset(self, platform: Platform) -> &'static str {
        match (self, platform) {
            (ToolId::YtDlp, Platform::Linux) => "yt-dlp_linux.zip",
            (ToolId::YtDlp, Platform::Windows) => "yt-dlp_win.zip",
            (ToolId::Deno, Platform::Linux) => "deno-x86_64-unknown-linux-gnu.zip",
            (ToolId::Deno, Platform::Windows) => "deno-x86_64-pc-windows-msvc.zip",
        }
    }

    /// Programme, à la racine de l'archive.
    pub fn binary(self, platform: Platform) -> &'static str {
        match (self, platform) {
            (ToolId::YtDlp, Platform::Linux) => "yt-dlp_linux",
            (ToolId::YtDlp, Platform::Windows) => "yt-dlp.exe",
            (ToolId::Deno, Platform::Linux) => "deno",
            (ToolId::Deno, Platform::Windows) => "deno.exe",
        }
    }

    /// Licence de l'outil (SPDX).
    pub fn license(self) -> &'static str {
        match self {
            ToolId::YtDlp => "Unlicense",
            ToolId::Deno => "MIT",
        }
    }

    /// Texte de la licence, en ligne.
    pub fn license_url(self) -> &'static str {
        match self {
            ToolId::YtDlp => "https://github.com/yt-dlp/yt-dlp/blob/master/LICENSE",
            ToolId::Deno => "https://github.com/denoland/deno/blob/main/LICENSE.md",
        }
    }

    /// Licences des composants embarqués, dans le dossier installé.
    pub fn third_party_licenses(self) -> Option<&'static str> {
        match self {
            ToolId::YtDlp => Some("_internal/THIRD_PARTY_LICENSES.txt"),
            ToolId::Deno => None,
        }
    }

    /// Taille estimée de l'archive (avant de connaître la vraie).
    pub fn estimate(self, platform: Platform) -> u64 {
        match (self, platform) {
            (ToolId::YtDlp, Platform::Linux) => 41_300_000,
            (ToolId::YtDlp, Platform::Windows) => 23_900_000,
            (ToolId::Deno, Platform::Linux) => 42_000_000,
            (ToolId::Deno, Platform::Windows) => 43_000_000,
        }
    }

    /// Intervalle entre deux relevés de la dernière version : un jour pour
    /// yt-dlp (versions fréquentes, souvent pour suivre YouTube), une
    /// semaine pour Deno.
    pub fn check_period(self) -> std::time::Duration {
        const DAY: u64 = 24 * 60 * 60;
        std::time::Duration::from_secs(match self {
            ToolId::YtDlp => DAY,
            ToolId::Deno => 7 * DAY,
        })
    }
}

/// Étape d'un outil.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum ToolPhase {
    /// Pas installé (ni en cours).
    Absent,
    /// Dernière version, sommes et signature en cours de lecture.
    Fetching,
    /// En file ou en téléchargement (progression).
    Downloading,
    /// Somme de l'archive reçue en cours de vérification.
    Verifying,
    /// Extraction, `--version`, bascule.
    Installing,
    Installed,
    /// Installé, une version plus récente est publiée.
    UpdateAvailable,
    /// Pas installé, et la dernière tentative a échoué (`error`).
    Failed,
}

/// Nature d'une erreur (le texte est dans `message`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum ToolErrorCode {
    /// Système sans archive officielle (autre que Linux ou Windows x86-64).
    Unsupported,
    /// GitHub injoignable ou réponse inattendue.
    Network,
    /// Release incomplète (archive ou somme absente : nom changé).
    Release,
    /// Signature des sommes fausse ou d'une autre clé (ChatonLM à mettre à
    /// jour si la clé de yt-dlp a changé).
    Signature,
    /// Téléchargement interrompu, archive abîmée (somme fausse).
    Download,
    /// Archive illisible ou dangereuse (chemin hors du dossier, lien).
    Archive,
    /// Le programme installé ne se lance pas (antivirus, bibliothèque).
    Launch,
    /// Version inattendue ou trop ancienne.
    Version,
    /// Place insuffisante.
    Space,
    /// Écriture ou effacement impossible.
    Io,
}

/// Erreur d'un outil : nature et message dans la langue du moment.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ToolError {
    pub code: ToolErrorCode,
    pub message: String,
}

/// État d'un outil.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ToolStatus {
    pub tool: ToolId,
    /// Nom montré (« yt-dlp », « Deno »).
    pub name: String,
    pub phase: ToolPhase,
    /// Version installée et vérifiée (« 2026.10.26 », « 2.9.8 »).
    pub version: Option<String>,
    /// Dernière version relevée (même forme).
    pub latest: Option<String>,
    /// Date du dernier relevé (ms depuis l'époque Unix).
    pub checked_at: Option<i64>,
    pub completed_bytes: u64,
    pub total_bytes: u64,
    /// À télécharger (taille réelle une fois lue, sinon estimée).
    pub download_bytes: u64,
    pub installed_bytes: Option<u64>,
    /// Licence de l'outil (SPDX) et son texte en ligne.
    pub license: String,
    pub license_url: String,
    /// Licences des composants embarqués (fichier installé), yt-dlp
    /// seulement.
    pub third_party_licenses: Option<String>,
    /// Dernière erreur (relevé, téléchargement, installation, retrait).
    pub error: Option<ToolError>,
}

/// État des deux outils.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ToolsStatus {
    /// Ce système a des archives officielles (Linux ou Windows x86-64).
    pub supported: bool,
    /// Les deux outils sont installés et vérifiés.
    pub ready: bool,
    /// Une transcription s'en sert : ni mise à jour ni retrait.
    pub in_use: bool,
    pub tools: Vec<ToolStatus>,
}

#[cfg(test)]
mod tests;
