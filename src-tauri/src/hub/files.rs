//! Fichiers d'un dépôt de Hugging Face pour un téléchargement direct (sans
//! Ollama, contrat 100 § 3.3) : fiche `GET /api/models/<dépôt>?blobs=true`
//! (commit du dépôt, tailles et SHA-256 LFS de chaque fichier), puis choix des
//! fichiers d'une version (morceaux dans l'ordre, projecteur de vision).
//!
//! La fiche est lue au début de chaque téléchargement : la révision est figée
//! à ce commit (adresses `resolve/<commit>/…`), et une version sans empreinte
//! n'est jamais téléchargée (rien pour vérifier le fichier reçu). Comme
//! `hub::manifest`, ces requêtes ne passent pas par `hub::hf::Hf` (quota
//! partagé du catalogue) : une par téléchargement.

use std::time::Duration;

use serde_json::Value;
use url::Url;

use super::errors;
use super::quant::{self, FileEntry};
use crate::i18n::t;
use crate::image_models::downloads::safe_name;
use crate::local_models::id::{ModelId, same_model};
use crate::ollama::client;

/// Taille lue au plus pour la fiche (la plus grosse relevée fait 1 Mo).
const MAX_BODY: usize = 8 * 1024 * 1024;

/// Fichier du dépôt.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteFile {
    /// Chemin dans le dépôt.
    pub path: String,
    pub size: u64,
    /// SHA-256 LFS (hexadécimal, minuscules) ; `None` : fichier hors LFS.
    pub sha256: Option<String>,
}

/// Fiche lue.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Listing {
    /// Identifiant écrit par Hugging Face (sa casse).
    pub id: Option<String>,
    /// Commit du dépôt.
    pub sha: String,
    pub gated: bool,
    pub files: Vec<RemoteFile>,
}

/// Échec de la lecture de la fiche ; texte lisible (langue du moment).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListingError(pub String);

/// Adresse de la fiche du dépôt `repo_id` (« auteur/dépôt »).
pub fn listing_url(hf_url: &str, repo_id: &str) -> String {
    format!(
        "{hf_url}/api/models/{repo_id}?blobs=true&expand[]=siblings&expand[]=sha&expand[]=gated"
    )
}

/// Lit la fiche du dépôt `repo_id`, en `timeout` au plus.
pub async fn fetch(
    hf_url: &str,
    repo_id: &str,
    timeout: Duration,
) -> Result<Listing, ListingError> {
    let fail = |message: String| Err(ListingError(message));
    let Ok(url) = Url::parse(&listing_url(hf_url, repo_id)) else {
        // i18n-ignore : adresse de débogage seulement (`CHATONLM_HF_URL`).
        return fail(format!("invalid Hugging Face address: {hf_url}"));
    };
    let client = match client::client(&url, Some(timeout)) {
        Ok(client) => client,
        Err(error) => return fail(unreachable(error)),
    };
    let mut response = match client.get(url).send().await {
        Ok(response) => response,
        Err(error) => return fail(unreachable(error)),
    };
    let status = response.status();
    if matches!(status.as_u16(), 401 | 403 | 404) {
        return fail(errors::not_found_or_private());
    }
    if !status.is_success() {
        return fail(t!("downloads.hfStatus", status = status.as_u16()));
    }
    let mut body = Vec::new();
    loop {
        match response.chunk().await {
            Ok(Some(chunk)) => {
                body.extend_from_slice(&chunk);
                if body.len() > MAX_BODY {
                    return fail(t!("localModels.unreadableListing"));
                }
            }
            Ok(None) => break,
            Err(error) => return fail(unreachable(error)),
        }
    }
    parse(&body).ok_or_else(|| ListingError(t!("localModels.unreadableListing")))
}

fn unreachable(error: reqwest::Error) -> String {
    if error.is_timeout() {
        t!("downloads.hfTimeout")
    } else {
        t!("downloads.hfUnreachable", error = error.without_url())
    }
}

/// Fiche de l'API ; `None` sans commit ou sans liste de fichiers. Une entrée
/// sans chemin est ignorée ; une empreinte qui n'est pas un SHA-256 compte
/// comme absente.
pub fn parse(body: &[u8]) -> Option<Listing> {
    let value: Value = serde_json::from_slice(body).ok()?;
    let sha = value["sha"]
        .as_str()
        .map(str::trim)
        .filter(|sha| !sha.is_empty())?;
    let siblings = value["siblings"].as_array()?;
    let files = siblings
        .iter()
        .filter_map(|sibling| {
            let path = sibling["rfilename"].as_str()?.to_owned();
            let size = sibling["lfs"]["size"]
                .as_u64()
                .or_else(|| sibling["size"].as_u64())
                .unwrap_or(0);
            let sha256 = sibling["lfs"]["sha256"]
                .as_str()
                .filter(|sha| sha.len() == 64 && sha.bytes().all(|b| b.is_ascii_hexdigit()))
                .map(str::to_ascii_lowercase);
            Some(RemoteFile { path, size, sha256 })
        })
        .collect();
    let gated = match &value["gated"] {
        Value::Bool(gated) => *gated,
        Value::String(mode) => !mode.is_empty() && mode != "false",
        _ => false,
    };
    Some(Listing {
        id: value["id"].as_str().map(str::to_owned),
        sha: sha.to_owned(),
        gated,
        files,
    })
}

/// Fichier à télécharger.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlanFile {
    /// Chemin dans le dépôt.
    pub path: String,
    pub size: u64,
    pub sha256: String,
}

/// Ce qu'un téléchargement doit recevoir.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    /// Identifiant écrit comme l'inventaire l'écrit (casse de la fiche).
    pub id: String,
    /// « auteur/dépôt ».
    pub repo: String,
    /// Commit figé.
    pub revision: String,
    pub quant: String,
    /// Poids, morceaux dans l'ordre.
    pub files: Vec<PlanFile>,
    pub mmproj: Option<PlanFile>,
}

impl Plan {
    /// Tous les fichiers : poids, puis projecteur.
    pub fn all(&self) -> impl Iterator<Item = &PlanFile> {
        self.files.iter().chain(&self.mmproj)
    }

    /// Octets à recevoir en tout.
    pub fn total(&self) -> u64 {
        self.all()
            .fold(0, |sum, file| sum.saturating_add(file.size))
    }

    /// Chemin relatif au dossier des modèles : `<auteur>/<dépôt>/<chemin>`.
    pub fn local(&self, file: &PlanFile) -> String {
        format!("{}/{}", self.repo, file.path)
    }
}

/// Fichiers de la version `id` dans la fiche `listing` ; texte lisible si la
/// version n'est pas téléchargeable. Projecteur de vision ajouté si le dépôt
/// en a un (voir [`quant::projector_for`]).
pub fn plan(id: &ModelId, listing: &Listing) -> Result<Plan, String> {
    let Some(requested_repo) = id.repo_id() else {
        return Err(errors::unavailable_version());
    };
    if listing.gated {
        return Err(errors::not_found_or_private());
    }
    let repo = listing
        .id
        .clone()
        .filter(|found| found.eq_ignore_ascii_case(&requested_repo))
        .unwrap_or(requested_repo);
    let entries: Vec<FileEntry> = listing
        .files
        .iter()
        .map(|file| FileEntry {
            path: &file.path,
            size: file.size,
        })
        .collect();
    let versions = quant::inventory(&repo, &entries);
    if versions.is_empty() {
        return Err(errors::not_gguf());
    }
    let wanted = id.to_string();
    let wanted_bare = format!("{repo}:{}", id.quant());
    let Some(version) = versions.iter().find(|version| {
        version
            .model
            .as_deref()
            .is_some_and(|model| same_model(model, &wanted) || same_model(model, &wanted_bare))
    }) else {
        return Err(errors::unavailable_version());
    };
    let remote = |path: &str| -> Result<PlanFile, String> {
        let file = listing
            .files
            .iter()
            .find(|file| file.path == path)
            .ok_or_else(errors::unavailable_version)?;
        if !safe_path(&file.path) {
            return Err(t!("localModels.unsafeName", file = file.path));
        }
        let sha256 = file
            .sha256
            .clone()
            .ok_or_else(|| t!("localModels.noChecksum", file = file.path))?;
        Ok(PlanFile {
            path: file.path.clone(),
            size: file.size,
            sha256,
        })
    };
    let files = version
        .files
        .iter()
        .map(|path| remote(path))
        .collect::<Result<Vec<_>, _>>()?;
    let mmproj = quant::projector_for(&entries, version.quant.as_deref())
        .map(|entry| remote(entry.path))
        .transpose()?;
    let id = version.model.clone().unwrap_or(wanted);
    let quant = id
        .rsplit_once(':')
        .map_or_else(String::new, |(_, quant)| quant.to_owned());
    Ok(Plan {
        id,
        repo,
        revision: listing.sha.clone(),
        quant,
        files,
        mmproj,
    })
}

/// Caractères laissés tels quels dans un segment d'adresse.
const SEGMENT: &percent_encoding::AsciiSet = &percent_encoding::NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.');

/// Adresse d'un fichier du dépôt `repo` (« auteur/dépôt ») au commit
/// `revision` : `<HF>/<auteur>/<dépôt>/resolve/<commit>/<chemin>`.
pub fn resolve_url(hf_url: &str, repo: &str, revision: &str, path: &str) -> String {
    let encode = |text: &str| percent_encoding::utf8_percent_encode(text, SEGMENT).to_string();
    let path: Vec<String> = path.split('/').map(encode).collect();
    format!(
        "{hf_url}/{repo}/resolve/{}/{}",
        encode(revision),
        path.join("/")
    )
}

/// Chemin de dépôt rangeable sous le dossier des modèles, sous Linux comme
/// sous Windows : chaque segment l'est ([`safe_name`]), sans segment vide.
pub fn safe_path(path: &str) -> bool {
    !path.is_empty()
        && path
            .split('/')
            .all(|segment| !segment.is_empty() && safe_name(segment))
}

/// Modèle périmé : un de ses fichiers (chemins relatifs au dossier des
/// modèles, `<auteur>/<dépôt>/<chemin>`) a disparu de la fiche ou n'y a plus
/// la même empreinte. Fichier sans empreinte dans la fiche, ou empreinte
/// locale inconnue : pas jugé. Appelée par `local_models::check_outdated`
/// (lot L5).
#[allow(dead_code)]
pub fn outdated<'a>(
    repo: &str,
    local: impl IntoIterator<Item = (&'a str, Option<&'a str>)>,
    listing: &Listing,
) -> bool {
    let prefix = format!("{repo}/");
    local.into_iter().any(|(path, sha256)| {
        let Some(in_repo) = path.strip_prefix(&prefix) else {
            return false;
        };
        match listing.files.iter().find(|file| file.path == in_repo) {
            None => true,
            Some(remote) => match (&remote.sha256, sha256) {
                (Some(remote), Some(local)) => !remote.eq_ignore_ascii_case(local),
                _ => false,
            },
        }
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn sha(c: char) -> String {
        c.to_string().repeat(64)
    }

    fn listing(files: &[(&str, u64, Option<char>)]) -> Listing {
        Listing {
            id: Some("Unsloth/Qwen3-VL-GGUF".into()),
            sha: "c0ffee".into(),
            gated: false,
            files: files
                .iter()
                .map(|&(path, size, digest)| RemoteFile {
                    path: path.into(),
                    size,
                    sha256: digest.map(sha),
                })
                .collect(),
        }
    }

    fn id(text: &str) -> ModelId {
        ModelId::parse(text).unwrap()
    }

    #[test]
    fn the_listing_is_read_from_the_api() {
        let body = json!({
            "id": "a/b-GGUF",
            "sha": "abc123",
            "gated": "manual",
            "siblings": [
                { "rfilename": ".gitattributes", "size": 1519 },
                { "rfilename": "b-Q4_K_M.gguf", "size": 10,
                  "lfs": { "sha256": "AB".repeat(32), "size": 10, "pointerSize": 134 } },
                { "rfilename": "bad.gguf", "lfs": { "sha256": "zz", "size": 4 } },
                { "size": 3 }
            ]
        });
        let listing = parse(body.to_string().as_bytes()).unwrap();
        assert_eq!(listing.sha, "abc123");
        assert!(listing.gated);
        assert_eq!(listing.id.as_deref(), Some("a/b-GGUF"));
        assert_eq!(
            listing.files,
            [
                RemoteFile {
                    path: ".gitattributes".into(),
                    size: 1519,
                    sha256: None
                },
                RemoteFile {
                    path: "b-Q4_K_M.gguf".into(),
                    size: 10,
                    sha256: Some("ab".repeat(32))
                },
                RemoteFile {
                    path: "bad.gguf".into(),
                    size: 4,
                    sha256: None
                },
            ]
        );
        assert!(
            !parse(br#"{"sha":"x","siblings":[],"gated":false}"#)
                .unwrap()
                .gated
        );
        for bad in [
            &b"<html>"[..],
            br#"{"siblings":[]}"#,
            br#"{"sha":"x"}"#,
            br#"{"sha":"","siblings":[]}"#,
        ] {
            assert_eq!(parse(bad), None);
        }
    }

    #[test]
    fn a_version_brings_its_parts_and_a_projector() {
        let listing = listing(&[
            ("V-Q8_0-00002-of-00002.gguf", 5, Some('2')),
            ("V-Q8_0-00001-of-00002.gguf", 7, Some('1')),
            ("V-Q4_K_M.gguf", 6, Some('4')),
            ("mmproj-F16.gguf", 3, Some('f')),
            ("mmproj-Q8_0.gguf", 2, Some('8')),
            ("README.md", 1, None),
        ]);
        let q8 = plan(&id("unsloth/qwen3-vl-gguf:q8_0"), &listing).unwrap();
        // Casse de la fiche ; morceaux dans l'ordre ; projecteur de même
        // précision.
        assert_eq!(q8.id, "Unsloth/Qwen3-VL-GGUF:Q8_0");
        assert_eq!(q8.repo, "Unsloth/Qwen3-VL-GGUF");
        assert_eq!(q8.revision, "c0ffee");
        assert_eq!(q8.quant, "Q8_0");
        let paths: Vec<_> = q8.files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(
            paths,
            ["V-Q8_0-00001-of-00002.gguf", "V-Q8_0-00002-of-00002.gguf"]
        );
        assert_eq!(q8.files[0].sha256, sha('1'));
        assert_eq!(q8.mmproj.as_ref().unwrap().path, "mmproj-Q8_0.gguf");
        assert_eq!(q8.total(), 7 + 5 + 2);
        assert_eq!(
            q8.local(&q8.files[0]),
            "Unsloth/Qwen3-VL-GGUF/V-Q8_0-00001-of-00002.gguf"
        );
        // Pas de projecteur Q4_K_M : celui en Q8_0.
        let q4 = plan(&id("Unsloth/Qwen3-VL-GGUF:Q4_K_M"), &listing).unwrap();
        assert_eq!(q4.mmproj.unwrap().path, "mmproj-Q8_0.gguf");
        assert_eq!(q4.files.len(), 1);
    }

    #[test]
    fn unusable_versions_are_told() {
        let english = crate::hub::errors::english::UNAVAILABLE_VERSION;
        let base = listing(&[("V-Q4_K_M.gguf", 6, Some('4'))]);
        assert_eq!(
            plan(&id("Unsloth/Qwen3-VL-GGUF:Q8_0"), &base).unwrap_err(),
            english
        );
        // Aucun GGUF.
        let weights = listing(&[("model.safetensors", 6, Some('4'))]);
        assert_eq!(
            plan(&id("Unsloth/Qwen3-VL-GGUF:Q4_K_M"), &weights).unwrap_err(),
            crate::hub::errors::english::NOT_GGUF
        );
        // Sans empreinte : jamais téléchargé.
        let nolfs = listing(&[("V-Q4_K_M.gguf", 6, None)]);
        assert_eq!(
            plan(&id("Unsloth/Qwen3-VL-GGUF:Q4_K_M"), &nolfs).unwrap_err(),
            "Hugging Face gives no checksum for V-Q4_K_M.gguf: it can't be checked, so it isn't downloaded."
        );
        // Projecteur sans empreinte : refusé aussi.
        let projector = listing(&[
            ("V-Q4_K_M.gguf", 6, Some('4')),
            ("mmproj-F16.gguf", 2, None),
        ]);
        assert!(plan(&id("Unsloth/Qwen3-VL-GGUF:Q4_K_M"), &projector).is_err());
        // Restreint.
        let mut gated = base.clone();
        gated.gated = true;
        assert_eq!(
            plan(&id("Unsloth/Qwen3-VL-GGUF:Q4_K_M"), &gated).unwrap_err(),
            crate::hub::errors::english::NOT_FOUND_OR_PRIVATE
        );
        // Nom impossible à ranger sous Windows.
        let reserved = listing(&[("CON/V-Q4_K_M.gguf", 6, Some('4'))]);
        assert_eq!(
            plan(&id("Unsloth/Qwen3-VL-GGUF:Q4_K_M"), &reserved).unwrap_err(),
            "CON/V-Q4_K_M.gguf has a name that can't be saved on this computer."
        );
        // Importé d'Ollama : rien à télécharger.
        assert!(plan(&id("@ollama/qwen3:0.6b"), &base).is_err());
    }

    #[test]
    fn files_are_fetched_at_the_pinned_commit() {
        assert_eq!(
            resolve_url(
                "http://127.0.0.1:9",
                "a/b-GGUF",
                "c0ffee",
                "BF16/x y-00001-of-00002.gguf"
            ),
            "http://127.0.0.1:9/a/b-GGUF/resolve/c0ffee/BF16/x%20y-00001-of-00002.gguf"
        );
    }

    #[test]
    fn paths_must_be_storable() {
        assert!(safe_path("BF16/model-BF16-00001-of-00002.gguf"));
        for bad in [
            "",
            "/a.gguf",
            "a//b.gguf",
            "a/../b.gguf",
            "a/nul.gguf",
            "a:b.gguf",
            "a/b.",
        ] {
            assert!(!safe_path(bad), "{bad}");
        }
    }

    #[test]
    fn a_changed_or_removed_file_makes_a_model_outdated() {
        let listing = listing(&[
            ("V-Q4_K_M.gguf", 6, Some('4')),
            ("mmproj-F16.gguf", 3, Some('f')),
        ]);
        let repo = "Unsloth/Qwen3-VL-GGUF";
        let weights = "Unsloth/Qwen3-VL-GGUF/V-Q4_K_M.gguf";
        let projector = "Unsloth/Qwen3-VL-GGUF/mmproj-F16.gguf";
        let four = sha('4');
        let eff = sha('f');
        let other = sha('0');
        assert!(!outdated(
            repo,
            [
                (weights, Some(four.as_str())),
                (projector, Some(eff.as_str()))
            ],
            &listing
        ));
        // Casse de l'empreinte ignorée.
        let upper = four.to_ascii_uppercase();
        assert!(!outdated(repo, [(weights, Some(upper.as_str()))], &listing));
        assert!(outdated(repo, [(weights, Some(other.as_str()))], &listing));
        assert!(outdated(
            repo,
            [("Unsloth/Qwen3-VL-GGUF/gone.gguf", Some(four.as_str()))],
            &listing
        ));
        // Empreinte locale inconnue : pas jugé.
        assert!(!outdated(repo, [(weights, None)], &listing));
    }
}
