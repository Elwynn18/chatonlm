//! Types échangés avec la page pour les modèles d'images (contrat :
//! `.tools/research/83-image-models-contract.md`). Tailles en octets.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::hub::types::HubProblem;

/// Rôle d'un fichier d'un modèle d'images.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum ImageComponentRole {
    /// Le modèle choisi (modèle de diffusion, ou modèle complet en un fichier).
    Model,
    /// Encodeur de texte (Qwen3, T5, CLIP...).
    TextEncoder,
    Vae,
}

/// Fichier d'un modèle d'images : le modèle choisi ou une pièce annexe de sa
/// famille (partagée entre les modèles qui la demandent).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ImageComponentInfo {
    pub role: ImageComponentRole,
    /// Dépôt Hugging Face (`auteur/nom`).
    pub repo: String,
    /// Chemin du fichier dans le dépôt.
    pub file: String,
    pub size_bytes: u64,
    /// Licence déclarée par le dépôt (`apache-2.0`, `other`...) ; `None` :
    /// aucune.
    pub license: Option<String>,
    /// Déjà sur le disque (pour un autre modèle de la même famille, ou celui-ci).
    pub present: bool,
}

/// Famille reconnue (pièces annexes et réglages connus de ChatonLM).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ImageFamily {
    /// Identifiant stable (`flux2-klein-4b`, `z-image-turbo`, `sdxl`...).
    pub id: String,
    /// Nom affiché (nom propre, non traduit : « FLUX.2 klein 4B »).
    pub name: String,
}

/// Modèle d'images installé, en cours de téléchargement ou recommandé.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ImageModelInfo {
    /// `<dépôt>/<chemin du fichier>` : identifiant du modèle, de son
    /// téléchargement et du réglage `imageModel`.
    pub id: String,
    /// Nom affiché (nom du fichier sans extension, ou nom donné par la
    /// recommandation).
    pub name: String,
    pub repo: String,
    pub file: String,
    /// Révision (commit) du dépôt : épinglée pour une recommandation, celle du
    /// choix pour un modèle en base. Repassée telle quelle à `image_download`
    /// (Download d'une recommandation, Try again).
    pub revision: String,
    pub family: ImageFamily,
    /// Licence du dépôt du modèle.
    pub license: Option<String>,
    /// Licence libre (Apache-2.0, MIT, BSD...) ; faux : à lire avant usage
    /// (non commerciale, OpenRAIL, « other »...).
    pub open_license: bool,
    /// Modèle non censuré (montré seulement avec le réglage `showUncensored`).
    pub uncensored: bool,
    pub recommended: bool,
    /// Tous les fichiers présents et vérifiés.
    pub installed: bool,
    /// Taille totale (modèle et pièces annexes).
    pub size_bytes: u64,
    /// Ce qui reste à télécharger (pièces déjà présentes déduites).
    pub missing_bytes: u64,
    /// Le modèle choisi pour la génération.
    pub selected: bool,
    pub components: Vec<ImageComponentInfo>,
}

/// Dépôt Hugging Face trouvé par la recherche.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ImageRepoSummary {
    /// `auteur/nom`.
    pub id: String,
    pub author: String,
    pub name: String,
    /// Famille reconnue ; `None` : non prise en charge (pas de téléchargement).
    pub family: Option<ImageFamily>,
    pub downloads: u64,
    pub likes: u64,
    /// Date ISO 8601 de la dernière modification.
    pub last_modified: Option<String>,
    pub license: Option<String>,
    pub open_license: bool,
    pub uncensored: bool,
    /// Accès restreint (compte Hugging Face nécessaire) : pas de téléchargement.
    pub gated: bool,
}

/// Résultat d'une recherche (ou des dépôts populaires si la recherche est vide).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ImageSearchResult {
    pub repos: Vec<ImageRepoSummary>,
    /// Hugging Face injoignable ou en panne : pourquoi (liste du cache si
    /// `stale`).
    pub problem: Option<HubProblem>,
    /// Liste reprise du cache faute de réponse.
    pub stale: bool,
}

/// Fichier d'un dépôt que l'on peut choisir comme modèle.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ImageFileChoice {
    pub file: String,
    pub size_bytes: u64,
    /// Quantification lue dans le nom (`Q4_0`, `Q8_0`, `BF16`...).
    pub quant: Option<String>,
    /// Choix conseillé pour cette machine (un seul par dépôt).
    pub recommended: bool,
    /// Déjà installé (ou en cours de téléchargement).
    pub installed: bool,
}

/// Pourquoi un dépôt ne peut pas être téléchargé.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase", tag = "kind")]
#[ts(export)]
pub enum ImageRepoUnsupported {
    /// Famille non reconnue (pièces annexes inconnues).
    UnknownFamily,
    /// Accès restreint.
    Gated,
    /// Aucun fichier utilisable (ni GGUF, ni fichier unique pour SD/SDXL).
    NoUsableFile,
    /// Famille écartée par ChatonLM (données ou code de licence non
    /// permissive retirés du moteur, vidéo...).
    UnsupportedFamily,
}

/// Fiche d'un dépôt.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ImageRepoDetail {
    pub summary: ImageRepoSummary,
    /// Révision (commit) épinglée pour le téléchargement.
    pub revision: String,
    pub files: Vec<ImageFileChoice>,
    /// Pièces annexes de la famille (sans le fichier du modèle).
    pub components: Vec<ImageComponentInfo>,
    pub unsupported: Option<ImageRepoUnsupported>,
    /// Hugging Face injoignable : pourquoi (fiche du cache si `stale`).
    pub problem: Option<HubProblem>,
    pub stale: bool,
}

/// Ce que la machine permet pour la génération d'images.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ImageMachine {
    /// Moteur d'images livré avec cette version.
    pub engine_available: bool,
    /// Une carte graphique est reconnue (sinon : calcul sur le processeur,
    /// plusieurs minutes par image).
    pub gpu: bool,
    /// Instructions du processeur qui manquent au moteur (AVX2...), noms
    /// affichés ; non vide : génération impossible.
    pub missing_cpu_features: Vec<String>,
    /// Place libre dans le dossier des modèles d'images ; `None` : inconnue.
    pub free_bytes: Option<u64>,
}
