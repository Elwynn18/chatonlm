//! Modèles d'images enregistrés (migration 0026, contrat 83 § 3) et modèle
//! choisi pour la génération (`settings.image_model`).

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};
use crate::image_gen::Component;
use crate::image_models::types::ImageComponentRole;

/// Fichier d'un modèle : le modèle choisi ou une pièce annexe, tel que relevé
/// au moment du choix.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredFile {
    pub role: ImageComponentRole,
    /// Option du moteur qui le reçoit.
    pub component: Component,
    pub repo: String,
    pub revision: String,
    /// Chemin dans le dépôt.
    pub file: String,
    pub size: u64,
    /// Empreinte SHA-256 (hexadécimal, minuscules).
    pub sha256: String,
    pub license: Option<String>,
}

/// Modèle enregistré.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredModel {
    /// `<dépôt>/<chemin du fichier>`.
    pub id: String,
    pub repo: String,
    pub revision: String,
    pub file: String,
    pub family: String,
    pub name: String,
    pub license: Option<String>,
    pub uncensored: bool,
    /// Installé au moins une fois (gardé par [`save`], posé par
    /// [`mark_installed`]).
    pub ever_installed: bool,
    /// Le modèle d'abord, puis les pièces annexes.
    pub files: Vec<StoredFile>,
    pub created_at: i64,
}

const COLUMNS: &str = "id, repo, revision, file, family, name, license, uncensored, components, \
     created_at, installed";

fn read(row: &rusqlite::Row) -> rusqlite::Result<(StoredModel, String)> {
    Ok((
        StoredModel {
            id: row.get(0)?,
            repo: row.get(1)?,
            revision: row.get(2)?,
            file: row.get(3)?,
            family: row.get(4)?,
            name: row.get(5)?,
            license: row.get(6)?,
            uncensored: row.get(7)?,
            ever_installed: row.get(10)?,
            files: Vec::new(),
            created_at: row.get(9)?,
        },
        row.get(8)?,
    ))
}

/// Modèle avec ses fichiers ; JSON illisible (base abîmée, version future) :
/// `None`, journalisé.
fn with_files((mut model, json): (StoredModel, String)) -> Option<StoredModel> {
    match serde_json::from_str(&json) {
        Ok(files) => {
            model.files = files;
            Some(model)
        }
        Err(error) => {
            // i18n-ignore : journal.
            eprintln!("chatonlm: image model {} ignored: {error}", model.id);
            None
        }
    }
}

/// Tous les modèles lisibles, du plus ancien au plus récent.
pub fn list(connection: &Connection) -> AppResult<Vec<StoredModel>> {
    let mut statement = connection.prepare(&format!(
        "SELECT {COLUMNS} FROM image_models ORDER BY created_at, id"
    ))?;
    let rows = statement.query_map([], read)?;
    let mut models = Vec::new();
    for row in rows {
        models.extend(with_files(row?));
    }
    Ok(models)
}

/// Modèle `id` (`None` : absent ou illisible).
pub fn get(connection: &Connection, id: &str) -> AppResult<Option<StoredModel>> {
    let row = connection
        .query_row(
            &format!("SELECT {COLUMNS} FROM image_models WHERE id = ?1"),
            [id],
            read,
        )
        .optional()?;
    Ok(row.and_then(with_files))
}

/// Note que tous les fichiers du modèle `id` sont arrivés.
pub fn mark_installed(connection: &Connection, id: &str) -> AppResult<()> {
    connection.execute("UPDATE image_models SET installed = 1 WHERE id = ?1", [id])?;
    Ok(())
}

/// Enregistre `model` ; déjà là : remplacé, sa date de création et
/// « installé au moins une fois » gardés.
pub fn save(connection: &Connection, model: &StoredModel) -> AppResult<()> {
    let files = serde_json::to_string(&model.files)
        .map_err(|error| AppError::Internal(error.to_string()))?;
    connection.execute(
        &format!(
            "INSERT INTO image_models ({COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
             ON CONFLICT(id) DO UPDATE SET repo = ?2, revision = ?3, file = ?4, family = ?5,
             name = ?6, license = ?7, uncensored = ?8, components = ?9"
        ),
        params![
            model.id,
            model.repo,
            model.revision,
            model.file,
            model.family,
            model.name,
            model.license,
            model.uncensored,
            files,
            model.created_at,
            model.ever_installed
        ],
    )?;
    Ok(())
}

/// Supprime le modèle `id` (rendu s'il existait) ; s'il était choisi pour la
/// génération, plus aucun ne l'est.
pub fn delete(connection: &mut Connection, id: &str) -> AppResult<Option<StoredModel>> {
    let transaction = connection.transaction()?;
    let found = get(&transaction, id)?;
    transaction.execute("DELETE FROM image_models WHERE id = ?1", [id])?;
    transaction.execute(
        "UPDATE settings SET image_model = '' WHERE id = 1 AND image_model = ?1",
        [id],
    )?;
    transaction.commit()?;
    Ok(found)
}

/// Modèle choisi pour la génération (vide : aucun).
pub fn selected(connection: &Connection) -> AppResult<String> {
    Ok(
        connection.query_row("SELECT image_model FROM settings WHERE id = 1", [], |row| {
            row.get(0)
        })?,
    )
}

pub fn select(connection: &Connection, id: &str) -> AppResult<()> {
    connection.execute("UPDATE settings SET image_model = ?1 WHERE id = 1", [id])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;

    fn model(id: &str, created_at: i64) -> StoredModel {
        StoredModel {
            id: id.into(),
            repo: "a/b".into(),
            revision: "0123".into(),
            file: "m.gguf".into(),
            family: "flux2-klein-4b".into(),
            name: "m".into(),
            license: Some("apache-2.0".into()),
            uncensored: false,
            ever_installed: false,
            files: vec![StoredFile {
                role: ImageComponentRole::Model,
                component: Component::DiffusionModel,
                repo: "a/b".into(),
                revision: "0123".into(),
                file: "m.gguf".into(),
                size: 12,
                sha256: "ab".repeat(32),
                license: None,
            }],
            created_at,
        }
    }

    #[test]
    fn models_are_saved_listed_and_deleted_with_their_selection() {
        let store = Store::open_in_memory().unwrap();
        store
            .blocking(|c| {
                assert!(list(c)?.is_empty());
                assert_eq!(selected(c)?, "");
                save(c, &model("a/b/second.gguf", 20))?;
                save(c, &model("a/b/first.gguf", 10))?;
                let ids: Vec<String> = list(c)?.into_iter().map(|m| m.id).collect();
                assert_eq!(ids, ["a/b/first.gguf", "a/b/second.gguf"]);
                // Remplacé : date de création gardée, fichiers relus.
                let mut changed = model("a/b/first.gguf", 99);
                changed.revision = "4567".into();
                changed.uncensored = true;
                save(c, &changed)?;
                let found = get(c, "a/b/first.gguf")?.unwrap();
                assert_eq!(found.revision, "4567");
                assert!(found.uncensored);
                assert_eq!(found.created_at, 10);
                assert_eq!(found.files, changed.files);
                assert!(!found.ever_installed);
                mark_installed(c, "a/b/first.gguf")?;
                save(c, &changed)?;
                assert!(get(c, "a/b/first.gguf")?.unwrap().ever_installed);

                select(c, "a/b/first.gguf")?;
                assert_eq!(selected(c)?, "a/b/first.gguf");
                assert!(delete(c, "a/b/second.gguf")?.is_some());
                assert_eq!(selected(c)?, "a/b/first.gguf");
                assert!(delete(c, "a/b/first.gguf")?.is_some());
                assert_eq!(selected(c)?, "");
                assert!(delete(c, "a/b/first.gguf")?.is_none());
                assert!(get(c, "a/b/first.gguf")?.is_none());
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn an_unreadable_row_is_skipped() {
        let store = Store::open_in_memory().unwrap();
        store
            .blocking(|c| {
                save(c, &model("a/b/good.gguf", 10))?;
                save(c, &model("a/b/bad.gguf", 20))?;
                c.execute(
                    "UPDATE image_models SET components = 'not json' WHERE id = 'a/b/bad.gguf'",
                    [],
                )?;
                let ids: Vec<String> = list(c)?.into_iter().map(|m| m.id).collect();
                assert_eq!(ids, ["a/b/good.gguf"]);
                assert!(get(c, "a/b/bad.gguf")?.is_none());
                Ok(())
            })
            .unwrap();
    }
}
