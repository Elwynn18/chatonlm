//! Essais de la base locale des modèles : chemins, suppression, modèles
//! périmés (faux Hugging Face).

use std::time::Duration;

use fake_services::hf::{FakeHf, HfMode};

use super::*;
use crate::hub::downloads::{HubDownloads, Options};
use crate::hub::types::{HubDownloadRequest, HubDownloadState};
use crate::store::local_models::tests::model;

#[test]
fn rows_become_models_with_absolute_paths() {
    let root = Path::new("/data/models");
    let row = model(
        "unsloth/Qwen3-VL-GGUF:Q4_K_M",
        &[
            "unsloth/Qwen3-VL-GGUF/Q4/V-00001-of-00002.gguf",
            "unsloth/Qwen3-VL-GGUF/Q4/V-00002-of-00002.gguf",
        ],
        Some("unsloth/Qwen3-VL-GGUF/mmproj-Q8_0.gguf"),
    );
    let found = resolve(root, row).unwrap();
    assert_eq!(found.display_name, "Qwen3-VL");
    assert_eq!(
        PathBuf::from(&found.path),
        root.join("unsloth")
            .join("Qwen3-VL-GGUF")
            .join("Q4")
            .join("V-00001-of-00002.gguf")
    );
    assert_eq!(
        found.mmproj.map(PathBuf::from),
        Some(root.join("unsloth/Qwen3-VL-GGUF/mmproj-Q8_0.gguf"))
    );
    assert_eq!(found.quant, "Q4_K_M");
    assert!(found.capabilities.vision);
    assert_eq!(found.source, ModelSource::Hf);
    assert_eq!(default_models_dir(Path::new("/d")), Path::new("/d/models"));
}

/// Chemins de la table revérifiés : jamais un fichier hors du dossier des
/// modèles (`..`, absolu, `\`, `:`, nom réservé de Windows) ; une ligne qui
/// en a un n'est pas servie.
#[test]
fn paths_from_the_table_never_leave_the_models_folder() {
    let root = Path::new("/data/models");
    assert_eq!(
        absolute(root, "u/R-GGUF/Q4/R.gguf"),
        Some(root.join("u").join("R-GGUF").join("Q4").join("R.gguf"))
    );
    for bad in [
        "",
        "../x.gguf",
        "u/R-GGUF/../../../etc/passwd",
        "/etc/passwd",
        "u//R.gguf",
        "u\\..\\R.gguf",
        "C:/R.gguf",
        "u/CON/R.gguf",
        "u/R-GGUF/.",
    ] {
        assert_eq!(absolute(root, bad), None, "{bad}");
    }
    let escaping = model("u/R-GGUF:Q4_K_M", &["u/R-GGUF/../../x.gguf"], None);
    assert_eq!(resolve(root, escaping), None);
    let projector = model(
        "u/R-GGUF:Q4_K_M",
        &["u/R-GGUF/R.gguf"],
        Some("/etc/mmproj.gguf"),
    );
    assert_eq!(resolve(root, projector), None);
}

/// Sous Windows, un projecteur cité par un autre modèle avec une autre
/// casse (dépôt renommé par Hugging Face) est le même fichier : gardé.
#[test]
fn shared_files_ignore_case_on_windows() {
    let a = model(
        "u/v-gguf:Q4_K_M",
        &["u/v-gguf/V-Q4_K_M.gguf"],
        Some("u/v-gguf/mmproj.gguf"),
    );
    let others = ["U/V-GGUF/V-Q8_0.gguf", "U/V-GGUF/mmproj.gguf"];
    assert_eq!(unshared_files(&a, others, true), ["u/v-gguf/V-Q4_K_M.gguf"]);
    assert_eq!(
        unshared_files(&a, others, false),
        ["u/v-gguf/V-Q4_K_M.gguf", "u/v-gguf/mmproj.gguf"]
    );
    assert_eq!(file_key_for("A/B.gguf", true), r"a\b.gguf");
    assert_eq!(
        file_key_for(r"C:\M\u/W-GGUF/mmproj.gguf", true),
        file_key_for(r"c:\m\U\W-GGUF\MMPROJ.gguf", true)
    );
    assert_eq!(file_key_for("A/B.gguf", false), "A/B.gguf");
}

#[test]
fn files_in_use_are_told_on_windows_only() {
    let denied = std::io::Error::from(std::io::ErrorKind::PermissionDenied);
    let sharing = std::io::Error::from_raw_os_error(32);
    assert_eq!(
        delete_error_for("a/b.gguf", &denied, true),
        "a/b.gguf is in use: stop the model, then try again."
    );
    assert_eq!(
        delete_error_for("a/b.gguf", &sharing, true),
        "a/b.gguf is in use: stop the model, then try again."
    );
    assert!(
        delete_error_for("a/b.gguf", &denied, false).starts_with("Could not delete a/b.gguf: ")
    );
}

#[tokio::test]
async fn deleting_removes_files_parts_and_empty_folders_only() {
    let store = Store::open_in_memory().unwrap();
    let data = tempfile::tempdir().unwrap();
    let root = default_models_dir(data.path());
    let a = model(
        "u/V-GGUF:Q4_K_M",
        &["u/V-GGUF/Q4/V-Q4_K_M.gguf"],
        Some("u/V-GGUF/mmproj.gguf"),
    );
    let b = model(
        "u/V-GGUF:Q8_0",
        &["u/V-GGUF/V-Q8_0.gguf"],
        Some("u/V-GGUF/mmproj.gguf"),
    );
    for path in a.all_files().chain(b.all_files()) {
        let path = absolute(&root, path).unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"x").unwrap();
    }
    // Morceau inachevé d'un autre téléchargement du premier : effacé aussi.
    std::fs::write(
        crate::image_models::downloads::part_path(
            &absolute(&root, "u/V-GGUF/Q4/V-Q4_K_M.gguf").unwrap(),
        ),
        b"x",
    )
    .unwrap();
    // Fichier étranger dans le dossier du dépôt : jamais touché.
    std::fs::write(root.join("u/V-GGUF/notes.txt"), b"mine").unwrap();
    store
        .blocking(|c| {
            table::save(c, &a)?;
            table::save(c, &b)
        })
        .unwrap();
    std::fs::write(
        expected_path(&absolute(&root, "u/V-GGUF/Q4/V-Q4_K_M.gguf").unwrap()),
        b"sha",
    )
    .unwrap();
    delete(&store, data.path(), "u/V-GGUF:q4_k_m", &Reserved::default())
        .await
        .unwrap();
    assert!(!root.join("u/V-GGUF/Q4").exists());
    assert!(root.join("u/V-GGUF/mmproj.gguf").exists());
    assert!(root.join("u/V-GGUF/V-Q8_0.gguf").exists());
    assert_eq!(
        get(&store, data.path(), "u/V-GGUF:Q4_K_M").await.unwrap(),
        None
    );
    delete(&store, data.path(), "u/V-GGUF:Q8_0", &Reserved::default())
        .await
        .unwrap();
    assert!(!root.join("u/V-GGUF/mmproj.gguf").exists());
    assert!(root.join("u/V-GGUF/notes.txt").exists());
    assert!(list(&store, data.path()).await.unwrap().is_empty());
}

/// Un fichier qu'un téléchargement a réservé (projecteur partagé par une
/// autre version en cours d'installation) n'est pas supprimé avec le
/// modèle ; libéré, il l'est à la suppression suivante.
#[tokio::test]
async fn a_reserved_file_survives_a_deletion() {
    let store = Store::open_in_memory().unwrap();
    let data = tempfile::tempdir().unwrap();
    let root = default_models_dir(data.path());
    let a = model(
        "u/W-GGUF:Q4_K_M",
        &["u/W-GGUF/W-Q4_K_M.gguf"],
        Some("u/W-GGUF/mmproj.gguf"),
    );
    for path in a.all_files() {
        let path = absolute(&root, path).unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"x").unwrap();
    }
    store.blocking(|c| table::save(c, &a)).unwrap();
    let reserved = Reserved::default();
    let projector = root.join("u/W-GGUF/mmproj.gguf");
    assert!(reserved.try_reserve(&projector));
    assert!(!reserved.try_reserve(&projector));
    delete(&store, data.path(), &a.id, &reserved).await.unwrap();
    assert!(projector.exists());
    assert!(!root.join("u/W-GGUF/W-Q4_K_M.gguf").exists());
    assert!(list(&store, data.path()).await.unwrap().is_empty());
    reserved.release(&projector);
    assert!(reserved.try_reserve(&projector));
}

/// Un fichier impossible à supprimer garde la ligne : une nouvelle
/// tentative finit le travail.
#[cfg(unix)]
#[tokio::test]
async fn a_file_that_cannot_be_deleted_keeps_the_model() {
    use std::os::unix::fs::PermissionsExt;
    let store = Store::open_in_memory().unwrap();
    let data = tempfile::tempdir().unwrap();
    let root = default_models_dir(data.path());
    let row = model("u/L-GGUF:Q4_K_M", &["u/L-GGUF/L.gguf"], None);
    let dir = root.join("u/L-GGUF");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("L.gguf"), b"x").unwrap();
    store.blocking(|c| table::save(c, &row)).unwrap();
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o500)).unwrap();
    let refused = delete(&store, data.path(), &row.id, &Reserved::default()).await;
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    let Err(AppError::System(message)) = refused else {
        panic!("expected a refusal: {refused:?}");
    };
    assert!(
        message.starts_with("Could not delete u/L-GGUF/L.gguf: "),
        "{message}"
    );
    assert_eq!(list(&store, data.path()).await.unwrap().len(), 1);
    delete(&store, data.path(), &row.id, &Reserved::default())
        .await
        .unwrap();
    assert!(list(&store, data.path()).await.unwrap().is_empty());
}

/// Modèle installé, puis nouvelle version publiée sur la branche `main` :
/// périmé ; un importé, ou une fiche injoignable, ne se juge pas.
#[tokio::test]
async fn a_new_version_on_hugging_face_makes_a_model_outdated() {
    let fake = FakeHf::start(HfMode::Normal).await.unwrap();
    let store = Store::open_in_memory().unwrap();
    let data = tempfile::tempdir().unwrap();
    let downloads = HubDownloads::with_options(
        |_| {},
        Options {
            hf_url: Some(fake.url()),
            ..Options::default()
        },
    );
    downloads.attach(store.clone(), data.path().to_owned());
    let id = "chatonlm-dl/Fresh-vision-GGUF:Q4_K_M";
    downloads
        .download(HubDownloadRequest {
            model: id.into(),
            repo: None,
            label: None,
        })
        .unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while downloads.list()[0].state != HubDownloadState::Done {
        assert!(
            std::time::Instant::now() < deadline,
            "{:?}",
            downloads.list()
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let row = store.blocking(|c| table::get(c, id)).unwrap().unwrap();
    assert_eq!(check_outdated(&fake.url(), &row).await, Some(false));
    fake.publish("chatonlm-dl/Fresh-vision-GGUF");
    assert_eq!(check_outdated(&fake.url(), &row).await, Some(true));
    let mut imported = row.clone();
    imported.source = ModelSource::Ollama;
    assert_eq!(check_outdated(&fake.url(), &imported).await, None);
    let url = fake.url();
    fake.stop().await;
    assert_eq!(check_outdated(&url, &row).await, None);
}
