//! Parcours de la machine d'états pour l'installateur de Windows (NSIS),
//! sur tout système : téléchargement, mise à jour gardée et retrouvée, paquet
//! d'un autre système refusé, issue d'une installation lue à la relance. Le
//! lancement lui-même est essayé dans `install/windows.rs` (sous Windows) et
//! ses règles dans `install/nsis.rs`.

use std::path::Path;
use std::sync::Arc;

use super::testkit::*;
use super::*;
use crate::updater::install::test_nsis;
use crate::updater::staging::tests::{
    APPIMAGE_SIGNATURE, NSIS_SIGNATURE, TEST_PUBKEY, test_appimage,
};

/// ChatonLM 0.1.0 installé par son installateur, données dans `dir`.
fn config(server: &FakeServer, dir: &Path) -> Config {
    Config {
        updater: Some(plugin_updater(
            &server.endpoint(),
            &dir.join("app").join("chatonlm.exe"),
        )),
        support: Support::Installable(UpdateInstaller::Nsis),
        platform: Platform::default(),
        staging: dir.join("data").join("updates"),
        pubkey: TEST_PUBKEY.into(),
        require_signed_version: true,
        current: Version::new(0, 1, 0),
        auto_download: true,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_windows_installer_is_downloaded_kept_and_found_again() {
    let server = FakeServer::start().await;
    server.serve(test_nsis(), NSIS_SIGNATURE);
    server.announce("9.9.9");
    let dir = tempfile::tempdir().unwrap();
    let staging = dir.path().join("data").join("updates");
    let engine = Arc::new(Engine::new(config(&server, dir.path())));
    assert_eq!(engine.status().installer, Some(UpdateInstaller::Nsis));

    engine.check(Trigger::Scheduled).await;
    wait_for(&engine, "prête", ready("9.9.9")).await;
    assert_eq!(files(&staging), ["chatonlm-9.9.9.update", "update.json"]);
    assert_eq!(
        std::fs::read(staging.join("chatonlm-9.9.9.update")).unwrap(),
        test_nsis()
    );
    assert_eq!(
        staging::load(&staging).unwrap().installer,
        UpdateInstaller::Nsis
    );

    // Relance sans installation : prête, sans nouveau téléchargement.
    let relaunched = Arc::new(Engine::new(config(&server, dir.path())));
    relaunched.restore().await;
    assert!(ready("9.9.9")(&relaunched.status().state));
    assert_eq!(server.count("/package"), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_package_of_another_system_is_never_kept() {
    // L'AppImage, bien signée, annoncée à ChatonLM pour Windows : refusée
    // (nom de fichier signé), rien n'est gardé.
    let server = FakeServer::start().await;
    server.serve(test_appimage(), APPIMAGE_SIGNATURE);
    server.announce("9.9.9");
    let dir = tempfile::tempdir().unwrap();
    let engine = Arc::new(Engine::new(config(&server, dir.path())));
    engine.check(Trigger::Scheduled).await;
    let status = wait_for(&engine, "refusée", |state| {
        matches!(state, UpdateState::Error { .. })
    })
    .await;
    assert_eq!(
        status.state,
        UpdateState::Error {
            code: UpdateErrorCode::Signature,
            detail: t!(
                "updates.signedForOtherFile",
                file = "chatonlm_9.9.9_amd64.AppImage"
            ),
        }
    );
    assert!(files(&dir.path().join("data").join("updates")).is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn the_outcome_of_the_installer_is_read_at_the_next_start() {
    // Avant de lancer l'installateur, ChatonLM note l'échec possible puis se
    // ferme. Relancé à la même version (installateur fermé, bloqué...) :
    // l'échec est montré, la mise à jour reste gardée.
    let server = FakeServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    let folder = dir.path().join("data").join("updates");
    let staged = Staged::new("9.9.9", None, NSIS_SIGNATURE, UpdateInstaller::Nsis);
    let pending = Failure {
        version: "9.9.9".into(),
        detail: t!("updates.windows.notCompleted"),
    };
    staging::save(&folder, &staged, &test_nsis()).unwrap();
    staging::save_failure(&folder, &pending);
    let same = Arc::new(Engine::new(config(&server, dir.path())));
    same.restore().await;
    assert_eq!(
        same.status().state,
        UpdateState::Error {
            code: UpdateErrorCode::Install,
            detail: t!("updates.windows.notCompleted"),
        }
    );
    assert_eq!(
        files(&folder),
        ["chatonlm-9.9.9.update", "update.json"],
        "mise à jour gardée pour un nouvel essai"
    );

    // Relancé par l'installateur à la nouvelle version : rien à montrer,
    // rien de gardé.
    staging::save_failure(&folder, &pending);
    let updated = Arc::new(Engine::new(Config {
        current: Version::new(9, 9, 9),
        ..config(&server, dir.path())
    }));
    updated.restore().await;
    assert_eq!(updated.status().state, UpdateState::Idle);
    assert!(files(&folder).is_empty());
    assert!(server.requests.lock().is_empty());
}
