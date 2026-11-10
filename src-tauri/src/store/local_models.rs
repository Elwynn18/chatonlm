//! Base locale des modèles de langage (table `local_models`) et réglages du
//! moteur (table `engine_settings`, une seule ligne) : migration 0039,
//! contrat 100 § 3. Chemins relatifs au dossier des modèles.

use std::collections::BTreeMap;

use rusqlite::{Connection, OptionalExtension, params};

use crate::error::{AppError, AppResult};
use crate::local_models::id::same_model;
use crate::local_models::{ModelCapabilities, ModelSource};

/// Ligne de `local_models`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredLocalModel {
    pub id: String,
    pub source: ModelSource,
    pub repo: String,
    pub revision: Option<String>,
    pub quant: String,
    /// Relatifs au dossier des modèles, morceaux dans l'ordre (jamais vide).
    pub files: Vec<String>,
    pub mmproj: Option<String>,
    pub size_bytes: u64,
    /// Chemin relatif → SHA-256 (hexadécimal, minuscules), projecteur compris.
    pub sha256: BTreeMap<String, String>,
    pub arch: Option<String>,
    pub context_max: Option<u32>,
    pub params_label: Option<String>,
    pub file_type: Option<u32>,
    pub capabilities: ModelCapabilities,
    pub installed_at: i64,
}

impl StoredLocalModel {
    /// Tous les fichiers du modèle : poids, puis projecteur.
    pub fn all_files(&self) -> impl Iterator<Item = &str> {
        self.files
            .iter()
            .map(String::as_str)
            .chain(self.mmproj.as_deref())
    }
}

const COLUMNS: &str = "id, source, repo, revision, quant, files, mmproj, size_bytes, sha256, \
     arch, context_max, params_label, file_type, capabilities, installed_at";

fn corrupt(what: &str, error: impl std::fmt::Display) -> AppError {
    // i18n-ignore : base modifiée à la main.
    AppError::Internal(format!("unreadable {what} in local_models: {error}"))
}

fn read(row: &rusqlite::Row) -> rusqlite::Result<RawRow> {
    Ok(RawRow {
        id: row.get(0)?,
        source: row.get(1)?,
        repo: row.get(2)?,
        revision: row.get(3)?,
        quant: row.get(4)?,
        files: row.get(5)?,
        mmproj: row.get(6)?,
        size_bytes: row.get(7)?,
        sha256: row.get(8)?,
        arch: row.get(9)?,
        context_max: row.get(10)?,
        params_label: row.get(11)?,
        file_type: row.get(12)?,
        capabilities: row.get(13)?,
        installed_at: row.get(14)?,
    })
}

/// Ligne telle que lue, avant le décodage des colonnes JSON.
struct RawRow {
    id: String,
    source: String,
    repo: String,
    revision: Option<String>,
    quant: String,
    files: String,
    mmproj: Option<String>,
    size_bytes: i64,
    sha256: String,
    arch: Option<String>,
    context_max: Option<i64>,
    params_label: Option<String>,
    file_type: Option<i64>,
    capabilities: String,
    installed_at: i64,
}

impl RawRow {
    fn decode(self) -> AppResult<StoredLocalModel> {
        let source = match self.source.as_str() {
            "hf" => ModelSource::Hf,
            "ollama" => ModelSource::Ollama,
            other => return Err(corrupt("source", other)),
        };
        let files: Vec<String> =
            serde_json::from_str(&self.files).map_err(|error| corrupt("files", error))?;
        if files.is_empty() {
            return Err(corrupt("files", "empty list"));
        }
        Ok(StoredLocalModel {
            id: self.id,
            source,
            repo: self.repo,
            revision: self.revision,
            quant: self.quant,
            files,
            mmproj: self.mmproj.filter(|path| !path.is_empty()),
            size_bytes: u64::try_from(self.size_bytes).unwrap_or(0),
            sha256: serde_json::from_str(&self.sha256).map_err(|error| corrupt("sha256", error))?,
            arch: self.arch,
            context_max: self.context_max.and_then(|n| u32::try_from(n).ok()),
            params_label: self.params_label,
            file_type: self.file_type.and_then(|n| u32::try_from(n).ok()),
            capabilities: serde_json::from_str(&self.capabilities)
                .map_err(|error| corrupt("capabilities", error))?,
            installed_at: self.installed_at,
        })
    }
}

/// Ligne décodée ; illisible (base modifiée à la main, colonne d'un type
/// inattendu) : journalisée et ignorée, pour que les autres modèles restent
/// utilisables (liste, téléchargements, suppressions).
fn decoded(row: rusqlite::Result<RawRow>) -> Option<StoredLocalModel> {
    match row.map_err(AppError::from).and_then(RawRow::decode) {
        Ok(model) => Some(model),
        Err(error) => {
            // i18n-ignore : journal.
            crate::app_log!("chatonlm: a local model is skipped: {error}");
            None
        }
    }
}

/// Modèles installés, du plus récent au plus ancien (identifiant à date
/// égale) ; une ligne illisible est ignorée (voir [`decoded`]).
pub fn list(connection: &Connection) -> AppResult<Vec<StoredLocalModel>> {
    let mut statement = connection.prepare(&format!(
        "SELECT {COLUMNS} FROM local_models ORDER BY installed_at DESC, id"
    ))?;
    let rows = statement.query_map([], read)?;
    Ok(rows.filter_map(decoded).collect())
}

/// Modèle `id` (quantification sans la casse) ; une ligne illisible compte
/// comme absente.
pub fn get(connection: &Connection, id: &str) -> AppResult<Option<StoredLocalModel>> {
    let exact = connection
        .query_row(
            &format!("SELECT {COLUMNS} FROM local_models WHERE id = ?1"),
            params![id],
            read,
        )
        .optional();
    if let Some(model) = exact.transpose().and_then(decoded) {
        return Ok(Some(model));
    }
    Ok(list(connection)?
        .into_iter()
        .find(|model| same_model(&model.id, id)))
}

/// Enregistre `model` ; une ligne du même modèle (casse de la quantification
/// comprise) est remplacée. Les autres lignes qui citent un de ses fichiers
/// prennent son empreinte (fichier partagé, projecteur de vision, remplacé
/// par ce téléchargement).
pub fn save(connection: &mut Connection, model: &StoredLocalModel) -> AppResult<()> {
    let transaction = connection.transaction()?;
    // Même identifiant exact, même illisible : remplacé.
    transaction.execute("DELETE FROM local_models WHERE id = ?1", params![model.id])?;
    let others = list(&transaction)?;
    for other in &others {
        if same_model(&other.id, &model.id) {
            transaction.execute("DELETE FROM local_models WHERE id = ?1", params![other.id])?;
            continue;
        }
        let mut sha256 = other.sha256.clone();
        let mut changed = false;
        for (path, digest) in &model.sha256 {
            if let Some(known) = sha256.get_mut(path)
                && known != digest
            {
                known.clone_from(digest);
                changed = true;
            }
        }
        if changed {
            transaction.execute(
                "UPDATE local_models SET sha256 = ?1 WHERE id = ?2",
                params![json(&sha256)?, other.id],
            )?;
        }
    }
    transaction.execute(
        &format!(
            "INSERT INTO local_models ({COLUMNS}) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)"
        ),
        params![
            model.id,
            match model.source {
                ModelSource::Hf => "hf",
                ModelSource::Ollama => "ollama",
            },
            model.repo,
            model.revision,
            model.quant,
            json(&model.files)?,
            model.mmproj,
            i64::try_from(model.size_bytes).unwrap_or(i64::MAX),
            json(&model.sha256)?,
            model.arch,
            model.context_max,
            model.params_label,
            model.file_type,
            json(&model.capabilities)?,
            model.installed_at,
        ],
    )?;
    transaction.commit()?;
    Ok(())
}

fn json(value: &impl serde::Serialize) -> AppResult<String> {
    serde_json::to_string(value).map_err(|error| AppError::Internal(error.to_string()))
}

/// Retire la ligne du modèle `id` (quantification sans la casse) ; vrai si
/// elle existait.
pub fn remove(connection: &Connection, id: &str) -> AppResult<bool> {
    // Identifiant exact d'abord : une ligne illisible part aussi.
    if connection.execute("DELETE FROM local_models WHERE id = ?1", params![id])? > 0 {
        return Ok(true);
    }
    let Some(model) = get(connection, id)? else {
        return Ok(false);
    };
    Ok(connection.execute("DELETE FROM local_models WHERE id = ?1", params![model.id])? > 0)
}

/// Dossier des modèles choisi ; `None` : le dossier par défaut.
pub fn models_dir(connection: &Connection) -> AppResult<Option<String>> {
    let dir: Option<String> = connection.query_row(
        "SELECT models_dir FROM engine_settings WHERE id = 1",
        [],
        |row| row.get(0),
    )?;
    Ok(dir.filter(|dir| !dir.trim().is_empty()))
}

/// `None` : revenir au dossier par défaut. Réglage du dossier des modèles :
/// lot L6 (aucun écran encore).
#[allow(dead_code)]
pub fn set_models_dir(connection: &Connection, dir: Option<&str>) -> AppResult<()> {
    connection.execute(
        "UPDATE engine_settings SET models_dir = ?1 WHERE id = 1",
        params![dir],
    )?;
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::store::Store;

    pub(crate) fn model(id: &str, files: &[&str], mmproj: Option<&str>) -> StoredLocalModel {
        let (repo, quant) = id.rsplit_once(':').unwrap();
        let mut sha256 = BTreeMap::new();
        for (index, path) in files.iter().copied().chain(mmproj).enumerate() {
            sha256.insert(path.to_owned(), format!("{index:064x}"));
        }
        StoredLocalModel {
            id: id.into(),
            source: ModelSource::Hf,
            repo: repo.into(),
            revision: Some("0123abcd".into()),
            quant: quant.into(),
            files: files.iter().map(|&path| path.to_owned()).collect(),
            mmproj: mmproj.map(str::to_owned),
            size_bytes: 1234,
            sha256,
            arch: Some("qwen3".into()),
            context_max: Some(40960),
            params_label: Some("0.6B".into()),
            file_type: Some(15),
            capabilities: ModelCapabilities {
                vision: mmproj.is_some(),
                tools: true,
                thinking: true,
                ..ModelCapabilities::default()
            },
            installed_at: 10,
        }
    }

    #[test]
    fn models_are_saved_listed_found_and_removed() {
        let store = Store::open_in_memory().unwrap();
        store
            .blocking(|c| {
                assert_eq!(list(c)?, vec![]);
                let a = model("u/A-GGUF:Q4_K_M", &["u/A-GGUF/A-Q4_K_M.gguf"], None);
                let mut b = model(
                    "u/B-GGUF:Q8_0",
                    &[
                        "u/B-GGUF/B-Q8_0-00001-of-00002.gguf",
                        "u/B-GGUF/B-Q8_0-00002-of-00002.gguf",
                    ],
                    Some("u/B-GGUF/mmproj-Q8_0.gguf"),
                );
                b.installed_at = 20;
                save(c, &a)?;
                save(c, &b)?;
                // Plus récent d'abord, tout relu tel quel.
                assert_eq!(list(c)?, vec![b.clone(), a.clone()]);
                // Quantification sans la casse ; dépôt exact.
                assert_eq!(get(c, "u/A-GGUF:q4_k_m")?, Some(a.clone()));
                assert_eq!(get(c, "u/a-gguf:Q4_K_M")?, None);
                // Le même modèle, autre casse : remplacé, pas doublé.
                let mut again = a.clone();
                again.id = "u/A-GGUF:q4_k_m".into();
                again.installed_at = 30;
                save(c, &again)?;
                assert_eq!(list(c)?, vec![again.clone(), b.clone()]);
                assert!(remove(c, "u/A-GGUF:Q4_K_M")?);
                assert!(!remove(c, "u/A-GGUF:Q4_K_M")?);
                assert_eq!(list(c)?, vec![b]);
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn a_shared_file_takes_the_latest_checksum_everywhere() {
        let store = Store::open_in_memory().unwrap();
        store
            .blocking(|c| {
                let projector = "u/V-GGUF/mmproj-Q8_0.gguf";
                let a = model(
                    "u/V-GGUF:Q4_K_M",
                    &["u/V-GGUF/V-Q4_K_M.gguf"],
                    Some(projector),
                );
                save(c, &a)?;
                let mut b = model("u/V-GGUF:Q8_0", &["u/V-GGUF/V-Q8_0.gguf"], Some(projector));
                b.sha256.insert(projector.into(), "f".repeat(64));
                save(c, &b)?;
                let a = get(c, &a.id)?.unwrap();
                assert_eq!(a.sha256[projector], "f".repeat(64));
                // Ses propres fichiers ne changent pas.
                assert_eq!(a.sha256["u/V-GGUF/V-Q4_K_M.gguf"], format!("{:064x}", 0));
                Ok(())
            })
            .unwrap();
    }

    /// Une ligne illisible est ignorée : les autres restent listées, un
    /// nouvel enregistrement du même modèle la remplace, une suppression
    /// l'efface.
    #[test]
    fn an_unreadable_row_is_skipped_not_blocking() {
        let store = Store::open_in_memory().unwrap();
        store
            .blocking(|c| {
                let good = model("u/A-GGUF:Q4_K_M", &["u/A-GGUF/A.gguf"], None);
                save(c, &good)?;
                c.execute_batch(
                    "INSERT INTO local_models (id, source, repo, quant, files, installed_at)
                         VALUES ('u/x:Q4', 'hf', 'u/x', 'Q4', 'not json', 1)",
                )?;
                assert_eq!(list(c)?, vec![good.clone()]);
                assert_eq!(get(c, "u/x:Q4")?, None);
                c.execute_batch("UPDATE local_models SET files = '[]' WHERE id = 'u/x:Q4';")?;
                assert_eq!(get(c, "u/x:Q4")?, None);
                assert_eq!(list(c)?, vec![good.clone()]);
                // Réinstallé : la ligne illisible est remplacée.
                let again = model("u/x:Q4", &["u/x/x.gguf"], None);
                save(c, &again)?;
                assert_eq!(get(c, "u/x:Q4")?, Some(again));
                c.execute_batch("UPDATE local_models SET capabilities = '[' WHERE id = 'u/x:Q4';")?;
                assert!(remove(c, "u/x:Q4")?);
                assert_eq!(list(c)?, vec![good]);
                // Une source inconnue est refusée par la table.
                assert!(
                    c.execute(
                        "INSERT INTO local_models (id, source, repo, quant, files, installed_at)
                             VALUES ('u/y:Q4', 'web', 'u/y', 'Q4', '[\"a\"]', 1)",
                        [],
                    )
                    .is_err()
                );
                Ok(())
            })
            .unwrap();
    }

    /// Base de la 0.9.4 (schéma 36) et de la 0.9.5 (schéma 38), avec un
    /// modèle choisi et un dossier de l'Ollama intégré : la migration ajoute
    /// une base locale vide et le dossier par défaut, sans rien toucher
    /// d'autre (le dossier de l'intégré reste le sien).
    #[test]
    fn a_0_9_database_gets_an_empty_model_base() {
        for version in [36, 38] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("chatonlm.sqlite");
            let connection = crate::store::database_at(&path, version);
            connection
                .execute_batch(
                    "UPDATE settings SET selected_model = 'hf.co/u/A-GGUF:Q4_K_M';
                     UPDATE builtin_ollama SET models_dir = '/mnt/ollama';",
                )
                .unwrap();
            drop(connection);
            let store = Store::open(&path).unwrap();
            store
                .blocking(|c| {
                    assert_eq!(list(c)?, vec![]);
                    assert_eq!(models_dir(c)?, None);
                    let selected: String =
                        c.query_row("SELECT selected_model FROM settings", [], |r| r.get(0))?;
                    assert_eq!(selected, "hf.co/u/A-GGUF:Q4_K_M");
                    assert_eq!(
                        crate::store::model_import::builtin_models_dir(c)?.as_deref(),
                        Some("/mnt/ollama")
                    );
                    save(c, &model("u/A-GGUF:Q4_K_M", &["u/A-GGUF/A.gguf"], None))?;
                    assert_eq!(list(c)?.len(), 1);
                    Ok(())
                })
                .unwrap();
        }
    }

    #[test]
    fn the_models_folder_is_the_default_until_chosen() {
        let store = Store::open_in_memory().unwrap();
        store
            .blocking(|c| {
                assert_eq!(models_dir(c)?, None);
                set_models_dir(c, Some("/data/models"))?;
                assert_eq!(models_dir(c)?.as_deref(), Some("/data/models"));
                set_models_dir(c, Some("  "))?;
                assert_eq!(models_dir(c)?, None);
                set_models_dir(c, None)?;
                assert_eq!(models_dir(c)?, None);
                // Une seule ligne.
                assert!(
                    c.execute("INSERT INTO engine_settings (id) VALUES (2)", [])
                        .is_err()
                );
                Ok(())
            })
            .unwrap();
    }
}
